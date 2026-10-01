use std::fmt;
use std::fs;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

use rayon::prelude::*;

use super::{
    convert_las_point, e57_points, visit_points, Bounds, LoadError, Point, PointCloud, SourceStamp,
};

// The LAZ compressor parallelizes only when a write contains multiple chunks.
// Eight default 50,000-point chunks keep memory bounded and can use eight cores.
const PARALLEL_LAZ_BATCH_POINTS: usize = 400_000;
const LAS_BATCH_POINTS: usize = 16_384;
const PARALLEL_FORMAT_BATCH_POINTS: usize = 65_536;
const FORMAT_CHUNK_POINTS: usize = 4_096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Xyz,
    Pts,
    Csv,
    PlyAscii,
    PlyBinary,
    Las,
    Laz,
    E57,
}

impl ExportFormat {
    pub const ALL: [Self; 8] = [
        Self::PlyBinary,
        Self::PlyAscii,
        Self::Las,
        Self::Laz,
        Self::E57,
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
            Self::E57 => "e57",
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
            Self::E57 => "E57",
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
    ) || matches!(
        (source_extension, format),
        (Some(extension), ExportFormat::E57) if extension.eq_ignore_ascii_case("e57")
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
    let options = las::WriterOptions::default().with_laz_parallelism(las::LazParallelism::Yes);
    let mut writer =
        las::Writer::with_options(temporary.reopen()?, builder.into_header()?, options)?;
    let read_result = (|| {
        let mut count = 0u64;
        let batch_limit = if format == ExportFormat::Laz {
            PARALLEL_LAZ_BATCH_POINTS
        } else {
            LAS_BATCH_POINTS
        };
        let mut batch = Vec::with_capacity(batch_limit);
        loop {
            batch.clear();
            let read = reader.read_points_into(batch_limit as u64, &mut batch)?;
            if read == 0 {
                break;
            }
            writer.write_points(&batch)?;
            count += read;
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
    if format == ExportFormat::E57 && source_is_e57(cloud) {
        return export_e57_filtered_count(
            cloud,
            destination.as_ref(),
            None,
            [0.0; 3],
            &mut |ordinal, point| section_contains(section, point.xyz) && include(ordinal, point),
        );
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
    if format == ExportFormat::E57 && source_is_e57(cloud) {
        export_e57_filtered_count(
            cloud,
            destination.as_ref(),
            Some(expected_count),
            [0.0; 3],
            &mut include,
        )?;
        return Ok(());
    }
    export_map(
        cloud,
        destination,
        format,
        expected_count,
        |ordinal, point| include(ordinal, &point).then_some(point),
    )
}

fn source_is_e57(cloud: &PointCloud) -> bool {
    cloud
        .path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("e57"))
}

/// Preserve E57 scan identities and raw point fields while translating all
/// scanner poses. Returns `None` when a source scan has no pose, so callers
/// can use a coordinate-writing export instead of inventing a station.
pub fn export_e57_translated_where(
    cloud: &PointCloud,
    destination: impl AsRef<Path>,
    expected_count: Option<u64>,
    translation: [f64; 3],
    mut include: impl FnMut(u64, &Point) -> bool,
) -> Result<Option<u64>, LoadError> {
    if !source_is_e57(cloud) {
        return Ok(None);
    }
    if !translation.iter().all(|value| value.is_finite()) {
        return Err(LoadError::InvalidData("non-finite E57 translation".into()));
    }
    cloud.validate_source()?;
    let reader = e57::E57Reader::from_file(&cloud.path)?;
    if reader
        .pointclouds()
        .iter()
        .any(|scan| scan.transform.is_none())
    {
        return Ok(None);
    }
    export_e57_filtered_count(
        cloud,
        destination.as_ref(),
        expected_count,
        translation,
        &mut include,
    )
    .map(Some)
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

/// Stream a mapped subset when its exact output count is only known after
/// evaluating the world-space filter. The writer backfills count headers.
pub fn export_map_auto_count(
    cloud: &PointCloud,
    destination: impl AsRef<Path>,
    format: ExportFormat,
    map: impl FnMut(u64, Point) -> Option<Point>,
) -> Result<u64, LoadError> {
    export_map_count(cloud, destination, format, None, map)
}

fn export_map_count(
    cloud: &PointCloud,
    destination: impl AsRef<Path>,
    format: ExportFormat,
    expected_count: Option<u64>,
    mut map: impl FnMut(u64, Point) -> Option<Point>,
) -> Result<u64, LoadError> {
    export_map_count_inner(
        cloud,
        destination.as_ref(),
        format,
        expected_count,
        &mut map,
    )
}

// This scan is intentionally compiled in the optimized core, including when
// a native desktop callback supplies the edit/selection predicate.
fn export_map_count_inner(
    cloud: &PointCloud,
    destination: &Path,
    format: ExportFormat,
    expected_count: Option<u64>,
    map: &mut dyn FnMut(u64, Point) -> Option<Point>,
) -> Result<u64, LoadError> {
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
    if format == ExportFormat::E57 {
        return export_e57_map_count(cloud, destination, expected_count, expected_stamp, map);
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
            ExportFormat::Xyz
            | ExportFormat::Csv
            | ExportFormat::Las
            | ExportFormat::Laz
            | ExportFormat::E57 => None,
        }
    } else {
        None
    };
    {
        let mut writer = BufWriter::new(temporary.as_file_mut());
        let mut point_batch = Vec::with_capacity(PARALLEL_FORMAT_BATCH_POINTS);
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
            ExportFormat::Xyz | ExportFormat::Las | ExportFormat::Laz | ExportFormat::E57 => {}
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
                ExportFormat::Xyz
                | ExportFormat::Pts
                | ExportFormat::Csv
                | ExportFormat::PlyAscii
                | ExportFormat::PlyBinary => {
                    point_batch.push(point);
                    if point_batch.len() == PARALLEL_FORMAT_BATCH_POINTS {
                        write_parallel_point_batch(&mut writer, &mut point_batch, cloud, format)?;
                    }
                }
                ExportFormat::Las | ExportFormat::Laz | ExportFormat::E57 => unreachable!(),
            }
            Ok(())
        })?;
        write_parallel_point_batch(&mut writer, &mut point_batch, cloud, format)?;
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

fn write_parallel_point_batch(
    writer: &mut impl Write,
    points: &mut Vec<Point>,
    cloud: &PointCloud,
    format: ExportFormat,
) -> Result<(), LoadError> {
    if points.is_empty() {
        return Ok(());
    }
    let chunks: Result<Vec<Vec<u8>>, LoadError> = points
        .par_chunks(FORMAT_CHUNK_POINTS)
        .map(|chunk| {
            let mut bytes = Vec::with_capacity(chunk.len() * 48);
            for point in chunk {
                match format {
                    ExportFormat::Xyz | ExportFormat::Pts | ExportFormat::Csv => {
                        write_text_point(&mut bytes, *point, cloud, format)?;
                    }
                    ExportFormat::PlyAscii => write_ply_ascii(&mut bytes, *point, cloud)?,
                    ExportFormat::PlyBinary => write_ply_binary(&mut bytes, *point, cloud)?,
                    _ => unreachable!("only point formats use parallel batches"),
                }
            }
            Ok(bytes)
        })
        .collect();
    for bytes in chunks? {
        writer.write_all(&bytes)?;
    }
    points.clear();
    Ok(())
}

fn export_e57_map_count(
    cloud: &PointCloud,
    destination: &Path,
    expected_count: Option<u64>,
    expected_stamp: SourceStamp,
    mut map: impl FnMut(u64, Point) -> Option<Point>,
) -> Result<u64, LoadError> {
    use e57::{E57Writer, Record, RecordDataType, RecordName, RecordValue};

    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temporary = tempfile::NamedTempFile::new_in(parent)?;
    let mut prototype = vec![
        Record::CARTESIAN_X_F64,
        Record::CARTESIAN_Y_F64,
        Record::CARTESIAN_Z_F64,
    ];
    if cloud.has_rgb {
        for name in [
            RecordName::ColorRed,
            RecordName::ColorGreen,
            RecordName::ColorBlue,
        ] {
            prototype.push(Record {
                name,
                data_type: RecordDataType::U8,
            });
        }
    }
    if cloud.has_intensity {
        prototype.push(Record {
            name: RecordName::Intensity,
            data_type: RecordDataType::U16,
        });
    }
    let file_guid = format!("{{{}}}", uuid::Uuid::new_v4().to_string().to_uppercase());
    let scan_guid = format!("{{{}}}", uuid::Uuid::new_v4().to_string().to_uppercase());
    let mut writer = E57Writer::new(temporary.reopen()?, &file_guid)?;
    let mut source_count = 0u64;
    let mut written_count = 0u64;
    {
        let mut scan = writer.add_pointcloud(&scan_guid, prototype)?;
        scan.set_name(Some("Open Pointcloud Studio export".into()));
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
            let mut values: Vec<RecordValue> =
                point.xyz.into_iter().map(RecordValue::Double).collect();
            if cloud.has_rgb {
                let rgb = point.rgb.unwrap_or([0, 0, 0]);
                values.extend(
                    rgb.into_iter()
                        .map(|value| RecordValue::Integer(i64::from(value))),
                );
            }
            if cloud.has_intensity {
                values.push(RecordValue::Integer(i64::from(
                    point.intensity.unwrap_or(0),
                )));
            }
            scan.add_point(values)?;
            written_count += 1;
            Ok(())
        })?;
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
        scan.finalize()?;
    }
    writer.finalize()?;
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

/// Keep scan identities and scanner poses when an E57 subset only removes
/// points. Coordinates stay in each scan's original local reference frame.
fn export_e57_filtered_count(
    cloud: &PointCloud,
    destination: &Path,
    expected_count: Option<u64>,
    translation: [f64; 3],
    include: &mut dyn FnMut(u64, &Point) -> bool,
) -> Result<u64, LoadError> {
    use e57::{CartesianCoordinate, E57Reader, E57Writer};

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
    cloud.validate_source()?;
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temporary = tempfile::NamedTempFile::new_in(parent)?;
    let mut reader = E57Reader::from_file(&cloud.path)?;
    let mut raw_reader = E57Reader::from_file(&cloud.path)?;
    let file_guid = format!("{{{}}}", uuid::Uuid::new_v4().to_string().to_uppercase());
    let mut writer = E57Writer::new(temporary.reopen()?, &file_guid)?;
    writer.set_creation(reader.creation());
    writer.set_coordinate_metadata(reader.coordinate_metadata().map(str::to_owned));
    for extension in reader.extensions() {
        writer.register_extension(extension)?;
    }
    let mut source_count = 0u64;
    let mut written_count = 0u64;
    for source_scan in reader.pointclouds() {
        let scan_guid = format!("{{{}}}", uuid::Uuid::new_v4().to_string().to_uppercase());
        let mut output_scan = writer.add_pointcloud(&scan_guid, source_scan.prototype.clone())?;
        output_scan.set_name(source_scan.name.clone());
        output_scan.set_description(source_scan.description.clone());
        output_scan.set_original_guids(source_scan.guid.clone().map(|guid| vec![guid]));
        let transform = source_scan.transform.clone().map(|mut pose| {
            pose.translation.x += translation[0];
            pose.translation.y += translation[1];
            pose.translation.z += translation[2];
            pose
        });
        if transform.as_ref().is_some_and(|pose| {
            ![pose.translation.x, pose.translation.y, pose.translation.z]
                .into_iter()
                .all(f64::is_finite)
        }) {
            return Err(LoadError::InvalidData("non-finite E57 scan pose".into()));
        }
        output_scan.set_transform(transform);
        output_scan.set_acquisition_start(source_scan.acquisition_start.clone());
        output_scan.set_acquisition_end(source_scan.acquisition_end.clone());
        output_scan.set_sensor_vendor(source_scan.sensor_vendor.clone());
        output_scan.set_sensor_model(source_scan.sensor_model.clone());
        output_scan.set_sensor_serial(source_scan.sensor_serial.clone());
        output_scan.set_sensor_sw_version(source_scan.sensor_sw_version.clone());
        output_scan.set_sensor_hw_version(source_scan.sensor_hw_version.clone());
        output_scan.set_sensor_fw_version(source_scan.sensor_fw_version.clone());
        output_scan.set_temperature(source_scan.temperature);
        output_scan.set_humidity(source_scan.humidity);
        output_scan.set_atmospheric_pressure(source_scan.atmospheric_pressure);
        output_scan.set_color_limits(source_scan.color_limits.clone());
        output_scan.set_intensity_limits(source_scan.intensity_limits.clone());

        let mut points = reader.pointcloud_simple(&source_scan)?;
        let mut raw_points = raw_reader.pointcloud_raw(&source_scan)?;
        points.spherical_to_cartesian(true);
        points.intensity_to_color(false);
        points.apply_pose(true);
        for simple in points {
            let simple = simple?;
            let values = raw_points.next().ok_or_else(|| {
                LoadError::InvalidData("E57 raw and decoded streams have different lengths".into())
            })??;
            let CartesianCoordinate::Valid { x, y, z } = &simple.cartesian else {
                continue;
            };
            let xyz = [*x, *y, *z];
            let point = e57_points::simple_point(simple, xyz);
            let ordinal = source_count;
            source_count += 1;
            if !include(ordinal, &point) {
                continue;
            }
            if !point.xyz.iter().all(|value| value.is_finite()) {
                return Err(LoadError::InvalidData("non-finite E57 coordinate".into()));
            }
            output_scan.add_point(values)?;
            written_count += 1;
        }
        if raw_points.next().is_some() {
            return Err(LoadError::InvalidData(
                "E57 raw and decoded streams have different lengths".into(),
            ));
        }
        output_scan.finalize()?;
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
    writer.finalize()?;
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

fn update_las_record(
    raw_point: &mut las::Point,
    original: Point,
    point: Point,
    transforms: &[las::Transform; 3],
) -> Result<(), LoadError> {
    if !point.xyz.iter().all(|value| value.is_finite()) {
        return Err(LoadError::InvalidData(
            "transform produced non-finite coordinates".into(),
        ));
    }
    for (axis, transform) in transforms.iter().enumerate() {
        transform.inverse(point.xyz[axis])?;
    }
    [raw_point.x, raw_point.y, raw_point.z] = point.xyz;
    if point.rgb != original.rgb {
        raw_point.color = point.rgb.map(|rgb| {
            las::Color::new(
                u16::from(rgb[0]) * 257,
                u16::from(rgb[1]) * 257,
                u16::from(rgb[2]) * 257,
            )
        });
    }
    if point.intensity != original.intensity {
        raw_point.intensity = point.intensity.unwrap_or(0);
    }
    if point.classification != original.classification {
        let class_code = point.classification.unwrap_or(1);
        raw_point.is_overlap = class_code == 12;
        raw_point.classification =
            las::point::Classification::new(if class_code == 12 { 1 } else { class_code })?;
    }
    Ok(())
}

/// Merge LAS/LAZ sources with matching point layouts and coordinate grids.
/// Original LAS attributes survive unless `map` edits a common point field.
/// The destination is published only after every source and count is verified.
pub fn merge_las_map_count(
    sources: &[&PointCloud],
    destination: impl AsRef<Path>,
    format: ExportFormat,
    expected_count: Option<u64>,
    map: &mut dyn FnMut(usize, u64, Point) -> Option<Point>,
    progress: &mut dyn FnMut(u64, u64, u64) -> Result<(), LoadError>,
) -> Result<u64, LoadError> {
    if sources.len() < 2 || !matches!(format, ExportFormat::Las | ExportFormat::Laz) {
        return Err(LoadError::InvalidData(
            "merge needs at least two LAS/LAZ sources and a LAS/LAZ destination".into(),
        ));
    }
    let destination = destination.as_ref();
    let mut total_points = 0u64;
    let mut headers = Vec::with_capacity(sources.len());
    for source in sources {
        if !source
            .path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| {
                value.eq_ignore_ascii_case("las") || value.eq_ignore_ascii_case("laz")
            })
        {
            return Err(LoadError::InvalidData(
                "merge accepts LAS and LAZ sources only".into(),
            ));
        }
        if destination == source.path
            || fs::canonicalize(destination).ok() == fs::canonicalize(&source.path).ok()
        {
            return Err(LoadError::InvalidData(
                "merge destination must differ from every source".into(),
            ));
        }
        source.validate_source()?;
        total_points = total_points
            .checked_add(source.total_points)
            .ok_or_else(|| LoadError::InvalidData("merged point count overflows".into()))?;
        let reader = las::Reader::from_path(&source.path)?;
        headers.push(reader.header().clone());
    }
    let first = &headers[0];
    let mut first_format = *first.point_format();
    first_format.is_compressed = false;
    let first_vlrs: Vec<_> = first
        .vlrs()
        .iter()
        .filter(|vlr| {
            !(vlr.record_id == 22204 && vlr.user_id.eq_ignore_ascii_case("laszip encoded"))
        })
        .collect();
    for header in headers.iter().skip(1) {
        let mut point_format = *header.point_format();
        point_format.is_compressed = false;
        let vlrs: Vec<_> = header
            .vlrs()
            .iter()
            .filter(|vlr| {
                !(vlr.record_id == 22204 && vlr.user_id.eq_ignore_ascii_case("laszip encoded"))
            })
            .collect();
        if header.version() != first.version()
            || point_format != first_format
            || header.transforms() != first.transforms()
            || vlrs != first_vlrs
            || header.evlrs() != first.evlrs()
        {
            return Err(LoadError::InvalidData(
                "LAS/LAZ sources have incompatible point formats, coordinate grids or metadata"
                    .into(),
            ));
        }
    }
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temporary = tempfile::NamedTempFile::new_in(parent)?;
    let mut builder = las::Builder::from(first.clone());
    builder.point_format.is_compressed = format == ExportFormat::Laz;
    builder.generating_software = "Open Pointcloud Studio".into();
    builder.vlrs.retain(|vlr| {
        !(vlr.record_id == 22204 && vlr.user_id.eq_ignore_ascii_case("laszip encoded"))
    });
    let transforms = [
        first.transforms().x,
        first.transforms().y,
        first.transforms().z,
    ];
    let options = las::WriterOptions::default().with_laz_parallelism(las::LazParallelism::Yes);
    let mut writer =
        las::Writer::with_options(temporary.reopen()?, builder.into_header()?, options)?;
    let output_limit = if format == ExportFormat::Laz {
        PARALLEL_LAZ_BATCH_POINTS
    } else {
        LAS_BATCH_POINTS
    };
    let mut output = Vec::with_capacity(output_limit);
    let mut processed = 0u64;
    let mut written = 0u64;
    let stream_result = (|| -> Result<(), LoadError> {
        for (source_index, source) in sources.iter().enumerate() {
            let mut reader = las::Reader::from_path(&source.path)?;
            let read_limit = if reader.header().point_format().is_compressed {
                PARALLEL_LAZ_BATCH_POINTS
            } else {
                LAS_BATCH_POINTS
            };
            let mut input = Vec::with_capacity(read_limit);
            let mut ordinal = 0u64;
            loop {
                input.clear();
                if reader.read_points_into(read_limit as u64, &mut input)? == 0 {
                    break;
                }
                for mut raw_point in input.drain(..) {
                    let original = convert_las_point(&raw_point);
                    if let Some(mapped) = map(source_index, ordinal, original) {
                        update_las_record(&mut raw_point, original, mapped, &transforms)?;
                        output.push(raw_point);
                        written += 1;
                        if output.len() == output_limit {
                            writer.write_points(&output)?;
                            output.clear();
                        }
                    }
                    ordinal += 1;
                    processed += 1;
                }
                progress(processed, total_points, written)?;
            }
            if ordinal != source.total_points {
                return Err(LoadError::InvalidData(format!(
                    "{} changed while merging",
                    source.path.display()
                )));
            }
            source.validate_source()?;
        }
        if !output.is_empty() {
            writer.write_points(&output)?;
        }
        if expected_count.is_some_and(|expected| expected != written) {
            return Err(LoadError::InvalidData(
                "merged point count differs from the expected view".into(),
            ));
        }
        for source in sources {
            source.validate_source()?;
        }
        progress(processed, total_points, written)?;
        Ok(())
    })();
    let close_result = writer.close();
    if close_result.is_err() {
        std::mem::forget(writer);
    } else {
        drop(writer);
    }
    stream_result?;
    close_result?;
    temporary
        .persist(destination)
        .map_err(|error| LoadError::Io(error.error))?;
    Ok(written)
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
    let extended = cloud.total_points > u64::from(u32::MAX);
    let mut builder = if let Some(header) = &source_header {
        let mut builder = Builder::from(header.clone());
        builder.vlrs.retain(|vlr| {
            !(vlr.record_id == 22204 && vlr.user_id.eq_ignore_ascii_case("laszip encoded"))
        });
        builder
    } else {
        let mut builder = Builder::from((1, 4));
        builder.generating_software = "Open Pointcloud Studio".into();
        builder.point_format = Format::new(match (extended, cloud.has_rgb) {
            (false, false) => 0,
            (false, true) => 2,
            (true, false) => 6,
            (true, true) => 7,
        })?;
        builder
    };
    builder.point_format.is_compressed = format == ExportFormat::Laz;
    let transforms: [Transform; 3] = if let Some(header) = &source_header {
        let source = header.transforms();
        [source.x, source.y, source.z]
    } else {
        std::array::from_fn(|axis| {
            let min = cloud.bounds.min[axis];
            let extent = cloud.bounds.max[axis] - min;
            Transform {
                scale: 0.001_f64.max(extent / (f64::from(i32::MAX) * 0.9)),
                offset: min,
            }
        })
    };
    builder.transforms = Vector {
        x: transforms[0],
        y: transforms[1],
        z: transforms[2],
    };
    let options = las::WriterOptions::default().with_laz_parallelism(las::LazParallelism::Yes);
    let mut writer = Writer::with_options(temporary.reopen()?, builder.into_header()?, options)?;
    let batch_limit = if format == ExportFormat::Laz {
        PARALLEL_LAZ_BATCH_POINTS
    } else {
        LAS_BATCH_POINTS
    };
    let mut batch = Vec::with_capacity(batch_limit);
    let mut source_count = 0u64;
    let mut written_count = 0u64;
    let stream_result = if source_header.is_some() {
        (|| -> Result<(), LoadError> {
            let mut reader = las::Reader::from_path(&cloud.path)?;
            let read_limit = if reader.header().point_format().is_compressed {
                PARALLEL_LAZ_BATCH_POINTS
            } else {
                LAS_BATCH_POINTS
            };
            let mut read_batch = Vec::with_capacity(read_limit);
            loop {
                read_batch.clear();
                if reader.read_points_into(read_limit as u64, &mut read_batch)? == 0 {
                    break;
                }
                for mut raw_point in read_batch.drain(..) {
                    let original = convert_las_point(&raw_point);
                    let ordinal = source_count;
                    source_count += 1;
                    let Some(point) = map(ordinal, original) else {
                        continue;
                    };
                    update_las_record(&mut raw_point, original, point, &transforms)?;
                    batch.push(raw_point);
                    if batch.len() == batch_limit {
                        writer.write_points(&batch)?;
                        batch.clear();
                    }
                    written_count += 1;
                }
            }
            Ok(())
        })()
    } else {
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
            for (axis, transform) in transforms.iter().enumerate() {
                transform.inverse(point.xyz[axis])?;
            }
            let class_code = point.classification.unwrap_or(1);
            let overlap = class_code == 12;
            let classification =
                las::point::Classification::new(if overlap { 1 } else { class_code })?;
            let color = point.rgb.map(|rgb| {
                Color::new(
                    u16::from(rgb[0]) * 257,
                    u16::from(rgb[1]) * 257,
                    u16::from(rgb[2]) * 257,
                )
            });
            batch.push(LasPoint {
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
            });
            if batch.len() == batch_limit {
                writer.write_points(&batch)?;
                batch.clear();
            }
            written_count += 1;
            Ok(())
        })
    };
    let stream_result = stream_result.and_then(|()| {
        if !batch.is_empty() {
            writer.write_points(&batch)?;
        }
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
    fn parallel_point_export_keeps_order_and_exact_count_across_batches() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("ordered.xyz");
        let mut source_file = BufWriter::new(fs::File::create(&source).unwrap());
        for ordinal in 0..65_541 {
            writeln!(source_file, "{ordinal} 1 2").unwrap();
        }
        source_file.flush().unwrap();
        let cloud = open(&source, 1).unwrap();
        for format in [
            ExportFormat::Xyz,
            ExportFormat::Pts,
            ExportFormat::Csv,
            ExportFormat::PlyAscii,
            ExportFormat::PlyBinary,
        ] {
            let destination = directory
                .path()
                .join(format!("ordered-{format:?}.{}", format.extension()));
            export_full(&cloud, &destination, format).unwrap();
            let mut count = 0u64;
            visit_points(&destination, &mut |point| {
                assert_eq!(point.xyz, [count as f64, 1.0, 2.0]);
                count += 1;
                Ok(())
            })
            .unwrap();
            assert_eq!(count, 65_541);
        }
    }

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
    fn filtered_e57_keeps_scan_poses_names_and_per_scan_attributes() {
        use e57::{
            E57Reader, E57Writer, Quaternion, Record, RecordDataType, RecordName, RecordValue,
            Transform, Translation,
        };

        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("two-scans.e57");
        let mut writer = E57Writer::new(
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(&source)
                .unwrap(),
            "{00000000-0000-4000-8000-000000000001}",
        )
        .unwrap();
        {
            let mut prototype = vec![
                Record::CARTESIAN_X_F64,
                Record::CARTESIAN_Y_F64,
                Record::CARTESIAN_Z_F64,
            ];
            for name in [
                RecordName::ColorRed,
                RecordName::ColorGreen,
                RecordName::ColorBlue,
            ] {
                prototype.push(Record {
                    name,
                    data_type: RecordDataType::U16,
                });
            }
            prototype.push(Record {
                name: RecordName::Intensity,
                data_type: RecordDataType::U16,
            });
            prototype.push(Record {
                name: RecordName::RowIndex,
                data_type: RecordDataType::U16,
            });
            let mut scan = writer
                .add_pointcloud("{00000000-0000-4000-8000-000000000002}", prototype)
                .unwrap();
            scan.set_name(Some("East station".into()));
            scan.set_transform(Some(Transform {
                rotation: Quaternion::default(),
                translation: Translation {
                    x: 100.0,
                    y: 200.0,
                    z: 10.0,
                },
            }));
            for (row, x) in [1.0, 2.0].into_iter().enumerate() {
                scan.add_point(vec![
                    RecordValue::Double(x),
                    RecordValue::Double(0.0),
                    RecordValue::Double(0.0),
                    RecordValue::Integer(1_000),
                    RecordValue::Integer(2_000),
                    RecordValue::Integer(3_000),
                    RecordValue::Integer(1_000),
                    RecordValue::Integer(row as i64),
                ])
                .unwrap();
            }
            scan.finalize().unwrap();
        }
        {
            let mut scan = writer
                .add_pointcloud(
                    "{00000000-0000-4000-8000-000000000003}",
                    vec![
                        Record::CARTESIAN_X_F64,
                        Record::CARTESIAN_Y_F64,
                        Record::CARTESIAN_Z_F64,
                    ],
                )
                .unwrap();
            scan.set_name(Some("North station".into()));
            scan.set_transform(Some(Transform {
                rotation: Quaternion {
                    w: std::f64::consts::FRAC_1_SQRT_2,
                    x: 0.0,
                    y: 0.0,
                    z: std::f64::consts::FRAC_1_SQRT_2,
                },
                translation: Translation {
                    x: 200.0,
                    y: 300.0,
                    z: 20.0,
                },
            }));
            for (x, y) in [(1.0, 0.0), (0.0, 1.0)] {
                scan.add_point(vec![
                    RecordValue::Double(x),
                    RecordValue::Double(y),
                    RecordValue::Double(0.0),
                ])
                .unwrap();
            }
            scan.finalize().unwrap();
        }
        writer.finalize().unwrap();

        let cloud = open(&source, 4).unwrap();
        assert_eq!(cloud.total_points, 4);
        let destination = dir.path().join("selected.e57");
        export_where(&cloud, &destination, ExportFormat::E57, 2, |ordinal, _| {
            ordinal == 0 || ordinal == 2
        })
        .unwrap();
        let reopened = open(&destination, 4).unwrap();
        assert_eq!(reopened.total_points, 2);
        assert_eq!(reopened.scan_poses.len(), 2);
        assert_eq!(reopened.scan_poses[0].label, "East station");
        assert_eq!(reopened.scan_poses[1].label, "North station");
        assert!((reopened.points[0].xyz[0] - 101.0).abs() < 1e-9);
        assert!((reopened.points[1].xyz[1] - 301.0).abs() < 1e-9);
        assert_eq!(reopened.points[0].rgb, Some([4, 8, 12]));
        assert_eq!(reopened.points[0].intensity, Some(1_000));
        assert_eq!(reopened.points[1].rgb, None);
        assert_eq!(reopened.points[1].intensity, None);
        let scan_headers = E57Reader::from_file(&destination).unwrap().pointclouds();
        assert_eq!(scan_headers.len(), 2);
        assert_eq!(scan_headers[0].records, 1);
        assert_eq!(scan_headers[1].records, 1);
        let source_headers = E57Reader::from_file(&source).unwrap().pointclouds();
        assert_eq!(
            scan_headers[0].prototype.len(),
            source_headers[0].prototype.len()
        );
        assert_eq!(scan_headers[0].prototype[3].name, RecordName::ColorRed);
        assert!(matches!(
            scan_headers[0].prototype[3].data_type,
            RecordDataType::Integer { min: 0, max: 65535 }
        ));
        assert_eq!(scan_headers[0].prototype[7].name, RecordName::RowIndex);
        let mut source_reader = E57Reader::from_file(&source).unwrap();
        let mut output_reader = E57Reader::from_file(&destination).unwrap();
        for (source_scan, output_scan) in source_headers.iter().zip(&scan_headers) {
            let source_point = source_reader
                .pointcloud_raw(source_scan)
                .unwrap()
                .next()
                .unwrap()
                .unwrap();
            let output_point = output_reader
                .pointcloud_raw(output_scan)
                .unwrap()
                .next()
                .unwrap()
                .unwrap();
            assert_eq!(output_point, source_point);
        }

        let translated = dir.path().join("translated.e57");
        assert_eq!(
            export_e57_translated_where(
                &cloud,
                &translated,
                Some(2),
                [10.0, -5.0, 2.0],
                |ordinal, _| ordinal == 0 || ordinal == 2,
            )
            .unwrap(),
            Some(2)
        );
        let moved = open(&translated, 4).unwrap();
        assert_eq!(moved.scan_poses.len(), 2);
        assert_eq!(moved.scan_poses[0].position, [110.0, 195.0, 12.0]);
        assert_eq!(moved.scan_poses[1].position, [210.0, 295.0, 22.0]);
        assert!((moved.points[0].xyz[0] - 111.0).abs() < 1e-9);
        assert!((moved.points[1].xyz[1] - 296.0).abs() < 1e-9);
        let moved_headers = E57Reader::from_file(&translated).unwrap().pointclouds();
        let mut moved_reader = E57Reader::from_file(&translated).unwrap();
        for (source_scan, moved_scan) in source_headers.iter().zip(&moved_headers) {
            let source_point = source_reader
                .pointcloud_raw(source_scan)
                .unwrap()
                .next()
                .unwrap()
                .unwrap();
            let moved_point = moved_reader
                .pointcloud_raw(moved_scan)
                .unwrap()
                .next()
                .unwrap()
                .unwrap();
            assert_eq!(moved_point, source_point);
        }

        let section = dir.path().join("section.e57");
        let written = export_section(
            &cloud,
            &section,
            ExportFormat::E57,
            Bounds {
                min: [199.5, 300.5, 19.5],
                max: [200.5, 301.5, 20.5],
            },
        )
        .unwrap();
        assert_eq!(written, 1);
        assert_eq!(open(&section, 2).unwrap().scan_poses.len(), 2);

        let existing = dir.path().join("existing.e57");
        fs::write(&existing, b"leave this untouched").unwrap();
        assert!(
            export_where(&cloud, &existing, ExportFormat::E57, 3, |ordinal, _| {
                ordinal == 0 || ordinal == 2
            })
            .is_err()
        );
        assert_eq!(fs::read(&existing).unwrap(), b"leave this untouched");
    }

    #[test]
    fn e57_export_round_trips_attributes_and_keeps_full_source_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("survey.ply");
        fs::write(
            &source,
            "ply\nformat ascii 1.0\nelement vertex 3\nproperty double x\nproperty double y\nproperty double z\nproperty uchar red\nproperty uchar green\nproperty uchar blue\nproperty ushort intensity\nend_header\n207000.001 474000.002 1.234 10 20 30 1234\n207001.003 474001.004 2.345 40 50 60 5678\n207002.005 474002.006 3.456 70 80 90 9012\n",
        )
        .unwrap();
        let cloud = open(&source, 1).unwrap();
        let output = dir.path().join("survey.e57");
        export_full(&cloud, &output, ExportFormat::E57).unwrap();
        let reopened = open(&output, 3).unwrap();
        assert_eq!(reopened.total_points, 3);
        assert_eq!(reopened.bounds, cloud.bounds);
        assert_eq!(reopened.points[1].rgb, Some([40, 50, 60]));
        assert_eq!(reopened.points[1].intensity, Some(5678));
        let no_pose_translation = dir.path().join("no-pose-translation.e57");
        assert_eq!(
            export_e57_translated_where(
                &reopened,
                &no_pose_translation,
                Some(3),
                [1.0, 2.0, 3.0],
                |_, _| true,
            )
            .unwrap(),
            None
        );
        assert!(!no_pose_translation.exists());

        let copy = dir.path().join("copy.e57");
        export_full(&reopened, &copy, ExportFormat::E57).unwrap();
        assert_eq!(fs::read(&copy).unwrap(), fs::read(&output).unwrap());

        let selected = dir.path().join("selected.e57");
        export_where(&reopened, &selected, ExportFormat::E57, 2, |ordinal, _| {
            ordinal != 1
        })
        .unwrap();
        let selected = open(&selected, 2).unwrap();
        assert_eq!(selected.total_points, 2);
        assert_eq!(selected.points[1].xyz, [207002.005, 474002.006, 3.456]);
        assert_eq!(selected.points[1].intensity, Some(9012));

        let moved = dir.path().join("moved.e57");
        export_affine(&reopened, &moved, ExportFormat::E57, [10.0, 0.0, 0.0], 1.0).unwrap();
        let moved = open(&moved, 3).unwrap();
        assert_eq!(moved.total_points, 3);
        assert_eq!(moved.points[0].xyz, [207010.001, 474000.002, 1.234]);

        let section = dir.path().join("section.e57");
        let written = export_section(
            &reopened,
            &section,
            ExportFormat::E57,
            Bounds {
                min: [207001.0, 474001.0, 2.0],
                max: [207003.0, 474003.0, 4.0],
            },
        )
        .unwrap();
        assert_eq!(written, 2);
        assert_eq!(open(&section, 2).unwrap().total_points, 2);
    }

    #[test]
    fn e57_export_round_trips_xyz_only() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("points.xyz");
        fs::write(&source, "1 2 3\n4 5 6\n7 8 9\n").unwrap();
        let cloud = open(&source, 1).unwrap();
        let output = dir.path().join("points.e57");
        export_full(&cloud, &output, ExportFormat::E57).unwrap();
        let reopened = open(&output, 3).unwrap();
        assert_eq!(reopened.total_points, 3);
        assert!(!reopened.has_rgb);
    }

    #[test]
    fn e57_intensity_only_does_not_invent_rgb() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("intensity.ply");
        fs::write(
            &source,
            "ply\nformat ascii 1.0\nelement vertex 2\nproperty double x\nproperty double y\nproperty double z\nproperty ushort intensity\nend_header\n1 2 3 1234\n4 5 6 5678\n",
        )
        .unwrap();
        let cloud = open(&source, 1).unwrap();
        let output = dir.path().join("intensity.e57");
        export_full(&cloud, &output, ExportFormat::E57).unwrap();
        let reopened = open(&output, 2).unwrap();
        assert_eq!(reopened.total_points, 2);
        assert!(!reopened.has_rgb);
        assert_eq!(reopened.points[1].intensity, Some(5678));
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
                    assert!((actual.xyz[axis] - expected.0[axis]).abs() < 0.000_01);
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
        builder.vlrs.push(las::Vlr {
            user_id: "OpenPTS".into(),
            record_id: 65_000,
            description: "Producer metadata".into(),
            data: vec![7, 8, 9],
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

        let selected = dir.path().join("selected.laz");
        export_where(&laz_cloud, &selected, ExportFormat::Laz, 1, |_, _| true).unwrap();
        let mut selected_reader = las::Reader::from_path(&selected).unwrap();
        assert_eq!(selected_reader.header().number_of_points(), 1);
        assert!(selected_reader
            .header()
            .vlrs()
            .iter()
            .any(|vlr| vlr.user_id == "LASF_Projection" && vlr.record_id == 34735));
        assert!(selected_reader.header().vlrs().iter().any(|vlr| {
            vlr.user_id == "OpenPTS" && vlr.record_id == 65_000 && vlr.data == [7, 8, 9]
        }));
        assert_eq!(
            selected_reader.points().next().unwrap().unwrap(),
            original_point
        );

        let moved = dir.path().join("moved.laz");
        export_affine(&laz_cloud, &moved, ExportFormat::Laz, [10.0, 0.0, 0.0], 1.0).unwrap();
        let moved_point = las::Reader::from_path(&moved)
            .unwrap()
            .points()
            .next()
            .unwrap()
            .unwrap();
        let mut expected_moved = original_point;
        expected_moved.x += 10.0;
        assert_eq!(moved_point, expected_moved);
    }

    #[test]
    fn merged_laz_keeps_attributes_edits_and_atomic_output() {
        let dir = tempfile::tempdir().unwrap();
        let make_source = |name: &str, points: &[las::Point]| {
            let path = dir.path().join(name);
            let mut builder = las::Builder::from((1, 4));
            builder.point_format = las::point::Format::new(7).unwrap();
            builder.point_format.extra_bytes = 4;
            builder.transforms = las::Vector {
                x: las::Transform {
                    scale: 0.001,
                    offset: 0.0,
                },
                y: las::Transform {
                    scale: 0.001,
                    offset: 0.0,
                },
                z: las::Transform {
                    scale: 0.001,
                    offset: 0.0,
                },
            };
            builder.vlrs.push(las::Vlr {
                user_id: "LASF_Projection".into(),
                record_id: 34735,
                description: "GeoKeyDirectoryTag".into(),
                data: vec![1, 0, 1, 0],
            });
            let mut writer = las::Writer::new(
                std::fs::File::create(&path).unwrap(),
                builder.into_header().unwrap(),
            )
            .unwrap();
            for point in points {
                writer.write_point(point.clone()).unwrap();
            }
            writer.close().unwrap();
            path
        };
        let make_point = |x: f64, time: f64| las::Point {
            x,
            y: 474_000.003,
            z: 1.234,
            return_number: 2,
            number_of_returns: 3,
            gps_time: Some(time),
            color: Some(las::Color::new(12_345, 23_456, 34_567)),
            scanner_channel: 2,
            extra_bytes: vec![1, 2, 3, 4],
            ..las::Point::default()
        };
        let first_points = [make_point(207_000.001, 1.25), make_point(207_000.002, 2.5)];
        let second_points = [make_point(208_000.001, 3.75), make_point(208_000.002, 5.0)];
        let first = make_source("first.las", &first_points);
        let second = make_source("second.las", &second_points);
        let first_cloud = crate::open_las_header(&first).unwrap();
        let second_cloud = crate::open_las_header(&second).unwrap();
        let sources = [&first_cloud, &second_cloud];
        let destination = dir.path().join("merged.laz");
        let mut last_progress = (0, 0, 0);
        let written = merge_las_map_count(
            &sources,
            &destination,
            ExportFormat::Laz,
            Some(3),
            &mut |index, ordinal, mut point| {
                if index == 0 && ordinal == 1 {
                    return None;
                }
                if index == 1 && ordinal == 0 {
                    point.xyz[0] += 10.0;
                }
                Some(point)
            },
            &mut |processed, total, written| {
                last_progress = (processed, total, written);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(written, 3);
        assert_eq!(last_progress, (4, 4, 3));
        let mut reader = las::Reader::from_path(&destination).unwrap();
        assert_eq!(reader.header().number_of_points(), 3);
        assert_eq!(
            reader.header().transforms(),
            las::Reader::from_path(&first)
                .unwrap()
                .header()
                .transforms()
        );
        assert!(reader
            .header()
            .vlrs()
            .iter()
            .any(|vlr| vlr.user_id == "LASF_Projection"));
        let points = reader.points().collect::<Result<Vec<_>, _>>().unwrap();
        let first_source_points = las::Reader::from_path(&first)
            .unwrap()
            .points()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let second_source_points = las::Reader::from_path(&second)
            .unwrap()
            .points()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(points[0], first_source_points[0]);
        let mut moved = second_source_points[0].clone();
        moved.x += 10.0;
        assert_eq!(points[1], moved);
        assert_eq!(points[2], second_source_points[1]);

        fs::write(&destination, b"existing output").unwrap();
        let cancelled = merge_las_map_count(
            &sources,
            &destination,
            ExportFormat::Laz,
            None,
            &mut |_, _, point| Some(point),
            &mut |_, _, _| Err(LoadError::Cancelled),
        );
        assert!(matches!(cancelled, Err(LoadError::Cancelled)));
        assert_eq!(fs::read(&destination).unwrap(), b"existing output");
    }

    #[test]
    fn filtered_las_keeps_original_millimeter_grid() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("survey.las");
        let mut builder = las::Builder::from((1, 4));
        builder.transforms = las::Vector {
            x: las::Transform {
                scale: 0.001,
                offset: 207_000.0,
            },
            y: las::Transform {
                scale: 0.001,
                offset: 474_000.0,
            },
            z: las::Transform {
                scale: 0.001,
                offset: 0.0,
            },
        };
        let mut writer = las::Writer::new(
            std::fs::File::create(&source).unwrap(),
            builder.into_header().unwrap(),
        )
        .unwrap();
        for x in [207_000.001, 207_000.002] {
            writer
                .write_point(las::Point {
                    x,
                    y: 474_000.003,
                    z: 1.234,
                    ..las::Point::default()
                })
                .unwrap();
        }
        writer.close().unwrap();
        drop(writer);

        let cloud = open(&source, 1).unwrap();
        let destination = dir.path().join("selected.laz");
        export_where(&cloud, &destination, ExportFormat::Laz, 1, |ordinal, _| {
            ordinal == 0
        })
        .unwrap();
        let mut source_reader = las::Reader::from_path(&source).unwrap();
        let mut output_reader = las::Reader::from_path(&destination).unwrap();
        assert_eq!(output_reader.header().number_of_points(), 1);
        assert_eq!(
            output_reader.header().transforms(),
            source_reader.header().transforms()
        );
        assert_eq!(
            output_reader.points().next().unwrap().unwrap(),
            source_reader.points().next().unwrap().unwrap()
        );
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
