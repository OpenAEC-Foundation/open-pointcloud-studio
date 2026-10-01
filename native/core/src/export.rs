use std::fmt;
use std::fs;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

use super::{visit_points, Bounds, LoadError, Point, PointCloud, SourceStamp};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Xyz,
    Pts,
    Csv,
    PlyAscii,
    PlyBinary,
    Las,
    Laz,
}

impl ExportFormat {
    pub const ALL: [Self; 7] = [
        Self::PlyBinary,
        Self::PlyAscii,
        Self::Las,
        Self::Laz,
        Self::Xyz,
        Self::Pts,
        Self::Csv,
    ];

    pub fn extension(self) -> &'static str {
        match self {
            Self::Xyz => "xyz",
            Self::Pts => "pts",
            Self::Csv => "csv",
            Self::PlyAscii | Self::PlyBinary => "ply",
            Self::Las => "las",
            Self::Laz => "laz",
        }
    }
}

impl fmt::Display for ExportFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Xyz => "XYZ",
            Self::Pts => "PTS",
            Self::Csv => "CSV",
            Self::PlyAscii => "PLY (ASCII)",
            Self::PlyBinary => "PLY (binary)",
            Self::Las => "LAS",
            Self::Laz => "LAZ",
        })
    }
}

/// Export the full source stream; the preview sample is never used as output.
/// The destination is replaced only after the entire export succeeds.
pub fn export_full(
    cloud: &PointCloud,
    destination: impl AsRef<Path>,
    format: ExportFormat,
) -> Result<(), LoadError> {
    let source_extension = cloud
        .path
        .extension()
        .and_then(|extension| extension.to_str());
    let source_is_las =
        source_extension.is_some_and(|extension| extension.eq_ignore_ascii_case("las"));
    let source_is_laz =
        source_extension.is_some_and(|extension| extension.eq_ignore_ascii_case("laz"));
    if matches!(
        (source_extension, format),
        (Some(extension), ExportFormat::Las) if extension.eq_ignore_ascii_case("las")
    ) || matches!(
        (source_extension, format),
        (Some(extension), ExportFormat::Laz) if extension.eq_ignore_ascii_case("laz")
    ) {
        return copy_full_source(cloud, destination.as_ref());
    }
    if (source_is_las || source_is_laz) && matches!(format, ExportFormat::Las | ExportFormat::Laz) {
        return reencode_las_full(cloud, destination.as_ref(), format);
    }
    export_where(cloud, destination, format, cloud.total_points, |_, _| true)
}

fn copy_full_source(cloud: &PointCloud, destination: &Path) -> Result<(), LoadError> {
    if destination == cloud.path
        || fs::canonicalize(destination).ok() == fs::canonicalize(&cloud.path).ok()
    {
        return Err(LoadError::InvalidData(
            "source and destination must differ".into(),
        ));
    }
    let expected_stamp = cloud
        .source_stamp
        .ok_or_else(|| LoadError::InvalidData("source identity is unavailable".into()))?;
    if SourceStamp::read(&cloud.path)? != expected_stamp {
        return Err(LoadError::InvalidData(
            "source changed since loading".into(),
        ));
    }
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temporary = tempfile::NamedTempFile::new_in(parent)?;
    fs::copy(&cloud.path, temporary.path())?;
    if SourceStamp::read(&cloud.path)? != expected_stamp {
        return Err(LoadError::InvalidData(
            "source changed during export".into(),
        ));
    }
    temporary
        .persist(destination)
        .map_err(|error| LoadError::Io(error.error))?;
    Ok(())
}

fn reencode_las_full(
    cloud: &PointCloud,
    destination: &Path,
    format: ExportFormat,
) -> Result<(), LoadError> {
    if destination == cloud.path
        || fs::canonicalize(destination).ok() == fs::canonicalize(&cloud.path).ok()
    {
        return Err(LoadError::InvalidData(
            "source and destination must differ".into(),
        ));
    }
    let expected_stamp = cloud
        .source_stamp
        .ok_or_else(|| LoadError::InvalidData("source identity is unavailable".into()))?;
    if SourceStamp::read(&cloud.path)? != expected_stamp {
        return Err(LoadError::InvalidData(
            "source changed since loading".into(),
        ));
    }
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temporary = tempfile::NamedTempFile::new_in(parent)?;
    let mut reader = las::Reader::from_path(&cloud.path)?;
    let mut builder = las::Builder::from(reader.header().clone());
    builder.point_format.is_compressed = format == ExportFormat::Laz;
    builder.vlrs.retain(|vlr| {
        !(vlr.record_id == 22204 && vlr.user_id.eq_ignore_ascii_case("laszip encoded"))
    });
    let mut writer = las::Writer::new(temporary.reopen()?, builder.into_header()?)?;
    let read_result = (|| {
        let mut count = 0u64;
        for point in reader.points() {
            writer.write_point(point?)?;
            count += 1;
        }
        Ok::<u64, LoadError>(count)
    })();
    let close_result = writer.close();
    if close_result.is_err() {
        // las-rs retries close in Drop and panics if it fails again.
        std::mem::forget(writer);
    } else {
        drop(writer);
    }
    let count = read_result?;
    close_result?;
    if count != cloud.total_points || SourceStamp::read(&cloud.path)? != expected_stamp {
        return Err(LoadError::InvalidData(
            "source changed during export".into(),
        ));
    }
    temporary
        .persist(destination)
        .map_err(|error| LoadError::Io(error.error))?;
    Ok(())
}

/// Export every source point inside an axis-aligned section box, including
/// points omitted from the bounded preview. Stream the source once and patch
/// the exact PLY/PTS count in the temporary output before atomically saving.
pub fn export_section(
    cloud: &PointCloud,
    destination: impl AsRef<Path>,
    format: ExportFormat,
    section: Bounds,
) -> Result<u64, LoadError> {
    export_section_where(cloud, destination, format, section, |_, _| true)
}

/// Export a section of the full source stream with an additional edit filter.
pub fn export_section_where(
    cloud: &PointCloud,
    destination: impl AsRef<Path>,
    format: ExportFormat,
    section: Bounds,
    mut include: impl FnMut(u64, &Point) -> bool,
) -> Result<u64, LoadError> {
    if (0..3).any(|axis| {
        !section.min[axis].is_finite()
            || !section.max[axis].is_finite()
            || section.min[axis] > section.max[axis]
    }) {
        return Err(LoadError::InvalidData("invalid section bounds".into()));
    }
    export_map_count(cloud, destination, format, None, |ordinal, point| {
        (section_contains(section, point.xyz) && include(ordinal, &point)).then_some(point)
    })
}

fn section_contains(section: Bounds, xyz: [f64; 3]) -> bool {
    (0..3).all(|axis| xyz[axis] >= section.min[axis] && xyz[axis] <= section.max[axis])
}

/// Export a full-resolution subset selected by source-point ordinal.
/// `expected_count` is checked so headers cannot silently disagree with data.
pub fn export_where(
    cloud: &PointCloud,
    destination: impl AsRef<Path>,
    format: ExportFormat,
    expected_count: u64,
    mut include: impl FnMut(u64, &Point) -> bool,
) -> Result<(), LoadError> {
    export_map(
        cloud,
        destination,
        format,
        expected_count,
        |ordinal, point| include(ordinal, &point).then_some(point),
    )
}

/// Keep an evenly distributed, exact percentage of the remaining source stream.
/// The source ordinal filter runs first, so deleted points do not consume a slot.
pub fn export_thin_percent_where(
    cloud: &PointCloud,
    destination: impl AsRef<Path>,
    format: ExportFormat,
    remaining: u64,
    percent: u8,
    mut include: impl FnMut(u64, &Point) -> bool,
) -> Result<u64, LoadError> {
    if !(1..=100).contains(&percent) || remaining > cloud.total_points {
        return Err(LoadError::InvalidData(
            "thin percentage or remaining count is invalid".into(),
        ));
    }
    let target = if remaining == 0 {
        0
    } else {
        u64::try_from((u128::from(remaining) * u128::from(percent) + 50) / 100)
            .unwrap_or(u64::MAX)
            .max(1)
            .min(remaining)
    };
    let mut seen = 0u64;
    export_where(cloud, destination, format, target, |ordinal, point| {
        if !include(ordinal, point) || remaining == 0 {
            return false;
        }
        let before = u128::from(seen) * u128::from(target) / u128::from(remaining);
        seen += 1;
        let after = u128::from(seen) * u128::from(target) / u128::from(remaining);
        after > before
    })?;
    Ok(target)
}

/// Export a transformed or filtered full-resolution stream.
pub fn export_map(
    cloud: &PointCloud,
    destination: impl AsRef<Path>,
    format: ExportFormat,
    expected_count: u64,
    map: impl FnMut(u64, Point) -> Option<Point>,
) -> Result<(), LoadError> {
    export_map_count(cloud, destination, format, Some(expected_count), map).map(|_| ())
}

fn export_map_count(
    cloud: &PointCloud,
    destination: impl AsRef<Path>,
    format: ExportFormat,
    expected_count: Option<u64>,
    mut map: impl FnMut(u64, Point) -> Option<Point>,
) -> Result<u64, LoadError> {
    let destination = destination.as_ref();
    if destination == cloud.path
        || fs::canonicalize(destination).ok() == fs::canonicalize(&cloud.path).ok()
    {
        return Err(LoadError::InvalidData(
            "source and destination must differ".into(),
        ));
    }
    let expected_stamp = cloud
        .source_stamp
        .ok_or_else(|| LoadError::InvalidData("source identity is unavailable".into()))?;
    if SourceStamp::read(&cloud.path)? != expected_stamp {
        return Err(LoadError::InvalidData(
            "source changed since loading".into(),
        ));
    }
    if matches!(format, ExportFormat::Las | ExportFormat::Laz) {
        return export_las_map_count(
            cloud,
            destination,
            format,
            expected_count,
            expected_stamp,
            map,
        );
    }
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    let mut source_count = 0u64;
    let mut written_count = 0u64;
    let count_placeholder = "00000000000000000000";
    let count_text = expected_count.map_or_else(|| count_placeholder.to_owned(), |n| n.to_string());
    let count_offset = if expected_count.is_none() {
        match format {
            ExportFormat::Pts => Some(0),
            ExportFormat::PlyAscii | ExportFormat::PlyBinary => {
                let encoding = if format == ExportFormat::PlyAscii {
                    "ascii"
                } else {
                    "binary_little_endian"
                };
                Some(format!("ply\nformat {encoding} 1.0\nelement vertex ").len() as u64)
            }
            ExportFormat::Xyz | ExportFormat::Csv | ExportFormat::Las | ExportFormat::Laz => None,
        }
    } else {
        None
    };
    {
        let mut writer = BufWriter::new(temporary.as_file_mut());
        match format {
            ExportFormat::Pts => writeln!(writer, "{count_text}")?,
            ExportFormat::Csv => {
                write!(writer, "x,y,z")?;
                if cloud.has_intensity {
                    write!(writer, ",intensity")?;
                }
                if cloud.has_rgb {
                    write!(writer, ",red,green,blue")?;
                }
                if cloud.has_classification {
                    write!(writer, ",classification")?;
                }
                writeln!(writer)?;
            }
            ExportFormat::PlyAscii | ExportFormat::PlyBinary => {
                write_ply_header(&mut writer, cloud, format, &count_text)?
            }
            ExportFormat::Xyz | ExportFormat::Las | ExportFormat::Laz => {}
        }

        visit_points(&cloud.path, &mut |point| {
            let ordinal = source_count;
            source_count += 1;
            let Some(point) = map(ordinal, point) else {
                return Ok(());
            };
            if !point.xyz.iter().all(|value| value.is_finite()) {
                return Err(LoadError::InvalidData(
                    "transform produced non-finite coordinates".into(),
                ));
            }
            written_count += 1;
            match format {
                ExportFormat::Xyz | ExportFormat::Pts | ExportFormat::Csv => {
                    write_text_point(&mut writer, point, cloud, format)?;
                }
                ExportFormat::PlyAscii => write_ply_ascii(&mut writer, point, cloud)?,
                ExportFormat::PlyBinary => write_ply_binary(&mut writer, point, cloud)?,
                ExportFormat::Las | ExportFormat::Laz => unreachable!(),
            }
            Ok(())
        })?;
        writer.flush()?;
    }

    if source_count != cloud.total_points {
        return Err(LoadError::InvalidData(format!(
            "source changed since loading (expected {} points, found {source_count})",
            cloud.total_points
        )));
    }
    if let Some(expected_count) = expected_count {
        if written_count != expected_count {
            return Err(LoadError::InvalidData(format!(
                "selection changed during export (expected {expected_count} points, wrote {written_count})"
            )));
        }
    }
    if let Some(offset) = count_offset {
        temporary.as_file_mut().seek(SeekFrom::Start(offset))?;
        write!(temporary.as_file_mut(), "{written_count:020}")?;
    }
    if SourceStamp::read(&cloud.path)? != expected_stamp {
        return Err(LoadError::InvalidData(
            "source changed during export".into(),
        ));
    }
    temporary
        .persist(destination)
        .map_err(|error| LoadError::Io(error.error))?;
    Ok(written_count)
}

fn export_las_map_count(
    cloud: &PointCloud,
    destination: &Path,
    format: ExportFormat,
    expected_count: Option<u64>,
    expected_stamp: SourceStamp,
    mut map: impl FnMut(u64, Point) -> Option<Point>,
) -> Result<u64, LoadError> {
    use las::{point::Format, Builder, Color, Point as LasPoint, Transform, Vector, Writer};

    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temporary = tempfile::NamedTempFile::new_in(parent)?;
    let source_header = cloud
        .path
        .extension()
        .and_then(|extension| extension.to_str())
        .filter(|extension| {
            extension.eq_ignore_ascii_case("las") || extension.eq_ignore_ascii_case("laz")
        })
        .map(|_| las::Reader::from_path(&cloud.path).map(|reader| reader.header().clone()))
        .transpose()?;
    let mut builder = Builder::from((1, 4));
    builder.generating_software = "Open Pointcloud Studio".into();
    let extended = cloud.total_points > u64::from(u32::MAX)
        || source_header
            .as_ref()
            .is_some_and(|header| header.point_format().is_extended);
    builder.point_format = Format::new(match (extended, cloud.has_rgb) {
        (false, false) => 0,
        (false, true) => 2,
        (true, false) => 6,
        (true, true) => 7,
    })?;
    builder.point_format.is_compressed = format == ExportFormat::Laz;
    if let Some(header) = &source_header {
        builder.file_source_id = header.file_source_id();
        builder.system_identifier = header.system_identifier().into();
        builder.has_wkt_crs = header.has_wkt_crs();
        builder.vlrs = header
            .vlrs()
            .iter()
            .filter(|vlr| vlr.user_id == "LASF_Projection")
            .cloned()
            .collect();
        builder.evlrs = header
            .evlrs()
            .iter()
            .filter(|vlr| vlr.user_id == "LASF_Projection")
            .cloned()
            .collect();
    }
    let transforms: [Transform; 3] = std::array::from_fn(|axis| {
        let center = cloud.bounds.center()[axis];
        let half_extent = (cloud.bounds.max[axis] - center)
            .abs()
            .max((cloud.bounds.min[axis] - center).abs());
        Transform {
            scale: 0.001_f64.max(half_extent / (f64::from(i32::MAX) * 0.9)),
            offset: center,
        }
    });
    builder.transforms = Vector {
        x: transforms[0],
        y: transforms[1],
        z: transforms[2],
    };
    let mut writer = Writer::new(temporary.reopen()?, builder.into_header()?)?;
    let mut source_count = 0u64;
    let mut written_count = 0u64;
    let stream_result = visit_points(&cloud.path, &mut |point| {
        let ordinal = source_count;
        source_count += 1;
        let Some(point) = map(ordinal, point) else {
            return Ok(());
        };
        if !point.xyz.iter().all(|value| value.is_finite()) {
            return Err(LoadError::InvalidData(
                "transform produced non-finite coordinates".into(),
            ));
        }
        for (axis, transform) in transforms.iter().enumerate() {
            transform.inverse(point.xyz[axis])?;
        }
        let class_code = point.classification.unwrap_or(1);
        let overlap = class_code == 12;
        let classification = las::point::Classification::new(if overlap { 1 } else { class_code })?;
        let color = point.rgb.map(|rgb| {
            Color::new(
                u16::from(rgb[0]) * 257,
                u16::from(rgb[1]) * 257,
                u16::from(rgb[2]) * 257,
            )
        });
        writer.write_point(LasPoint {
            x: point.xyz[0],
            y: point.xyz[1],
            z: point.xyz[2],
            intensity: point.intensity.unwrap_or(0),
            return_number: 1,
            number_of_returns: 1,
            classification,
            is_overlap: overlap,
            gps_time: extended.then_some(0.0),
            color: if cloud.has_rgb {
                Some(color.unwrap_or_else(|| Color::new(0, 0, 0)))
            } else {
                None
            },
            ..LasPoint::default()
        })?;
        written_count += 1;
        Ok(())
    });
    let close_result = writer.close();
    if close_result.is_err() {
        // las-rs retries close in Drop and panics if it fails again.
        std::mem::forget(writer);
    } else {
        drop(writer);
    }
    stream_result?;
    close_result?;
    if source_count != cloud.total_points {
        return Err(LoadError::InvalidData(format!(
            "source changed since loading (expected {} points, found {source_count})",
            cloud.total_points
        )));
    }
    if let Some(expected) = expected_count {
        if expected != written_count {
            return Err(LoadError::InvalidData(format!(
                "selection changed during export (expected {expected} points, wrote {written_count})"
            )));
        }
    }
    if SourceStamp::read(&cloud.path)? != expected_stamp {
        return Err(LoadError::InvalidData(
            "source changed during export".into(),
        ));
    }
    temporary
        .persist(destination)
        .map_err(|error| LoadError::Io(error.error))?;
    Ok(written_count)
}

/// Transform coordinates around the cloud center, writing a new file.
pub fn export_affine(
    cloud: &PointCloud,
    destination: impl AsRef<Path>,
    format: ExportFormat,
    translation: [f64; 3],
    scale: f64,
) -> Result<(), LoadError> {
    export_affine_where(
        cloud,
        destination,
        format,
        translation,
        scale,
        cloud.total_points,
        |_, _| true,
    )
}

/// Apply an affine transform to an exact full-resolution subset.
pub fn export_affine_where(
    cloud: &PointCloud,
    destination: impl AsRef<Path>,
    format: ExportFormat,
    translation: [f64; 3],
    scale: f64,
    expected_count: u64,
    mut include: impl FnMut(u64, &Point) -> bool,
) -> Result<(), LoadError> {
    if !scale.is_finite() || scale <= 0.0 {
        return Err(LoadError::InvalidData(
            "invalid transform parameters".into(),
        ));
    }
    export_affine_axes_where(
        cloud,
        destination,
        format,
        translation,
        [scale; 3],
        expected_count,
        &mut include,
    )
}

/// Transform each coordinate axis independently around the source bounds center.
pub fn export_affine_axes(
    cloud: &PointCloud,
    destination: impl AsRef<Path>,
    format: ExportFormat,
    translation: [f64; 3],
    scale: [f64; 3],
) -> Result<(), LoadError> {
    export_affine_axes_where(
        cloud,
        destination,
        format,
        translation,
        scale,
        cloud.total_points,
        |_, _| true,
    )
}

/// Apply per-axis scale and translation to an exact full-resolution subset.
pub fn export_affine_axes_where(
    cloud: &PointCloud,
    destination: impl AsRef<Path>,
    format: ExportFormat,
    translation: [f64; 3],
    scale: [f64; 3],
    expected_count: u64,
    mut include: impl FnMut(u64, &Point) -> bool,
) -> Result<(), LoadError> {
    if !translation.iter().all(|value| value.is_finite())
        || !scale.iter().all(|value| value.is_finite())
    {
        return Err(LoadError::InvalidData(
            "invalid transform parameters".into(),
        ));
    }
    let center = cloud.bounds.center();
    export_map(
        cloud,
        destination,
        format,
        expected_count,
        |ordinal, mut point| {
            if !include(ordinal, &point) {
                return None;
            }
            for axis in 0..3 {
                point.xyz[axis] = center[axis]
                    + (point.xyz[axis] - center[axis]) * scale[axis]
                    + translation[axis];
            }
            Some(point)
        },
    )
}

fn write_text_point(
    writer: &mut impl Write,
    point: Point,
    cloud: &PointCloud,
    format: ExportFormat,
) -> Result<(), LoadError> {
    let separator = if format == ExportFormat::Csv {
        ','
    } else {
        ' '
    };
    write!(
        writer,
        "{}{}{}{}{}",
        point.xyz[0], separator, point.xyz[1], separator, point.xyz[2]
    )?;
    if cloud.has_intensity {
        let normalized = f64::from(point.intensity.unwrap_or(0)) / 65535.0;
        write!(writer, "{separator}{normalized:.10}")?;
    }
    if cloud.has_rgb {
        let [red, green, blue] = point.rgb.unwrap_or([0, 0, 0]);
        write!(
            writer,
            "{separator}{red}{separator}{green}{separator}{blue}"
        )?;
    }
    if format == ExportFormat::Csv && cloud.has_classification {
        write!(writer, "{separator}{}", point.classification.unwrap_or(0))?;
    }
    writeln!(writer)?;
    Ok(())
}

fn write_ply_header(
    writer: &mut impl Write,
    cloud: &PointCloud,
    format: ExportFormat,
    point_count: &str,
) -> Result<(), LoadError> {
    let encoding = if format == ExportFormat::PlyAscii {
        "ascii"
    } else {
        "binary_little_endian"
    };
    writeln!(
        writer,
        "ply\nformat {encoding} 1.0\nelement vertex {}",
        point_count
    )?;
    writeln!(
        writer,
        "property double x\nproperty double y\nproperty double z"
    )?;
    if cloud.has_rgb {
        writeln!(
            writer,
            "property uchar red\nproperty uchar green\nproperty uchar blue"
        )?;
    }
    if cloud.has_intensity {
        writeln!(writer, "property ushort intensity")?;
    }
    if cloud.has_classification {
        writeln!(writer, "property uchar classification")?;
    }
    writeln!(writer, "end_header")?;
    Ok(())
}

fn write_ply_ascii(
    writer: &mut impl Write,
    point: Point,
    cloud: &PointCloud,
) -> Result<(), LoadError> {
    write!(writer, "{} {} {}", point.xyz[0], point.xyz[1], point.xyz[2])?;
    if cloud.has_rgb {
        let [red, green, blue] = point.rgb.unwrap_or([0, 0, 0]);
        write!(writer, " {red} {green} {blue}")?;
    }
    if cloud.has_intensity {
        write!(writer, " {}", point.intensity.unwrap_or(0))?;
    }
    if cloud.has_classification {
        write!(writer, " {}", point.classification.unwrap_or(0))?;
    }
    writeln!(writer)?;
    Ok(())
}

fn write_ply_binary(
    writer: &mut impl Write,
    point: Point,
    cloud: &PointCloud,
) -> Result<(), LoadError> {
    for coordinate in point.xyz {
        writer.write_all(&coordinate.to_le_bytes())?;
    }
    if cloud.has_rgb {
        writer.write_all(&point.rgb.unwrap_or([0, 0, 0]))?;
    }
    if cloud.has_intensity {
        writer.write_all(&point.intensity.unwrap_or(0).to_le_bytes())?;
    }
    if cloud.has_classification {
        writer.write_all(&[point.classification.unwrap_or(0)])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::open;

    #[test]
    fn exports_full_source_not_sample() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.xyz");
        fs::write(&source, "1 2 3 10 20 30\n4 5 6 40 50 60\n7 8 9 70 80 90\n").unwrap();
        let cloud = open(&source, 1).unwrap();
        assert_eq!(cloud.points.len(), 1);
        for format in [
            ExportFormat::Xyz,
            ExportFormat::Pts,
            ExportFormat::Csv,
            ExportFormat::PlyAscii,
            ExportFormat::PlyBinary,
        ] {
            let destination = dir
                .path()
                .join(format!("output-{format:?}.{}", format.extension()));
            export_full(&cloud, &destination, format).unwrap();
            let reopened = open(&destination, 10).unwrap();
            assert_eq!(reopened.total_points, 3);
            assert_eq!(reopened.bounds, cloud.bounds);
            assert!(reopened.has_rgb);
        }
    }

    #[test]
    fn las_and_laz_export_stream_full_source_with_attributes_and_selection() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("survey.ply");
        fs::write(
            &source,
            "ply\nformat ascii 1.0\nelement vertex 3\nproperty double x\nproperty double y\nproperty double z\nproperty ushort intensity\nproperty uchar red\nproperty uchar green\nproperty uchar blue\nproperty uchar classification\nend_header\n207000.001 474000.002 1.234 1234 10 20 30 2\n207001.003 474001.004 2.345 5678 40 50 60 6\n207002.005 474002.006 3.456 9012 70 80 90 5\n",
        )
        .unwrap();
        let cloud = open(&source, 1).unwrap();
        assert_eq!(cloud.points.len(), 1);
        for format in [ExportFormat::Las, ExportFormat::Laz] {
            let output = dir.path().join(format!("subset.{}", format.extension()));
            export_where(&cloud, &output, format, 2, |ordinal, _| ordinal != 1).unwrap();
            let reopened = open(&output, 1).unwrap();
            assert_eq!(reopened.total_points, 2);
            assert_eq!(reopened.points.len(), 1);
            let mut points = Vec::new();
            visit_points(&output, &mut |point| {
                points.push(point);
                Ok(())
            })
            .unwrap();
            assert_eq!(points.len(), 2);
            for (actual, expected) in points.iter().zip([
                ([207000.001, 474000.002, 1.234], [10, 20, 30], 1234, 2),
                ([207002.005, 474002.006, 3.456], [70, 80, 90], 9012, 5),
            ]) {
                for axis in 0..3 {
                    assert!((actual.xyz[axis] - expected.0[axis]).abs() < 0.000_51);
                }
                assert_eq!(actual.rgb, Some(expected.1));
                assert_eq!(actual.intensity, Some(expected.2));
                assert_eq!(actual.classification, Some(expected.3));
            }
        }
    }

    #[test]
    fn laz_export_keeps_las_projection_records() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.las");
        let mut builder = las::Builder::from((1, 4));
        builder.point_format = las::point::Format::new(3).unwrap();
        builder.vlrs.push(las::Vlr {
            user_id: "LASF_Projection".into(),
            record_id: 34735,
            description: "GeoKeyDirectoryTag".into(),
            data: vec![1, 0, 1, 0],
        });
        let mut writer = las::Writer::new(
            std::fs::File::create(&source).unwrap(),
            builder.into_header().unwrap(),
        )
        .unwrap();
        writer
            .write_point(las::Point {
                x: 1.0,
                y: 2.0,
                z: 3.0,
                return_number: 2,
                number_of_returns: 3,
                gps_time: Some(123.456),
                color: Some(las::Color::new(12_345, 23_456, 34_567)),
                ..las::Point::default()
            })
            .unwrap();
        writer.close().unwrap();
        drop(writer);

        let cloud = open(&source, 1).unwrap();
        let exact_copy = dir.path().join("exact-copy.las");
        export_full(&cloud, &exact_copy, ExportFormat::Las).unwrap();
        assert_eq!(fs::read(&exact_copy).unwrap(), fs::read(&source).unwrap());
        let destination = dir.path().join("converted.laz");
        export_full(&cloud, &destination, ExportFormat::Laz).unwrap();
        let mut output = las::Reader::from_path(&destination).unwrap();
        assert_eq!(output.header().number_of_points(), 1);
        assert!(output
            .header()
            .vlrs()
            .iter()
            .any(|vlr| vlr.user_id == "LASF_Projection"
                && vlr.record_id == 34735
                && vlr.data == [1, 0, 1, 0]));
        let original_point = las::Reader::from_path(&source)
            .unwrap()
            .points()
            .next()
            .unwrap()
            .unwrap();
        let converted_point = output.points().next().unwrap().unwrap();
        assert_eq!(converted_point, original_point);
        let laz_cloud = open(&destination, 1).unwrap();
        let laz_copy = dir.path().join("exact-copy.laz");
        export_full(&laz_cloud, &laz_copy, ExportFormat::Laz).unwrap();
        assert_eq!(
            fs::read(&laz_copy).unwrap(),
            fs::read(&destination).unwrap()
        );
        let restored = dir.path().join("restored.las");
        export_full(&laz_cloud, &restored, ExportFormat::Las).unwrap();
        let restored_point = las::Reader::from_path(&restored)
            .unwrap()
            .points()
            .next()
            .unwrap()
            .unwrap();
        assert_eq!(restored_point, original_point);
    }

    #[test]
    fn las_export_does_not_publish_unrepresentable_coordinates() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("survey.xyz");
        fs::write(&source, "0 0 0\n1 1 1\n").unwrap();
        let cloud = open(&source, 1).unwrap();
        let destination = dir.path().join("invalid.las");
        let result = export_map(
            &cloud,
            &destination,
            ExportFormat::Las,
            2,
            |_, mut point| {
                point.xyz[0] += 1e12;
                Some(point)
            },
        );
        assert!(result.is_err());
        assert!(!destination.exists());
    }

    #[test]
    fn affine_export_transforms_full_source() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.xyz");
        fs::write(&source, "1 2 3\n3 4 5\n").unwrap();
        let cloud = open(&source, 1).unwrap();
        let destination = dir.path().join("moved.ply");
        export_affine(
            &cloud,
            &destination,
            ExportFormat::PlyBinary,
            [10.0, 0.0, -2.0],
            2.0,
        )
        .unwrap();
        let transformed = open(destination, 2).unwrap();
        assert_eq!(transformed.total_points, 2);
        assert_eq!(transformed.bounds.min, [10.0, 1.0, 0.0]);
        assert_eq!(transformed.bounds.max, [14.0, 5.0, 4.0]);
        let filtered = dir.path().join("moved-edited.ply");
        export_affine_where(
            &cloud,
            &filtered,
            ExportFormat::PlyBinary,
            [10.0, 0.0, -2.0],
            2.0,
            1,
            |ordinal, _| ordinal == 1,
        )
        .unwrap();
        let edited = open(filtered, 2).unwrap();
        assert_eq!(edited.total_points, 1);
        assert_eq!(edited.points[0].xyz, [14.0, 5.0, 4.0]);
    }

    #[test]
    fn per_axis_scale_streams_and_filters_full_source() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.xyz");
        fs::write(&source, "1 2 3\n3 4 5\n").unwrap();
        let cloud = open(&source, 1).unwrap();
        let all_path = dir.path().join("scaled.ply");
        export_affine_axes(
            &cloud,
            &all_path,
            ExportFormat::PlyBinary,
            [10.0, 0.0, -2.0],
            [2.0, -1.0, 0.5],
        )
        .unwrap();
        let all = open(&all_path, 2).unwrap();
        assert_eq!(all.total_points, 2);
        assert_eq!(all.bounds.min, [10.0, 2.0, 1.5]);
        assert_eq!(all.bounds.max, [14.0, 4.0, 2.5]);

        let edited_path = dir.path().join("scaled-edited.ply");
        export_affine_axes_where(
            &cloud,
            &edited_path,
            ExportFormat::PlyBinary,
            [10.0, 0.0, -2.0],
            [2.0, -1.0, 0.5],
            1,
            |ordinal, _| ordinal == 1,
        )
        .unwrap();
        let edited = open(edited_path, 2).unwrap();
        assert_eq!(edited.total_points, 1);
        assert_eq!(edited.points[0].xyz, [14.0, 2.0, 2.5]);

        let invalid_path = dir.path().join("invalid.ply");
        assert!(export_affine_axes(
            &cloud,
            &invalid_path,
            ExportFormat::PlyBinary,
            [0.0; 3],
            [1.0, f64::NAN, 1.0],
        )
        .is_err());
        assert!(!invalid_path.exists());
    }

    #[test]
    fn thin_percent_uses_exact_remaining_count_from_full_source() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.xyz");
        fs::write(
            &source,
            (0..10).map(|x| format!("{x} 0 0\n")).collect::<String>(),
        )
        .unwrap();
        let cloud = open(&source, 1).unwrap();

        let output = dir.path().join("thinned.ply");
        let count =
            export_thin_percent_where(&cloud, &output, ExportFormat::PlyBinary, 10, 30, |_, _| {
                true
            })
            .unwrap();
        let thinned = open(&output, 10).unwrap();
        assert_eq!(count, 3);
        assert_eq!(thinned.total_points, 3);
        assert_eq!(
            thinned.points.iter().map(|p| p.xyz[0]).collect::<Vec<_>>(),
            [3.0, 6.0, 9.0]
        );

        let edited_output = dir.path().join("thinned-edited.ply");
        let count = export_thin_percent_where(
            &cloud,
            &edited_output,
            ExportFormat::PlyBinary,
            5,
            40,
            |ordinal, _| ordinal % 2 == 1,
        )
        .unwrap();
        let edited = open(edited_output, 10).unwrap();
        assert_eq!(count, 2);
        assert_eq!(
            edited.points.iter().map(|p| p.xyz[0]).collect::<Vec<_>>(),
            [5.0, 9.0]
        );

        let invalid_output = dir.path().join("invalid.ply");
        assert!(export_thin_percent_where(
            &cloud,
            &invalid_output,
            ExportFormat::PlyBinary,
            10,
            0,
            |_, _| true,
        )
        .is_err());
        assert!(!invalid_output.exists());
    }

    #[test]
    fn refuses_changed_source_without_creating_output() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.xyz");
        let destination = dir.path().join("output.ply");
        fs::write(&source, "1 2 3\n").unwrap();
        let cloud = open(&source, 10).unwrap();
        fs::write(&source, "1 2 3\n4 5 6\n").unwrap();
        assert!(export_full(&cloud, &destination, ExportFormat::PlyBinary).is_err());
        assert!(!destination.exists());
    }

    #[test]
    fn exports_exact_subset_with_matching_header() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.xyz");
        fs::write(&source, "1 2 3\n4 5 6\n7 8 9\n").unwrap();
        let cloud = open(&source, 1).unwrap();
        let destination = dir.path().join("selected.ply");
        export_where(
            &cloud,
            &destination,
            ExportFormat::PlyBinary,
            2,
            |ordinal, _| ordinal != 1,
        )
        .unwrap();
        let selected = open(&destination, 10).unwrap();
        assert_eq!(selected.total_points, 2);
        assert_eq!(selected.bounds.min, [1.0, 2.0, 3.0]);
        assert_eq!(selected.bounds.max, [7.0, 8.0, 9.0]);
    }

    #[test]
    fn section_export_reads_full_source_and_writes_exact_header() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("section.xyz");
        fs::write(&source, "1 2 3\n4 5 6\n7 8 9\n10 11 12\n").unwrap();
        let cloud = open(&source, 1).unwrap();
        let section = Bounds {
            min: [4.0, 5.0, 6.0],
            max: [7.0, 8.0, 9.0],
        };
        for format in ExportFormat::ALL {
            let destination = dir
                .path()
                .join(format!("section-{format:?}.{}", format.extension()));
            assert_eq!(
                export_section(&cloud, &destination, format, section).unwrap(),
                2
            );
            let exported = open(destination, 10).unwrap();
            assert_eq!(exported.total_points, 2);
            assert_eq!(exported.bounds, section);
        }
        let edited = dir.path().join("section-edited.ply");
        assert_eq!(
            export_section_where(
                &cloud,
                &edited,
                ExportFormat::PlyBinary,
                section,
                |ordinal, _| ordinal != 1,
            )
            .unwrap(),
            1
        );
        let reopened = open(edited, 10).unwrap();
        assert_eq!(reopened.total_points, 1);
        assert_eq!(reopened.points[0].xyz, [7.0, 8.0, 9.0]);
    }

    #[test]
    fn section_export_rejects_invalid_bounds_without_output() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("section.xyz");
        let destination = dir.path().join("section.pts");
        fs::write(&source, "1 2 3\n").unwrap();
        let cloud = open(&source, 1).unwrap();
        assert!(export_section(
            &cloud,
            &destination,
            ExportFormat::Pts,
            Bounds {
                min: [2.0, 0.0, 0.0],
                max: [1.0, 9.0, 9.0],
            }
        )
        .is_err());
        assert!(!destination.exists());
    }
}
