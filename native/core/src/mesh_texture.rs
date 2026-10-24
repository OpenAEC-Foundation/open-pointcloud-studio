//! Bounded-memory image atlases for an OBJ mesh with measured vertex colours.
//!
//! Each triangle gets its own chart, so folds, disconnected parts and shared
//! vertices with different normals cannot smear colour across one another.
//! A page is encoded before the next page is allocated. The OBJ is published
//! last, after all of its images and material library have been written.

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::obj_mesh::{MeshGeometry, MAX_TRIANGLES, MAX_VERTICES};
use crate::LoadError;

const MAX_PAGES: u32 = 512;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AtlasOptions {
    /// Target metres per pixel. Long edges use more texels than short ones.
    pub texel_size: f64,
    /// Width and height of each square PNG, from 256 to 4096 pixels.
    pub max_resolution: u32,
}

impl Default for AtlasOptions {
    fn default() -> Self {
        Self {
            texel_size: 0.02,
            max_resolution: 4096,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AtlasReport {
    pub triangles: usize,
    pub pages: u32,
    /// Triangles whose chart reached the page resolution and therefore has
    /// coarser pixels than requested.
    pub resolution_limited: usize,
    pub texel_size: f64,
    pub max_resolution: u32,
}

fn valid(mesh: &MeshGeometry, options: AtlasOptions) -> bool {
    !mesh.vertices.is_empty()
        && !mesh.triangles.is_empty()
        && mesh.vertices.len() <= MAX_VERTICES
        && mesh.triangles.len() <= MAX_TRIANGLES
        && mesh
            .colors
            .as_ref()
            .is_some_and(|colors| colors.len() == mesh.vertices.len())
        && mesh
            .vertices
            .iter()
            .all(|vertex| vertex.iter().all(|coordinate| coordinate.is_finite()))
        && mesh.triangles.iter().all(|face| {
            face.iter()
                .all(|vertex| (*vertex as usize) < mesh.vertices.len())
        })
        && options.texel_size.is_finite()
        && options.texel_size > 0.0
        && (256..=4096).contains(&options.max_resolution)
}

fn edge(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter()
        .zip(b)
        .map(|(left, right)| (left - right).powi(2))
        .sum::<f64>()
        .sqrt()
}

fn chart_size(mesh: &MeshGeometry, face: [u32; 3], options: AtlasOptions) -> (u32, bool) {
    let [a, b, c] = face.map(|index| mesh.vertices[index as usize]);
    let longest = edge(a, b).max(edge(b, c)).max(edge(c, a));
    let wanted = (longest / options.texel_size).ceil().max(2.0);
    let limit = f64::from(options.max_resolution - 2);
    (wanted.min(limit) as u32, wanted > limit)
}

fn paint_chart(pixels: &mut [u8], resolution: u32, at: [u32; 2], side: u32, colors: [[u8; 3]; 3]) {
    for row in 0..side + 2 {
        for column in 0..side + 2 {
            let mut u = (column as f64 - 1.0) / f64::from(side);
            let mut v = (row as f64 - 1.0) / f64::from(side);
            u = u.clamp(0.0, 1.0);
            v = v.clamp(0.0, 1.0);
            if u + v > 1.0 {
                let sum = u + v;
                u /= sum;
                v /= sum;
            }
            let index = (((at[1] + row) * resolution + at[0] + column) * 3) as usize;
            for channel in 0..3 {
                let value = (1.0 - u - v) * f64::from(colors[0][channel])
                    + u * f64::from(colors[1][channel])
                    + v * f64::from(colors[2][channel]);
                pixels[index + channel] = value.round() as u8;
            }
        }
    }
}

fn write_page(path: &Path, pixels: &[u8], resolution: u32) -> Result<(), LoadError> {
    let file = BufWriter::new(File::create(path)?);
    let mut encoder = png::Encoder::new(file, resolution, resolution);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|error| LoadError::InvalidData(format!("cannot begin texture PNG: {error}")))?;
    writer
        .write_image_data(pixels)
        .map_err(|error| LoadError::InvalidData(format!("cannot write texture PNG: {error}")))?;
    Ok(())
}

/// Write an OBJ, its MTL and one or more PNG pages from measured mesh colours.
///
/// The destination must be a new `.obj` name. Existing files are never
/// replaced, and cancellation or a failed write removes every new file that
/// was published. The report tells when triangles exceeded the chosen image
/// resolution. The atlas uses the mesh's stored vertex colours; photo-colour
/// edits must have been applied before that surface was reconstructed. This
/// writer does not project E57 photo pixels directly onto texels.
pub fn write_colored_obj_atlas(
    mesh: &MeshGeometry,
    destination: impl AsRef<Path>,
    options: AtlasOptions,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<AtlasReport, LoadError> {
    if !valid(mesh, options) {
        return Err(LoadError::InvalidData(
            "invalid coloured mesh or texture settings".into(),
        ));
    }
    let destination = destination.as_ref();
    if !destination
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("obj"))
    {
        return Err(LoadError::InvalidData(
            "texture destination must end in .obj".into(),
        ));
    }
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let stem = destination
        .file_stem()
        .ok_or_else(|| LoadError::InvalidData("texture destination has no name".into()))?
        .to_string_lossy();
    let material_name = format!("{stem}.mtl");
    let material_path = parent.join(&material_name);
    if destination.exists() || material_path.exists() {
        return Err(LoadError::InvalidData(
            "choose a new texture file name".into(),
        ));
    }
    let stage = tempfile::tempdir_in(parent)?;
    let mut obj = BufWriter::new(File::create(
        stage.path().join(destination.file_name().unwrap()),
    )?);
    let mut mtl = BufWriter::new(File::create(stage.path().join(&material_name))?);
    writeln!(obj, "# Textured mesh exported by Open Pointcloud Studio")?;
    writeln!(obj, "mtllib {material_name}")?;
    let colors = mesh.colors.as_ref().expect("validated colours");
    for (vertex, color) in mesh.vertices.iter().zip(colors) {
        writeln!(
            obj,
            "v {} {} {} {} {} {}",
            vertex[0], vertex[1], vertex[2], color[0], color[1], color[2]
        )?;
    }
    if let Some(normals) = &mesh.normals {
        if normals.len() != mesh.vertices.len()
            || normals
                .iter()
                .any(|normal| normal.iter().any(|value| !value.is_finite()))
        {
            return Err(LoadError::InvalidData("invalid mesh normals".into()));
        }
        for normal in normals {
            writeln!(obj, "vn {} {} {}", normal[0], normal[1], normal[2])?;
        }
    }
    let resolution = options.max_resolution;
    let mut pixels = vec![0; resolution as usize * resolution as usize * 3];
    let mut page = 0;
    let mut x = 0;
    let mut y = 0;
    let mut row_height = 0;
    let mut occupied = false;
    let mut images = Vec::<(PathBuf, PathBuf)>::new();
    let mut limited = 0;
    for (place, face) in mesh.triangles.iter().copied().enumerate() {
        if cancelled() {
            return Err(LoadError::Cancelled);
        }
        let (side, is_limited) = chart_size(mesh, face, options);
        limited += usize::from(is_limited);
        let tile = side + 2;
        if x + tile > resolution {
            x = 0;
            y += row_height;
            row_height = 0;
        }
        if y + tile > resolution {
            if occupied {
                let name = format!("{stem}-atlas-{page:03}.png");
                write_page(&stage.path().join(&name), &pixels, resolution)?;
                images.push((stage.path().join(&name), parent.join(&name)));
            }
            page += 1;
            if page >= MAX_PAGES {
                return Err(LoadError::InvalidData("texture needs too many atlas pages; increase the texel size or use a smaller mesh".into()));
            }
            pixels.fill(0);
            x = 0;
            y = 0;
            row_height = 0;
            occupied = false;
        }
        if !occupied {
            let name = format!("{stem}-atlas-{page:03}.png");
            writeln!(mtl, "newmtl atlas_{page:03}\nKd 1 1 1\nmap_Kd {name}\n")?;
            writeln!(obj, "usemtl atlas_{page:03}")?;
        }
        occupied = true;
        let rgb = face.map(|index| colors[index as usize]);
        paint_chart(&mut pixels, resolution, [x, y], side, rgb);
        let uv = [
            [f64::from(x) + 1.5, f64::from(y) + 1.5],
            [f64::from(x + side) + 0.5, f64::from(y) + 1.5],
            [f64::from(x) + 1.5, f64::from(y + side) + 0.5],
        ];
        for [u, v] in uv {
            writeln!(
                obj,
                "vt {} {}",
                u / f64::from(resolution),
                1.0 - v / f64::from(resolution)
            )?;
        }
        let first_uv = place * 3 + 1;
        if mesh.normals.is_some() {
            writeln!(
                obj,
                "f {}/{}/{} {}/{}/{} {}/{}/{}",
                face[0] + 1,
                first_uv,
                face[0] + 1,
                face[1] + 1,
                first_uv + 1,
                face[1] + 1,
                face[2] + 1,
                first_uv + 2,
                face[2] + 1
            )?;
        } else {
            writeln!(
                obj,
                "f {}/{} {}/{} {}/{}",
                face[0] + 1,
                first_uv,
                face[1] + 1,
                first_uv + 1,
                face[2] + 1,
                first_uv + 2
            )?;
        }
        x += tile;
        row_height = row_height.max(tile);
        if place % 4096 == 0 {
            progress(place + 1, mesh.triangles.len());
        }
    }
    if occupied {
        let name = format!("{stem}-atlas-{page:03}.png");
        write_page(&stage.path().join(&name), &pixels, resolution)?;
        images.push((stage.path().join(&name), parent.join(&name)));
    }
    obj.flush()?;
    mtl.flush()?;
    drop(obj);
    drop(mtl);
    progress(mesh.triangles.len(), mesh.triangles.len());
    if cancelled() {
        return Err(LoadError::Cancelled);
    }
    let mut files = images;
    files.push((stage.path().join(&material_name), material_path));
    files.push((
        stage.path().join(destination.file_name().unwrap()),
        destination.to_path_buf(),
    ));
    if files.iter().any(|(_, target)| target.exists()) {
        return Err(LoadError::InvalidData(
            "choose a new texture file name".into(),
        ));
    }
    let mut published = Vec::new();
    for (source, target) in files {
        if cancelled() {
            for path in published {
                let _ = fs::remove_file(path);
            }
            return Err(LoadError::Cancelled);
        }
        if let Err(error) = fs::rename(source, &target) {
            for path in published {
                let _ = fs::remove_file(path);
            }
            return Err(LoadError::Io(error));
        }
        published.push(target);
    }
    Ok(AtlasReport {
        triangles: mesh.triangles.len(),
        pages: page + 1,
        resolution_limited: limited,
        texel_size: options.texel_size,
        max_resolution: resolution,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_coloured_triangle_writes_obj_mtl_and_measured_png() {
        let mesh = MeshGeometry {
            vertices: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            triangles: vec![[0, 1, 2]],
            colors: Some(vec![[255, 0, 0], [0, 255, 0], [0, 0, 255]]),
            normals: None,
        };
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("triangle.obj");
        let report = write_colored_obj_atlas(
            &mesh,
            &path,
            AtlasOptions {
                texel_size: 0.01,
                max_resolution: 256,
            },
            &|| false,
            &mut |_, _| {},
        )
        .unwrap();
        assert_eq!(report.pages, 1);
        let obj = fs::read_to_string(&path).unwrap();
        assert!(obj.contains("mtllib triangle.mtl"));
        assert!(obj.contains("f 1/1 2/2 3/3"));
        let reopened = crate::read_obj_mesh(&path).unwrap();
        assert_eq!(reopened.colors, mesh.colors);
        assert!(fs::read_to_string(folder.path().join("triangle.mtl"))
            .unwrap()
            .contains("map_Kd triangle-atlas-000.png"));
        let decoder =
            png::Decoder::new(File::open(folder.path().join("triangle-atlas-000.png")).unwrap());
        let mut reader = decoder.read_info().unwrap();
        let mut pixels = vec![0; reader.output_buffer_size()];
        reader.next_frame(&mut pixels).unwrap();
        let at = |x: usize, y: usize| -> [u8; 3] {
            pixels[(y * 256 + x) * 3..][..3].try_into().unwrap()
        };
        assert_eq!(at(1, 1), [255, 0, 0]);
        assert_eq!(at(143, 1), [0, 255, 0]);
        assert_eq!(at(1, 143), [0, 0, 255]);
    }

    #[test]
    fn cancellation_keeps_the_destination_empty() {
        let mesh = MeshGeometry {
            vertices: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            triangles: vec![[0, 1, 2]],
            colors: Some(vec![[255, 0, 0]; 3]),
            normals: None,
        };
        let folder = tempfile::tempdir().unwrap();
        let result = write_colored_obj_atlas(
            &mesh,
            folder.path().join("cancel.obj"),
            AtlasOptions::default(),
            &|| true,
            &mut |_, _| {},
        );
        assert!(matches!(result, Err(LoadError::Cancelled)));
        assert!(fs::read_dir(folder.path()).unwrap().next().is_none());
    }
}
