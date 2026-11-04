//! Stream mixed scanner files into an E57 scene. E57 sources retain raw
//! records, station poses and images; other formats become separate scans.
use crate::{scene_transform::SceneTransform, LoadError, Point, PointCloud};
use std::collections::HashMap;
use std::path::Path;

mod posed;

fn point_values(cloud: &PointCloud, point: Point, xyz: [f64; 3]) -> Vec<e57::RecordValue> {
    use e57::RecordValue;
    let mut values: Vec<_> = xyz.map(RecordValue::Double).into();
    if cloud.has_rgb {
        values.extend(
            point
                .rgb
                .unwrap_or([0; 3])
                .map(|v| RecordValue::Integer(i64::from(v))),
        );
    }
    if cloud.has_intensity {
        values.push(RecordValue::Integer(i64::from(
            point.intensity.unwrap_or(0),
        )));
    }
    if cloud.has_classification {
        values.push(RecordValue::Integer(i64::from(
            point.classification.unwrap_or(0),
        )));
    }
    values
}

pub struct SceneSource<'a> {
    pub cloud: &'a PointCloud,
    pub transform: SceneTransform,
}

#[derive(Default)]
struct SceneCrs {
    key: Option<String>,
    metadata: Option<String>,
    vlrs: Vec<las::Vlr>,
}
fn scene_crs(sources: &[SceneSource<'_>]) -> Result<SceneCrs, LoadError> {
    let mut common = SceneCrs::default();
    for source in sources {
        let extension = source
            .cloud
            .path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let mut current = SceneCrs::default();
        if extension == "e57" {
            if let Some(system) = crate::e57_points::open_reader(&source.cloud.path)?
                .coordinate_metadata()
                .filter(|s| !s.trim().is_empty())
            {
                current.key = Some(system.split_whitespace().collect::<String>());
                current.metadata = Some(system.into());
            }
        } else if extension == "las" || extension == "laz" {
            let reader = las::Reader::from_path(&source.cloud.path)?;
            current.vlrs = reader
                .header()
                .vlrs()
                .iter()
                .filter(|v| v.user_id == "LASF_Projection")
                .cloned()
                .collect();
            current.vlrs.sort_by_key(|v| v.record_id);
            if let Some(wkt) = current.vlrs.iter().find(|v| v.record_id == 2112) {
                let value = String::from_utf8_lossy(&wkt.data)
                    .trim_end_matches('\0')
                    .to_owned();
                current.key = Some(value.split_whitespace().collect::<String>());
                current.metadata = Some(value);
            } else if !current.vlrs.is_empty() {
                current.key = Some(format!(
                    "GeoTIFF:{:?}",
                    current
                        .vlrs
                        .iter()
                        .map(|v| (v.record_id, &v.data))
                        .collect::<Vec<_>>()
                ));
            }
        }
        if let Some(key) = &current.key {
            if common.key.as_ref().is_some_and(|old| old != key) {
                return Err(LoadError::InvalidData("scans declare different coordinate reference systems; reproject to a common system before merging".into()));
            }
            if common.key.is_none() {
                common = current;
            } else if common.vlrs.is_empty() {
                common.vlrs = current.vlrs;
            }
        }
    }
    Ok(common)
}

/// A normalised LAS 1.4 scene for inputs with different record layouts.
/// Common coordinates, colours, intensity and classification are retained;
/// E57 camera imagery is available only in the E57 scene writer.
pub fn write_las_scene(
    sources: &[SceneSource<'_>],
    destination: &Path,
    compressed: bool,
    include: &mut dyn FnMut(usize, u64, &Point) -> bool,
    progress: &mut dyn FnMut(u64, u64) -> Result<(), LoadError>,
) -> Result<u64, LoadError> {
    use las::{Builder, Color, Transform, Vector, Writer};
    if sources.is_empty() {
        return Err(LoadError::InvalidData("choose at least one scan".into()));
    }
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    let mut total = 0u64;
    for source in sources {
        source.transform.validate()?;
        source.cloud.validate_source()?;
        if destination == source.cloud.path
            || std::fs::canonicalize(destination).ok()
                == std::fs::canonicalize(&source.cloud.path).ok()
        {
            return Err(LoadError::InvalidData(
                "merge destination must differ from every source".into(),
            ));
        }
        total = total
            .checked_add(source.cloud.total_points)
            .ok_or_else(|| LoadError::InvalidData("point count overflows".into()))?;
        for corner in crate::bounds_corners(source.cloud.bounds) {
            let point = source.transform.xyz(corner);
            for i in 0..3 {
                min[i] = min[i].min(point[i]);
                max[i] = max[i].max(point[i]);
            }
        }
    }
    let rgb = sources.iter().any(|s| s.cloud.has_rgb);
    let crs = scene_crs(sources)?;
    let mut builder = Builder::from((1, 4));
    builder.generating_software = "Open Pointcloud Studio".into();
    builder.point_format = las::point::Format::new(if rgb { 7 } else { 6 })?;
    builder.point_format.is_compressed = compressed;
    builder.vlrs = crs.vlrs;
    let legacy_geotiff = builder.vlrs.iter().any(|v| v.record_id == 34735)
        && !builder.vlrs.iter().any(|v| v.record_id == 2112);
    if legacy_geotiff {
        // Extended LAS point layouts require WKT. Keep the supplied GeoTIFF
        // CRS in a legacy layout rather than falsely setting the WKT flag.
        builder.point_format = las::point::Format::new(if rgb { 3 } else { 1 })?;
        builder.point_format.is_compressed = compressed;
    }
    if builder.vlrs.is_empty() {
        if let Some(metadata) = crs.metadata {
            if !metadata.contains('[') {
                return Err(LoadError::InvalidData("LAS export needs a WKT coordinate system; reproject or export E57 to retain this CRS declaration".into()));
            }
            builder.vlrs.push(las::Vlr {
                user_id: "LASF_Projection".into(),
                record_id: 2112,
                description: "OGC WKT".into(),
                data: metadata.bytes().chain([0]).collect(),
            });
            builder.has_wkt_crs = true;
        }
    }
    let grid: [Transform; 3] = std::array::from_fn(|i| Transform {
        scale: 0.000001_f64.max((max[i] - min[i]) / (f64::from(i32::MAX) * 1.8)),
        offset: (min[i] + max[i]) * 0.5,
    });
    builder.transforms = Vector {
        x: grid[0],
        y: grid[1],
        z: grid[2],
    };
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temporary = tempfile::NamedTempFile::new_in(parent)?;
    let mut writer = Writer::with_options(
        temporary.reopen()?,
        builder.into_header()?,
        las::WriterOptions::default().with_laz_parallelism(las::LazParallelism::Yes),
    )?;
    let mut batch = Vec::with_capacity(400_000);
    let mut processed = 0;
    let mut written = 0;
    let result = (|| -> Result<(), LoadError> {
        for (index, source) in sources.iter().enumerate() {
            let mut ordinal = 0;
            crate::visit_points(&source.cloud.path, &mut |point| {
                if include(index, ordinal, &point) {
                    let p = source.transform.xyz(point.xyz);
                    let class = point.classification.unwrap_or(0);
                    if legacy_geotiff && class > 31 {
                        return Err(LoadError::InvalidData("this GeoTIFF CRS needs a WKT conversion before merging classifications above 31 into LAS; use E57 instead".into()));
                    }
                    batch.push(las::Point {
                        x: p[0],
                        y: p[1],
                        z: p[2],
                        intensity: point.intensity.unwrap_or(0),
                        return_number: 1,
                        number_of_returns: 1,
                        classification: las::point::Classification::new(if class == 12 {
                            1
                        } else {
                            class
                        })?,
                        is_overlap: class == 12,
                        gps_time: Some(0.),
                        color: rgb.then(|| {
                            let c = point.rgb.unwrap_or([0; 3]);
                            Color::new(
                                u16::from(c[0]) * 257,
                                u16::from(c[1]) * 257,
                                u16::from(c[2]) * 257,
                            )
                        }),
                        ..Default::default()
                    });
                    written += 1;
                    if batch.len() == 400_000 {
                        writer.write_points(&batch)?;
                        batch.clear();
                    }
                }
                ordinal += 1;
                processed += 1;
                if processed % 4096 == 0 {
                    progress(processed, total)?;
                }
                Ok(())
            })?;
            if ordinal != source.cloud.total_points {
                return Err(LoadError::InvalidData("source point count changed".into()));
            }
            source.cloud.validate_source()?;
        }
        if !batch.is_empty() {
            writer.write_points(&batch)?;
        }
        progress(processed, total)?;
        Ok(())
    })();
    let closed = writer.close();
    if closed.is_err() {
        std::mem::forget(writer);
    } else {
        drop(writer);
    }
    result?;
    closed?;
    for source in sources {
        source.cloud.validate_source()?;
    }
    temporary
        .persist(destination)
        .map_err(|e| LoadError::Io(e.error))?;
    Ok(written)
}

pub fn write_e57_scene(
    sources: &[SceneSource<'_>],
    destination: &Path,
    preserve_images: bool,
    include: &mut dyn FnMut(usize, u64, &Point) -> bool,
    progress: &mut dyn FnMut(u64, u64) -> Result<(), LoadError>,
) -> Result<u64, LoadError> {
    use e57::{CartesianCoordinate, E57Writer, Record, RecordDataType, RecordName, RecordValue};
    if sources.is_empty() {
        return Err(LoadError::InvalidData("choose at least one scan".into()));
    }
    let total = sources
        .iter()
        .try_fold(0u64, |sum, s| sum.checked_add(s.cloud.total_points))
        .ok_or_else(|| LoadError::InvalidData("scene point count overflows".into()))?;
    for source in sources {
        source.transform.validate()?;
        source.cloud.validate_source()?;
        if destination == source.cloud.path
            || std::fs::canonicalize(destination).ok()
                == std::fs::canonicalize(&source.cloud.path).ok()
        {
            return Err(LoadError::InvalidData(
                "scene destination must differ from every source".into(),
            ));
        }
    }
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temporary = tempfile::NamedTempFile::new_in(parent)?;
    let mut writer = E57Writer::new(
        temporary.reopen()?,
        &format!("{{{}}}", uuid::Uuid::new_v4()),
    )?;
    writer.set_coordinate_metadata(scene_crs(sources)?.metadata);
    writer.register_extension(e57::Extension::new(
        "opsStudio",
        "https://open-aec.com/e57/point-attributes/1",
    ))?;
    let mut processed = 0;
    let mut written = 0;
    let mut coordinate_system: Option<String> = None;
    let mut extensions = HashMap::from([(
        "opsStudio".to_owned(),
        "https://open-aec.com/e57/point-attributes/1".to_owned(),
    )]);
    for (source_index, source) in sources.iter().enumerate() {
        let edits = crate::photo_edits::PhotoEdits::load(&source.cloud.path)?;
        if crate::is_e57(&source.cloud.path) && preserve_images {
            let mut reader = crate::e57_points::open_reader(&source.cloud.path)?;
            let mut raw = crate::e57_points::open_reader(&source.cloud.path)?;
            if let Some(system) = reader
                .coordinate_metadata()
                .filter(|s| !s.trim().is_empty())
            {
                if coordinate_system
                    .as_deref()
                    .is_some_and(|old| old != system)
                {
                    return Err(LoadError::InvalidData("scans state different coordinate reference systems; align/reproject them before merging".into()));
                }
                coordinate_system = Some(system.into());
                writer.set_coordinate_metadata(coordinate_system.clone());
            }
            for extension in reader.extensions() {
                match extensions.get(&extension.namespace) {
                    Some(url) if *url == extension.url => {}
                    Some(_) => {
                        return Err(LoadError::InvalidData(format!(
                            "conflicting E57 extension namespace {}",
                            extension.namespace
                        )))
                    }
                    None => {
                        extensions.insert(extension.namespace.clone(), extension.url.clone());
                        writer.register_extension(extension)?;
                    }
                }
            }
            let mut guids = HashMap::new();
            let mut ordinal = 0;
            for scan in reader.pointclouds() {
                let guid = format!("{{{}}}", uuid::Uuid::new_v4());
                if let Some(old) = &scan.guid {
                    guids.insert(old.clone(), guid.clone());
                }
                let mut prototype = scan.prototype.clone();
                if source.transform.scale != 1. {
                    for record in &mut prototype {
                        if matches!(
                            record.name,
                            RecordName::CartesianX
                                | RecordName::CartesianY
                                | RecordName::CartesianZ
                                | RecordName::SphericalRange
                        ) {
                            record.data_type = RecordDataType::Double {
                                min: None,
                                max: None,
                            };
                        }
                    }
                }
                let mut output = writer.add_pointcloud(&guid, prototype)?;
                output.set_name(scan.name.clone().or_else(|| {
                    Some(
                        source
                            .cloud
                            .path
                            .file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into(),
                    )
                }));
                output.set_description(scan.description.clone());
                output.set_original_guids(scan.guid.clone().map(|g| vec![g]));
                output.set_transform(Some(source.transform.pose(scan.transform.clone())));
                output.set_acquisition_start(scan.acquisition_start.clone());
                output.set_acquisition_end(scan.acquisition_end.clone());
                output.set_sensor_vendor(scan.sensor_vendor.clone());
                output.set_sensor_model(scan.sensor_model.clone());
                output.set_sensor_serial(scan.sensor_serial.clone());
                output.set_sensor_sw_version(scan.sensor_sw_version.clone());
                output.set_sensor_hw_version(scan.sensor_hw_version.clone());
                output.set_sensor_fw_version(scan.sensor_fw_version.clone());
                output.set_temperature(scan.temperature);
                output.set_humidity(scan.humidity);
                output.set_atmospheric_pressure(scan.atmospheric_pressure);
                output.set_color_limits(scan.color_limits.clone());
                output.set_intensity_limits(scan.intensity_limits.clone());
                let mut simple = reader.pointcloud_simple(&scan)?;
                simple.spherical_to_cartesian(true);
                simple.apply_pose(true);
                simple.intensity_to_color(false);
                let mut records = raw.pointcloud_raw(&scan)?;
                for point in simple {
                    let point = point?;
                    let mut values = records.next().ok_or_else(|| {
                        LoadError::InvalidData("E57 stream length mismatch".into())
                    })??;
                    let CartesianCoordinate::Valid { x, y, z } = point.cartesian else {
                        continue;
                    };
                    let point = crate::e57_points::simple_point(point, [x, y, z]);
                    if include(source_index, ordinal, &point) {
                        if source.transform.scale != 1. {
                            for (value, record) in values.iter_mut().zip(&scan.prototype) {
                                if matches!(
                                    record.name,
                                    RecordName::CartesianX
                                        | RecordName::CartesianY
                                        | RecordName::CartesianZ
                                        | RecordName::SphericalRange
                                ) {
                                    *value = RecordValue::Double(
                                        value.to_f64(&record.data_type)? * source.transform.scale,
                                    );
                                }
                            }
                        }
                        output.add_point(values)?;
                        written += 1;
                    }
                    ordinal += 1;
                    processed += 1;
                    if processed % 4096 == 0 {
                        progress(processed, total)?;
                    }
                }
                if records.next().is_some() {
                    return Err(LoadError::InvalidData("E57 stream length mismatch".into()));
                }
                output.finalize()?;
            }
            if ordinal != source.cloud.total_points {
                return Err(LoadError::InvalidData("source point count changed".into()));
            }
            if preserve_images {
                crate::export::copy_e57_images_scene(
                    &mut reader,
                    &mut writer,
                    &guids,
                    source.transform,
                    &edits,
                )?;
            }
        } else {
            let mut prototype = vec![
                Record::CARTESIAN_X_F64,
                Record::CARTESIAN_Y_F64,
                Record::CARTESIAN_Z_F64,
            ];
            if source.cloud.has_rgb {
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
            if source.cloud.has_intensity {
                prototype.push(Record {
                    name: RecordName::Intensity,
                    data_type: RecordDataType::U16,
                });
            }
            if source.cloud.has_classification {
                prototype.push(Record {
                    name: RecordName::Unknown {
                        namespace: "opsStudio".into(),
                        name: "classification".into(),
                    },
                    data_type: RecordDataType::U8,
                });
            }
            let guid = format!("{{{}}}", uuid::Uuid::new_v4());
            let has_scan_blocks = source
                .cloud
                .path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    extension.eq_ignore_ascii_case("pcd") || extension.eq_ignore_ascii_case("ptx")
                });
            if preserve_images && has_scan_blocks {
                let before = processed;
                written += posed::write(
                    &mut writer,
                    source,
                    prototype,
                    &guid,
                    &mut |ordinal, point| include(source_index, ordinal, point),
                    &mut |read| progress(before + read, total),
                )?;
                processed += source.cloud.total_points;
            } else {
                let mut output = writer.add_pointcloud(&guid, prototype)?;
                if preserve_images {
                    output.set_name(Some(
                        source
                            .cloud
                            .path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into(),
                    ));
                }
                let mut ordinal = 0;
                crate::visit_points(&source.cloud.path, &mut |point| {
                    if include(source_index, ordinal, &point) {
                        output.add_point(point_values(
                            source.cloud,
                            point,
                            source.transform.xyz(point.xyz),
                        ))?;
                        written += 1;
                    }
                    ordinal += 1;
                    processed += 1;
                    if processed % 4096 == 0 {
                        progress(processed, total)?;
                    }
                    Ok(())
                })?;
                if ordinal != source.cloud.total_points {
                    return Err(LoadError::InvalidData("source point count changed".into()));
                }
                output.finalize()?;
            }
            if preserve_images {
                crate::photo_edits::write_added(
                    &mut writer,
                    &edits,
                    source.transform,
                    Some(&guid),
                )?;
            }
        }
        source.cloud.validate_source()?;
    }
    progress(processed, total)?;
    writer.finalize_customized_xml(|xml| Ok(xml.replace("readius", "radius")))?;
    for source in sources {
        source.cloud.validate_source()?;
    }
    temporary
        .persist(destination)
        .map_err(|e| LoadError::Io(e.error))?;
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{photo_edits::PhotoEdits, FilePhoto, PhotoProjection, ScanImageFormat};

    #[test]
    fn mixed_export_keeps_pcd_and_each_ptx_station_without_inventing_origins() {
        let dir = tempfile::tempdir().unwrap();
        let pcd = dir.path().join("posed.pcd");
        std::fs::write(&pcd, "FIELDS x y z\nSIZE 4 4 4\nTYPE F F F\nPOINTS 2\nVIEWPOINT 10 20 30 0.7071067811865476 0 0 0.7071067811865476\nDATA ascii\n1 2 3\n2 3 4\n").unwrap();
        let ptx = dir.path().join("blocks.ptx");
        std::fs::write(&ptx, crate::ptx::tests::THREE_BLOCKS).unwrap();
        let xyz = dir.path().join("unposed.xyz");
        std::fs::write(&xyz, "1 2 3\n2 3 4\n").unwrap();
        let clouds: Vec<_> = [&pcd, &ptx, &xyz]
            .map(|path| crate::open(path, 100).unwrap())
            .into();
        let transform = SceneTransform {
            rotation: [[0., -1., 0.], [1., 0., 0.], [0., 0., 1.]],
            translation: [100., 200., 300.],
            scale: 2.,
        };
        let sources: Vec<_> = clouds
            .iter()
            .map(|cloud| SceneSource { cloud, transform })
            .collect();
        let out = dir.path().join("posed.e57");
        assert_eq!(
            write_e57_scene(
                &sources,
                &out,
                true,
                &mut |_, _, _| true,
                &mut |_, _| Ok(())
            )
            .unwrap(),
            10
        );
        let actual = crate::open(&out, 100).unwrap();
        let expected: Vec<_> = clouds
            .iter()
            .flat_map(|cloud| cloud.points.iter().map(|point| transform.xyz(point.xyz)))
            .collect();
        for (point, expected) in actual.points.iter().zip(expected) {
            assert!((0..3).all(|i| (point.xyz[i] - expected[i]).abs() < 1e-9));
        }
        let expected_poses: Vec<_> = clouds.iter().flat_map(|cloud| &cloud.scan_poses).collect();
        assert_eq!(actual.scan_poses.len(), expected_poses.len());
        for (pose, expected) in actual.scan_poses.iter().zip(expected_poses) {
            let position = transform.xyz(expected.position);
            assert!((0..3).all(|i| (pose.position[i] - position[i]).abs() < 1e-9));
            let axes = expected.axes.unwrap().map(|axis| transform.direction(axis));
            let actual_axes = pose.axes.unwrap();
            assert!((0..3).all(|i| (0..3).all(|j| (actual_axes[i][j] - axes[i][j]).abs() < 1e-9)));
        }
        assert_eq!(
            crate::e57_points::open_reader(&out)
                .unwrap()
                .pointclouds()
                .len(),
            5
        );
        let bare = dir.path().join("bare.e57");
        write_e57_scene(&sources, &bare, false, &mut |_, _, _| true, &mut |_, _| {
            Ok(())
        })
        .unwrap();
        assert!(crate::open(&bare, 100).unwrap().scan_poses.is_empty());
        let reader = crate::e57_points::open_reader(&out).unwrap();
        assert!(reader.pointclouds()[0]
            .name
            .as_ref()
            .unwrap()
            .contains("posed.pcd"));
    }

    #[test]
    fn all_unposed_ptx_blocks_keep_boundaries_and_unique_source_names() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("unknown.PTX");
        let text = crate::ptx::tests::THREE_BLOCKS
            .replace("\n10 20 30\n", "\nnan nan nan\n")
            .replace("\n40 50 60\n", "\nnan nan nan\n");
        std::fs::write(&path, text).unwrap();
        let cloud = crate::open(&path, 100).unwrap();
        assert!(cloud.scan_poses.is_empty());
        let sources = [SceneSource {
            cloud: &cloud,
            transform: SceneTransform::default(),
        }];
        let out = dir.path().join("unknown.e57");
        assert_eq!(
            write_e57_scene(
                &sources,
                &out,
                true,
                &mut |_, _, _| true,
                &mut |_, _| Ok(())
            )
            .unwrap(),
            6
        );
        let reader = crate::e57_points::open_reader(&out).unwrap();
        let scans = reader.pointclouds();
        assert_eq!(
            scans.iter().map(|scan| scan.records).collect::<Vec<_>>(),
            [3, 2, 1]
        );
        for (index, scan) in scans.iter().enumerate() {
            assert!(scan.transform.is_none());
            assert_eq!(
                scan.name.as_deref(),
                Some(format!("unknown.PTX — block {}", index + 1).as_str())
            );
        }
        let actual = crate::open(&out, 100).unwrap();
        assert!(actual.scan_poses.is_empty());
        assert_eq!(actual.points.len(), cloud.points.len());
        for (actual, original) in actual.points.iter().zip(&cloud.points) {
            assert_eq!(actual.xyz, original.xyz);
            assert_eq!(actual.rgb, original.rgb);
            assert_eq!(actual.intensity, original.intensity);
        }
    }

    #[test]
    fn filtered_ptx_keeps_empty_scan_boundaries_and_unposed_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blocks.ptx");
        std::fs::write(&path, crate::ptx::tests::THREE_BLOCKS).unwrap();
        let cloud = crate::open(&path, 100).unwrap();
        let sources = [SceneSource {
            cloud: &cloud,
            transform: SceneTransform::default(),
        }];
        let out = dir.path().join("filtered.e57");
        assert_eq!(
            write_e57_scene(
                &sources,
                &out,
                true,
                &mut |_, ordinal, _| ordinal >= 3,
                &mut |_, _| Ok(())
            )
            .unwrap(),
            3
        );
        let reader = crate::e57_points::open_reader(&out).unwrap();
        assert_eq!(
            reader
                .pointclouds()
                .iter()
                .map(|scan| scan.records)
                .collect::<Vec<_>>(),
            [0, 2, 1]
        );
        let opened = crate::open(&out, 100).unwrap();
        assert_eq!(opened.scan_poses.len(), 2);
        assert!(opened.station_of(0).is_none());
        assert_eq!(opened.station_pose(2).unwrap().position, [40., 50., 60.]);
    }

    #[test]
    fn cancelling_a_posed_stream_releases_backpressure_and_keeps_the_destination() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.pcd");
        std::fs::write(&path, format!("FIELDS x y z\nSIZE 4 4 4\nTYPE F F F\nPOINTS 50000\nVIEWPOINT 10 20 30 1 0 0 0\nDATA ascii\n{}", "1 2 3\n".repeat(50000))).unwrap();
        let cloud = crate::open(&path, 100).unwrap();
        let sources = [SceneSource {
            cloud: &cloud,
            transform: SceneTransform::default(),
        }];
        let out = dir.path().join("cancel.e57");
        std::fs::write(&out, b"existing output").unwrap();
        let result = write_e57_scene(&sources, &out, true, &mut |_, _, _| true, &mut |_, _| {
            Err(LoadError::Cancelled)
        });
        assert!(matches!(result, Err(LoadError::Cancelled)));
        assert_eq!(std::fs::read(&out).unwrap(), b"existing output");
    }

    #[test]
    fn mixed_scene_keeps_registered_photos_and_photo_edits_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("slam.xyz");
        std::fs::write(&source, "0 0 0\n1 0 0\n0 1 0\n").unwrap();
        let jpeg = dir.path().join("camera.jpg");
        std::fs::write(&jpeg, b"encoded camera bytes").unwrap();
        let mut edits = PhotoEdits::default();
        edits
            .add(
                jpeg.clone(),
                FilePhoto {
                    name: Some("Walk panorama".into()),
                    station: None,
                    position: [1., 2., 3.],
                    axes: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
                    width: 4,
                    height: 2,
                    projection: PhotoProjection::Spherical {
                        pixel_size: [std::f64::consts::FRAC_PI_2; 2],
                    },
                    format: ScanImageFormat::Jpeg,
                    offset: 0,
                    length: 0,
                },
            )
            .unwrap();
        edits.save(&source).unwrap();
        let cloud = crate::open(&source, 10).unwrap();
        let transform = SceneTransform {
            rotation: [[0., -1., 0.], [1., 0., 0.], [0., 0., 1.]],
            translation: [10., 20., 30.],
            scale: 1.,
        };
        let static_file = dir.path().join("registered.e57");
        write_e57_scene(
            &[SceneSource {
                cloud: &cloud,
                transform,
            }],
            &static_file,
            true,
            &mut |_, _, _| true,
            &mut |_, _| Ok(()),
        )
        .unwrap();
        let registered = crate::open(&static_file, 10).unwrap();
        assert_eq!(registered.total_points, 3);
        assert_eq!(registered.points[1].xyz, [10., 21., 30.]);
        let photos = crate::file_photos(&static_file).unwrap();
        assert_eq!(photos.photos.len(), 1);
        let mut photo = photos.photos[0].clone();
        assert_eq!(photo.position, [8., 21., 33.]);
        assert_eq!(
            crate::read_file_photo(&static_file, &photo).unwrap(),
            b"encoded camera bytes"
        );
        photo.name = Some("Adjusted panorama".into());
        photo.position = [12., 23., 34.];
        let mut changed = PhotoEdits::default();
        changed.changes.insert(photo.offset, Some(photo.clone()));
        changed.save(&static_file).unwrap();
        let edited = dir.path().join("edited.e57");
        crate::export_full(&registered, &edited, crate::ExportFormat::E57).unwrap();
        let reread = crate::file_photos(&edited).unwrap();
        assert_eq!(reread.photos[0].name, photo.name);
        assert_eq!(reread.photos[0].position, photo.position);
        let mixed = dir.path().join("mixed.e57");
        write_e57_scene(
            &[
                SceneSource {
                    cloud: &registered,
                    transform: Default::default(),
                },
                SceneSource {
                    cloud: &cloud,
                    transform: Default::default(),
                },
            ],
            &mixed,
            true,
            &mut |_, _, _| true,
            &mut |_, _| Ok(()),
        )
        .unwrap();
        assert_eq!(crate::open(&mixed, 10).unwrap().total_points, 6);
        assert_eq!(crate::file_photos(&mixed).unwrap().photos.len(), 2);
        let bare = dir.path().join("points-only.e57");
        write_e57_scene(
            &[SceneSource {
                cloud: &registered,
                transform: Default::default(),
            }],
            &bare,
            false,
            &mut |_, _, _| true,
            &mut |_, _| Ok(()),
        )
        .unwrap();
        let bare_cloud = crate::open(&bare, 10).unwrap();
        assert!(bare_cloud.scan_poses.is_empty());
        assert!(crate::file_photos(&bare).unwrap().photos.is_empty());
        changed.changes.insert(photo.offset, None);
        changed.save(&static_file).unwrap();
        let deleted = dir.path().join("deleted.e57");
        crate::export_full(&registered, &deleted, crate::ExportFormat::E57).unwrap();
        assert!(crate::file_photos(&deleted).unwrap().photos.is_empty());
        let cancelled = dir.path().join("cancelled.e57");
        std::fs::write(&cancelled, b"existing").unwrap();
        assert!(write_e57_scene(
            &[SceneSource {
                cloud: &cloud,
                transform: Default::default()
            }],
            &cancelled,
            true,
            &mut |_, _, _| true,
            &mut |_, _| Err(LoadError::Cancelled)
        )
        .is_err());
        assert_eq!(std::fs::read(&cancelled).unwrap(), b"existing");
    }
    #[test]
    fn different_las_and_text_layouts_merge_into_normalised_laz() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.xyz");
        std::fs::write(&path, "0 0 0\n1 2 3\n").unwrap();
        let source = crate::open(&path, 10).unwrap();
        let las = dir.path().join("first.las");
        crate::export_full(&source, &las, crate::ExportFormat::Las).unwrap();
        let first = crate::open(&las, 10).unwrap();
        let output = dir.path().join("mixed.laz");
        assert_eq!(
            write_las_scene(
                &[
                    SceneSource {
                        cloud: &first,
                        transform: Default::default()
                    },
                    SceneSource {
                        cloud: &source,
                        transform: SceneTransform {
                            translation: [10., 0., 0.],
                            ..Default::default()
                        }
                    }
                ],
                &output,
                true,
                &mut |_, _, _| true,
                &mut |_, _| Ok(())
            )
            .unwrap(),
            4
        );
        let merged = crate::open(&output, 10).unwrap();
        assert_eq!(merged.total_points, 4);
        assert!((merged.bounds.max[0] - 11.).abs() < 1e-6);
    }
}
