//! Saved views written as a BCF 2.1 file: a ZIP container with one topic per
//! view, each holding its markup, its viewpoint and its snapshot.
//!
//! The first half derives what a viewpoint states from a saved view: the
//! perspective camera, the clipping planes of the section box and the lines
//! of the annotations. The second half writes the XML documents and the
//! container. Coordinates are model coordinates in metres.

use std::io::{self, Write};
use std::path::Path;

use iced::Size;
use pointcloud_core::Bounds;

use crate::camera_views::{self, Annotation, SavedView, SectionBox};
use crate::selection::Projection;
use crate::station_photos::WalkView;

type Xyz = [f64; 3];

/// Length of the upright line that marks the point of a note.
pub const NOTE_MARKER_LENGTH: f64 = 0.25;
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

fn dot(a: Xyz, b: Xyz) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalised(vector: Xyz) -> Xyz {
    let length = dot(vector, vector).sqrt();
    vector.map(|value| value / length)
}

/// A perspective camera as a viewpoint states it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub eye: Xyz,
    pub direction: Xyz,
    pub up: Xyz,
    /// The whole vertical field of view in degrees.
    pub field_of_view: f64,
}

/// Vertical field of view in degrees of a camera with a focal length in
/// pixels and a viewport height.
fn vertical_field_of_view(focal: f64, height: f64) -> f64 {
    (2.0 * (0.5 * height / focal).atan()).to_degrees()
}

/// The camera of a saved view. A view that carries the scene bounds and the
/// viewport size it was saved with uses those; an older view is taken to be
/// relative to the given ones.
///
/// The walking camera maps directly. The orbit camera stands 1.8 scene
/// extents from the scene centre and looks at that centre; panning shifts
/// its picture on the screen instead of moving it. The camera written here
/// stands in the same place and is turned towards what is in the middle of
/// the viewport, so a panned view keeps its centre, and its focal length is
/// the distance from the eye to that middle in pixels.
pub fn camera(view: &SavedView, scene: Bounds, viewport: Size) -> Camera {
    let (scene, viewport) = view.frame.map_or((scene, viewport), |frame| {
        (
            Bounds {
                min: frame.scene_min,
                max: frame.scene_max,
            },
            Size::new(frame.viewport[0], frame.viewport[1]),
        )
    });
    let height = f64::from(viewport.height);
    if let Some(walk) = view.walk {
        let walk = WalkView {
            eye: walk.eye,
            yaw: walk.yaw,
            pitch: walk.pitch,
            field_of_view: walk.field_of_view,
        };
        let [_, up, forward] = walk.basis();
        return Camera {
            eye: walk.eye,
            direction: forward,
            up,
            field_of_view: vertical_field_of_view(f64::from(walk.focal(viewport)), height),
        };
    }
    let orbit = Projection::new(
        scene,
        view.yaw,
        view.pitch,
        view.zoom,
        view.pan,
        viewport.width,
        viewport.height,
    );
    let centre = scene.center();
    let pan = view.pan.map(f64::from);
    // From the eye through the middle of the viewport, in pixels.
    let ray: Xyz = std::array::from_fn(|axis| {
        orbit.right[axis] * -pan[0] + orbit.up[axis] * pan[1]
            - orbit.toward_camera[axis] * orbit.scale
    });
    let focal = dot(ray, ray).sqrt();
    let direction = ray.map(|value| value / focal);
    let lean = dot(orbit.up, direction);
    Camera {
        eye: std::array::from_fn(|axis| centre[axis] + orbit.toward_camera[axis] * orbit.eye[2]),
        direction,
        up: normalised(std::array::from_fn(|axis| {
            orbit.up[axis] - direction[axis] * lean
        })),
        field_of_view: vertical_field_of_view(focal, height),
    }
}

/// A clipping plane: a point on it and the direction of the side that is cut away.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plane {
    pub location: Xyz,
    pub direction: Xyz,
}

/// The six faces of a section box, each facing outwards. The faces of a
/// turned box are turned with it.
pub fn clipping_planes(section: &SectionBox) -> Vec<Plane> {
    let turned = section.oriented();
    let centre: Xyz = std::array::from_fn(|axis| (section.min[axis] + section.max[axis]) * 0.5);
    let mut planes = Vec::with_capacity(6);
    for axis in 0..3 {
        for (limit, outwards) in [(section.min[axis], -1.0), (section.max[axis], 1.0)] {
            let mut location = centre;
            location[axis] = limit;
            let mut direction = [0.0; 3];
            direction[axis] = outwards;
            if turned.is_turned() && axis < 2 {
                location = turned.to_scene(location);
                direction = turned.axes()[axis].map(|value| value * outwards);
            }
            planes.push(Plane {
                location,
                direction,
            });
        }
    }
    planes
}

/// The lines of a viewpoint: every line annotation, and a short upright
/// line at the point of every note.
pub fn marker_lines(annotations: &[Annotation]) -> Vec<(Xyz, Xyz)> {
    annotations
        .iter()
        .map(|annotation| match annotation {
            Annotation::Line { from, to } => (*from, *to),
            Annotation::Note { point, .. } => {
                (*point, [point[0], point[1], point[2] + NOTE_MARKER_LENGTH])
            }
        })
        .collect()
}

/// A moment as `xs:dateTime` in UTC, from seconds since 1970.
pub fn timestamp(seconds: u64) -> String {
    let (year, month, day, hour, minute, second) = civil(seconds);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Calendar date and time of day in UTC.
fn civil(seconds: u64) -> (u64, u64, u64, u64, u64, u64) {
    let time = seconds % 86_400;
    // Days since 1 March 0000, in eras of 400 years.
    let days = seconds / 86_400 + 719_468;
    let era = days / 146_097;
    let day_of_era = days % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    (year, month, day, time / 3_600, time / 60 % 60, time % 60)
}

/// A remark on a topic.
#[derive(Debug, Clone, PartialEq)]
pub struct Comment {
    pub guid: String,
    pub date: String,
    pub text: String,
}

/// One topic of the file: a saved view with what its documents state.
#[derive(Debug, Clone, PartialEq)]
pub struct Topic {
    pub guid: String,
    pub viewpoint_guid: String,
    pub title: String,
    pub created: String,
    pub author: String,
    /// File name of the scan the view belongs to.
    pub source_name: Option<String>,
    pub comments: Vec<Comment>,
    pub camera: Camera,
    pub planes: Vec<Plane>,
    pub lines: Vec<(Xyz, Xyz)>,
    /// PNG image of the view.
    pub snapshot: Option<Vec<u8>>,
}

/// What an export adds to the views: who exports, when, and the scene and
/// viewport that views saved without their own are relative to.
#[derive(Debug, Clone, Copy)]
pub struct Context<'a> {
    pub author: &'a str,
    pub now: u64,
    pub scene: Bounds,
    pub viewport: Size,
}

impl Topic {
    pub fn from_view(view: &SavedView, context: &Context<'_>, snapshot: Option<Vec<u8>>) -> Self {
        let created = if view.created == 0 {
            context.now
        } else {
            view.created
        };
        Self {
            guid: view.guid.clone(),
            viewpoint_guid: camera_views::stable_guid(&[view.guid.as_bytes(), b"viewpoint"]),
            title: view.name.clone(),
            created: timestamp(created),
            author: context.author.to_owned(),
            source_name: view
                .source
                .file_name()
                .map(|name| name.to_string_lossy().into_owned()),
            comments: view
                .annotations
                .iter()
                .filter_map(|annotation| match annotation {
                    Annotation::Note {
                        text,
                        guid,
                        created: noted,
                        ..
                    } => Some(Comment {
                        guid: guid.clone(),
                        date: timestamp(if *noted == 0 { created } else { *noted }),
                        text: text.clone(),
                    }),
                    Annotation::Line { .. } => None,
                })
                .collect(),
            camera: camera(view, context.scene, context.viewport),
            planes: view
                .section
                .filter(|section| section.enabled)
                .map_or_else(Vec::new, |section| clipping_planes(&section)),
            lines: marker_lines(&view.annotations),
            snapshot: snapshot.filter(|bytes| bytes.starts_with(&PNG_SIGNATURE)),
        }
    }
}

/// Text as XML character data or an attribute value. Characters that XML 1.0
/// cannot hold are left out.
pub fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            // A reader would otherwise turn these into a line feed or a space.
            '\r' => escaped.push_str("&#13;"),
            '\n' => escaped.push_str("&#10;"),
            '\t' => escaped.push_str("&#9;"),
            '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}' => {
                escaped.push(character);
            }
            _ => {}
        }
    }
    escaped
}

const XML_DECLARATION: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n";

fn element(xml: &mut String, indent: usize, name: &str, content: &str) {
    xml.push_str(&format!(
        "{:indent$}<{name}>{}</{name}>\n",
        "",
        escape(content)
    ));
}

fn xyz_element(xml: &mut String, indent: usize, name: &str, xyz: Xyz) {
    xml.push_str(&format!(
        "{:indent$}<{name}><X>{}</X><Y>{}</Y><Z>{}</Z></{name}>\n",
        "", xyz[0], xyz[1], xyz[2]
    ));
}

pub fn version_xml() -> String {
    format!(
        "{XML_DECLARATION}<Version VersionId=\"2.1\">\n  <DetailedVersion>2.1</DetailedVersion>\n</Version>\n"
    )
}

pub fn markup_xml(topic: &Topic) -> String {
    let mut xml = String::from(XML_DECLARATION);
    xml.push_str("<Markup>\n");
    if let Some(name) = &topic.source_name {
        xml.push_str("  <Header>\n    <File isExternal=\"true\">\n");
        element(&mut xml, 6, "Filename", name);
        xml.push_str("    </File>\n  </Header>\n");
    }
    xml.push_str(&format!("  <Topic Guid=\"{}\">\n", escape(&topic.guid)));
    element(&mut xml, 4, "Title", &topic.title);
    element(&mut xml, 4, "CreationDate", &topic.created);
    element(&mut xml, 4, "CreationAuthor", &topic.author);
    xml.push_str("  </Topic>\n");
    for comment in &topic.comments {
        xml.push_str(&format!("  <Comment Guid=\"{}\">\n", escape(&comment.guid)));
        element(&mut xml, 4, "Date", &comment.date);
        element(&mut xml, 4, "Author", &topic.author);
        element(&mut xml, 4, "Comment", &comment.text);
        xml.push_str(&format!(
            "    <Viewpoint Guid=\"{}\"/>\n  </Comment>\n",
            escape(&topic.viewpoint_guid)
        ));
    }
    xml.push_str(&format!(
        "  <Viewpoints Guid=\"{}\">\n",
        escape(&topic.viewpoint_guid)
    ));
    element(&mut xml, 4, "Viewpoint", "viewpoint.bcfv");
    if topic.snapshot.is_some() {
        element(&mut xml, 4, "Snapshot", "snapshot.png");
    }
    xml.push_str("  </Viewpoints>\n</Markup>\n");
    xml
}

pub fn viewpoint_xml(topic: &Topic) -> String {
    let mut xml = String::from(XML_DECLARATION);
    xml.push_str(&format!(
        "<VisualizationInfo Guid=\"{}\">\n  <PerspectiveCamera>\n",
        escape(&topic.viewpoint_guid)
    ));
    xyz_element(&mut xml, 4, "CameraViewPoint", topic.camera.eye);
    xyz_element(&mut xml, 4, "CameraDirection", topic.camera.direction);
    xyz_element(&mut xml, 4, "CameraUpVector", topic.camera.up);
    xml.push_str(&format!(
        "    <FieldOfView>{}</FieldOfView>\n  </PerspectiveCamera>\n",
        topic.camera.field_of_view
    ));
    if !topic.lines.is_empty() {
        xml.push_str("  <Lines>\n");
        for (start, end) in &topic.lines {
            xml.push_str("    <Line>\n");
            xyz_element(&mut xml, 6, "StartPoint", *start);
            xyz_element(&mut xml, 6, "EndPoint", *end);
            xml.push_str("    </Line>\n");
        }
        xml.push_str("  </Lines>\n");
    }
    if !topic.planes.is_empty() {
        xml.push_str("  <ClippingPlanes>\n");
        for plane in &topic.planes {
            xml.push_str("    <ClippingPlane>\n");
            xyz_element(&mut xml, 6, "Location", plane.location);
            xyz_element(&mut xml, 6, "Direction", plane.direction);
            xml.push_str("    </ClippingPlane>\n");
        }
        xml.push_str("  </ClippingPlanes>\n");
    }
    xml.push_str("</VisualizationInfo>\n");
    xml
}

const LOCAL_HEADER: [u8; 4] = *b"PK\x03\x04";
const CENTRAL_HEADER: [u8; 4] = *b"PK\x01\x02";
const END_OF_DIRECTORY: [u8; 4] = *b"PK\x05\x06";
/// Version 2.0 of the ZIP format: deflate, and folders in entry names.
const ZIP_VERSION: u16 = 20;

fn too_large() -> io::Error {
    io::Error::other("the views are too large for one BCF file")
}

/// A ZIP container built in memory, with entries stored or deflated.
struct Container {
    bytes: Vec<u8>,
    directory: Vec<u8>,
    entries: u16,
    /// Modification time and date of every entry in UTC, in the packed form
    /// of ZIP headers: two-second steps and years from 1980.
    stamp: [u16; 2],
}

impl Container {
    fn new(now: u64) -> Self {
        let (year, month, day, hour, minute, second) = civil(now);
        let year = year.clamp(1980, 2107) - 1980;
        Self {
            bytes: Vec::new(),
            directory: Vec::new(),
            entries: 0,
            stamp: [
                ((hour << 11) | (minute << 5) | (second / 2)) as u16,
                ((year << 9) | (month << 5) | day) as u16,
            ],
        }
    }

    /// Add an entry. Content that deflating does not shrink is stored.
    fn add(&mut self, name: &str, content: &[u8], deflate: bool) -> io::Result<()> {
        let packed = if deflate {
            let mut encoder =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(content)?;
            Some(encoder.finish()?).filter(|packed| packed.len() < content.len())
        } else {
            None
        };
        let method: u16 = if packed.is_some() { 8 } else { 0 };
        let data = packed.as_deref().unwrap_or(content);
        let mut crc = flate2::Crc::new();
        crc.update(content);
        let offset = u32::try_from(self.bytes.len()).map_err(|_| too_large())?;
        let size = u32::try_from(content.len()).map_err(|_| too_large())?;
        let packed_size = u32::try_from(data.len()).map_err(|_| too_large())?;
        let name_length = u16::try_from(name.len()).map_err(|_| too_large())?;
        self.entries = self.entries.checked_add(1).ok_or_else(too_large)?;

        // The fields a local header and a directory entry share.
        let mut fields = Vec::with_capacity(26);
        fields.extend(ZIP_VERSION.to_le_bytes());
        // Bit 11: the entry name is UTF-8.
        fields.extend(0x0800u16.to_le_bytes());
        fields.extend(method.to_le_bytes());
        fields.extend(self.stamp[0].to_le_bytes());
        fields.extend(self.stamp[1].to_le_bytes());
        fields.extend(crc.sum().to_le_bytes());
        fields.extend(packed_size.to_le_bytes());
        fields.extend(size.to_le_bytes());
        fields.extend(name_length.to_le_bytes());
        // No extra field.
        fields.extend(0u16.to_le_bytes());

        self.bytes.extend(LOCAL_HEADER);
        self.bytes.extend(&fields);
        self.bytes.extend(name.as_bytes());
        self.bytes.extend(data);

        self.directory.extend(CENTRAL_HEADER);
        self.directory.extend(ZIP_VERSION.to_le_bytes());
        self.directory.extend(&fields);
        // No comment, first disk, no internal or external attributes.
        self.directory.extend([0u8; 10]);
        self.directory.extend(offset.to_le_bytes());
        self.directory.extend(name.as_bytes());
        Ok(())
    }

    fn finish(mut self) -> io::Result<Vec<u8>> {
        let directory_offset = u32::try_from(self.bytes.len()).map_err(|_| too_large())?;
        let directory_size = u32::try_from(self.directory.len()).map_err(|_| too_large())?;
        directory_offset
            .checked_add(directory_size)
            .ok_or_else(too_large)?;
        self.bytes.extend(&self.directory);
        self.bytes.extend(END_OF_DIRECTORY);
        // This disk and the disk of the directory.
        self.bytes.extend([0u8; 4]);
        self.bytes.extend(self.entries.to_le_bytes());
        self.bytes.extend(self.entries.to_le_bytes());
        self.bytes.extend(directory_size.to_le_bytes());
        self.bytes.extend(directory_offset.to_le_bytes());
        // No comment.
        self.bytes.extend(0u16.to_le_bytes());
        Ok(self.bytes)
    }
}

/// The BCF file of the topics: `bcf.version`, and a folder per topic named
/// by its identifier with `markup.bcf`, `viewpoint.bcfv` and `snapshot.png`.
pub fn archive(topics: &[Topic], now: u64) -> io::Result<Vec<u8>> {
    let mut container = Container::new(now);
    container.add("bcf.version", version_xml().as_bytes(), true)?;
    for topic in topics {
        if !camera_views::is_guid(&topic.guid) {
            return Err(io::Error::other("a view has no valid identifier"));
        }
        let folder = &topic.guid;
        container.add(
            &format!("{folder}/markup.bcf"),
            markup_xml(topic).as_bytes(),
            true,
        )?;
        container.add(
            &format!("{folder}/viewpoint.bcfv"),
            viewpoint_xml(topic).as_bytes(),
            true,
        )?;
        if let Some(snapshot) = &topic.snapshot {
            // A PNG image is deflated already.
            container.add(&format!("{folder}/snapshot.png"), snapshot, false)?;
        }
    }
    container.finish()
}

/// Write a file through a temporary one beside it, so a failed write leaves
/// an existing file as it was.
pub fn write_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let directory = path
        .parent()
        .filter(|directory| !directory.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    temporary.write_all(bytes)?;
    temporary.as_file_mut().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

/// Readers for the tests: the entries of a ZIP container and a strict XML
/// reader. They share no code with the writers above.
#[cfg(test)]
pub mod reading {
    use std::io::Read;

    /// An entry as the directory lists it, with its content read back
    /// through its local header and checked against its CRC-32.
    pub fn entries(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
        let field16 = |at: usize| -> Result<usize, String> {
            bytes
                .get(at..at + 2)
                .map(|field| usize::from(u16::from_le_bytes([field[0], field[1]])))
                .ok_or_else(|| "truncated".to_owned())
        };
        let field32 = |at: usize| -> Result<usize, String> {
            bytes
                .get(at..at + 4)
                .map(|field| u32::from_le_bytes([field[0], field[1], field[2], field[3]]) as usize)
                .ok_or_else(|| "truncated".to_owned())
        };
        let end = bytes.len().checked_sub(22).ok_or("no end of directory")?;
        if bytes[end..end + 4] != *b"PK\x05\x06" {
            return Err("no end of directory".into());
        }
        let count = field16(end + 10)?;
        if field16(end + 8)? != count {
            return Err("entry counts differ".into());
        }
        let (directory_size, directory_offset) = (field32(end + 12)?, field32(end + 16)?);
        if directory_offset + directory_size != end {
            return Err("the directory does not end at the end record".into());
        }
        let mut entries = Vec::with_capacity(count);
        let mut at = directory_offset;
        let mut expected_local = 0;
        for _ in 0..count {
            if bytes.get(at..at + 4) != Some(b"PK\x01\x02") {
                return Err("bad directory entry".into());
            }
            let method = field16(at + 10)?;
            let (crc, packed_size, size) =
                (field32(at + 16)?, field32(at + 20)?, field32(at + 24)?);
            let (name_length, extra, comment) =
                (field16(at + 28)?, field16(at + 30)?, field16(at + 32)?);
            let local = field32(at + 42)?;
            let name = std::str::from_utf8(&bytes[at + 46..at + 46 + name_length])
                .map_err(|error| error.to_string())?
                .to_owned();
            at += 46 + name_length + extra + comment;

            // The local header repeats the directory entry.
            if local != expected_local || bytes.get(local..local + 4) != Some(b"PK\x03\x04") {
                return Err(format!("bad local header of {name}"));
            }
            if field16(local + 8)? != method
                || field32(local + 14)? != crc
                || field32(local + 18)? != packed_size
                || field32(local + 22)? != size
                || field16(local + 26)? != name_length
                || bytes[local + 30..local + 30 + name_length] != *name.as_bytes()
            {
                return Err(format!("the headers of {name} differ"));
            }
            let data_start = local + 30 + name_length + field16(local + 28)?;
            let data = bytes
                .get(data_start..data_start + packed_size)
                .ok_or("truncated entry")?;
            expected_local = data_start + packed_size;
            let content = match method {
                0 => data.to_vec(),
                8 => {
                    let mut content = Vec::new();
                    flate2::read::DeflateDecoder::new(data)
                        .read_to_end(&mut content)
                        .map_err(|error| error.to_string())?;
                    content
                }
                other => return Err(format!("unsupported method {other}")),
            };
            let mut check = flate2::Crc::new();
            check.update(&content);
            if content.len() != size || check.sum() as usize != crc {
                return Err(format!("{name} is damaged"));
            }
            entries.push((name, content));
        }
        if at != end || expected_local != directory_offset {
            return Err("the directory and the entries do not fill the file".into());
        }
        Ok(entries)
    }

    /// An element with its attributes, child elements and own text.
    #[derive(Debug, Clone, PartialEq)]
    pub struct Node {
        pub name: String,
        pub attributes: Vec<(String, String)>,
        pub children: Vec<Node>,
        pub text: String,
    }

    impl Node {
        pub fn child(&self, name: &str) -> Option<&Node> {
            self.children.iter().find(|child| child.name == name)
        }

        pub fn all(&self, name: &str) -> Vec<&Node> {
            self.children
                .iter()
                .filter(|child| child.name == name)
                .collect()
        }

        pub fn attribute(&self, name: &str) -> Option<&str> {
            self.attributes
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        }

        /// Text of a child element.
        pub fn value(&self, name: &str) -> Option<&str> {
            self.child(name).map(|child| child.text.as_str())
        }

        /// The X, Y and Z of a child element.
        pub fn xyz(&self, name: &str) -> Option<[f64; 3]> {
            let node = self.child(name)?;
            let axis = |axis: &str| node.value(axis)?.parse::<f64>().ok();
            Some([axis("X")?, axis("Y")?, axis("Z")?])
        }
    }

    struct Reader<'a> {
        rest: &'a str,
    }

    impl<'a> Reader<'a> {
        fn skip_space(&mut self) {
            self.rest = self.rest.trim_start_matches([' ', '\t', '\r', '\n']);
        }

        fn take(&mut self, token: &str) -> bool {
            match self.rest.strip_prefix(token) {
                Some(rest) => {
                    self.rest = rest;
                    true
                }
                None => false,
            }
        }

        fn name(&mut self) -> Result<&'a str, String> {
            let length = self
                .rest
                .find(|character: char| {
                    !(character.is_alphanumeric() || matches!(character, '_' | '-' | '.' | ':'))
                })
                .unwrap_or(self.rest.len());
            let (name, rest) = self.rest.split_at(length);
            if name.is_empty()
                || name.starts_with(|first: char| first.is_ascii_digit() || first == '-')
            {
                return Err(format!(
                    "expected a name at {:?}",
                    &self.rest[..self.rest.len().min(20)]
                ));
            }
            self.rest = rest;
            Ok(name)
        }

        /// Character data up to one of the given characters, with its
        /// references resolved.
        fn text(&mut self, ends: &[char]) -> Result<String, String> {
            let mut text = String::new();
            loop {
                let Some(character) = self.rest.chars().next() else {
                    return Err("the document ends inside text".into());
                };
                if ends.contains(&character) {
                    return Ok(text);
                }
                self.rest = &self.rest[character.len_utf8()..];
                match character {
                    '&' => {
                        let end = self.rest.find(';').ok_or("unfinished reference")?;
                        let reference = &self.rest[..end];
                        self.rest = &self.rest[end + 1..];
                        text.push(match reference {
                            "amp" => '&',
                            "lt" => '<',
                            "gt" => '>',
                            "quot" => '"',
                            "apos" => '\'',
                            _ => {
                                let code = match reference.strip_prefix("#x") {
                                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                                    None => reference
                                        .strip_prefix('#')
                                        .and_then(|decimal| decimal.parse().ok()),
                                };
                                code.and_then(char::from_u32)
                                    .ok_or_else(|| format!("unknown reference &{reference};"))?
                            }
                        });
                    }
                    '<' => return Err("a bare < in text".into()),
                    '\u{9}'
                    | '\u{A}'
                    | '\u{D}'
                    | '\u{20}'..='\u{D7FF}'
                    | '\u{E000}'..='\u{FFFD}'
                    | '\u{10000}'..='\u{10FFFF}' => {
                        text.push(character);
                    }
                    other => return Err(format!("character {other:?} is not allowed")),
                }
            }
        }

        fn element(&mut self) -> Result<Node, String> {
            if !self.take("<") {
                return Err("expected an element".into());
            }
            let name = self.name()?.to_owned();
            let mut attributes: Vec<(String, String)> = Vec::new();
            loop {
                let spaced = self.rest.starts_with([' ', '\t', '\r', '\n']);
                self.skip_space();
                if self.take("/>") {
                    return Ok(Node {
                        name,
                        attributes,
                        children: Vec::new(),
                        text: String::new(),
                    });
                }
                if self.take(">") {
                    break;
                }
                if !spaced {
                    return Err(format!("missing space in <{name}>"));
                }
                let key = self.name()?.to_owned();
                if !self.take("=\"") {
                    return Err(format!("attribute {key} has no quoted value"));
                }
                let value = self.text(&['"'])?;
                self.take("\"");
                if attributes.iter().any(|(known, _)| *known == key) {
                    return Err(format!("attribute {key} is repeated"));
                }
                attributes.push((key, value));
            }
            let mut children = Vec::new();
            let mut text = String::new();
            loop {
                text.push_str(&self.text(&['<'])?);
                if self.take("</") {
                    let end = self.name()?;
                    if end != name {
                        return Err(format!("<{name}> is closed by </{end}>"));
                    }
                    self.skip_space();
                    if !self.take(">") {
                        return Err(format!("</{name} is not closed"));
                    }
                    return Ok(Node {
                        name,
                        attributes,
                        children,
                        text,
                    });
                }
                children.push(self.element()?);
            }
        }
    }

    /// Read a whole document: a declaration, then one element.
    pub fn parse(document: &str) -> Result<Node, String> {
        let mut reader = Reader { rest: document };
        if reader.take("<?xml") {
            let end = reader.rest.find("?>").ok_or("unfinished declaration")?;
            if !reader.rest[..end].contains("encoding=\"UTF-8\"") {
                return Err("the declaration does not state UTF-8".into());
            }
            reader.rest = &reader.rest[end + 2..];
        }
        reader.skip_space();
        let root = reader.element()?;
        reader.skip_space();
        if !reader.rest.is_empty() {
            return Err("content after the root element".into());
        }
        Ok(root)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::reading::{entries, parse, Node};
    use super::*;
    use crate::camera_views::{ViewFrame, WalkCamera};

    fn cross(a: Xyz, b: Xyz) -> Xyz {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    }

    /// Screen position of a point as a reader of the file sees it: a
    /// symmetric perspective camera with the stated vertical field of view.
    fn seen(camera: &Camera, size: Size, xyz: Xyz) -> (f64, f64) {
        let right = cross(camera.direction, camera.up);
        let relative: Xyz = std::array::from_fn(|axis| xyz[axis] - camera.eye[axis]);
        let depth = dot(relative, camera.direction);
        assert!(depth > 0.0, "{xyz:?} is behind the camera");
        let (width, height) = (f64::from(size.width), f64::from(size.height));
        let focal = 0.5 * height / (camera.field_of_view.to_radians() * 0.5).tan();
        (
            width * 0.5 + dot(relative, right) * focal / depth,
            height * 0.5 - dot(relative, camera.up) * focal / depth,
        )
    }

    fn assert_orthonormal(camera: &Camera) {
        assert!((dot(camera.direction, camera.direction) - 1.0).abs() < 1e-12);
        assert!((dot(camera.up, camera.up) - 1.0).abs() < 1e-12);
        assert!(dot(camera.direction, camera.up).abs() < 1e-12);
    }

    fn scene() -> Bounds {
        Bounds {
            min: [207_430.0, 474_010.0, -2.0],
            max: [207_470.0, 474_034.0, 9.0],
        }
    }

    fn orbit_view(yaw: f32, pitch: f32, zoom: f32, pan: [f32; 2], size: Size) -> SavedView {
        let mut view =
            SavedView::camera(PathBuf::from("scan.e57"), "View 1", yaw, pitch, zoom, pan);
        view.frame = Some(ViewFrame {
            scene_min: scene().min,
            scene_max: scene().max,
            viewport: [size.width, size.height],
        });
        view
    }

    /// Points spread through the scene.
    fn samples() -> Vec<Xyz> {
        let scene = scene();
        let mut points = vec![scene.center()];
        for step in 0..27 {
            let fraction = |axis: u32| f64::from(step / 3u32.pow(axis) % 3) * 0.35 + 0.15;
            points.push(std::array::from_fn(|axis| {
                scene.min[axis] + (scene.max[axis] - scene.min[axis]) * fraction(axis as u32)
            }));
        }
        points
    }

    #[test]
    fn orbit_camera_reproduces_the_view_of_the_application() {
        for (size, yaw, pitch, zoom) in [
            (Size::new(915.0, 743.0), -0.8, 0.6, 1.0),
            (Size::new(1280.0, 620.0), 2.4, -0.3, 0.35),
            (Size::new(500.0, 900.0), 0.1, 1.3, 2.5),
            (Size::new(915.0, 743.0), -1.570_796_4, 1.570_796_4, 0.8),
        ] {
            let view = orbit_view(yaw, pitch, zoom, [0.0, 0.0], size);
            let camera = camera(&view, scene(), size);
            assert_orthonormal(&camera);
            let projection = Projection::new(
                scene(),
                yaw,
                pitch,
                zoom,
                [0.0, 0.0],
                size.width,
                size.height,
            );
            for point in samples() {
                let (x, y, _) = projection.project_unclipped(point).unwrap();
                let (seen_x, seen_y) = seen(&camera, size, point);
                assert!((seen_x - f64::from(x)).abs() < 0.02, "{seen_x} {x}");
                assert!((seen_y - f64::from(y)).abs() < 0.02, "{seen_y} {y}");
            }
            // The orbit camera's focal length is 1.25 times the shorter
            // side of the viewport, divided by the zoom.
            let focal = f64::from(size.width.min(size.height)) * 1.25 / f64::from(zoom);
            let expected = (2.0 * (0.5 * f64::from(size.height) / focal).atan()).to_degrees();
            assert!((camera.field_of_view - expected).abs() < 1e-9);
            // It looks at the middle of the scene from 1.8 extents away.
            let centre = scene().center();
            let distance: f64 = (0..3)
                .map(|axis| (camera.eye[axis] - centre[axis]).powi(2))
                .sum::<f64>()
                .sqrt();
            assert!((distance - scene().extent() * 1.8).abs() < 1e-6);
        }
    }

    #[test]
    fn panned_orbit_camera_keeps_what_is_in_the_middle_of_the_viewport() {
        let size = Size::new(915.0, 743.0);
        let (yaw, pitch, zoom, pan) = (0.9, 0.4, 0.2, [260.0, -140.0]);
        let view = orbit_view(yaw, pitch, zoom, pan, size);
        let camera = camera(&view, scene(), size);
        assert_orthonormal(&camera);
        let projection = Projection::new(scene(), yaw, pitch, zoom, pan, size.width, size.height);
        // Points along the camera's axis are in the middle of the viewport
        // for the application as well.
        for depth in [5.0, 40.0, 90.0] {
            let point: Xyz =
                std::array::from_fn(|axis| camera.eye[axis] + camera.direction[axis] * depth);
            let (x, y, _) = projection.project_unclipped(point).unwrap();
            assert!((x - size.width * 0.5).abs() < 0.02, "{x}");
            assert!((y - size.height * 0.5).abs() < 0.02, "{y}");
            let (seen_x, seen_y) = seen(&camera, size, point);
            assert!((seen_x - f64::from(x)).abs() < 0.02);
            assert!((seen_y - f64::from(y)).abs() < 0.02);
        }
        // Turning a camera in place changes no line of sight, so whatever
        // the application shows near the middle stays near it: within a
        // tenth of the viewport the two pictures differ by a few pixels.
        let right = cross(camera.direction, camera.up);
        for (across, upwards) in [(0.02, 0.0), (0.0, 0.02), (-0.015, 0.015), (0.01, -0.02)] {
            let point: Xyz = std::array::from_fn(|axis| {
                camera.eye[axis]
                    + (camera.direction[axis] + right[axis] * across + camera.up[axis] * upwards)
                        * 60.0
            });
            let (x, y, _) = projection.project_unclipped(point).unwrap();
            let (seen_x, seen_y) = seen(&camera, size, point);
            let offset =
                (seen_x - f64::from(size.width) * 0.5).hypot(seen_y - f64::from(size.height) * 0.5);
            assert!(offset > 40.0 && offset < 120.0, "{offset}");
            assert!((seen_x - f64::from(x)).hypot(seen_y - f64::from(y)) < 0.06 * offset);
        }
        // The focal length is the distance from the eye to the middle of
        // the viewport, in pixels.
        let scale = f64::from(size.height) * 1.25 / f64::from(zoom);
        let focal = (scale * scale + 260.0 * 260.0 + 140.0 * 140.0).sqrt();
        let expected = (2.0 * (0.5 * f64::from(size.height) / focal).atan()).to_degrees();
        assert!((camera.field_of_view - expected).abs() < 1e-9);
    }

    #[test]
    fn panned_orbit_camera_drifts_from_the_application_towards_the_edges() {
        // A viewport of 800 by 600 at zoom 1 has a focal length of 750
        // pixels. The drift is measured along the pan, at a distance from
        // the middle of the viewport, on the plane through the scene centre.
        let size = Size::new(800.0, 600.0);
        let drift = |pan: f32, from_middle: f64| {
            let view = orbit_view(0.9, 0.4, 1.0, [pan, 0.0], size);
            let camera = camera(&view, scene(), size);
            let projection =
                Projection::new(scene(), 0.9, 0.4, 1.0, [pan, 0.0], size.width, size.height);
            let along = (from_middle - f64::from(pan)) * projection.eye[2] / projection.scale;
            let point: Xyz =
                std::array::from_fn(|axis| scene().center()[axis] + projection.right[axis] * along);
            let (x, y, _) = projection.project_unclipped(point).unwrap();
            assert!((f64::from(x) - 400.0 - from_middle).abs() < 0.02, "{x}");
            let (seen_x, seen_y) = seen(&camera, size, point);
            (seen_x - f64::from(x)).hypot(seen_y - f64::from(y))
        };
        // Pan, distance from the middle, and the drift in pixels: nothing in
        // the middle, and towards the edges about the distance squared times
        // the pan, divided by the focal length squared.
        for (pan, from_middle, low, high) in [
            (0.0, 400.0, 0.0, 0.02),
            (50.0, 0.0, 0.0, 0.02),
            (50.0, 100.0, 0.5, 0.8),
            (50.0, 200.0, 3.0, 3.3),
            (50.0, 400.0, 13.5, 14.0),
            (50.0, -400.0, 14.3, 14.8),
            (200.0, 200.0, 6.8, 7.2),
            (200.0, 400.0, 45.0, 46.5),
            (200.0, -400.0, 58.0, 59.5),
        ] {
            let drift = drift(pan, from_middle);
            assert!((low..=high).contains(&drift), "{pan} {from_middle} {drift}");
        }
    }

    #[test]
    fn walking_camera_reproduces_the_view_of_the_application() {
        let size = Size::new(1100.0, 640.0);
        let mut view = orbit_view(0.3, 0.2, 1.0, [50.0, 60.0], size);
        let walk = WalkCamera {
            eye: [207_441.5, 474_020.25, 1.6],
            yaw: 0.7,
            pitch: -0.25,
            field_of_view: 1.5,
        };
        view.walk = Some(walk);
        let camera = camera(&view, scene(), size);
        assert_orthonormal(&camera);
        assert_eq!(camera.eye, walk.eye);
        let walking = WalkView {
            eye: walk.eye,
            yaw: walk.yaw,
            pitch: walk.pitch,
            field_of_view: walk.field_of_view,
        };
        let projection = Projection::from_eye(
            scene(),
            walk.eye,
            walking.basis(),
            walking.focal(size),
            size.width,
            size.height,
        );
        let mut compared = 0;
        for point in samples() {
            let Some((x, y, _)) = projection.project_unclipped(point) else {
                continue;
            };
            let (seen_x, seen_y) = seen(&camera, size, point);
            assert!((seen_x - f64::from(x)).abs() < 0.02, "{seen_x} {x}");
            assert!((seen_y - f64::from(y)).abs() < 0.02, "{seen_y} {y}");
            compared += 1;
        }
        assert!(compared >= 8);
        // The walking camera states a horizontal field of view.
        let expected = (2.0
            * ((0.75f64).tan() * f64::from(size.height) / f64::from(size.width)).atan())
        .to_degrees();
        assert!((camera.field_of_view - expected).abs() < 1e-4);
        // The top and bottom of the viewport are half that angle from the axis.
        let half = (camera.field_of_view * 0.5).to_radians().tan();
        for (side, edge) in [(1.0, 0.0), (-1.0, f64::from(size.height))] {
            let point: Xyz = std::array::from_fn(|axis| {
                camera.eye[axis] + (camera.direction[axis] + camera.up[axis] * half * side) * 12.0
            });
            let (_, y, _) = projection.project_unclipped(point).unwrap();
            assert!((f64::from(y) - edge).abs() < 0.02, "{y}");
        }
    }

    #[test]
    fn a_view_without_its_own_frame_uses_the_scene_and_viewport_of_the_export() {
        let size = Size::new(800.0, 600.0);
        let mut view = orbit_view(0.4, 0.5, 1.5, [0.0, 0.0], size);
        let framed = camera(
            &view,
            Bounds {
                min: [0.0; 3],
                max: [1.0; 3],
            },
            Size::new(10.0, 10.0),
        );
        view.frame = None;
        assert_eq!(camera(&view, scene(), size), framed);
    }

    #[test]
    fn six_clipping_planes_bound_exactly_the_section_box() {
        let section = SectionBox {
            enabled: true,
            min: [207_440.0, 474_015.5, 0.25],
            max: [207_452.5, 474_021.0, 3.0],
            rotation: 0.0,
        };
        let planes = clipping_planes(&section);
        assert_eq!(planes.len(), 6);
        // Each plane lies on one face and faces away from the box.
        let mut limits = [[f64::NAN; 2]; 3];
        for plane in &planes {
            let axis = plane
                .direction
                .iter()
                .position(|value| *value != 0.0)
                .unwrap();
            assert_eq!(
                plane
                    .direction
                    .iter()
                    .filter(|value| **value == 0.0)
                    .count(),
                2
            );
            let side = usize::from(plane.direction[axis] > 0.0);
            assert_eq!(plane.direction[axis].abs(), 1.0);
            assert!(limits[axis][side].is_nan());
            limits[axis][side] = plane.location[axis];
            for other in (0..3).filter(|other| *other != axis) {
                assert!(plane.location[other] > section.min[other]);
                assert!(plane.location[other] < section.max[other]);
            }
        }
        assert_eq!(limits.map(|pair| pair[0]), section.min);
        assert_eq!(limits.map(|pair| pair[1]), section.max);

        // A point is kept when no plane cuts it away.
        let kept = |point: Xyz| {
            planes.iter().all(|plane| {
                let offset: Xyz = std::array::from_fn(|axis| point[axis] - plane.location[axis]);
                dot(offset, plane.direction) <= 0.0
            })
        };
        let inside = |point: Xyz| {
            (0..3).all(|axis| point[axis] >= section.min[axis] && point[axis] <= section.max[axis])
        };
        let mut checked = [0, 0];
        for step in 0..125 {
            let point: Xyz = std::array::from_fn(|axis| {
                let fraction = f64::from(step / 5u32.pow(axis as u32) % 5) * 0.5 - 0.5;
                section.min[axis] + (section.max[axis] - section.min[axis]) * fraction
            });
            assert_eq!(kept(point), inside(point), "{point:?}");
            checked[usize::from(inside(point))] += 1;
        }
        assert_eq!(checked, [98, 27]);
        assert!(kept(section.min) && kept(section.max));
        assert!(!kept([
            section.max[0] + 1e-6,
            section.max[1],
            section.max[2]
        ]));
    }

    #[test]
    fn the_clipping_planes_of_a_turned_box_keep_what_the_turned_box_holds() {
        let section = SectionBox {
            enabled: true,
            min: [207_440.0, 474_015.5, 0.25],
            max: [207_452.5, 474_021.0, 3.0],
            rotation: 30.0,
        };
        let turned = section.oriented();
        let planes = clipping_planes(&section);
        assert_eq!(planes.len(), 6);
        let kept = |point: Xyz| {
            planes.iter().all(|plane| {
                let offset: Xyz = std::array::from_fn(|axis| point[axis] - plane.location[axis]);
                dot(offset, plane.direction) <= 1e-9
            })
        };
        let around = turned.aabb();
        let mut checked = [0, 0];
        for step in 0..1_000u32 {
            let point: Xyz = std::array::from_fn(|axis| {
                let fraction = f64::from(step / 10u32.pow(axis as u32) % 10) / 9.0;
                around.min[axis] + (around.max[axis] - around.min[axis]) * (fraction * 1.2 - 0.1)
            });
            assert_eq!(kept(point), turned.contains(point), "{point:?}");
            checked[usize::from(turned.contains(point))] += 1;
        }
        assert!(checked[0] > 100 && checked[1] > 100, "{checked:?}");
        // Every plane has a direction of unit length and lies on a face.
        for plane in &planes {
            assert!((dot(plane.direction, plane.direction) - 1.0).abs() < 1e-12);
            let local = turned.to_box(plane.location);
            assert!((0..3).all(|axis| local[axis] >= section.min[axis] - 1e-6
                && local[axis] <= section.max[axis] + 1e-6));
        }
    }

    #[test]
    fn dates_are_written_as_utc_date_times() {
        assert_eq!(timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(timestamp(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(timestamp(1_709_251_199), "2024-02-29T23:59:59Z");
        assert_eq!(timestamp(1_709_251_200), "2024-03-01T00:00:00Z");
        assert_eq!(timestamp(1_790_942_096), "2026-10-02T11:54:56Z");
        assert_eq!(timestamp(4_102_444_799), "2099-12-31T23:59:59Z");
    }

    /// A small PNG image.
    fn png() -> Vec<u8> {
        let image =
            ::image::RgbImage::from_fn(6, 4, |x, y| ::image::Rgb([x as u8 * 40, y as u8 * 60, 90]));
        let mut bytes = Vec::new();
        image
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                ::image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    const AWKWARD: &str = "Kozijn <A&B> \"links\" 'boven' — hoogte ≥ 2,5 m · façade 𝛑";

    fn annotated_view(name: &str) -> SavedView {
        let size = Size::new(915.0, 743.0);
        let mut view = orbit_view(-0.8, 0.6, 0.5, [20.0, -10.0], size);
        view.name = name.to_owned();
        view.created = 1_790_942_096;
        view.section = Some(SectionBox {
            enabled: true,
            min: [207_440.0, 474_015.5, 0.25],
            max: [207_452.5, 474_021.0, 3.0],
            rotation: 0.0,
        });
        view.annotations = vec![
            Annotation::Note {
                point: [207_445.0, 474_018.0, 1.5],
                text: AWKWARD.to_owned(),
                guid: camera_views::new_guid(),
                created: 1_790_942_200,
            },
            Annotation::Line {
                from: [207_445.0, 474_018.0, 1.5],
                to: [207_446.0, 474_019.0, 2.5],
            },
            Annotation::note([207_447.0, 474_018.5, 0.5], "Tweede <notitie>"),
        ];
        view
    }

    fn context() -> Context<'static> {
        Context {
            author: "Inspecteur <J&J>",
            now: 1_790_950_000,
            scene: scene(),
            viewport: Size::new(915.0, 743.0),
        }
    }

    fn document(entries: &[(String, Vec<u8>)], name: &str) -> Node {
        let (_, content) = entries
            .iter()
            .find(|(entry, _)| entry == name)
            .unwrap_or_else(|| panic!("no entry {name}"));
        parse(std::str::from_utf8(content).unwrap())
            .unwrap_or_else(|error| panic!("{name}: {error}"))
    }

    #[test]
    fn file_reads_back_with_a_topic_per_view() {
        let views = [annotated_view(AWKWARD), {
            let mut plain = orbit_view(0.2, 0.1, 1.0, [0.0, 0.0], Size::new(640.0, 480.0));
            plain.name = "Zonder aantekeningen".into();
            plain.created = 0;
            plain
        }];
        let topics: Vec<Topic> = views
            .iter()
            .zip([Some(png()), None])
            .map(|(view, snapshot)| Topic::from_view(view, &context(), snapshot))
            .collect();
        let bytes = archive(&topics, context().now).unwrap();
        let entries = entries(&bytes).unwrap();
        let names: Vec<&str> = entries.iter().map(|(name, _)| name.as_str()).collect();
        let (first, second) = (&views[0].guid, &views[1].guid);
        assert_eq!(
            names,
            [
                "bcf.version".to_owned(),
                format!("{first}/markup.bcf"),
                format!("{first}/viewpoint.bcfv"),
                format!("{first}/snapshot.png"),
                format!("{second}/markup.bcf"),
                format!("{second}/viewpoint.bcfv"),
            ]
        );

        let version = document(&entries, "bcf.version");
        assert_eq!(version.name, "Version");
        assert_eq!(version.attribute("VersionId"), Some("2.1"));
        assert_eq!(version.value("DetailedVersion"), Some("2.1"));

        // The first view: names and notes with characters that XML escapes.
        let markup = document(&entries, &format!("{first}/markup.bcf"));
        assert_eq!(markup.name, "Markup");
        let order: Vec<&str> = markup
            .children
            .iter()
            .map(|child| child.name.as_str())
            .collect();
        assert_eq!(
            order,
            ["Header", "Topic", "Comment", "Comment", "Viewpoints"]
        );
        assert_eq!(
            markup
                .child("Header")
                .unwrap()
                .child("File")
                .unwrap()
                .value("Filename"),
            Some("scan.e57")
        );
        let topic = markup.child("Topic").unwrap();
        assert_eq!(topic.attribute("Guid"), Some(first.as_str()));
        let fields: Vec<&str> = topic
            .children
            .iter()
            .map(|child| child.name.as_str())
            .collect();
        assert_eq!(fields, ["Title", "CreationDate", "CreationAuthor"]);
        assert_eq!(topic.value("Title"), Some(AWKWARD));
        assert_eq!(topic.value("CreationDate"), Some("2026-10-02T11:54:56Z"));
        assert_eq!(topic.value("CreationAuthor"), Some("Inspecteur <J&J>"));
        let viewpoints = markup.child("Viewpoints").unwrap();
        let viewpoint_guid = viewpoints.attribute("Guid").unwrap();
        assert!(camera_views::is_guid(viewpoint_guid));
        assert_ne!(viewpoint_guid, first);
        assert_eq!(viewpoints.value("Viewpoint"), Some("viewpoint.bcfv"));
        assert_eq!(viewpoints.value("Snapshot"), Some("snapshot.png"));
        let comments = markup.all("Comment");
        assert_eq!(comments[0].value("Comment"), Some(AWKWARD));
        assert_eq!(comments[0].value("Date"), Some("2026-10-02T11:56:40Z"));
        assert_eq!(comments[1].value("Comment"), Some("Tweede <notitie>"));
        let mut guids = vec![first.as_str(), viewpoint_guid];
        for comment in &comments {
            let parts: Vec<&str> = comment
                .children
                .iter()
                .map(|child| child.name.as_str())
                .collect();
            assert_eq!(parts, ["Date", "Author", "Comment", "Viewpoint"]);
            assert_eq!(comment.value("Author"), Some("Inspecteur <J&J>"));
            assert_eq!(
                comment.child("Viewpoint").unwrap().attribute("Guid"),
                Some(viewpoint_guid)
            );
            let guid = comment.attribute("Guid").unwrap();
            assert!(camera_views::is_guid(guid) && !guids.contains(&guid));
            guids.push(guid);
        }

        let viewpoint = document(&entries, &format!("{first}/viewpoint.bcfv"));
        assert_eq!(viewpoint.name, "VisualizationInfo");
        assert_eq!(viewpoint.attribute("Guid"), Some(viewpoint_guid));
        let parts: Vec<&str> = viewpoint
            .children
            .iter()
            .map(|child| child.name.as_str())
            .collect();
        assert_eq!(parts, ["PerspectiveCamera", "Lines", "ClippingPlanes"]);
        let stated = viewpoint.child("PerspectiveCamera").unwrap();
        let fields: Vec<&str> = stated
            .children
            .iter()
            .map(|child| child.name.as_str())
            .collect();
        assert_eq!(
            fields,
            [
                "CameraViewPoint",
                "CameraDirection",
                "CameraUpVector",
                "FieldOfView"
            ]
        );
        // Numbers are written so that they read back exactly.
        let expected = camera(&views[0], scene(), Size::new(1.0, 1.0));
        assert_eq!(stated.xyz("CameraViewPoint"), Some(expected.eye));
        assert_eq!(stated.xyz("CameraDirection"), Some(expected.direction));
        assert_eq!(stated.xyz("CameraUpVector"), Some(expected.up));
        assert_eq!(
            stated.value("FieldOfView").unwrap().parse::<f64>().unwrap(),
            expected.field_of_view
        );
        let lines = viewpoint.child("Lines").unwrap().all("Line");
        assert_eq!(lines.len(), 3);
        assert_eq!(
            lines[0].xyz("StartPoint"),
            Some([207_445.0, 474_018.0, 1.5])
        );
        assert_eq!(lines[0].xyz("EndPoint"), Some([207_445.0, 474_018.0, 1.75]));
        assert_eq!(
            lines[1].xyz("StartPoint"),
            Some([207_445.0, 474_018.0, 1.5])
        );
        assert_eq!(lines[1].xyz("EndPoint"), Some([207_446.0, 474_019.0, 2.5]));
        assert_eq!(lines[2].xyz("EndPoint"), Some([207_447.0, 474_018.5, 0.75]));
        let planes = viewpoint
            .child("ClippingPlanes")
            .unwrap()
            .all("ClippingPlane");
        let expected_planes = clipping_planes(&views[0].section.unwrap());
        assert_eq!(planes.len(), 6);
        for (plane, expected) in planes.iter().zip(&expected_planes) {
            assert_eq!(plane.xyz("Location"), Some(expected.location));
            assert_eq!(plane.xyz("Direction"), Some(expected.direction));
        }

        let (_, snapshot) = &entries[3];
        assert_eq!(snapshot, &png());
        let decoded =
            ::image::load_from_memory_with_format(snapshot, ::image::ImageFormat::Png).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (6, 4));

        // The second view: no notes, section box or snapshot, and a date
        // of export for a view saved without one.
        let markup = document(&entries, &format!("{second}/markup.bcf"));
        assert!(markup.all("Comment").is_empty());
        let topic = markup.child("Topic").unwrap();
        assert_eq!(topic.attribute("Guid"), Some(second.as_str()));
        assert_eq!(topic.value("Title"), Some("Zonder aantekeningen"));
        assert_eq!(
            topic.value("CreationDate"),
            Some(timestamp(context().now).as_str())
        );
        assert!(markup
            .child("Viewpoints")
            .unwrap()
            .child("Snapshot")
            .is_none());
        let viewpoint = document(&entries, &format!("{second}/viewpoint.bcfv"));
        let parts: Vec<&str> = viewpoint
            .children
            .iter()
            .map(|child| child.name.as_str())
            .collect();
        assert_eq!(parts, ["PerspectiveCamera"]);

        // Every topic folder is named by the identifier of its topic.
        for (name, content) in &entries {
            if let Some(folder) = name.strip_suffix("/markup.bcf") {
                let markup = parse(std::str::from_utf8(content).unwrap()).unwrap();
                assert_eq!(
                    markup.child("Topic").unwrap().attribute("Guid"),
                    Some(folder)
                );
            }
        }
    }

    #[test]
    fn a_section_box_that_is_off_and_a_file_that_is_no_image_are_left_out() {
        let mut view = annotated_view("Hal");
        view.section = view.section.map(|section| SectionBox {
            enabled: false,
            ..section
        });
        let topic = Topic::from_view(&view, &context(), Some(b"not an image".to_vec()));
        assert!(topic.planes.is_empty());
        assert!(topic.snapshot.is_none());
        let viewpoint = parse(&viewpoint_xml(&topic)).unwrap();
        assert!(viewpoint.child("ClippingPlanes").is_none());

        let mut broken = topic;
        broken.guid = "../outside".into();
        assert!(archive(&[broken], 0).is_err());
        // Without views the file holds its version alone.
        let empty = entries(&archive(&[], 0).unwrap()).unwrap();
        assert_eq!(empty.len(), 1);
    }

    #[test]
    fn text_is_escaped_and_reads_back_unchanged() {
        assert_eq!(
            escape("a<b>&\"c\" 'd'"),
            "a&lt;b&gt;&amp;&quot;c&quot; &apos;d&apos;"
        );
        assert_eq!(escape("één ≥ 2 · 𝛑"), "één ≥ 2 · 𝛑");
        // Control characters that XML cannot hold are left out.
        assert_eq!(escape("a\u{0}b\u{1b}c\u{fffe}d"), "abcd");
        assert_eq!(escape("a\tb\r\nc"), "a&#9;b&#13;&#10;c");
        for text in [AWKWARD, "]]> &amp; <!-- -->", "a\tb\r\nc", "  spaced  "] {
            let mut xml = String::from(XML_DECLARATION);
            xml.push_str(&format!("<Root Name=\"{}\">\n", escape(text)));
            element(&mut xml, 2, "Text", text);
            xml.push_str("</Root>\n");
            let root = parse(&xml).unwrap();
            assert_eq!(root.attribute("Name"), Some(text));
            assert_eq!(root.value("Text"), Some(text));
        }
        // The reader refuses what is not well-formed.
        for broken in [
            "<a><b></a></b>",
            "<a>&nbsp;</a>",
            "<a>1 < 2</a>",
            "<a b=\"1\" b=\"2\"/>",
            "<a></a><b/>",
            "<a>",
        ] {
            assert!(parse(broken).is_err(), "{broken}");
        }
    }

    #[test]
    fn large_and_small_entries_are_deflated_or_stored_with_their_checksum() {
        let mut container = Container::new(1_790_942_096);
        let repetitive = "<Line/>".repeat(400);
        container
            .add("a/text.xml", repetitive.as_bytes(), true)
            .unwrap();
        container.add("a/short.xml", b"<a/>", true).unwrap();
        container.add("a/image.png", &png(), false).unwrap();
        let bytes = container.finish().unwrap();
        assert!(bytes.len() < repetitive.len());
        let read = entries(&bytes).unwrap();
        assert_eq!(read.len(), 3);
        assert_eq!(read[0], ("a/text.xml".to_owned(), repetitive.into_bytes()));
        assert_eq!(read[1].1, b"<a/>");
        assert_eq!(read[2].1, png());
        // Methods: deflated, then stored twice.
        let method = |name: &str| {
            let at = bytes
                .windows(name.len())
                .position(|window| window == name.as_bytes())
                .unwrap();
            u16::from_le_bytes([bytes[at - 22], bytes[at - 21]])
        };
        assert_eq!(method("a/text.xml"), 8);
        assert_eq!(method("a/short.xml"), 0);
        assert_eq!(method("a/image.png"), 0);
        // A damaged byte is found through the checksum.
        let mut damaged = bytes.clone();
        let at = damaged
            .windows(4)
            .position(|window| window == b"<a/>")
            .unwrap();
        damaged[at + 1] = b'b';
        assert!(entries(&damaged).is_err());

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("views.bcf");
        write_file(&path, &bytes).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}
