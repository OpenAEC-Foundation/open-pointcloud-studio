//! A first look at a large E57 file within seconds: point records taken from
//! evenly spaced data packets, read on several threads, while everything in
//! between stays unread.
//!
//! This works for scans whose record fields all fill whole bytes and whose
//! packets all hold the same number of records, because such a packet can be
//! decoded without the packets before it. The sampled records are packed
//! into a small E57 image in memory and decoded by the same reader as a full
//! pass, so both give the same values.
//!
//! Only the packets that are read can be checked. A file whose unread
//! packets break that layout shows a wrong preview, which is why the result
//! is marked provisional and gives way to the checked cloud of the full pass.

use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{Duration, Instant};

use e57::{E57Reader, Record, RecordDataType};
use rayon::prelude::*;

use super::e57_points;
use super::window_reader::WindowReader;
use super::{Bounds, LoadError, Point, PointCloud, SourceStamp};

const FILE_HEADER_BYTES: usize = 48;
const SECTION_HEADER_BYTES: usize = 32;
const PACKET_HEADER_BYTES: usize = 6;
const CHECKSUM_BYTES: u64 = 4;
const MAX_PAGE_BYTES: u64 = 1024 * 1024;
const MAX_PACKET_BYTES: usize = 65_536;
const DATA_PACKET: u8 = 1;
/// Page size of the image built in memory.
const IMAGE_PAGE_BYTES: usize = 1024;
/// Records taken from one packet when the file has packets to spare: few
/// enough to spread the preview over many places in the file.
const RECORDS_PER_PACKET: u64 = 256;
/// Most source bytes one preview reads.
const MAX_READ_BYTES: u64 = 512 * 1024 * 1024;
/// Time after which a preview makes do with the packets read so far. A
/// source that seeks slowly, such as disks behind a network share, would
/// otherwise take as long over the preview as over reading the whole file.
const SAMPLING_TIME: Duration = Duration::from_secs(6);

/// A source file read by logical offset, which skips the checksum that ends
/// every page and verifies it on the way.
struct Source {
    file: File,
    page: u64,
    length: u64,
    raw: Vec<u8>,
}

impl Source {
    fn open(path: &Path, page: u64) -> Result<Self, LoadError> {
        let file = File::open(path)?;
        Ok(Self {
            length: file.metadata()?.len(),
            file,
            page,
            raw: Vec::new(),
        })
    }

    fn read(&mut self, at: u64, length: usize, out: &mut Vec<u8>) -> Result<(), LoadError> {
        out.clear();
        if length == 0 {
            return Ok(());
        }
        let data = self.page - CHECKSUM_BYTES;
        let first = at / data;
        let last = (at + length as u64 - 1) / data;
        self.raw
            .resize(((last - first + 1) * self.page) as usize, 0);
        self.file.seek(SeekFrom::Start(first * self.page))?;
        self.file.read_exact(&mut self.raw)?;
        for (number, page) in (first..).zip(self.raw.chunks_exact(self.page as usize)) {
            let (content, checksum) = page.split_at(data as usize);
            if crc32c::crc32c(content).to_be_bytes() != checksum {
                return Err(LoadError::InvalidData(format!(
                    "E57 page {number} fails its checksum"
                )));
            }
            let from = if number == first {
                at - first * data
            } else {
                0
            };
            let to = if number == last {
                at + length as u64 - last * data
            } else {
                data
            };
            out.extend_from_slice(&content[from as usize..to as usize]);
        }
        Ok(())
    }
}

fn logical_offset(physical: u64, page: u64) -> u64 {
    physical - physical / page * CHECKSUM_BYTES
}

/// Bytes per value of every record field, when all of them fill whole bytes.
fn field_widths(prototype: &[Record]) -> Option<Vec<usize>> {
    let widths = prototype
        .iter()
        .map(|record| {
            let bits = match record.data_type {
                RecordDataType::Single { .. } => 32,
                RecordDataType::Double { .. } => 64,
                RecordDataType::ScaledInteger { min, max, .. }
                | RecordDataType::Integer { min, max } => {
                    let range = i128::from(max) - i128::from(min);
                    if range > 0 {
                        range.ilog2() + 1
                    } else {
                        0
                    }
                }
            };
            bits.is_multiple_of(8).then_some(bits as usize / 8)
        })
        .collect::<Option<Vec<_>>>()?;
    widths.iter().any(|width| *width > 0).then_some(widths)
}

fn packet_length(bytes: &[u8]) -> usize {
    usize::from(u16::from_le_bytes([bytes[2], bytes[3]])) + 1
}

/// Length and record count of the data packet that starts `bytes`, when
/// every field stream in it holds the same number of whole records.
fn data_packet(bytes: &[u8], widths: &[usize]) -> Option<(usize, usize)> {
    let streams = PACKET_HEADER_BYTES + 2 * widths.len();
    if bytes.len() < streams || bytes[0] != DATA_PACKET {
        return None;
    }
    let length = packet_length(bytes);
    if !length.is_multiple_of(4)
        || usize::from(u16::from_le_bytes([bytes[4], bytes[5]])) != widths.len()
    {
        return None;
    }
    let mut payload = streams;
    let mut records = None;
    for (index, width) in widths.iter().enumerate() {
        let at = PACKET_HEADER_BYTES + 2 * index;
        let stream = usize::from(u16::from_le_bytes([bytes[at], bytes[at + 1]]));
        payload += stream;
        if *width == 0 {
            if stream != 0 {
                return None;
            }
            continue;
        }
        if !stream.is_multiple_of(*width)
            || *records.get_or_insert(stream / width) != stream / width
        {
            return None;
        }
    }
    let records = records.filter(|records| *records > 0)?;
    // A packet ends at the first multiple of four after its streams.
    (payload <= length && length - payload < 4).then_some((length, records))
}

/// A packet reduced to the sampled records, and how many it holds.
type Thinned = (Vec<u8>, usize);

/// A data packet holding `take` records spread evenly over those of the
/// complete source packet at the start of `bytes`.
fn thinned_packet(bytes: &[u8], widths: &[usize], records: usize, take: usize) -> Vec<u8> {
    let take = take.clamp(1, records);
    let mut packet = vec![DATA_PACKET, 0, 0, 0];
    packet.extend_from_slice(&(widths.len() as u16).to_le_bytes());
    for width in widths {
        packet.extend_from_slice(&((take * width) as u16).to_le_bytes());
    }
    let mut stream = PACKET_HEADER_BYTES + 2 * widths.len();
    for width in widths {
        for pick in 0..take {
            let record = pick * records / take;
            packet.extend_from_slice(&bytes[stream + record * width..][..*width]);
        }
        stream += records * width;
    }
    packet.resize(packet.len().next_multiple_of(4), 0);
    let length = (packet.len() - 1) as u16;
    packet[2..4].copy_from_slice(&length.to_le_bytes());
    packet
}

/// Where the packets of one scan lie and how the preview samples them.
/// Every packet but the last has the length and record count of the first,
/// and together they hold the stated number of records: field streams that
/// run in step like that can be decoded from any packet on.
struct ScanPlan {
    widths: Vec<usize>,
    /// Logical offsets of the first packet and of the end of the section.
    start: u64,
    end: u64,
    /// Length and record count of every packet but the last.
    stride: u64,
    records: usize,
    packets: u64,
    picks: u64,
    take: usize,
}

impl ScanPlan {
    fn new(
        source: &mut Source,
        scan: &e57::PointCloud,
        share: f64,
        limit: usize,
    ) -> Result<Option<Self>, LoadError> {
        let Some(widths) = field_widths(&scan.prototype) else {
            return Ok(None);
        };
        let mut bytes = Vec::new();
        let section = logical_offset(scan.file_offset, source.page);
        source.read(section, SECTION_HEADER_BYTES, &mut bytes)?;
        let field = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
        let (length, data) = (field(8), field(16));
        let start = logical_offset(data, source.page);
        let end = section
            .checked_add(length)
            .filter(|end| *end > start && *end <= logical_offset(source.length, source.page));
        let Some(end) = end else {
            return Ok(None);
        };
        if bytes[0] != 1 || start < section + SECTION_HEADER_BYTES as u64 {
            return Ok(None);
        }
        let first = (end - start).min(MAX_PACKET_BYTES as u64);
        source.read(start, first as usize, &mut bytes)?;
        let Some((stride, records)) = data_packet(&bytes, &widths) else {
            return Ok(None);
        };
        let stride = stride as u64;
        let packets = (end - start).div_ceil(stride);
        let last = start + (packets - 1) * stride;
        source.read(last, (end - last) as usize, &mut bytes)?;
        let closes =
            data_packet(&bytes, &widths).filter(|(length, _)| last + *length as u64 == end);
        let Some((_, last_records)) = closes else {
            return Ok(None);
        };
        let stated = (packets - 1)
            .checked_mul(records as u64)
            .and_then(|before| before.checked_add(last_records as u64));
        if stated != Some(scan.records) {
            return Ok(None);
        }
        let quota = ((limit as f64 * share).ceil() as u64).max(1);
        let affordable = ((MAX_READ_BYTES as f64 * share) as u64 / stride).max(1);
        let picks = quota
            .div_ceil(RECORDS_PER_PACKET)
            .clamp(1, packets.min(affordable));
        Ok(Some(Self {
            widths,
            start,
            end,
            stride,
            records,
            packets,
            picks,
            take: (quota.div_ceil(picks) as usize).min(records),
        }))
    }

    /// The packet at one of the evenly spaced places in the section, or
    /// `None` when the packet there is not laid out like the first.
    fn pick(
        &self,
        source: &mut Source,
        pick: u64,
        bytes: &mut Vec<u8>,
    ) -> Result<Option<Thinned>, LoadError> {
        let index = (u128::from(pick) * u128::from(self.packets) / u128::from(self.picks)) as u64;
        let at = self.start + index * self.stride;
        source.read(at, (self.end - at).min(self.stride) as usize, bytes)?;
        let Some((length, records)) = data_packet(bytes, &self.widths) else {
            return Ok(None);
        };
        let expected = if index + 1 < self.packets {
            length as u64 == self.stride && records == self.records
        } else {
            at + length as u64 == self.end
        };
        if !expected || bytes.len() < length {
            return Ok(None);
        }
        let take = self.take.min(records);
        Ok(Some((
            thinned_packet(bytes, &self.widths, records, take),
            take,
        )))
    }
}

/// A step that visits every pick once and puts consecutive turns far apart,
/// so that the picks read before time runs out are spread through the file.
fn spread_step(picks: u64) -> u64 {
    let gcd = |mut a: u64, mut b: u64| {
        while b != 0 {
            (a, b) = (b, a % b);
        }
        a
    };
    let mut step = ((picks as f64 * 0.618) as u64).max(1);
    while gcd(step, picks) != 1 {
        step -= 1;
    }
    step
}

/// Lay logical bytes out in pages that each end with their checksum.
fn paged(logical: &[u8]) -> Vec<u8> {
    let data = IMAGE_PAGE_BYTES - CHECKSUM_BYTES as usize;
    let mut physical = Vec::with_capacity(logical.len().div_ceil(data) * IMAGE_PAGE_BYTES);
    for content in logical.chunks(data) {
        let start = physical.len();
        physical.extend_from_slice(content);
        physical.resize(start + data, 0);
        let checksum = crc32c::crc32c(&physical[start..]);
        physical.extend_from_slice(&checksum.to_be_bytes());
    }
    physical
}

fn image_physical(logical: usize) -> u64 {
    let data = IMAGE_PAGE_BYTES - CHECKSUM_BYTES as usize;
    (logical / data * IMAGE_PAGE_BYTES + logical % data) as u64
}

/// Open a bounded preview of an E57 file without reading all of it. Returns
/// `None` when a scan in the file cannot be sampled this way. The count is
/// the stated number of records and the bounds cover the sampled points
/// only, so the caller replaces this cloud with the checked result of a full
/// pass and must not index or export it.
pub(crate) fn preview(path: &Path, limit: usize) -> Result<Option<PointCloud>, LoadError> {
    preview_within(path, limit, SAMPLING_TIME)
}

fn preview_within(
    path: &Path,
    limit: usize,
    time: Duration,
) -> Result<Option<PointCloud>, LoadError> {
    let stamp = SourceStamp::read(path)?;
    let mut header = [0u8; FILE_HEADER_BYTES];
    File::open(path)?.read_exact(&mut header)?;
    let page = u64::from_le_bytes(header[40..48].try_into().unwrap());
    if !(CHECKSUM_BYTES + 1..=MAX_PAGE_BYTES).contains(&page) {
        return Ok(None);
    }
    let scans = e57_points::open_reader(path)?.pointclouds();
    let total: u64 = scans.iter().map(|scan| scan.records).sum();
    if limit == 0 || total == 0 {
        return Ok(None);
    }

    let mut source = Source::open(path, page)?;
    let mut plans = Vec::new();
    for (index, scan) in scans.iter().enumerate() {
        if scan.records == 0 {
            continue;
        }
        let share = scan.records as f64 / total as f64;
        let Some(plan) = ScanPlan::new(&mut source, scan, share, limit)? else {
            return Ok(None);
        };
        plans.push((index, time.mul_f64(share), plan));
    }

    // The image: a file header, one section per scan and the source's XML.
    let mut logical = vec![0u8; FILE_HEADER_BYTES];
    let mut sections = Vec::new();
    for (index, time, plan) in &plans {
        let step = spread_step(plan.picks);
        let deadline = Instant::now() + *time;
        let read = (0..plan.picks)
            .into_par_iter()
            .map_init(
                || (Source::open(path, page), Vec::new()),
                |(source, bytes), turn| {
                    if turn > 0 && Instant::now() >= deadline {
                        return Ok(Some(None));
                    }
                    let pick = turn * step % plan.picks;
                    match source {
                        Ok(source) => Ok(plan
                            .pick(source, pick, bytes)?
                            .map(|thinned| Some((pick, thinned)))),
                        Err(error) => Err(LoadError::InvalidData(error.to_string())),
                    }
                },
            )
            .collect::<Result<Option<Vec<_>>, LoadError>>()?;
        let Some(read) = read else {
            return Ok(None);
        };
        // Back in file order, whatever order the turns were read in.
        let mut read: Vec<(u64, Thinned)> = read.into_iter().flatten().collect();
        read.sort_unstable_by_key(|(pick, _)| *pick);
        let thinned: Vec<Thinned> = read.into_iter().map(|(_, thinned)| thinned).collect();
        let records: u64 = thinned.iter().map(|(_, records)| *records as u64).sum();
        if records == 0 {
            continue;
        }
        let offset = logical.len();
        let length: usize = thinned.iter().map(|(packet, _)| packet.len()).sum();
        logical.push(1);
        logical.resize(offset + 8, 0);
        logical.extend_from_slice(&((SECTION_HEADER_BYTES + length) as u64).to_le_bytes());
        let data = image_physical(offset + SECTION_HEADER_BYTES);
        logical.extend_from_slice(&data.to_le_bytes());
        logical.extend_from_slice(&0u64.to_le_bytes());
        for (packet, _) in &thinned {
            logical.extend_from_slice(packet);
        }
        sections.push((*index, image_physical(offset), records));
    }
    if sections.is_empty() {
        return Ok(None);
    }
    let xml = E57Reader::raw_xml(WindowReader::open(path)?)?;
    let xml_offset = logical.len();
    logical.extend_from_slice(&xml);
    logical[..16].copy_from_slice(&header[..16]);
    let pages = logical
        .len()
        .div_ceil(IMAGE_PAGE_BYTES - CHECKSUM_BYTES as usize);
    logical[16..24].copy_from_slice(&((pages * IMAGE_PAGE_BYTES) as u64).to_le_bytes());
    logical[24..32].copy_from_slice(&image_physical(xml_offset).to_le_bytes());
    logical[32..40].copy_from_slice(&(xml.len() as u64).to_le_bytes());
    logical[40..48].copy_from_slice(&(IMAGE_PAGE_BYTES as u64).to_le_bytes());
    let mut image = E57Reader::new(Cursor::new(paged(&logical)))?;
    drop(logical);

    let mut points = Vec::new();
    let mut bounds: Option<Bounds> = None;
    for (index, offset, records) in sections {
        let mut scan = scans[index].clone();
        scan.file_offset = offset;
        scan.records = records;
        e57_points::read_scan(&mut image, &scan, &mut |point: Point| {
            if !point.xyz.iter().all(|value| value.is_finite()) {
                return Err(LoadError::InvalidData("non-finite coordinate".into()));
            }
            match &mut bounds {
                Some(bounds) => bounds.include(point.xyz),
                None => {
                    bounds = Some(Bounds {
                        min: point.xyz,
                        max: point.xyz,
                    })
                }
            }
            points.push(point);
            Ok(())
        })?;
    }
    let Some(bounds) = bounds else {
        return Ok(None);
    };
    Ok(Some(PointCloud {
        path: path.to_path_buf(),
        total_points: total,
        bounds,
        point_ordinals: vec![u64::MAX; points.len()],
        has_rgb: points.iter().any(|point| point.rgb.is_some()),
        has_intensity: points.iter().any(|point| point.intensity.is_some()),
        has_classification: false,
        points,
        scan_poses: e57_points::stations(&scans).into_iter().flatten().collect(),
        scan_images: super::scan_images(path),
        source_stamp: Some(stamp),
        provisional: true,
    }))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;

    use e57::{E57Writer, Quaternion, RecordName, RecordValue, Transform, Translation};

    use super::super::octree::{IndexConfig, OctreeIndex};
    use super::*;

    /// A registered scan whose fields all fill whole bytes, unless a row
    /// index of ten bits is added.
    fn write_scan(path: &Path, count: usize, row_index: bool) {
        let mut writer =
            E57Writer::from_file(path, "{00000000-0000-4000-8000-000000000021}").unwrap();
        let mut prototype = vec![
            Record::CARTESIAN_X_F64,
            Record::CARTESIAN_Y_F64,
            Record::CARTESIAN_Z_F64,
            Record::COLOR_RED_U8,
            Record::COLOR_GREEN_U8,
            Record::COLOR_BLUE_U8,
            Record::INTENSITY_U16,
        ];
        if row_index {
            prototype.push(Record {
                name: RecordName::RowIndex,
                data_type: RecordDataType::Integer { min: 0, max: 1000 },
            });
        }
        let mut scan = writer
            .add_pointcloud("{00000000-0000-4000-8000-000000000022}", prototype)
            .unwrap();
        let half = std::f64::consts::FRAC_1_SQRT_2;
        scan.set_transform(Some(Transform {
            rotation: Quaternion {
                w: half,
                x: 0.0,
                y: 0.0,
                z: half,
            },
            translation: Translation {
                x: 10.0,
                y: 20.0,
                z: 3.0,
            },
        }));
        for index in 0..count {
            let mut values = vec![
                RecordValue::Double(index as f64 * 0.01),
                RecordValue::Double((index % 97) as f64),
                RecordValue::Double((index / 97) as f64 * 0.5),
                RecordValue::Integer((index % 256) as i64),
                RecordValue::Integer((index / 3 % 256) as i64),
                RecordValue::Integer((index / 7 % 256) as i64),
                RecordValue::Integer((index % 65_536) as i64),
            ];
            if row_index {
                values.push(RecordValue::Integer((index % 1000) as i64));
            }
            scan.add_point(values).unwrap();
        }
        scan.finalize().unwrap();
        writer.finalize().unwrap();
    }

    fn full_pass(path: &Path) -> Vec<Point> {
        let mut points = Vec::new();
        e57_points::read(
            path,
            &mut |point| {
                points.push(point);
                Ok(())
            },
            &mut |_| {},
        )
        .unwrap();
        points
    }

    fn same(left: &Point, right: &Point) -> bool {
        left.xyz == right.xyz && left.rgb == right.rgb && left.intensity == right.intensity
    }

    #[test]
    fn sampling_every_record_matches_a_full_pass() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("small.e57");
        write_scan(&source, 50_000, false);
        let expected = full_pass(&source);
        assert_eq!(expected.len(), 50_000);

        let cloud = preview(&source, 10_000_000).unwrap().unwrap();
        assert_eq!(cloud.total_points, 50_000);
        assert_eq!(cloud.points.len(), expected.len());
        assert!(cloud.points.iter().zip(&expected).all(|(a, b)| same(a, b)));
        assert!(cloud.has_rgb && cloud.has_intensity);
        // A nameless scan without a scanner sweep is a merged cloud: its
        // pose places the points but marks no station.
        assert!(cloud.scan_poses.is_empty());
        assert!(cloud
            .point_ordinals
            .iter()
            .all(|ordinal| *ordinal == u64::MAX));
        let mut bounds = Bounds {
            min: expected[0].xyz,
            max: expected[0].xyz,
        };
        for point in &expected {
            bounds.include(point.xyz);
        }
        assert_eq!(cloud.bounds, bounds);
    }

    #[test]
    fn a_large_scan_is_sampled_from_packets_spread_through_the_file() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("large.e57");
        write_scan(&source, 200_000, false);
        let expected = full_pass(&source);
        let places: HashMap<[u64; 3], usize> = expected
            .iter()
            .enumerate()
            .map(|(index, point)| (point.xyz.map(f64::to_bits), index))
            .collect();

        let cloud = preview(&source, 2_000).unwrap().unwrap();
        assert_eq!(cloud.total_points, 200_000);
        assert_eq!(cloud.points.len(), 2_000);
        let mut indices = Vec::new();
        for point in &cloud.points {
            let index = places[&point.xyz.map(f64::to_bits)];
            assert!(same(point, &expected[index]));
            indices.push(index);
        }
        // File order is kept, and the samples reach from the start of the
        // file to its last eighth in eight separate runs.
        assert!(indices.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(indices[0] < 100 && *indices.last().unwrap() > 175_000);
        let runs = indices
            .windows(2)
            .filter(|pair| pair[1] - pair[0] > 1_000)
            .count()
            + 1;
        assert_eq!(runs, 8);
    }

    /// Overwrite bytes at a logical offset, keeping the page checksums right.
    fn patch(path: &Path, at: u64, bytes: &[u8]) {
        let mut file = fs::read(path).unwrap();
        for (offset, byte) in (at..).zip(bytes) {
            file[(offset / 1020 * 1024 + offset % 1020) as usize] = *byte;
        }
        for page in file.as_chunks_mut::<1024>().0 {
            let checksum = crc32c::crc32c(&page[..1020]).to_be_bytes();
            page[1020..].copy_from_slice(&checksum);
        }
        fs::write(path, file).unwrap();
    }

    fn plan(path: &Path, limit: usize) -> ScanPlan {
        let scan = e57_points::open_reader(path).unwrap().pointclouds()[0].clone();
        let mut source = Source::open(path, 1024).unwrap();
        ScanPlan::new(&mut source, &scan, 1.0, limit)
            .unwrap()
            .unwrap()
    }

    #[test]
    fn a_packet_unlike_the_first_gives_no_preview() {
        let directory = tempfile::tempdir().unwrap();

        // The fourth packet is one to step over, so the packets after it no
        // longer continue the records of the first three.
        let skipped = directory.path().join("skipped.e57");
        write_scan(&skipped, 100_000, false);
        let layout = plan(&skipped, 1_000);
        assert!(layout.packets > 10);
        patch(&skipped, layout.start + 3 * layout.stride, &[2]);
        assert!(preview(&skipped, 10_000_000).unwrap().is_none());

        // The sixth packet holds one more X and one fewer Z value: the same
        // length, but its streams are out of step.
        let shifted = directory.path().join("shifted.e57");
        write_scan(&shifted, 100_000, false);
        let stream = (layout.records * 8) as u16;
        let header = layout.start + 5 * layout.stride + PACKET_HEADER_BYTES as u64;
        patch(&shifted, header, &(stream + 8).to_le_bytes());
        patch(&shifted, header + 4, &(stream - 8).to_le_bytes());
        assert!(preview(&shifted, 10_000_000).unwrap().is_none());

        // The last packet ends before the section does.
        let short = directory.path().join("short.e57");
        write_scan(&short, 100_000, false);
        let last = layout.start + (layout.packets - 1) * layout.stride;
        let length = ((layout.end - last - 4 - 1) as u16).to_le_bytes();
        patch(&short, last + 2, &length);
        assert!(ScanPlan::new(
            &mut Source::open(&short, 1024).unwrap(),
            &e57_points::open_reader(&short).unwrap().pointclouds()[0],
            1.0,
            1_000,
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn fields_that_do_not_fill_bytes_and_damaged_pages_give_no_preview() {
        let directory = tempfile::tempdir().unwrap();
        let packed = directory.path().join("packed.e57");
        write_scan(&packed, 20_000, true);
        assert!(preview(&packed, 1_000).unwrap().is_none());
        // Its row index is part of a scanner sweep, so the pose is a station.
        assert_eq!(e57_points::scan_poses(&packed).unwrap().len(), 1);

        // A page with the section header fails its checksum.
        let damaged = directory.path().join("damaged.e57");
        write_scan(&damaged, 200_000, false);
        let layout = plan(&damaged, 2_000);
        let mut bytes = fs::read(&damaged).unwrap();
        bytes[600] ^= 0x40;
        fs::write(&damaged, &bytes).unwrap();
        assert!(preview(&damaged, 2_000).is_err());

        // So does a page of a packet that only a later pick reads.
        bytes[600] ^= 0x40;
        assert_eq!(layout.picks, 8);
        let inside = layout.start + layout.packets / 8 * layout.stride + 100;
        bytes[(inside / 1020 * 1024 + inside % 1020) as usize] ^= 0x40;
        fs::write(&damaged, &bytes).unwrap();
        assert!(matches!(
            preview(&damaged, 2_000),
            Err(LoadError::InvalidData(_))
        ));
    }

    #[test]
    fn a_slow_source_shows_the_packets_read_in_time() {
        for picks in [1, 2, 3, 8, 90, 7_813, 10_000] {
            let step = spread_step(picks);
            let mut seen = vec![false; picks as usize];
            for turn in 0..picks {
                seen[(turn * step % picks) as usize] = true;
            }
            assert!(seen.iter().all(|seen| *seen), "{picks} picks");
        }
        // Consecutive turns land far apart.
        assert_eq!(spread_step(1_000), 617);

        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("slow.e57");
        write_scan(&source, 200_000, false);
        let expected = full_pass(&source);
        // No time at all still reads the first packet.
        let cloud = preview_within(&source, 2_000, Duration::ZERO)
            .unwrap()
            .unwrap();
        assert_eq!(cloud.points.len(), 250);
        assert!(same(&cloud.points[0], &expected[0]));
        assert_eq!(cloud.total_points, 200_000);
        assert!(cloud.provisional);
    }

    #[test]
    fn only_a_lone_nameless_scan_without_a_sweep_has_no_station() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("setups.e57");
        let mut writer =
            E57Writer::from_file(&source, "{00000000-0000-4000-8000-000000000031}").unwrap();
        for (guid, x) in [
            ("{00000000-0000-4000-8000-000000000032}", 1.0),
            ("{00000000-0000-4000-8000-000000000033}", 5.0),
        ] {
            let mut scan = writer
                .add_pointcloud(
                    guid,
                    vec![
                        Record::CARTESIAN_X_F64,
                        Record::CARTESIAN_Y_F64,
                        Record::CARTESIAN_Z_F64,
                    ],
                )
                .unwrap();
            scan.set_transform(Some(Transform {
                rotation: Quaternion {
                    w: 1.0,
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                },
                translation: Translation { x, y: 0.0, z: 0.0 },
            }));
            scan.add_point(vec![
                RecordValue::Double(0.0),
                RecordValue::Double(1.0),
                RecordValue::Double(2.0),
            ])
            .unwrap();
            scan.finalize().unwrap();
        }
        writer.finalize().unwrap();
        // Two nameless scans are two setups, each at its own place.
        let poses = e57_points::scan_poses(&source).unwrap();
        assert_eq!(poses.len(), 2);
        assert_eq!(poses[1].position, [5.0, 0.0, 0.0]);

        let merged = directory.path().join("merged.e57");
        write_scan(&merged, 1_000, false);
        assert!(e57_points::scan_poses(&merged).unwrap().is_empty());
    }

    #[test]
    fn indexing_shows_the_spread_preview_before_the_checked_cloud() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("indexed.e57");
        write_scan(&source, 60_000, false);
        let config = IndexConfig {
            scratch_dir: Some(directory.path().join("cache")),
            leaf_points: 8_192,
            ..IndexConfig::default()
        };

        let mut previews = Vec::new();
        let (cloud, index) = OctreeIndex::open_and_build(
            &source,
            500,
            config,
            Some(0),
            |preview| {
                previews.push(preview.clone());
                Ok(())
            },
            |_| Ok(()),
        )
        .unwrap();
        // Only the spread preview was published; the checked cloud is returned.
        assert_eq!(previews.len(), 1);
        assert!(previews[0].points.len() > 500);
        assert!(previews[0]
            .point_ordinals
            .iter()
            .all(|ordinal| *ordinal == u64::MAX));
        assert_eq!(cloud.total_points, 60_000);
        assert_eq!(cloud.points.len(), 500);
        assert!(cloud.point_ordinals.iter().all(|ordinal| *ordinal < 60_000));
        assert_eq!(index.root.total_points, 60_000);
        assert!((0..3).all(|axis| {
            previews[0].bounds.min[axis] >= cloud.bounds.min[axis]
                && previews[0].bounds.max[axis] <= cloud.bounds.max[axis]
        }));
    }
}
