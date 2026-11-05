//! Bounded-memory image atlases for an OBJ mesh with measured vertex colours.
//!
//! Each triangle gets its own chart, so folds, disconnected parts and shared
//! vertices with different normals cannot smear colour across one another.
//! A page is encoded before the next page is allocated. The OBJ is published
//! last, after all of its images and material library have been written.

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::mesh_photo::{ChartCoverage, PhotoBaker};
pub use crate::mesh_photo::{PhotoTextureCoverage, PhotoTextureOptions};
use crate::obj_mesh::{MeshGeometry, MAX_TRIANGLES, MAX_VERTICES};
use crate::photo_colour::PhotoPixels;
use crate::{FilePhoto, LoadError};

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

/// Compact per-face chart placement, in the same order as the mesh faces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtlasChart {
    pub page: u32,
    pub position: [u32; 2],
    pub side: u32,
}

#[derive(Debug, Clone)]
pub struct BakedAtlas {
    pub atlas: AtlasReport,
    pub charts: Vec<AtlasChart>,
}

/// Photographs and the surface share source coordinates. The output mesh
/// may have moved, but must retain the source topology: UV sampling still
/// refers to the original measurements, including after nonuniform scaling.
pub struct PhotoAtlasRequest<'a> {
    pub source_mesh: &'a MeshGeometry,
    pub output_mesh: &'a MeshGeometry,
    pub photos: &'a [FilePhoto],
    pub atlas: AtlasOptions,
    pub photographs: PhotoTextureOptions,
}

#[derive(Debug, Clone)]
pub struct PhotoAtlasReport {
    pub atlas: AtlasReport,
    pub coverage: PhotoTextureCoverage,
    pub charts: Vec<AtlasChart>,
}

/// Bake calibrated E57 photo pixels onto independent triangle UV charts.
/// Missing observations use measured point colours, otherwise neutral grey;
/// their counts remain separate. Decoding is lazy and cached for four images
/// of at most 2048×2048 RGBA pixels. Rays reject hidden/back-facing surfaces.
pub fn write_photographic_obj_atlas(
    request: PhotoAtlasRequest<'_>,
    destination: impl AsRef<Path>,
    decode: &mut dyn FnMut(usize) -> Result<PhotoPixels, LoadError>,
    cancelled: &(dyn Fn() -> bool + Sync),
    progress: &mut dyn FnMut(usize, usize),
) -> Result<PhotoAtlasReport, LoadError> {
    if request.source_mesh.vertices.len() != request.output_mesh.vertices.len()
        || request.source_mesh.triangles.len() != request.output_mesh.triangles.len()
        || request
            .source_mesh
            .triangles
            .iter()
            .zip(&request.output_mesh.triangles)
            .any(|(source, output)| {
                if source == output {
                    return false;
                }
                let mut a = *source;
                let mut b = *output;
                a.sort_unstable();
                b.sort_unstable();
                a != b
            })
    {
        return Err(LoadError::InvalidData(
            "texture source and output need the same mesh topology".into(),
        ));
    }
    if cancelled() {
        return Err(LoadError::Cancelled);
    }
    let mut baker = PhotoBaker::new(request.source_mesh, request.photos, request.photographs)?;
    let mut coverage = PhotoTextureCoverage::default();
    let mut used = vec![false; request.photos.len()];
    let mut charts = Vec::with_capacity(request.source_mesh.triangles.len());
    let atlas = write_sampled_obj_atlas(
        request.output_mesh,
        destination.as_ref(),
        request.atlas,
        cancelled,
        progress,
        &mut |pixels, resolution, at, side, face, place| {
            if cancelled() {
                return Err(LoadError::Cancelled);
            }
            let chart = baker.chart_oriented(face, request.source_mesh.triangles[place], decode)?;
            let indices = chart.photo_indices().collect::<Vec<_>>();
            let counts = pixels
                .par_chunks_exact_mut(resolution as usize * 3)
                .skip(at[1] as usize)
                .take(side as usize + 2)
                .enumerate()
                .map(|(row, pixels)| -> Result<ChartCoverage, LoadError> {
                    let mut counts = ChartCoverage::default();
                    let mut pending = Vec::with_capacity(32);
                    for column in 0..side as usize + 2 {
                        if column % 64 == 0 && cancelled() {
                            return Err(LoadError::Cancelled);
                        }
                        let mut u = ((column as f64 - 1.0) / f64::from(side - 1)).clamp(0.0, 1.0);
                        let mut v = ((row as f64 - 1.0) / f64::from(side - 1)).clamp(0.0, 1.0);
                        if u + v > 1.0 {
                            let sum = u + v;
                            u /= sum;
                            v /= sum;
                        }
                        let (rgb, category, mask) = chart.sample([1.0 - u - v, u, v], &mut pending);
                        let offset = (at[0] as usize + column) * 3;
                        pixels[offset..offset + 3].copy_from_slice(&rgb);
                        if row >= 1 && column >= 1 && row + column <= side as usize + 1 {
                            match category {
                                0 => counts.photographs += 1,
                                1 => counts.points += 1,
                                _ => counts.unseen += 1,
                            }
                            counts.used |= mask;
                        }
                    }
                    Ok(counts)
                })
                .try_reduce(ChartCoverage::default, |mut a, b| {
                    a.photographs += b.photographs;
                    a.points += b.points;
                    a.unseen += b.unseen;
                    a.used |= b.used;
                    Ok(a)
                })?;
            coverage.photograph_texels += counts.photographs;
            coverage.point_colour_texels += counts.points;
            coverage.unobserved_texels += counts.unseen;
            for (bit, index) in indices.into_iter().enumerate() {
                if counts.used & (1 << bit) != 0 {
                    used[index] = true;
                }
            }
            Ok(())
        },
        &mut |chart| charts.push(chart),
    )?;
    coverage.photos_used = used
        .iter()
        .enumerate()
        .filter_map(|(index, used)| used.then_some(index))
        .collect();
    coverage.photos_failed = baker.failed();
    Ok(PhotoAtlasReport {
        atlas,
        coverage,
        charts,
    })
}

fn valid(mesh: &MeshGeometry, options: AtlasOptions) -> bool {
    !mesh.vertices.is_empty()
        && !mesh.triangles.is_empty()
        && mesh.vertices.len() <= MAX_VERTICES
        && mesh.triangles.len() <= MAX_TRIANGLES
        && mesh
            .colors
            .as_ref()
            .is_none_or(|colors| colors.len() == mesh.vertices.len())
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
            let mut u = (column as f64 - 1.0) / f64::from(side - 1);
            let mut v = (row as f64 - 1.0) / f64::from(side - 1);
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
    write_colored_inner(
        mesh,
        destination.as_ref(),
        options,
        cancelled,
        progress,
        &mut |_| {},
    )
}

/// The same export with its exact UV chart placement for native preview.
pub fn write_colored_obj_atlas_with_layout(
    mesh: &MeshGeometry,
    destination: impl AsRef<Path>,
    options: AtlasOptions,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<BakedAtlas, LoadError> {
    let mut charts = Vec::new();
    let atlas = write_colored_inner(
        mesh,
        destination.as_ref(),
        options,
        cancelled,
        progress,
        &mut |chart| charts.push(chart),
    )?;
    Ok(BakedAtlas { atlas, charts })
}

fn write_colored_inner(
    mesh: &MeshGeometry,
    destination: &Path,
    options: AtlasOptions,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(usize, usize),
    placed: &mut dyn FnMut(AtlasChart),
) -> Result<AtlasReport, LoadError> {
    let Some(colors) = mesh
        .colors
        .as_ref()
        .filter(|colors| colors.len() == mesh.vertices.len())
    else {
        return Err(LoadError::InvalidData(
            "mesh has no measured colours".into(),
        ));
    };
    write_sampled_obj_atlas(
        mesh,
        destination,
        options,
        cancelled,
        progress,
        &mut |pixels, resolution, at, side, face, _| {
            paint_chart(
                pixels,
                resolution,
                at,
                side,
                face.map(|index| colors[index as usize]),
            );
            Ok(())
        },
        placed,
    )
}

type ChartPainter<'a> =
    dyn FnMut(&mut [u8], u32, [u32; 2], u32, [u32; 3], usize) -> Result<(), LoadError> + 'a;

fn write_sampled_obj_atlas(
    mesh: &MeshGeometry,
    destination: &Path,
    options: AtlasOptions,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(usize, usize),
    paint: &mut ChartPainter<'_>,
    placed: &mut dyn FnMut(AtlasChart),
) -> Result<AtlasReport, LoadError> {
    if !valid(mesh, options) {
        return Err(LoadError::InvalidData(
            "invalid mesh or texture settings".into(),
        ));
    }
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
    for (index, vertex) in mesh.vertices.iter().enumerate() {
        let color = mesh
            .colors
            .as_ref()
            .map_or([128; 3], |colors| colors[index]);
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
    let mut last_progress = std::time::Instant::now();
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
        placed(AtlasChart {
            page,
            position: [x, y],
            side,
        });
        paint(&mut pixels, resolution, [x, y], side, face, place)?;
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
        if place == 0 || last_progress.elapsed() >= std::time::Duration::from_millis(100) {
            progress(place + 1, mesh.triangles.len());
            last_progress = std::time::Instant::now();
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
        let publication = tempfile::TempPath::try_from_path(source)
            .and_then(|path| path.persist_noclobber(&target).map_err(|error| error.error));
        if let Err(error) = publication {
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
        assert_eq!(at(142, 1), [0, 255, 0]); // centre addressed by the second UV
        assert_eq!(at(143, 1), [0, 255, 0]);
        assert_eq!(at(1, 142), [0, 0, 255]); // centre addressed by the third UV
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
