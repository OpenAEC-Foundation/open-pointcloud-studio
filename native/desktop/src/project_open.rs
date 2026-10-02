//! Expands what the user chose to open into the scan files to load: a scan
//! file stays as it is, a folder becomes the scans directly inside it, and a
//! scan project file (.rcp) becomes the scans it lists.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::iter::Peekable;
use std::path::{Path, PathBuf};
use std::str::Chars;

use flate2::read::DeflateDecoder;

/// Extensions of the point-cloud and mesh files the application opens.
pub const SCAN_EXTENSIONS: [&str; 15] = [
    "las", "laz", "ply", "xyz", "csv", "asc", "txt", "pts", "ptx", "pcd", "obj", "off", "stl",
    "dxf", "e57",
];

/// Extension of a scan project file: a ZIP container whose XML document lists
/// the scans of one project.
pub const PROJECT_EXTENSION: &str = "rcp";

/// The project document is about 100 KB in practice; anything far beyond that
/// is not a project document and is refused instead of being read into memory.
const MAX_PROJECT_XML_BYTES: u64 = 16 * 1024 * 1024;
const MAX_CONTAINER_ENTRIES: usize = 4096;

const LOCAL_HEADER: [u8; 4] = *b"PK\x03\x04";

/// The outcome of expanding a selection of files, folders and project files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Expansion {
    /// Scan files to open, in a stable order and without duplicates.
    pub files: Vec<PathBuf>,
    /// Scans listed by a project file that were not found on disk.
    pub missing: usize,
    /// Scans left out because they are already open.
    pub already_open: usize,
    /// Chosen paths that could not be used, each with its reason.
    pub errors: Vec<String>,
}

impl Expansion {
    /// Leave out every file that is already open in the application.
    pub fn skip_open(&mut self, already_open: &[PathBuf]) {
        if already_open.is_empty() {
            return;
        }
        let open: HashSet<String> = already_open.iter().map(|path| path_key(path)).collect();
        let before = self.files.len();
        self.files.retain(|file| !open.contains(&path_key(file)));
        self.already_open += before - self.files.len();
    }

    /// True when there is more to tell than "opening these files".
    pub fn has_notes(&self) -> bool {
        self.missing > 0 || self.already_open > 0 || !self.errors.is_empty()
    }

    /// Status-line text: how many scans are being opened and what was left out.
    pub fn summary(&self) -> String {
        let mut notes = Vec::new();
        if self.missing > 0 {
            notes.push(format!(
                "{} not found",
                counted(self.missing, "listed scan")
            ));
        }
        if self.already_open > 0 {
            notes.push(format!("{} already open", self.already_open));
        }
        notes.extend(self.errors.iter().cloned());
        match (self.files.len(), notes.is_empty()) {
            (0, true) => "No supported scan files found".into(),
            (0, false) => format!("No scans opened: {}", notes.join("; ")),
            (count, true) => format!("Opening {}…", counted(count, "scan")),
            (count, false) => {
                format!("Opening {}; {}", counted(count, "scan"), notes.join("; "))
            }
        }
    }
}

fn counted(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// Expand the chosen paths into the scan files to open. The order follows the
/// chosen paths; a folder contributes its scans in natural name order and a
/// project file in the order it lists them. A file reached twice is listed
/// once, and files in `already_open` are left out.
pub fn expand(paths: &[PathBuf], already_open: &[PathBuf]) -> Expansion {
    let mut expansion = Expansion::default();
    let mut seen = HashSet::new();
    let mut listings = Listings::default();
    for path in paths {
        let found = match fs::metadata(path) {
            Err(error) => Err(format!("Cannot open {}: {error}", path.display())),
            Ok(metadata) if metadata.is_dir() => scans_in_folder(path)
                .map_err(|error| format!("Cannot list folder {}: {error}", path.display())),
            Ok(_) if has_extension(path, &[PROJECT_EXTENSION]) => {
                scans_in_project(path, &mut listings).map(|(files, missing)| {
                    expansion.missing += missing;
                    files
                })
            }
            Ok(_) if is_scan_file(path) => Ok(vec![path.clone()]),
            Ok(_) => Err(format!("{} is not a supported scan file", path.display())),
        };
        match found {
            Ok(files) => {
                for file in files {
                    if seen.insert(path_key(&file)) {
                        expansion.files.push(file);
                    }
                }
            }
            Err(error) => expansion.errors.push(error),
        }
    }
    expansion.skip_open(already_open);
    expansion
}

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extensions
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
}

fn is_scan_file(path: &Path) -> bool {
    has_extension(path, &SCAN_EXTENSIONS)
}

/// Identity of a file for duplicate detection: its absolute path, without
/// letter case on Windows. The disk is not consulted.
fn path_key(path: &Path) -> String {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let key = absolute.to_string_lossy();
    if cfg!(windows) {
        key.to_lowercase()
    } else {
        key.into_owned()
    }
}

/// Orders names the way people read them: runs of digits compare as numbers
/// ("scan 2" before "scan 10") and letters compare without case.
fn natural_cmp(left: &str, right: &str) -> Ordering {
    let mut a = left.chars().peekable();
    let mut b = right.chars().peekable();
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return left.cmp(right),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let (x, y) = (take_number(&mut a), take_number(&mut b));
                // Without leading zeros the longer run is the larger number.
                let order = x.len().cmp(&y.len()).then_with(|| x.cmp(&y));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                let order = x.to_lowercase().cmp(y.to_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                a.next();
                b.next();
            }
        }
    }
}

fn take_number(characters: &mut Peekable<Chars>) -> String {
    let mut digits = String::new();
    while let Some(digit) = characters.next_if(char::is_ascii_digit) {
        if digit != '0' || !digits.is_empty() {
            digits.push(digit);
        }
    }
    digits
}

struct Listed {
    name: OsString,
    folded: String,
    is_file: bool,
    is_dir: bool,
}

fn read_listing(directory: &Path) -> io::Result<Vec<Listed>> {
    let mut listing = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        // A link counts as what it points to.
        let target = kind
            .is_symlink()
            .then(|| fs::metadata(entry.path()).ok())
            .flatten();
        let name = entry.file_name();
        listing.push(Listed {
            folded: name.to_string_lossy().to_lowercase(),
            name,
            is_file: target
                .as_ref()
                .map_or(kind.is_file(), fs::Metadata::is_file),
            is_dir: target.as_ref().map_or(kind.is_dir(), fs::Metadata::is_dir),
        });
    }
    Ok(listing)
}

/// Directory listings read so far. The scans of a project file are nearly
/// always in one folder, so each folder is listed once instead of asking the
/// disk about every scan.
#[derive(Default)]
struct Listings(HashMap<PathBuf, Vec<Listed>>);

impl Listings {
    /// The entry of `directory` with this name, preferring an exact match
    /// over one that differs in letter case. A folder that cannot be listed
    /// has no entries.
    fn find(&mut self, directory: &Path, name: &str) -> Option<&Listed> {
        if !self.0.contains_key(directory) {
            let listing = read_listing(directory).unwrap_or_default();
            self.0.insert(directory.to_path_buf(), listing);
        }
        let listing = &self.0[directory];
        let folded = name.to_lowercase();
        listing
            .iter()
            .find(|entry| entry.name.to_str() == Some(name))
            .or_else(|| listing.iter().find(|entry| entry.folded == folded))
    }
}

/// The supported scan files directly inside a folder, in natural name order.
fn scans_in_folder(directory: &Path) -> io::Result<Vec<PathBuf>> {
    let mut names: Vec<OsString> = read_listing(directory)?
        .into_iter()
        .filter(|entry| entry.is_file && is_scan_file(Path::new(&entry.name)))
        .map(|entry| entry.name)
        .collect();
    names.sort_by(|left, right| natural_cmp(&left.to_string_lossy(), &right.to_string_lossy()));
    Ok(names.into_iter().map(|name| directory.join(name)).collect())
}

/// One scan listed in a project document. The absolute path stored beside
/// the relative one belongs to the machine that wrote the project and is
/// deliberately not kept.
#[derive(Debug, PartialEq, Eq)]
struct ListedScan {
    name: String,
    relative: Option<String>,
}

/// The existing scan files a project file lists, and how many of its scans
/// were not found.
fn scans_in_project(
    project: &Path,
    listings: &mut Listings,
) -> Result<(Vec<PathBuf>, usize), String> {
    let scans = File::open(project)
        .map_err(|error| error.to_string())
        .and_then(|file| read_listed_scans(&mut BufReader::new(file)))
        .map_err(|reason| {
            format!(
                "Cannot read scan project file {}: {reason}",
                project.display()
            )
        })?;
    if scans.is_empty() {
        return Err(format!(
            "Scan project file {} lists no scans",
            project.display()
        ));
    }
    let directory = match project.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let mut files = Vec::new();
    let mut missing = 0;
    for scan in &scans {
        let resolved = scan
            .relative
            .as_deref()
            .and_then(|relative| resolve_relative(directory, relative, listings));
        match resolved.or_else(|| resolve_by_name(directory, &scan.name, listings)) {
            Some(file) => files.push(file),
            None => missing += 1,
        }
    }
    Ok((files, missing))
}

/// Follow a stored relative path from the project folder, matching every
/// component against the directory listing without letter case. Projects are
/// written with `.\` prefixes, backslashes and often different case than the
/// files have on disk.
fn resolve_relative(directory: &Path, relative: &str, listings: &mut Listings) -> Option<PathBuf> {
    let relative = relative.trim().replace('\\', "/");
    // A rooted path or one with a drive letter is an absolute path of another
    // machine, not a location beside the project file.
    if relative.starts_with('/') || relative.contains(':') {
        return None;
    }
    let mut parts = relative
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .peekable();
    let mut resolved = directory.to_path_buf();
    while let Some(part) = parts.next() {
        if part == ".." {
            resolved.push(part);
            continue;
        }
        let entry = listings.find(&resolved, part)?;
        let expected = if parts.peek().is_some() {
            entry.is_dir
        } else {
            entry.is_file
        };
        if !expected {
            return None;
        }
        let name = entry.name.clone();
        resolved.push(name);
    }
    (resolved != directory && is_scan_file(&resolved)).then_some(resolved)
}

/// A file beside the project file named after the scan, in the first
/// supported format that exists.
fn resolve_by_name(directory: &Path, name: &str, listings: &mut Listings) -> Option<PathBuf> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    SCAN_EXTENSIONS.iter().find_map(|extension| {
        let entry = listings.find(directory, &format!("{name}.{extension}"))?;
        entry.is_file.then(|| directory.join(&entry.name))
    })
}

/// Read the scans listed in a project container. Only the XML document is
/// read: the local file headers are walked from the start and every other
/// entry (previews, thumbnails) is stepped over without being inflated.
fn read_listed_scans<R: Read + Seek>(container: &mut R) -> Result<Vec<ListedScan>, String> {
    let mut documents = 0;
    for entry in 0..MAX_CONTAINER_ENTRIES {
        let mut header = [0u8; 30];
        if let Err(error) = container.read_exact(&mut header) {
            if entry == 0 {
                return Err(if error.kind() == io::ErrorKind::UnexpectedEof {
                    "not a ZIP container".into()
                } else {
                    error.to_string()
                });
            }
            // The container ends without a directory; use what was found.
            break;
        }
        if header[..4] != LOCAL_HEADER {
            if entry == 0 {
                return Err("not a ZIP container".into());
            }
            // The directory that follows the last entry.
            break;
        }
        let field16 = |at: usize| u16::from_le_bytes([header[at], header[at + 1]]);
        let field32 = |at: usize| {
            u32::from_le_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]])
        };
        let (flags, method) = (field16(6), field16(8));
        let (checksum, packed_size, size) = (field32(14), field32(18), field32(22));
        let (name_length, extra_length) = (field16(26), field16(28));
        if flags & 0x0008 != 0 {
            return Err("entries without a size in their header are not supported".into());
        }
        if packed_size == u32::MAX || size == u32::MAX {
            return Err("entries of 4 GiB or more are not supported".into());
        }
        let mut name = vec![0u8; usize::from(name_length)];
        container
            .read_exact(&mut name)
            .map_err(|_| "the container is truncated".to_owned())?;
        let is_document = name.len() >= 4 && name[name.len() - 4..].eq_ignore_ascii_case(b".xml");
        if !is_document {
            container
                .seek_relative(i64::from(extra_length) + i64::from(packed_size))
                .map_err(|error| error.to_string())?;
            continue;
        }
        documents += 1;
        if flags & 0x0001 != 0 {
            return Err("the XML document is encrypted".into());
        }
        if u64::from(size) > MAX_PROJECT_XML_BYTES || u64::from(packed_size) > MAX_PROJECT_XML_BYTES
        {
            return Err(too_large());
        }
        container
            .seek_relative(i64::from(extra_length))
            .map_err(|error| error.to_string())?;
        let data_start = container
            .stream_position()
            .map_err(|error| error.to_string())?;
        let mut packed = container.by_ref().take(u64::from(packed_size));
        let mut document = Vec::with_capacity(size as usize);
        let read = match method {
            0 => packed.read_to_end(&mut document),
            // The declared size is not trusted: stop one byte past the limit.
            8 => DeflateDecoder::new(&mut packed)
                .take(MAX_PROJECT_XML_BYTES + 1)
                .read_to_end(&mut document),
            other => return Err(format!("unsupported compression method {other}")),
        };
        read.map_err(|_| "the XML document is damaged".to_owned())?;
        if document.len() as u64 > MAX_PROJECT_XML_BYTES {
            return Err(too_large());
        }
        let mut crc = flate2::Crc::new();
        crc.update(&document);
        if document.len() != size as usize || crc.sum() != checksum {
            return Err("the XML document is damaged".into());
        }
        let scans = listed_scans(&decode_text(&document));
        if !scans.is_empty() {
            return Ok(scans);
        }
        // A document without scans; the next entry may be the project list.
        container
            .seek(SeekFrom::Start(data_start + u64::from(packed_size)))
            .map_err(|error| error.to_string())?;
    }
    if documents == 0 {
        return Err("it holds no XML document".into());
    }
    Ok(Vec::new())
}

fn too_large() -> String {
    format!(
        "the XML document is larger than {} MiB",
        MAX_PROJECT_XML_BYTES / (1024 * 1024)
    )
}

/// XML text from its bytes: UTF-8 unless a byte-order mark says UTF-16.
fn decode_text(bytes: &[u8]) -> String {
    let utf16 = |bytes: &[u8], unit: fn([u8; 2]) -> u16| {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| unit([pair[0], pair[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    };
    match bytes {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, u16::from_le_bytes),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, u16::from_be_bytes),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// Find the scans in a project document: every element that carries a `name`
/// together with a raw scan path. Elements are recognized by these attributes
/// rather than by their tag, and everything that is not a well-formed start
/// tag is stepped over, so unknown or newer documents still yield their scans.
fn listed_scans(xml: &str) -> Vec<ListedScan> {
    let mut scans = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find('<') {
        rest = &rest[start + 1..];
        if let Some(comment) = rest.strip_prefix("!--") {
            rest = comment.find("-->").map_or("", |end| &comment[end + 3..]);
            continue;
        }
        if let Some(data) = rest.strip_prefix("![CDATA[") {
            rest = data.find("]]>").map_or("", |end| &data[end + 3..]);
            continue;
        }
        if rest.starts_with(['/', '?', '!']) {
            continue;
        }
        let (mut name, mut relative, mut has_absolute) = (None, None, false);
        let mut tag = rest.trim_start_matches(|c: char| !c.is_whitespace() && c != '>' && c != '/');
        while let Some((key, value, after)) = next_attribute(tag) {
            tag = after;
            if key.eq_ignore_ascii_case("name") {
                name = Some(unescape(value));
            } else if key.eq_ignore_ascii_case("relativeRawScanPath") {
                relative = Some(unescape(value)).filter(|path| !path.trim().is_empty());
            } else if key.eq_ignore_ascii_case("rawScanPath") {
                has_absolute = true;
            }
        }
        rest = tag;
        if let Some(name) = name {
            if relative.is_some() || has_absolute {
                scans.push(ListedScan { name, relative });
            }
        }
    }
    scans
}

/// The next `key="value"` of a start tag and the text after it, or `None` at
/// the end of the tag or where it stops being well formed.
fn next_attribute(tag: &str) -> Option<(&str, &str, &str)> {
    let tag = tag.trim_start();
    let key_end = tag.find(|c: char| c.is_whitespace() || matches!(c, '=' | '>' | '/' | '<'))?;
    let key = &tag[..key_end];
    let value = tag[key_end..].trim_start().strip_prefix('=')?.trim_start();
    let quote = value.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    let value = &value[1..];
    let value_end = value.find(quote)?;
    (!key.is_empty()).then_some((key, &value[..value_end], &value[value_end + 1..]))
}

/// Replace the predefined and numeric XML character references.
fn unescape(value: &str) -> String {
    let mut text = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find('&') {
        text.push_str(&rest[..start]);
        rest = &rest[start + 1..];
        let reference = rest.find(';').and_then(|end| {
            let character = match &rest[..end] {
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" => '\'',
                entity => {
                    let code = entity.strip_prefix('#')?;
                    let code = match code.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16),
                        None => code.parse(),
                    };
                    char::from_u32(code.ok()?)?
                }
            };
            Some((character, end + 1))
        });
        match reference {
            Some((character, length)) => {
                text.push(character);
                rest = &rest[length..];
            }
            None => text.push('&'),
        }
    }
    text.push_str(rest);
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    const CENTRAL_HEADER: [u8; 4] = *b"PK\x01\x02";
    const END_OF_DIRECTORY: [u8; 4] = *b"PK\x05\x06";

    /// A ZIP container with the given entries: name, content and whether the
    /// content is deflated or stored. Every local header carries an extra
    /// field, as real writers add.
    fn container(entries: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut directory = Vec::new();
        for (name, content, deflate) in entries {
            let packed = if *deflate {
                let mut encoder =
                    flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                encoder.write_all(content).unwrap();
                encoder.finish().unwrap()
            } else {
                content.to_vec()
            };
            let mut crc = flate2::Crc::new();
            crc.update(content);
            let offset = bytes.len() as u32;
            let mut fields = Vec::new();
            fields.extend(20u16.to_le_bytes());
            fields.extend(0u16.to_le_bytes());
            let method: u16 = if *deflate { 8 } else { 0 };
            fields.extend(method.to_le_bytes());
            fields.extend([0u8; 4]);
            fields.extend(crc.sum().to_le_bytes());
            fields.extend((packed.len() as u32).to_le_bytes());
            fields.extend((content.len() as u32).to_le_bytes());
            fields.extend((name.len() as u16).to_le_bytes());
            bytes.extend(LOCAL_HEADER);
            bytes.extend(&fields);
            bytes.extend(4u16.to_le_bytes());
            bytes.extend(name.as_bytes());
            bytes.extend([0xCA, 0xFE, 0, 0]);
            bytes.extend(&packed);
            directory.extend(CENTRAL_HEADER);
            directory.extend(20u16.to_le_bytes());
            directory.extend(&fields);
            directory.extend([0u8; 12]);
            directory.extend(offset.to_le_bytes());
            directory.extend(name.as_bytes());
        }
        let directory_offset = bytes.len() as u32;
        bytes.extend(&directory);
        bytes.extend(END_OF_DIRECTORY);
        bytes.extend([0u8; 4]);
        bytes.extend((entries.len() as u16).to_le_bytes());
        bytes.extend((entries.len() as u16).to_le_bytes());
        bytes.extend((directory.len() as u32).to_le_bytes());
        bytes.extend(directory_offset.to_le_bytes());
        bytes.extend(0u16.to_le_bytes());
        bytes
    }

    fn touch(directory: &Path, names: &[&str]) {
        for name in names {
            let path = directory.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"").unwrap();
        }
    }

    fn names(files: &[PathBuf]) -> Vec<String> {
        files
            .iter()
            .map(|file| file.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    const PROJECT_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<project version="1.0">
  <!-- <station name="Commented out" relativeRawScanPath=".\hall 1.e57"/> -->
  <group name="Ground floor">
    <station name="Hall 1" id="a" rawScanPath="Q:\elsewhere\hall 1.e57" relativeRawScanPath=".\HALL 1.E57">
      <view name="front"/>
    </station>
    <station name = 'Attic' relativeRawScanPath='.\Raw\ATTIC.e57' />
    <station name="Roof" rawScanPath="Q:\elsewhere\roof.e57"/>
    <station name="Cellar &amp; stairs" relativeRawScanPath=".\moved away.e57"/>
    <station name="Garden" rawScanPath="Q:\elsewhere\garden.e57" relativeRawScanPath=".\garden.e57"/>
    <station name="Hall 1" relativeRawScanPath="hall 1.e57"/>
  </group>
</project>
"#;

    fn project_folder() -> tempfile::TempDir {
        let folder = tempfile::tempdir().unwrap();
        touch(
            folder.path(),
            &[
                "hall 1.e57",
                "raw/attic.e57",
                "roof.laz",
                "Cellar & stairs.e57",
                "unlisted.e57",
            ],
        );
        folder
    }

    #[test]
    fn names_sort_by_number_and_without_case() {
        let mut names = vec!["scan 10", "Scan 2", "scan 1", "scan 02b", "attic", "Scan"];
        names.sort_by(|left, right| natural_cmp(left, right));
        assert_eq!(
            names,
            ["attic", "Scan", "scan 1", "Scan 2", "scan 02b", "scan 10"]
        );
        assert_eq!(natural_cmp("scan 007", "scan 7"), Ordering::Less);
        assert_eq!(natural_cmp("scan 7", "scan 7"), Ordering::Equal);
    }

    #[test]
    fn folder_expands_to_its_own_scans_in_natural_order() {
        let folder = tempfile::tempdir().unwrap();
        touch(
            folder.path(),
            &[
                "scan 10.e57",
                "scan 2.E57",
                "Scan 1.las",
                "notes.md",
                "project.rcp",
                "deeper/scan 3.e57",
            ],
        );
        let expansion = expand(&[folder.path().to_path_buf()], &[]);
        assert_eq!(
            names(&expansion.files),
            ["Scan 1.las", "scan 2.E57", "scan 10.e57"]
        );
        assert!(!expansion.has_notes());
        assert_eq!(expansion.summary(), "Opening 3 scans…");

        let empty = tempfile::tempdir().unwrap();
        let expansion = expand(&[empty.path().to_path_buf()], &[]);
        assert!(expansion.files.is_empty());
        assert_eq!(expansion.summary(), "No supported scan files found");
    }

    #[test]
    fn project_file_resolves_listed_scans_beside_it() {
        for deflate in [true, false] {
            let folder = project_folder();
            let project = folder.path().join("house.rcp");
            fs::write(
                &project,
                container(&[
                    ("preview.bin", &[7u8; 300], true),
                    ("thumbnail.bin", b"not inflated", false),
                    ("house.XML", PROJECT_XML.as_bytes(), deflate),
                ]),
            )
            .unwrap();
            let expansion = expand(std::slice::from_ref(&project), &[]);
            assert_eq!(expansion.errors, Vec::<String>::new());
            // Document order: relative path with other letter case, a
            // sub-folder with backslashes, then two matches by scan name.
            assert_eq!(
                expansion.files,
                [
                    folder.path().join("hall 1.e57"),
                    folder.path().join("raw").join("attic.e57"),
                    folder.path().join("roof.laz"),
                    folder.path().join("Cellar & stairs.e57"),
                ]
            );
            assert_eq!(expansion.missing, 1);
            assert_eq!(
                expansion.summary(),
                "Opening 4 scans; 1 listed scan not found"
            );
        }
    }

    #[test]
    fn stored_absolute_paths_are_never_followed() {
        let elsewhere = tempfile::tempdir().unwrap();
        touch(elsewhere.path(), &["far.e57"]);
        let far = elsewhere.path().join("far.e57");
        let xml = format!(
            r#"<p><s name="Remote" rawScanPath="{0}" relativeRawScanPath="{0}"/>
               <s name="{0}" rawScanPath="{0}"/></p>"#,
            far.display()
        );
        let folder = tempfile::tempdir().unwrap();
        let project = folder.path().join("remote.rcp");
        fs::write(&project, container(&[("list.xml", xml.as_bytes(), true)])).unwrap();
        let expansion = expand(&[project], &[]);
        assert!(expansion.files.is_empty());
        assert_eq!(expansion.missing, 2);
        assert_eq!(
            expansion.summary(),
            "No scans opened: 2 listed scans not found"
        );
    }

    #[test]
    fn repeated_and_already_open_scans_are_skipped() {
        let folder = project_folder();
        let project = folder.path().join("house.rcp");
        fs::write(
            &project,
            container(&[("house.xml", PROJECT_XML.as_bytes(), true)]),
        )
        .unwrap();
        let hall = folder.path().join("hall 1.e57");
        let roof = folder.path().join("roof.laz");
        let open = if cfg!(windows) {
            PathBuf::from(roof.to_string_lossy().to_uppercase())
        } else {
            roof
        };
        let chosen = [
            hall.clone(),
            folder.path().to_path_buf(),
            project,
            folder.path().join(".").join("unlisted.e57"),
        ];
        let expansion = expand(&chosen, &[open]);
        assert_eq!(
            expansion.files,
            [
                hall,
                folder.path().join("Cellar & stairs.e57"),
                folder.path().join("unlisted.e57"),
                folder.path().join("raw").join("attic.e57"),
            ]
        );
        assert_eq!(expansion.already_open, 1);
        assert_eq!(expansion.missing, 1);

        let mut again = expand(&chosen, &[]);
        again.skip_open(&expansion.files);
        assert_eq!(names(&again.files), ["roof.laz"]);
        assert_eq!(again.already_open, 4);
    }

    #[test]
    fn unusable_paths_are_reported_without_stopping_the_rest() {
        let folder = tempfile::tempdir().unwrap();
        touch(folder.path(), &["scan.e57", "report.pdf"]);
        fs::write(
            folder.path().join("text.rcp"),
            "plain text, not a container",
        )
        .unwrap();
        fs::write(
            folder.path().join("empty.rcp"),
            container(&[("preview.bin", b"pixels", false)]),
        )
        .unwrap();
        fs::write(
            folder.path().join("blank.rcp"),
            container(&[(
                "list.xml",
                b"<project><group name=\"Empty\"/></project>",
                true,
            )]),
        )
        .unwrap();
        let expansion = expand(
            &[
                folder.path().join("text.rcp"),
                folder.path().join("scan.e57"),
                folder.path().join("empty.rcp"),
                folder.path().join("blank.rcp"),
                folder.path().join("report.pdf"),
                folder.path().join("absent.e57"),
            ],
            &[],
        );
        assert_eq!(names(&expansion.files), ["scan.e57"]);
        assert_eq!(expansion.errors.len(), 5);
        assert!(expansion.errors[0].starts_with("Cannot read scan project file"));
        assert!(expansion.errors[0].ends_with("text.rcp: not a ZIP container"));
        assert!(expansion.errors[1].ends_with("empty.rcp: it holds no XML document"));
        assert!(expansion.errors[2].ends_with("blank.rcp lists no scans"));
        assert!(expansion.errors[3].ends_with("report.pdf is not a supported scan file"));
        assert!(expansion.errors[4].starts_with("Cannot open"));
        assert!(expansion
            .summary()
            .starts_with("Opening 1 scan; Cannot read"));
    }

    #[test]
    fn damaged_or_oversized_project_documents_are_refused() {
        let read = |bytes: Vec<u8>| read_listed_scans(&mut Cursor::new(bytes));
        let good = container(&[("list.xml", PROJECT_XML.as_bytes(), true)]);
        assert_eq!(read(good.clone()).unwrap().len(), 6);

        // Cut inside the compressed document.
        let mut truncated = good.clone();
        truncated.truncate(120);
        assert_eq!(read(truncated).unwrap_err(), "the XML document is damaged");

        // One flipped content byte fails the checksum.
        let mut stored = container(&[("list.xml", PROJECT_XML.as_bytes(), false)]);
        let position = stored
            .windows(4)
            .position(|window| window == b"Hall")
            .unwrap();
        stored[position] = b'W';
        assert_eq!(read(stored).unwrap_err(), "the XML document is damaged");

        // A header that announces more than the limit is refused unread.
        let mut huge = good.clone();
        huge[22..26].copy_from_slice(&(MAX_PROJECT_XML_BYTES as u32 + 1).to_le_bytes());
        assert_eq!(read(huge).unwrap_err(), too_large());

        let mut method = good.clone();
        method[8..10].copy_from_slice(&12u16.to_le_bytes());
        assert_eq!(
            read(method).unwrap_err(),
            "unsupported compression method 12"
        );

        let mut streamed = good;
        streamed[6] |= 0x08;
        assert!(read(streamed).unwrap_err().contains("not supported"));
        assert_eq!(read(Vec::new()).unwrap_err(), "not a ZIP container");
    }

    #[test]
    fn scan_elements_are_found_by_their_attributes() {
        let scans = listed_scans(PROJECT_XML);
        assert_eq!(
            scans
                .iter()
                .map(|scan| scan.name.as_str())
                .collect::<Vec<_>>(),
            [
                "Hall 1",
                "Attic",
                "Roof",
                "Cellar & stairs",
                "Garden",
                "Hall 1"
            ]
        );
        assert_eq!(scans[0].relative.as_deref(), Some(r".\HALL 1.E57"));
        assert_eq!(scans[2].relative, None);

        // Broken markup is stepped over without losing later elements.
        let scans = listed_scans(
            "<a name=broken <b name=\"One\" relativeRawScanPath=\"1.e57\"><![CDATA[<c name=\"Two\" \
             relativeRawScanPath=\"2.e57\">]]><d NAME=\"Three &#x41;&#66;&unknown;\" \
             RELATIVERAWSCANPATH=\"3.e57\"",
        );
        assert_eq!(
            scans,
            [
                ListedScan {
                    name: "One".into(),
                    relative: Some("1.e57".into())
                },
                ListedScan {
                    name: "Three AB&unknown;".into(),
                    relative: Some("3.e57".into())
                },
            ]
        );
    }

    #[test]
    fn utf16_documents_are_decoded() {
        let mut bytes = vec![0xFF, 0xFE];
        for unit in "<s name=\"Zolder é\" relativeRawScanPath=\"z.e57\"/>".encode_utf16() {
            bytes.extend(unit.to_le_bytes());
        }
        let scans = listed_scans(&decode_text(&bytes));
        assert_eq!(scans[0].name, "Zolder é");
        assert_eq!(decode_text(b"\xEF\xBB\xBF<a/>"), "<a/>");
    }
}
