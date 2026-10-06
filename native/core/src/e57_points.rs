//! Stream all scans in an E57 file through the pure-Rust e57 decoder.

use std::io::{Read, Seek};
use std::path::Path;

use e57::{
    Blob, CartesianCoordinate, E57Reader, ImageFormat, PinholeImageProperties, PointCloud,
    Projection, RecordName,
};

use super::window_reader::WindowReader;
use super::{
    quaternion_axes, Bounds, FilePhoto, FilePhotos, LoadError, PhotoProjection, Point, ScanImage,
    ScanImageFormat, ScanPose, ScanRange,
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

/// Read the station photos listed in E57 metadata without decoding them:
/// the pinhole photos with a pose that a scanner station took.
pub(crate) fn scan_images(path: &Path) -> Result<Vec<ScanImage>, LoadError> {
    Ok(placed_images(&open_reader(path)?).0)
}

/// Read the other photos listed in E57 metadata without decoding them, with
/// the coordinate system the file states.
pub(crate) fn file_photos(path: &Path) -> Result<FilePhotos, LoadError> {
    Ok(placed_images(&open_reader(path)?).1)
}

/// Focal length of a pinhole photo in pixels along its columns and rows.
/// The standard gives the focal length and the size of a pixel in metres; a
/// file that states a pixel size of zero gives the focal length in pixels.
/// A negative pixel size gives no focal length.
fn pinhole_focal(properties: &PinholeImageProperties) -> [f64; 2] {
    let along = |pixel: f64| {
        if pixel > 0.0 {
            properties.focal_length / pixel
        } else if pixel == 0.0 {
            properties.focal_length
        } else {
            f64::NAN
        }
    };
    [
        along(properties.pixel_width),
        along(properties.pixel_height),
    ]
}

/// Every photo of a file that has a pose and a projection, without decoding
/// it: the pinhole photos of scanner stations, and the other photos with the
/// coordinate system the file states. Stations are numbered like
/// `scan_poses`: scans without a pose are skipped.
fn placed_images<T: Read + Seek>(file: &E57Reader<T>) -> (Vec<ScanImage>, FilePhotos) {
    let scans = file.pointclouds();
    let stations: Vec<_> = scans
        .iter()
        .zip(stations(&scans))
        .filter_map(|(scan, pose)| Some((scan.guid.clone(), pose?.position)))
        .collect();
    let mut images = Vec::new();
    let mut photos = FilePhotos {
        coordinate_system: file
            .coordinate_metadata()
            .map(str::trim)
            .filter(|system| !system.is_empty())
            .map(str::to_owned),
        ..FilePhotos::default()
    };
    for image in file.images() {
        let (Some(projection), Some(transform)) = (&image.projection, &image.transform) else {
            photos.skipped += 1;
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
            photos.skipped += 1;
            continue;
        };
        let (blob, width, height, projection) = match projection {
            Projection::Pinhole(pinhole) => {
                let properties = &pinhole.properties;
                (
                    &pinhole.blob,
                    properties.width,
                    properties.height,
                    PhotoProjection::Pinhole {
                        focal: pinhole_focal(properties),
                        principal: [properties.principal_x, properties.principal_y],
                    },
                )
            }
            Projection::Spherical(spherical) => {
                let properties = &spherical.properties;
                (
                    &spherical.blob,
                    properties.width,
                    properties.height,
                    PhotoProjection::Spherical {
                        pixel_size: [properties.pixel_width, properties.pixel_height],
                    },
                )
            }
            Projection::Cylindrical(cylindrical) => {
                let properties = &cylindrical.properties;
                (
                    &cylindrical.blob,
                    properties.width,
                    properties.height,
                    PhotoProjection::Cylindrical {
                        pixel_size: [properties.pixel_width, properties.pixel_height],
                        radius: properties.radius,
                        principal_row: properties.principal_y,
                    },
                )
            }
        };
        // Lengths and angles that place the photo, which are all positive,
        // and the pixels it is placed from.
        let (sizes, offsets) = match projection {
            PhotoProjection::Pinhole { focal, principal } => ([focal[0], focal[1], 1.0], principal),
            PhotoProjection::Spherical { pixel_size } => {
                ([pixel_size[0], pixel_size[1], 1.0], [0.0; 2])
            }
            PhotoProjection::Cylindrical {
                pixel_size,
                radius,
                principal_row,
            } => ([pixel_size[0], pixel_size[1], radius], [principal_row, 0.0]),
        };
        if width == 0
            || height == 0
            || blob.data.length == 0
            || blob.data.length > MAX_IMAGE_BYTES
            || !position
                .iter()
                .chain(&sizes)
                .chain(&offsets)
                .all(|value| value.is_finite())
            || sizes.iter().any(|value| *value <= 0.0)
        {
            photos.skipped += 1;
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
        let format = match blob.format {
            ImageFormat::Png => ScanImageFormat::Png,
            ImageFormat::Jpeg => ScanImageFormat::Jpeg,
        };
        match (projection, station) {
            (PhotoProjection::Pinhole { focal, principal }, Some(station)) => {
                images.push(ScanImage {
                    station: Some(station),
                    position,
                    axes,
                    width,
                    height,
                    focal,
                    principal,
                    format,
                    offset: blob.data.offset,
                    length: blob.data.length,
                })
            }
            (projection, station) => photos.photos.push(FilePhoto {
                name: image.name.clone().filter(|name| !name.trim().is_empty()),
                station,
                position,
                axes,
                width,
                height,
                projection,
                format,
                offset: blob.data.offset,
                length: blob.data.length,
            }),
        }
    }
    (images, photos)
}

/// Read the stored bytes of one photo from a reader of its file.
fn read_blob<T: Read + Seek>(
    file: &mut E57Reader<T>,
    source_length: u64,
    offset: u64,
    length: u64,
) -> Result<Vec<u8>, LoadError> {
    if length > MAX_IMAGE_BYTES
        || offset
            .checked_add(length)
            .is_none_or(|end| end > source_length)
    {
        return Err(LoadError::InvalidData("photo lies outside the file".into()));
    }
    let mut bytes = Vec::with_capacity(length as usize);
    let written = file.blob(&Blob::new(offset, length), &mut bytes)?;
    if written != length {
        return Err(LoadError::InvalidData("photo is truncated".into()));
    }
    Ok(bytes)
}

/// Read the encoded bytes of station photos, in the order given.
pub(crate) fn read_images(path: &Path, images: &[ScanImage]) -> Result<Vec<Vec<u8>>, LoadError> {
    let mut file = open_reader(path)?;
    let source_length = std::fs::metadata(path)?.len();
    images
        .iter()
        .map(|image| read_blob(&mut file, source_length, image.offset, image.length))
        .collect()
}

/// Read the encoded bytes of one photo of a file.
pub(crate) fn read_photo(path: &Path, photo: &FilePhoto) -> Result<Vec<u8>, LoadError> {
    let mut file = open_reader(path)?;
    let source_length = std::fs::metadata(path)?.len();
    read_blob(&mut file, source_length, photo.offset, photo.length)
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
        CartesianBounds, CylindricalImageProperties, E57Writer, PointCloudWriter, Quaternion,
        Record, RecordValue, SphericalImageProperties, Transform, Translation,
        VisualReferenceImageProperties,
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

    const SCAN_GUID: &str = "{00000000-0000-4000-8000-000000000200}";

    /// Write one scan of four points, a station when `station` is given and
    /// a scan placed only by its pose otherwise, let `photos` add images and
    /// let `xml` change the metadata before it is written.
    pub(crate) fn write_with_photos(
        path: &Path,
        station: Option<[f64; 3]>,
        coordinate_system: Option<&str>,
        photos: impl FnOnce(&mut E57Writer<fs::File>),
        xml: impl Fn(String) -> String,
    ) {
        let mut writer =
            E57Writer::from_file(path, "{00000000-0000-4000-8000-000000000100}").unwrap();
        writer.set_coordinate_metadata(coordinate_system.map(str::to_owned));
        let mut scan = writer
            .add_pointcloud(
                SCAN_GUID,
                vec![
                    Record::CARTESIAN_X_F64,
                    Record::CARTESIAN_Y_F64,
                    Record::CARTESIAN_Z_F64,
                ],
            )
            .unwrap();
        let [x, y, z] = station.unwrap_or([1_000.0, 2_000.0, 50.0]);
        if station.is_some() {
            scan.set_name(Some("Station 0".into()));
        }
        scan.set_transform(Some(Transform {
            rotation: Default::default(),
            translation: Translation { x, y, z },
        }));
        for index in 0..4 {
            scan.add_point(vec![
                RecordValue::Double(f64::from(index)),
                RecordValue::Double(1.0),
                RecordValue::Double(0.5),
            ])
            .unwrap();
        }
        scan.finalize().unwrap();
        photos(&mut writer);
        writer
            .finalize_customized_xml(|text| Ok(xml(text)))
            .unwrap();
    }

    pub(crate) fn pose(rotation: [f64; 4], position: [f64; 3]) -> Transform {
        let [w, x, y, z] = rotation;
        let [px, py, pz] = position;
        Transform {
            rotation: Quaternion { w, x, y, z },
            translation: Translation {
                x: px,
                y: py,
                z: pz,
            },
        }
    }

    fn image_guid(index: usize) -> String {
        format!("{{00000000-0000-4000-8000-0000000003{index:02}}}")
    }

    /// The rotation of a cube face of a scanner station turned 27.44
    /// degrees about the vertical.
    const FACE: [f64; 4] = [0.6869, 0.6869, 0.1677, 0.1677];
    const STATION: [f64; 3] = [-2.19, 3.81, 0.0];

    /// The station photos of a scanner keep exactly the numbers the reader
    /// has always taken from them: the focal length divided by the size of a
    /// pixel, both in metres, and the principal point as stated.
    #[test]
    fn station_photos_keep_their_focal_length_and_principal_point() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("station-photos.e57");
        let bytes = b"stand-in for a stored JPEG".to_vec();
        write_with_photos(
            &source,
            Some(STATION),
            None,
            |writer| {
                // One photo names its scan; the other stands at the station.
                for (index, (named, position)) in [(true, STATION), (false, [-2.19, 3.81, 0.005])]
                    .into_iter()
                    .enumerate()
                {
                    let mut image = writer.add_image(&image_guid(index)).unwrap();
                    image.set_transform(pose(FACE, position));
                    if named {
                        image.set_pointcloud_guid(SCAN_GUID);
                    }
                    image
                        .add_pinhole(
                            ImageFormat::Jpeg,
                            &mut bytes.as_slice(),
                            PinholeImageProperties {
                                width: 2048,
                                height: 2048,
                                focal_length: 0.002_047,
                                pixel_width: 0.000_002,
                                pixel_height: 0.000_002,
                                principal_x: 1023.5,
                                principal_y: 1023.5,
                            },
                            None,
                        )
                        .unwrap();
                    image.finalize().unwrap();
                }
            },
            |xml| xml,
        );
        let images = scan_images(&source).unwrap();
        assert_eq!(images.len(), 2);
        for (image, position) in images.iter().zip([STATION, [-2.19, 3.81, 0.005]]) {
            assert_eq!(image.station, Some(0));
            assert_eq!(image.position, position);
            assert_eq!(image.axes, quaternion_axes(FACE).unwrap());
            assert_eq!((image.width, image.height), (2048, 2048));
            assert_eq!(image.focal, [0.002_047 / 0.000_002; 2]);
            assert!((image.focal[0] - 1023.5).abs() < 1e-9);
            assert_eq!(image.principal, [1023.5, 1023.5]);
            assert_eq!(image.format, ScanImageFormat::Jpeg);
            assert_eq!(image.length, bytes.len() as u64);
        }
        assert_eq!(
            read_images(&source, &images).unwrap(),
            vec![bytes.clone(), bytes]
        );
        // They are station photos, not photos of their own.
        let photos = file_photos(&source).unwrap();
        assert!(photos.photos.is_empty());
        assert_eq!(photos.skipped, 0);
        assert_eq!(photos.coordinate_system, None);
    }

    /// Photos taken along a path by a camera that measured no points, as
    /// some files store them: no pixel size, so the focal length and the
    /// principal point are in pixels.
    #[test]
    fn a_pinhole_photo_without_a_pixel_size_states_its_focal_length_in_pixels() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("path-photos.e57");
        let bytes = b"another stand-in".to_vec();
        let rotation = [0.7, 0.5, 0.1, 0.5];
        write_with_photos(
            &source,
            None,
            Some(" EPSG:28992 "),
            |writer| {
                let mut image = writer.add_image(&image_guid(0)).unwrap();
                image.set_transform(pose(rotation, [1_001.0, 2_002.0, 51.5]));
                image
                    .add_pinhole(
                        ImageFormat::Png,
                        &mut bytes.as_slice(),
                        PinholeImageProperties {
                            width: 1200,
                            height: 900,
                            focal_length: 1_000.0,
                            pixel_width: 0.0,
                            pixel_height: 0.0,
                            principal_x: 600.0,
                            principal_y: 450.0,
                        },
                        None,
                    )
                    .unwrap();
                image.finalize().unwrap();
            },
            // Such files leave the elements empty.
            |xml| {
                xml.replace(
                    "<pixelWidth type=\"Float\">0</pixelWidth>",
                    "<pixelWidth type=\"Float\"/>",
                )
                .replace(
                    "<pixelHeight type=\"Float\">0</pixelHeight>",
                    "<pixelHeight type=\"Float\"/>",
                )
            },
        );
        let xml = E57Reader::raw_xml(fs::File::open(&source).unwrap()).unwrap();
        assert!(String::from_utf8(xml)
            .unwrap()
            .contains("<pixelWidth type=\"Float\"/>"));
        assert!(scan_images(&source).unwrap().is_empty());
        let photos = file_photos(&source).unwrap();
        assert_eq!(photos.coordinate_system.as_deref(), Some("EPSG:28992"));
        assert_eq!(photos.skipped, 0);
        assert_eq!(
            photos.photos,
            [FilePhoto {
                name: None,
                station: None,
                position: [1_001.0, 2_002.0, 51.5],
                axes: quaternion_axes(rotation).unwrap(),
                width: 1200,
                height: 900,
                projection: PhotoProjection::Pinhole {
                    focal: [1_000.0, 1_000.0],
                    principal: [600.0, 450.0],
                },
                format: ScanImageFormat::Png,
                offset: photos.photos[0].offset,
                length: bytes.len() as u64,
            }]
        );
        assert_eq!(read_photo(&source, &photos.photos[0]).unwrap(), bytes);
        let moved = FilePhoto {
            offset: u64::MAX - 4,
            ..photos.photos[0].clone()
        };
        assert!(read_photo(&source, &moved).is_err());
    }

    #[test]
    fn panoramas_of_every_kind_are_listed_and_previews_are_skipped() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("panoramas.e57");
        let bytes = b"panorama".to_vec();
        let station = [5.0, 6.0, 7.0];
        let half = std::f64::consts::FRAC_1_SQRT_2;
        write_with_photos(
            &source,
            Some(station),
            None,
            |writer| {
                let mut image = writer.add_image(&image_guid(0)).unwrap();
                image.set_name("Panorama 1");
                image.set_transform(pose([half, 0.0, 0.0, half], [1.0, 2.0, 3.0]));
                let spherical = SphericalImageProperties {
                    width: 64,
                    height: 32,
                    pixel_width: std::f64::consts::TAU / 64.0,
                    pixel_height: std::f64::consts::PI / 32.0,
                };
                image
                    .add_spherical(
                        ImageFormat::Jpeg,
                        &mut bytes.as_slice(),
                        spherical.clone(),
                        None,
                    )
                    .unwrap();
                image.finalize().unwrap();
                // A cylinder at the station.
                let mut image = writer.add_image(&image_guid(1)).unwrap();
                image.set_transform(pose([1.0, 0.0, 0.0, 0.0], station));
                image
                    .add_cylindrical(
                        ImageFormat::Jpeg,
                        &mut bytes.as_slice(),
                        CylindricalImageProperties {
                            width: 360,
                            height: 100,
                            radius: 1.0,
                            principal_y: 50.0,
                            pixel_width: std::f64::consts::TAU / 360.0,
                            pixel_height: 0.01,
                        },
                        None,
                    )
                    .unwrap();
                image.finalize().unwrap();
                // A preview cannot be placed.
                let mut image = writer.add_image(&image_guid(2)).unwrap();
                image.set_transform(pose([1.0, 0.0, 0.0, 0.0], station));
                image
                    .add_visual_reference(
                        ImageFormat::Jpeg,
                        &mut bytes.as_slice(),
                        VisualReferenceImageProperties {
                            width: 64,
                            height: 32,
                        },
                        None,
                    )
                    .unwrap();
                image.finalize().unwrap();
                // Nor can a panorama without a pose.
                let mut image = writer.add_image(&image_guid(3)).unwrap();
                image
                    .add_spherical(ImageFormat::Jpeg, &mut bytes.as_slice(), spherical, None)
                    .unwrap();
                image.finalize().unwrap();
            },
            // The writer misspells one element that the reader requires.
            |xml| xml.replace("readius", "radius"),
        );
        let photos = file_photos(&source).unwrap();
        assert_eq!(photos.skipped, 2);
        assert_eq!(photos.photos.len(), 2);
        let panorama = &photos.photos[0];
        assert_eq!(panorama.name.as_deref(), Some("Panorama 1"));
        assert_eq!(panorama.station, None);
        assert_eq!(panorama.kind(), crate::PhotoKind::Spherical);
        assert_eq!((panorama.width, panorama.height), (64, 32));
        assert_eq!(panorama.position, [1.0, 2.0, 3.0]);
        let middle = panorama.project([0.0, 1.0, 0.0]).unwrap();
        assert!((middle[0] - 31.5).abs() < 1e-9 && (middle[1] - 15.5).abs() < 1e-9);
        let cylinder = &photos.photos[1];
        assert_eq!(cylinder.station, Some(0));
        assert_eq!(
            cylinder.projection,
            PhotoProjection::Cylindrical {
                pixel_size: [std::f64::consts::TAU / 360.0, 0.01],
                radius: 1.0,
                principal_row: 50.0,
            }
        );
        assert!(scan_images(&source).unwrap().is_empty());
        assert_eq!(read_photo(&source, cylinder).unwrap(), bytes);
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
