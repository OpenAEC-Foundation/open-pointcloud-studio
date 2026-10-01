//! File loading and bounded point sampling shared by the native UI and future clients.

use std::fmt;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

mod bag3d;
mod dxf;
mod e57_points;
mod export;
mod mesh_formats;
mod mesh_points;
mod mesher;
mod obj_mesh;
mod octree;
mod pcd;
mod ply;
mod ply_mesh;
mod ptx;
mod surface_mesh;

pub use bag3d::{fetch_bag3d_obj, BagBounds, BagLod, BagStats};
pub use dxf::read_mesh as read_dxf_mesh;
pub use export::{
    export_affine, export_affine_axes, export_affine_axes_where, export_affine_where, export_full,
    export_map, export_section, export_section_where, export_thin_percent_where, export_where,
    ExportFormat,
};
pub use mesh_formats::{read_off_mesh, read_stl_mesh};
pub use mesher::{mesh_terrain_obj, mesh_terrain_obj_where, MeshConfig, MeshStats};
pub use obj_mesh::{read_obj_mesh, MeshGeometry};
pub use octree::{IndexConfig, IndexedNode, IndexedPoint, OctreeIndex};
pub use ply_mesh::read_ply_mesh;
pub use surface_mesh::{mesh_surface_obj, mesh_surface_obj_where, SurfaceMeshConfig};

#[derive(Debug, Clone, Copy)]
pub struct Point {
    pub xyz: [f64; 3],
    pub rgb: Option<[u8; 3]>,
    pub intensity: Option<u16>,
    pub classification: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl Bounds {
    pub fn center(self) -> [f64; 3] {
        std::array::from_fn(|axis| (self.min[axis] + self.max[axis]) * 0.5)
    }

    pub fn extent(self) -> f64 {
        (0..3)
            .map(|axis| self.max[axis] - self.min[axis])
            .fold(0.0, f64::max)
    }

    fn include(&mut self, xyz: [f64; 3]) {
        for (axis, value) in xyz.into_iter().enumerate() {
            self.min[axis] = self.min[axis].min(value);
            self.max[axis] = self.max[axis].max(value);
        }
    }
}

#[derive(Debug)]
pub struct PointCloud {
    pub path: PathBuf,
    pub total_points: u64,
    pub bounds: Bounds,
    pub points: Vec<Point>,
    /// Source ordinals parallel to the bounded preview points. A value of
    /// `u64::MAX` means the compressed random-access reader could not prove
    /// the exact source ordinal; the disk octree always has exact ordinals.
    pub point_ordinals: Vec<u64>,
    pub has_rgb: bool,
    pub has_intensity: bool,
    pub has_classification: bool,
    source_stamp: Option<SourceStamp>,
}

impl PointCloud {
    /// Compare the loaded file revision, even when one view contains only a header.
    pub fn same_source_revision(&self, other: &Self) -> bool {
        self.path == other.path && self.source_stamp == other.source_stamp
    }

    /// Ensure a long-running operation still reads the loaded source revision.
    pub fn validate_source(&self) -> Result<(), LoadError> {
        let stamp = self
            .source_stamp
            .ok_or_else(|| LoadError::InvalidData("source identity is unavailable".into()))?;
        if SourceStamp::read(&self.path)? != stamp {
            return Err(LoadError::InvalidData(
                "source changed since loading".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SourceStamp {
    length: u64,
    modified: Option<SystemTime>,
}

impl SourceStamp {
    fn read(path: &Path) -> Result<Self, LoadError> {
        let metadata = fs::metadata(path)?;
        Ok(Self {
            length: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
}

#[derive(Debug)]
pub enum LoadError {
    Io(std::io::Error),
    Las(las::Error),
    E57(e57::Error),
    UnsupportedFormat(String),
    InvalidData(String),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "I/O error: {error}"),
            Self::Las(error) => write!(f, "LAS/LAZ error: {error}"),
            Self::E57(error) => write!(f, "E57 error: {error}"),
            Self::UnsupportedFormat(extension) => write!(f, "Unsupported format: {extension}"),
            Self::InvalidData(reason) => write!(f, "Invalid point cloud: {reason}"),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<std::io::Error> for LoadError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<las::Error> for LoadError {
    fn from(error: las::Error) -> Self {
        Self::Las(error)
    }
}

impl From<e57::Error> for LoadError {
    fn from(error: e57::Error) -> Self {
        Self::E57(error)
    }
}

/// Open a point cloud while keeping at most `sample_limit` points in memory.
/// Bounds and point counts are computed from the full input stream.
pub fn open(path: impl AsRef<Path>, sample_limit: usize) -> Result<PointCloud, LoadError> {
    if sample_limit == 0 {
        return Err(LoadError::InvalidData(
            "sample limit must be greater than zero".into(),
        ));
    }
    let path = path.as_ref();
    let before = SourceStamp::read(path)?;
    let mut collector = Collector::new(sample_limit);
    visit_points(path, &mut |point| collector.push(point))?;
    let after = SourceStamp::read(path)?;
    if before != after {
        return Err(LoadError::InvalidData(
            "source changed while loading".into(),
        ));
    }
    let mut cloud = collector.finish(path.to_path_buf())?;
    cloud.source_stamp = Some(after);
    Ok(cloud)
}

/// Open LAS/LAZ metadata immediately, before the preview sampling pass finishes.
/// The caller may replace this cloud with `open`'s fully checked result later.
pub fn open_las_header(path: impl AsRef<Path>) -> Result<PointCloud, LoadError> {
    let path = path.as_ref();
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if extension != "las" && extension != "laz" {
        return Err(LoadError::UnsupportedFormat(extension));
    }
    let stamp = SourceStamp::read(path)?;
    let reader = las::Reader::from_path(path)?;
    let header = reader.header();
    let count = header.number_of_points();
    let raw = header.bounds();
    let bounds = Bounds {
        min: [raw.min.x, raw.min.y, raw.min.z],
        max: [raw.max.x, raw.max.y, raw.max.z],
    };
    if count == 0
        || !bounds
            .min
            .iter()
            .chain(bounds.max.iter())
            .all(|v| v.is_finite())
    {
        return Err(LoadError::InvalidData("empty or invalid LAS header".into()));
    }
    Ok(PointCloud {
        path: path.to_path_buf(),
        total_points: count,
        bounds,
        points: Vec::new(),
        point_ordinals: Vec::new(),
        has_rgb: header.point_format().has_color,
        has_intensity: true,
        has_classification: true,
        source_stamp: Some(stamp),
    })
}

/// Sample LAS/LAZ in bounded, evenly spaced source ranges. Header bounds and
/// point count remain exact while a large preview no longer requires decoding
/// every point. Editing and selection still stream the full source.
pub fn open_las_preview(
    path: impl AsRef<Path>,
    sample_limit: usize,
) -> Result<PointCloud, LoadError> {
    if sample_limit == 0 {
        return Err(LoadError::InvalidData(
            "sample limit must be greater than zero".into(),
        ));
    }
    let path = path.as_ref();
    let stamp = SourceStamp::read(path)?;
    let mut cloud = open_las_header(path)?;
    let count = cloud.total_points;
    let target = count.min(sample_limit as u64);
    let compressed = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("laz"));
    let blocks = if count <= target { 1 } else { 256.min(target) };
    let block_len = target.div_ceil(blocks);
    cloud.points.reserve(target as usize);
    let mut reader = las::Reader::from_path(path)?;
    for block in 0..blocks {
        let start = if blocks == 1 {
            0
        } else {
            block * (count - block_len) / (blocks - 1)
        };
        reader.seek(start)?;
        let length = block_len
            .min(count - start)
            .min(target - cloud.points.len() as u64);
        for (offset, point) in reader.read_points(length)?.into_iter().enumerate() {
            cloud.points.push(convert_las_point(&point));
            cloud.point_ordinals.push(if compressed && count > target {
                u64::MAX
            } else {
                start + offset as u64
            });
        }
    }
    if cloud.points.len() as u64 != target || SourceStamp::read(path)? != stamp {
        return Err(LoadError::InvalidData(
            "LAS source changed while sampling".into(),
        ));
    }
    Ok(cloud)
}

/// Stream every point from a supported source file. The callback may stop the
/// read by returning an error; the caller controls any retained memory.
pub fn visit_points(
    path: impl AsRef<Path>,
    push: &mut impl FnMut(Point) -> Result<(), LoadError>,
) -> Result<(), LoadError> {
    let path = path.as_ref();
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "las" | "laz" => read_las(path, push),
        "ply" => ply::read(path, push),
        "obj" => mesh_points::read_obj(path, push),
        "off" => mesh_points::read_off(path, push),
        "stl" => mesh_points::read_stl(path, push),
        "ptx" => ptx::read(path, push),
        "pcd" => pcd::read(path, push),
        "dxf" => dxf::read(path, push),
        "e57" => e57_points::read(path, push),
        "xyz" | "asc" | "txt" | "csv" | "pts" => read_text(path, push, extension == "pts"),
        _ => Err(LoadError::UnsupportedFormat(extension)),
    }
}

struct Collector {
    points: Vec<Point>,
    ordinals: Vec<u64>,
    limit: usize,
    total: u64,
    bounds: Option<Bounds>,
    has_rgb: bool,
    has_intensity: bool,
    has_classification: bool,
    random_state: u64,
}

impl Collector {
    fn new(limit: usize) -> Self {
        Self {
            points: Vec::with_capacity(limit.min(100_000)),
            ordinals: Vec::with_capacity(limit.min(100_000)),
            limit,
            total: 0,
            bounds: None,
            has_rgb: false,
            has_intensity: false,
            has_classification: false,
            random_state: 0x9e37_79b9_7f4a_7c15,
        }
    }

    fn push(&mut self, point: Point) -> Result<(), LoadError> {
        if !point.xyz.iter().all(|value| value.is_finite()) {
            return Err(LoadError::InvalidData("non-finite coordinate".into()));
        }
        let ordinal = self.total;
        self.total += 1;
        match &mut self.bounds {
            Some(bounds) => bounds.include(point.xyz),
            None => {
                self.bounds = Some(Bounds {
                    min: point.xyz,
                    max: point.xyz,
                })
            }
        }
        self.has_rgb |= point.rgb.is_some();
        self.has_intensity |= point.intensity.is_some();
        self.has_classification |= point.classification.is_some();

        if self.points.len() < self.limit {
            self.points.push(point);
            self.ordinals.push(ordinal);
        } else {
            // Reservoir sampling gives every input point the same inclusion chance.
            self.random_state ^= self.random_state << 13;
            self.random_state ^= self.random_state >> 7;
            self.random_state ^= self.random_state << 17;
            let index = self.random_state % self.total;
            if index < self.limit as u64 {
                self.points[index as usize] = point;
                self.ordinals[index as usize] = ordinal;
            }
        }
        Ok(())
    }

    fn finish(self, path: PathBuf) -> Result<PointCloud, LoadError> {
        let bounds = self
            .bounds
            .ok_or_else(|| LoadError::InvalidData("file contains no points".into()))?;
        Ok(PointCloud {
            path,
            total_points: self.total,
            bounds,
            points: self.points,
            point_ordinals: self.ordinals,
            has_rgb: self.has_rgb,
            has_intensity: self.has_intensity,
            has_classification: self.has_classification,
            source_stamp: None,
        })
    }
}

fn read_las(
    path: &Path,
    push: &mut impl FnMut(Point) -> Result<(), LoadError>,
) -> Result<(), LoadError> {
    let mut reader = las::Reader::from_path(path)?;
    for point in reader.points() {
        push(convert_las_point(&point?))?;
    }
    Ok(())
}

fn convert_las_point(point: &las::Point) -> Point {
    let rgb = point.color.map(|color| {
        let channels = [color.red, color.green, color.blue];
        if channels.iter().all(|channel| *channel <= 255) {
            channels.map(|channel| channel as u8)
        } else {
            channels.map(|channel| (channel / 257) as u8)
        }
    });
    Point {
        xyz: [point.x, point.y, point.z],
        rgb,
        intensity: Some(point.intensity),
        classification: Some(u8::from(point.classification)),
    }
}

fn read_text(
    path: &Path,
    push: &mut impl FnMut(Point) -> Result<(), LoadError>,
    pts: bool,
) -> Result<(), LoadError> {
    let reader = BufReader::new(File::open(path)?);
    let mut first_content_line = true;
    let mut seen_points = false;
    for (line_number, line) in reader.lines().enumerate() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        if pts && first_content_line && line.parse::<u64>().is_ok() {
            first_content_line = false;
            continue;
        }
        first_content_line = false;
        let fields: Vec<&str> = line
            .split(|character: char| {
                character.is_ascii_whitespace() || character == ',' || character == ';'
            })
            .filter(|field| !field.is_empty())
            .collect();
        if fields.len() < 3 {
            return Err(LoadError::InvalidData(format!(
                "line {} has fewer than three coordinates",
                line_number + 1
            )));
        }
        let coordinates: Option<Vec<f64>> =
            fields[..3].iter().map(|field| field.parse().ok()).collect();
        let Some(coordinates) = coordinates else {
            // Allow a single header row such as x,y,z,r,g,b.
            if !seen_points && fields[0].eq_ignore_ascii_case("x") {
                continue;
            }
            return Err(LoadError::InvalidData(format!(
                "invalid coordinate on line {}",
                line_number + 1
            )));
        };
        let has_intensity = fields.len() == 4 || fields.len() >= 7;
        let intensity = if has_intensity {
            let raw = fields[3].parse::<f64>().map_err(|_| {
                LoadError::InvalidData(format!("invalid intensity on line {}", line_number + 1))
            })?;
            let normalized = if raw < 0.0 {
                (raw + 2048.0) / 4095.0
            } else if raw <= 1.0 {
                raw
            } else {
                raw / 255.0
            };
            Some((normalized.clamp(0.0, 1.0) * 65535.0).round() as u16)
        } else {
            None
        };
        let rgb = if fields.len() >= 6 {
            let start = if has_intensity { 4 } else { 3 };
            let channels: Vec<u8> = fields[start..start + 3]
                .iter()
                .map(|field| field.parse().ok())
                .collect::<Option<Vec<u8>>>()
                .ok_or_else(|| {
                    LoadError::InvalidData(format!("invalid RGB color on line {}", line_number + 1))
                })?;
            Some([channels[0], channels[1], channels[2]])
        } else {
            None
        };
        push(Point {
            xyz: [coordinates[0], coordinates[1], coordinates[2]],
            rgb,
            intensity,
            classification: None,
        })?;
        seen_points = true;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reservoir_keeps_bound_and_count() {
        let mut collector = Collector::new(3);
        for index in 0..10_000 {
            collector
                .push(Point {
                    xyz: [index as f64, 0.0, 0.0],
                    rgb: None,
                    intensity: None,
                    classification: None,
                })
                .unwrap();
        }
        let cloud = collector.finish(PathBuf::from("test.xyz")).unwrap();
        assert_eq!(cloud.total_points, 10_000);
        assert_eq!(cloud.points.len(), 3);
        assert_eq!(cloud.bounds.min, [0.0, 0.0, 0.0]);
        assert_eq!(cloud.bounds.max, [9_999.0, 0.0, 0.0]);
    }

    #[test]
    fn parses_text_header_and_color() {
        let path = std::env::temp_dir().join(format!("pointcloud-core-{}.csv", std::process::id()));
        std::fs::write(&path, "x,y,z,r,g,b\n1,2,3,12,34,56\n4,5,6,77,88,99\n").unwrap();
        let cloud = open(&path, 1).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(cloud.total_points, 2);
        assert_eq!(cloud.bounds.min, [1.0, 2.0, 3.0]);
        assert_eq!(cloud.bounds.max, [4.0, 5.0, 6.0]);
        assert!(cloud.has_rgb);
    }

    #[test]
    fn parses_pts_intensity_before_rgb() {
        let path = std::env::temp_dir().join(format!("pointcloud-core-{}.pts", std::process::id()));
        std::fs::write(&path, "1\n1 2 3 128 10 20 30\n").unwrap();
        let cloud = open(&path, 10).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(cloud.total_points, 1);
        assert_eq!(cloud.points[0].rgb, Some([10, 20, 30]));
        assert!(cloud.points[0].intensity.is_some());
    }

    #[test]
    fn reads_las_and_laz_color_and_classification() {
        for extension in ["las", "laz"] {
            let path = std::env::temp_dir().join(format!(
                "pointcloud-core-{}-{extension}.{extension}",
                std::process::id()
            ));
            let mut builder = las::Builder::from((1, 2));
            builder.point_format = las::point::Format::new(2).unwrap();
            let header = builder.into_header().unwrap();
            let mut writer = las::Writer::from_path(&path, header).unwrap();
            writer
                .write_point(las::Point {
                    x: 12.0,
                    y: 34.0,
                    z: 56.0,
                    color: Some(las::Color {
                        red: 65535,
                        green: 0,
                        blue: 32768,
                    }),
                    classification: las::point::Classification::Ground,
                    intensity: 1234,
                    ..las::Point::default()
                })
                .unwrap();
            drop(writer);

            let quick = open_las_header(&path).unwrap();
            assert_eq!(quick.total_points, 1);
            assert_eq!(quick.bounds.min, [12.0, 34.0, 56.0]);
            assert!(quick.points.is_empty());
            quick.validate_source().unwrap();

            let preview = open_las_preview(&path, 1).unwrap();
            assert!(quick.same_source_revision(&preview));
            assert_eq!(preview.total_points, 1);
            assert_eq!(preview.bounds, quick.bounds);
            assert_eq!(preview.points.len(), 1);

            let cloud = open(&path, 10).unwrap();
            assert!(cloud.same_source_revision(&quick));
            assert_eq!(cloud.total_points, 1);
            assert_eq!(cloud.points[0].xyz, [12.0, 34.0, 56.0]);
            assert_eq!(cloud.points[0].rgb, Some([255, 0, 127]));
            assert_eq!(cloud.points[0].classification, Some(2));
            assert_eq!(cloud.points[0].intensity, Some(1234));

            let exported = path.with_extension("ply");
            export_full(&cloud, &exported, ExportFormat::PlyBinary).unwrap();
            let reopened = open(&exported, 10).unwrap();
            assert_eq!(reopened.points[0].classification, Some(2));
            assert_eq!(reopened.points[0].intensity, Some(1234));
            assert_eq!(reopened.points[0].rgb, Some([255, 0, 127]));
            std::fs::remove_file(path).unwrap();
            std::fs::remove_file(exported).unwrap();
        }
    }

    #[test]
    fn las_preview_samples_across_entire_source() {
        let dir = tempfile::tempdir().unwrap();
        for extension in ["las", "laz"] {
            let path = dir.path().join(format!("spread.{extension}"));
            let header = las::Builder::from((1, 2)).into_header().unwrap();
            let mut writer = las::Writer::from_path(&path, header).unwrap();
            for x in 0..1_000 {
                writer
                    .write_point(las::Point {
                        x: f64::from(x),
                        ..las::Point::default()
                    })
                    .unwrap();
            }
            drop(writer);
            let preview = open_las_preview(&path, 40).unwrap();
            assert_eq!(preview.total_points, 1_000);
            assert_eq!(preview.points.len(), 40);
            assert_eq!(preview.point_ordinals.len(), preview.points.len());
            for (point, ordinal) in preview.points.iter().zip(&preview.point_ordinals) {
                if extension == "laz" {
                    assert_eq!(*ordinal, u64::MAX);
                } else {
                    assert_eq!(point.xyz[0] as u64, *ordinal);
                }
            }
            assert_eq!(preview.bounds.min[0], 0.0);
            assert_eq!(preview.bounds.max[0], 999.0);
            assert_eq!(preview.points.first().unwrap().xyz[0], 0.0);
            // LAZ chunk seeking may land a few points before the requested
            // ordinal; preview coverage still reaches the far end.
            assert!(preview.points.last().unwrap().xyz[0] > 900.0);
        }
    }
}
