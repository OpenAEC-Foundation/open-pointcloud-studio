//! Stream all scans in an E57 file through the pure-Rust e57 decoder.

use std::io::{Read, Seek};
use std::path::Path;

use e57::{Blob, CartesianCoordinate, E57Reader, ImageFormat, PointCloud, Projection, RecordName};

use super::window_reader::WindowReader;
use super::{
    quaternion_axes, Bounds, LoadError, Point, ScanImage, ScanImageFormat, ScanPose, ScanRange,
};

/// A stored photo larger than this is treated as damaged metadata.
const MAX_IMAGE_BYTES: u64 = 256 * 1024 * 1024;
/// Photos this close to a station belong to it when the file names no scan.
const STATION_MATCH_DISTANCE: f64 = 0.01;

/// Open an E57 file through a reader that keeps its buffer across the
/// decoder's page-by-page seeks.
pub(crate) fn open_reader(path: &Path) -> Result<E57Reader<WindowReader>, LoadError> {
    Ok(E57Reader::new(WindowReader::open(path)?)?)
}

/// The scanner station of every scan in a file that has one. A merged cloud
/// is stored as a scan too, but its pose only places its coordinates: the
/// only scan of a file has no station when it has neither the grid or angles
/// of a scanner sweep nor a name.
pub(crate) fn stations(scans: &[PointCloud]) -> Vec<Option<ScanPose>> {
    scans
        .iter()
        .enumerate()
        .map(|(index, scan)| scan_pose(index, scan, scans.len() == 1))
        .collect()
}

fn scan_pose(index: usize, scan: &PointCloud, alone: bool) -> Option<ScanPose> {
    let transform = scan.transform.as_ref()?;
    let swept = !alone
        || scan.index_bounds.is_some()
        || scan.spherical_bounds.is_some()
        || scan.prototype.iter().any(|record| {
            matches!(
                record.name,
                RecordName::RowIndex
                    | RecordName::ColumnIndex
                    | RecordName::SphericalRange
                    | RecordName::SphericalAzimuth
                    | RecordName::SphericalElevation
            )
        });
    if !swept && scan.name.as_ref().is_none_or(|name| name.is_empty()) {
        return None;
    }
    let position = [
        transform.translation.x,
        transform.translation.y,
        transform.translation.z,
    ];
    position
        .iter()
        .all(|value| value.is_finite())
        .then(|| ScanPose {
            label: scan
                .name
                .clone()
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| format!("Scan {}", index + 1)),
            position,
            axes: quaternion_axes([
                transform.rotation.w,
                transform.rotation.x,
                transform.rotation.y,
                transform.rotation.z,
            ]),
        })
}

/// The ordinal range of every scan as the metadata states it. A record that
/// holds no valid point is not read as a point, so the scans after it begin
/// earlier than stated: these ranges are exact only when the stated records
/// add up to the points that were read.
fn stated_ranges(scans: &[PointCloud]) -> Vec<ScanRange> {
    let (mut first_ordinal, mut station) = (0u64, 0u32);
    scans
        .iter()
        .zip(stations(scans))
        .map(|(scan, pose)| {
            let range = ScanRange {
                first_ordinal,
                station: pose.is_some().then_some(station),
            };
            first_ordinal = first_ordinal.saturating_add(scan.records);
            station += u32::from(pose.is_some());
            range
        })
        .collect()
}

/// Scan ranges for a cloud of `total_points` whose cache was written before
/// ranges were recorded, without decoding point records. `None` when the
/// metadata cannot tell them and only a pass over the points can.
pub(crate) fn ranges_for_count(
    path: &Path,
    total_points: u64,
) -> Result<Option<Vec<ScanRange>>, LoadError> {
    let file = open_reader(path)?;
    let scans = file.pointclouds();
    let records = scans
        .iter()
        .fold(0u64, |sum, scan| sum.saturating_add(scan.records));
    Ok(certain_ranges(stated_ranges(&scans), records, total_points))
}

/// Keep stated ranges that are certain: every stated record was read as a
/// point, or there is only one scan to own them. Otherwise nothing tells
/// where a scan ends.
fn certain_ranges(
    stated: Vec<ScanRange>,
    records: u64,
    total_points: u64,
) -> Option<Vec<ScanRange>> {
    (records == total_points || stated.len() <= 1).then_some(stated)
}

/// Read scanner stations from E57 metadata without decoding point records.
pub(crate) fn scan_poses(path: &Path) -> Result<Vec<ScanPose>, LoadError> {
    let file = open_reader(path)?;
    Ok(stations(&file.pointclouds())
        .into_iter()
        .flatten()
        .collect())
}

/// What E57 metadata says about a file before any point is decoded.
pub(crate) struct Summary {
    /// Stored point records; invalid records make the decoded count smaller.
    pub records: u64,
    /// Registered box around every scan that states its extent.
    pub bounds: Option<Bounds>,
    pub has_rgb: bool,
    pub has_intensity: bool,
    pub poses: Vec<ScanPose>,
}

/// Summarise an E57 file from its metadata alone.
pub(crate) fn summary(path: &Path) -> Result<Summary, LoadError> {
    let file = open_reader(path)?;
    let scans = file.pointclouds();
    let mut summary = Summary {
        records: 0,
        bounds: None,
        has_rgb: false,
        has_intensity: false,
        // Stations are numbered as the pass and the photos number them,
        // whatever a scan states about its extent below.
        poses: stations(&scans).into_iter().flatten().collect(),
    };
    for scan in &scans {
        summary.records += scan.records;
        summary.has_rgb |= scan.has_color();
        summary.has_intensity |= scan.has_intensity();
        if let Some(local) = scan.get_cartesian_bounds() {
            let (Some(x0), Some(x1), Some(y0), Some(y1), Some(z0), Some(z1)) = (
                local.x_min,
                local.x_max,
                local.y_min,
                local.y_max,
                local.z_min,
                local.z_max,
            ) else {
                continue;
            };
            // The stated extent is in the scan's own frame: register its
            // corners with the scan pose.
            let placed = scan.transform.as_ref().and_then(|transform| {
                quaternion_axes([
                    transform.rotation.w,
                    transform.rotation.x,
                    transform.rotation.y,
                    transform.rotation.z,
                ])
                .map(|axes| {
                    (
                        axes,
                        [
                            transform.translation.x,
                            transform.translation.y,
                            transform.translation.z,
                        ],
                    )
                })
            });
            for corner in 0..8 {
                let local = [
                    if corner & 1 == 0 { x0 } else { x1 },
                    if corner & 2 == 0 { y0 } else { y1 },
                    if corner & 4 == 0 { z0 } else { z1 },
                ];
                let xyz = match placed {
                    Some((axes, origin)) => std::array::from_fn(|axis| {
                        origin[axis]
                            + axes[0][axis] * local[0]
                            + axes[1][axis] * local[1]
                            + axes[2][axis] * local[2]
                    }),
                    None => local,
                };
                if !xyz.iter().all(|value: &f64| value.is_finite()) {
                    continue;
                }
                match &mut summary.bounds {
                    Some(bounds) => bounds.include(xyz),
                    None => summary.bounds = Some(Bounds { min: xyz, max: xyz }),
                }
            }
        }
    }
    Ok(summary)
}

/// Read the station photos listed in E57 metadata without decoding them.
/// Only pinhole photos with a pose are returned.
pub(crate) fn scan_images(path: &Path) -> Result<Vec<ScanImage>, LoadError> {
    let file = open_reader(path)?;
    // Stations are numbered like `scan_poses`: scans without a pose are skipped.
    let scans = file.pointclouds();
    let stations: Vec<_> = scans
        .iter()
        .zip(stations(&scans))
        .filter_map(|(scan, pose)| Some((scan.guid.clone(), pose?.position)))
        .collect();
    let mut images = Vec::new();
    for image in file.images() {
        let (Some(Projection::Pinhole(pinhole)), Some(transform)) =
            (&image.projection, &image.transform)
        else {
            continue;
        };
        let position = [
            transform.translation.x,
            transform.translation.y,
            transform.translation.z,
        ];
        let Some(axes) = quaternion_axes([
            transform.rotation.w,
            transform.rotation.x,
            transform.rotation.y,
            transform.rotation.z,
        ]) else {
            continue;
        };
        let properties = &pinhole.properties;
        let focal = [
            properties.focal_length / properties.pixel_width,
            properties.focal_length / properties.pixel_height,
        ];
        let principal = [properties.principal_x, properties.principal_y];
        let blob = &pinhole.blob.data;
        if properties.width == 0
            || properties.height == 0
            || blob.length == 0
            || blob.length > MAX_IMAGE_BYTES
            || !position
                .iter()
                .chain(&focal)
                .chain(&principal)
                .all(|value| value.is_finite())
            || focal.iter().any(|value| *value <= 0.0)
        {
            continue;
        }
        let station = image
            .pointcloud_guid
            .as_ref()
            .and_then(|guid| {
                stations
                    .iter()
                    .position(|(scan, _)| scan.as_ref() == Some(guid))
            })
            .or_else(|| {
                stations.iter().position(|(_, station)| {
                    (0..3).all(|axis| {
                        (station[axis] - position[axis]).abs() <= STATION_MATCH_DISTANCE
                    })
                })
            });
        images.push(ScanImage {
            station,
            position,
            axes,
            width: properties.width,
            height: properties.height,
            focal,
            principal,
            format: match pinhole.blob.format {
                ImageFormat::Png => ScanImageFormat::Png,
                ImageFormat::Jpeg => ScanImageFormat::Jpeg,
            },
            offset: blob.offset,
            length: blob.length,
        });
    }
    Ok(images)
}

/// Read the encoded bytes of station photos, in the order given.
pub(crate) fn read_images(path: &Path, images: &[ScanImage]) -> Result<Vec<Vec<u8>>, LoadError> {
    let mut file = open_reader(path)?;
    let source_length = std::fs::metadata(path)?.len();
    images
        .iter()
        .map(|image| {
            if image.length > MAX_IMAGE_BYTES
                || image
                    .offset
                    .checked_add(image.length)
                    .is_none_or(|end| end > source_length)
            {
                return Err(LoadError::InvalidData(
                    "station photo lies outside the file".into(),
                ));
            }
            let mut bytes = Vec::with_capacity(image.length as usize);
            let written = file.blob(&Blob::new(image.offset, image.length), &mut bytes)?;
            if written != image.length {
                return Err(LoadError::InvalidData("station photo is truncated".into()));
            }
            Ok(bytes)
        })
        .collect()
}

pub fn read(
    path: &Path,
    push: &mut impl FnMut(Point) -> Result<(), LoadError>,
    scan_begin: &mut impl FnMut(Option<ScanPose>),
) -> Result<(), LoadError> {
    let mut file = open_reader(path)?;
    let scans = file.pointclouds();
    for (scan, pose) in scans.iter().zip(stations(&scans)) {
        // A scan without a station is reported too: its points must not be
        // taken for those of the scan before it.
        scan_begin(pose);
        read_scan(&mut file, scan, push)?;
    }
    Ok(())
}

/// Stream the registered, valid points of one scan.
pub(crate) fn read_scan<T: Read + Seek>(
    file: &mut E57Reader<T>,
    scan: &PointCloud,
    push: &mut impl FnMut(Point) -> Result<(), LoadError>,
) -> Result<(), LoadError> {
    let mut points = file.pointcloud_simple(scan)?;
    points.spherical_to_cartesian(true);
    // Preserve whether the scan actually contains RGB. The e57 crate's
    // default converts intensity-only points into synthetic grey colors.
    points.intensity_to_color(false);
    points.apply_pose(true);
    for point in points {
        let point = point?;
        let CartesianCoordinate::Valid { x, y, z } = &point.cartesian else {
            continue;
        };
        let xyz = [*x, *y, *z];
        push(simple_point(point, xyz))?;
    }
    Ok(())
}

pub(crate) fn simple_point(point: e57::Point, xyz: [f64; 3]) -> Point {
    Point {
        xyz,
        rgb: point.color.map(|color| {
            [color.red, color.green, color.blue]
                .map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8)
        }),
        intensity: point
            .intensity
            .map(|value| (value.clamp(0.0, 1.0) * 65535.0).round() as u16),
        classification: None,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::fs;

    use e57::{
        CartesianBounds, E57Writer, PointCloudWriter, Record, RecordValue, Transform, Translation,
    };

    use super::*;

    /// Position of the station of scan `index` in a file `write_scans` wrote.
    pub(crate) fn station_position(index: usize) -> [f64; 3] {
        [100.0 * (index + 1) as f64, 0.0, 0.0]
    }

    /// Write one scan per entry: whether it has a station, and for each of
    /// its records whether it holds a valid point. Points lie along X from
    /// the origin of their scan. A scan takes four records at a time: the
    /// validity field is two bits wide and is only read back in whole bytes.
    pub(crate) fn write_scans(path: &Path, scans: &[(bool, &[bool])]) {
        write_scans_with(path, scans, |_, _| {});
    }

    /// Like `write_scans`, and let `adjust` change the metadata of every
    /// scan after its last record, which is when the writer takes bounds.
    fn write_scans_with(
        path: &Path,
        scans: &[(bool, &[bool])],
        adjust: impl Fn(usize, &mut PointCloudWriter<fs::File>),
    ) {
        assert!(scans
            .iter()
            .all(|(_, records)| records.len().is_multiple_of(4)));
        let mut writer = E57Writer::new(
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(path)
                .unwrap(),
            "{00000000-0000-4000-8000-000000000100}",
        )
        .unwrap();
        for (index, (station, records)) in scans.iter().enumerate() {
            let mut scan = writer
                .add_pointcloud(
                    &format!("{{00000000-0000-4000-8000-0000000002{index:02}}}"),
                    vec![
                        Record::CARTESIAN_X_F64,
                        Record::CARTESIAN_Y_F64,
                        Record::CARTESIAN_Z_F64,
                        Record::CARTESIAN_INVALID_STATE,
                    ],
                )
                .unwrap();
            if *station {
                let [x, y, z] = station_position(index);
                scan.set_name(Some(format!("Station {index}")));
                scan.set_transform(Some(Transform {
                    rotation: Default::default(),
                    translation: Translation { x, y, z },
                }));
            }
            for (record, valid) in records.iter().enumerate() {
                scan.add_point(vec![
                    RecordValue::Double(record as f64 + 1.0),
                    RecordValue::Double(0.0),
                    RecordValue::Double(0.0),
                    RecordValue::Integer(if *valid { 0 } else { 2 }),
                ])
                .unwrap();
            }
            adjust(index, &mut scan);
            scan.finalize().unwrap();
        }
        writer.finalize().unwrap();
    }

    fn range(first_ordinal: u64, station: Option<u32>) -> ScanRange {
        ScanRange {
            first_ordinal,
            station,
        }
    }

    #[test]
    fn every_scan_begins_a_scan_range() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("three-scans.e57");
        write_scans(
            &source,
            &[(true, &[true; 4]), (false, &[true; 4]), (true, &[true; 4])],
        );
        let cloud = crate::open(&source, 20).unwrap();
        assert_eq!(cloud.total_points, 12);
        assert_eq!(cloud.scan_poses.len(), 2);
        let expected = [range(0, Some(0)), range(4, None), range(8, Some(1))];
        assert_eq!(cloud.scan_ranges, expected);
        assert_eq!(cloud.point_ordinals, (0..12).collect::<Vec<u64>>());
        for (point, ordinal) in cloud.points.iter().zip(&cloud.point_ordinals) {
            let station = cloud.station_pose(*ordinal).map(|pose| pose.position);
            let expected = match point.xyz[0] {
                x if x < 100.0 => None,
                x if x < 300.0 => Some(station_position(0)),
                _ => Some(station_position(2)),
            };
            assert_eq!(station, expected, "ordinal {ordinal}");
        }
        assert!(cloud.station_pose(12).is_none());

        // The metadata states the same ranges when every record is a point.
        assert_eq!(ranges_for_count(&source, 12).unwrap().unwrap(), expected);

        // A cloud made from the metadata alone has the stations and tells
        // none: its count is the stated one and its points are not read yet.
        let header = crate::open_e57_header(&source).unwrap();
        assert_eq!(header.scan_poses, cloud.scan_poses);
        assert!(header.scan_ranges.is_empty() && !header.scan_ranges_known());
        assert!((0..12).all(|ordinal| header.station_of(ordinal).is_none()));
        assert!(cloud.scan_ranges_known());
    }

    #[test]
    fn records_without_a_point_shift_the_ranges_of_later_scans() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("invalid-record.e57");
        write_scans(
            &source,
            &[
                (true, &[true, false, true, true]),
                (false, &[true; 4]),
                (true, &[true; 4]),
            ],
        );
        let cloud = crate::open(&source, 20).unwrap();
        assert_eq!(cloud.total_points, 11);
        assert_eq!(
            cloud.scan_ranges,
            [range(0, Some(0)), range(3, None), range(7, Some(1))]
        );
        assert_eq!(cloud.station_pose(2).unwrap().position, station_position(0));
        assert!(cloud.station_pose(3).is_none());
        assert_eq!(cloud.station_pose(7).unwrap().position, station_position(2));

        // The stated counts no longer add up to the points, so they cannot
        // stand in for the ranges of the pass.
        assert_eq!(
            stated_ranges(&open_reader(&source).unwrap().pointclouds()),
            [range(0, Some(0)), range(4, None), range(8, Some(1))]
        );
        assert!(ranges_for_count(&source, 11).unwrap().is_none());
    }

    #[test]
    fn a_scan_that_states_part_of_its_extent_keeps_its_station() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("partial-bounds.e57");
        write_scans_with(
            &source,
            &[(true, &[true; 4]), (true, &[true; 4])],
            |index, scan| {
                if index == 0 {
                    scan.set_cartesian_bounds(Some(CartesianBounds {
                        x_min: Some(1.0),
                        x_max: Some(4.0),
                        y_min: Some(0.0),
                        y_max: Some(0.0),
                        z_min: Some(0.0),
                        z_max: None,
                    }));
                }
            },
        );
        // The stations are those of the pass and of the photo numbering,
        // whatever the scans state about their extent.
        let header = crate::open_e57_header(&source).unwrap();
        assert_eq!(header.scan_poses, scan_poses(&source).unwrap());
        assert_eq!(header.scan_poses.len(), 2);
        assert_eq!(header.scan_poses[0].position, station_position(0));
        assert_eq!(header.scan_poses[1].position, station_position(1));
        assert_eq!(
            crate::open(&source, 8).unwrap().scan_poses,
            header.scan_poses
        );
    }

    #[test]
    fn stated_ranges_are_kept_only_when_they_are_certain() {
        let stated = vec![range(0, Some(0)), range(40, None), range(100, Some(1))];
        assert_eq!(
            certain_ranges(stated.clone(), 150, 150),
            Some(stated.clone())
        );
        // One record held no point: which scan lost it is not known.
        assert_eq!(certain_ranges(stated.clone(), 150, 149), None);
        assert_eq!(certain_ranges(stated, 150, 151), None);
        // A single scan owns every point, however many records were valid.
        let single = vec![range(0, Some(0))];
        assert_eq!(certain_ranges(single.clone(), 150, 149), Some(single));
        assert_eq!(certain_ranges(Vec::new(), 0, 0), Some(Vec::new()));
    }
}
