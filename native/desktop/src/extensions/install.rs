//! Installing an extension: its folder or ZIP archive is copied into a
//! staging folder beside the installed extensions and checked there, the
//! user confirms what it declares, and then the staging folder takes the
//! place of `extensions/<id>`.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use flate2::read::DeflateDecoder;

use super::manifest::{self, Manifest, LOGS, MANIFEST, MAX_FILES, MAX_TOTAL_BYTES};

/// The largest archive that is read.
pub const MAX_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;
/// The most entries an archive may hold, folders included.
const MAX_ARCHIVE_ENTRIES: usize = 4 * MAX_FILES;
/// Folders of a source folder that are never copied: the logs of runs and
/// the history of a version control system.
const NOT_COPIED: [&str; 2] = [LOGS, ".git"];
const STAGING_PREFIX: &str = ".staging-";
const OLD_PREFIX: &str = ".old-";

/// An extension copied into a staging folder and checked, waiting for the
/// user to confirm it.
#[derive(Debug, Clone)]
pub struct Staged {
    pub folder: PathBuf,
    pub manifest: Manifest,
    /// The folder or archive it came from.
    pub source: PathBuf,
    pub files: usize,
    pub bytes: u64,
}

impl Staged {
    /// Throw the staging folder away, when the install is cancelled.
    pub fn discard(&self) {
        let _ = fs::remove_dir_all(&self.folder);
    }
}

/// Remove what an install that ended halfway left behind.
pub fn remove_leftovers(root: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(STAGING_PREFIX) || name.starts_with(OLD_PREFIX) {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// Copy an extension from a folder, the `extension.json` in a folder, or a
/// `.zip` archive into a new staging folder under `root`, and check it.
pub fn stage(source: &Path, root: &Path, known_commands: &[&str]) -> Result<Staged, String> {
    let metadata =
        fs::metadata(source).map_err(|error| format!("{}: {error}", source.display()))?;
    let is_zip = source
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"));
    let is_manifest = source
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case(MANIFEST));
    fs::create_dir_all(root).map_err(|error| format!("{}: {error}", root.display()))?;
    let folder = root.join(format!("{STAGING_PREFIX}{}", uuid::Uuid::new_v4().simple()));
    fs::create_dir(&folder).map_err(|error| format!("{}: {error}", folder.display()))?;
    let copied = if metadata.is_dir() {
        copy_folder(source, &folder)
    } else if metadata.is_file() && is_manifest {
        match source.parent() {
            Some(parent) => copy_folder(parent, &folder),
            None => Err("the manifest lies in no folder".into()),
        }
    } else if metadata.is_file() && is_zip {
        unpack(source, &folder)
    } else {
        Err(format!(
            "choose a .zip archive or the {MANIFEST} of an extension folder"
        ))
    };
    let checked = copied.and_then(|()| {
        let summary = manifest::check_folder(&folder, &[])?;
        let manifest = manifest::read(&folder, known_commands)?;
        // An archive keeps no permissions: a program that starts by itself
        // is made executable.
        #[cfg(unix)]
        if manifest.launch.interpreter.is_none() {
            use std::os::unix::fs::PermissionsExt;
            let program = manifest
                .launch
                .program
                .split('/')
                .fold(folder.clone(), |path, part| path.join(part));
            fs::set_permissions(&program, fs::Permissions::from_mode(0o755))
                .map_err(|error| format!("{}: {error}", manifest.launch.program))?;
        }
        Ok((summary, manifest))
    });
    match checked {
        Ok((summary, manifest)) => Ok(Staged {
            folder,
            manifest,
            source: source.to_path_buf(),
            files: summary.files.len(),
            bytes: summary.bytes,
        }),
        Err(error) => {
            let _ = fs::remove_dir_all(&folder);
            Err(error)
        }
    }
}

/// Copy the files of a checked source folder; links, odd names and too many
/// or too large files are refused before anything is copied.
fn copy_folder(source: &Path, target: &Path) -> Result<(), String> {
    let summary = manifest::check_folder(source, &NOT_COPIED)?;
    for relative in &summary.files {
        let to = target.join(relative);
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::copy(source.join(relative), &to)
            .map_err(|error| format!("{}: {error}", manifest::shown(relative)))?;
    }
    Ok(())
}

/// One file of an archive as its central directory describes it.
#[derive(Debug, Clone)]
struct ArchiveEntry {
    name: String,
    method: u16,
    flags: u16,
    checksum: u32,
    packed: u64,
    size: u64,
    /// Where its local header starts.
    offset: u64,
    /// Whether the archive marks it as a symbolic link.
    link: bool,
}

const LOCAL_HEADER: [u8; 4] = *b"PK\x03\x04";
const CENTRAL_HEADER: [u8; 4] = *b"PK\x01\x02";
const END_OF_DIRECTORY: [u8; 4] = *b"PK\x05\x06";

fn field16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn field32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// The entries of an archive, read from its central directory.
fn archive_entries<R: Read + Seek>(
    archive: &mut R,
    length: u64,
) -> Result<Vec<ArchiveEntry>, String> {
    let damaged = |_| "the archive is damaged".to_owned();
    // The end record lies within the last 64 KiB and 22 bytes.
    let tail_length = length.min(22 + 65_535);
    archive
        .seek(SeekFrom::Start(length - tail_length))
        .map_err(damaged)?;
    let mut tail = vec![0u8; tail_length as usize];
    archive.read_exact(&mut tail).map_err(damaged)?;
    let end = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&at| tail[at..at + 4] == END_OF_DIRECTORY)
        .ok_or_else(|| "it is not a ZIP archive".to_owned())?;
    let record = &tail[end..];
    if field16(record, 4) != 0 || field16(record, 6) != 0 {
        return Err("archives in several parts are not supported".into());
    }
    let count = usize::from(field16(record, 10));
    let directory_size = u64::from(field32(record, 12));
    let directory_offset = u64::from(field32(record, 16));
    if count == usize::from(u16::MAX) || directory_offset == u64::from(u32::MAX) {
        return Err("ZIP64 archives are not supported".into());
    }
    if count > MAX_ARCHIVE_ENTRIES {
        return Err(format!(
            "the archive holds more than {MAX_ARCHIVE_ENTRIES} entries"
        ));
    }
    if directory_offset + directory_size > length {
        return Err("the archive is damaged".into());
    }
    archive
        .seek(SeekFrom::Start(directory_offset))
        .map_err(damaged)?;
    let mut directory = vec![0u8; directory_size as usize];
    archive.read_exact(&mut directory).map_err(damaged)?;
    let mut entries = Vec::with_capacity(count);
    let mut at = 0usize;
    for _ in 0..count {
        let header = directory
            .get(at..at + 46)
            .filter(|header| header[..4] == CENTRAL_HEADER)
            .ok_or_else(|| "the archive is damaged".to_owned())?;
        let made_by = field16(header, 4) >> 8;
        let flags = field16(header, 8);
        let method = field16(header, 10);
        let checksum = field32(header, 16);
        let packed = u64::from(field32(header, 20));
        let size = u64::from(field32(header, 24));
        let name_length = usize::from(field16(header, 28));
        let extra_length = usize::from(field16(header, 30));
        let comment_length = usize::from(field16(header, 32));
        let external = field32(header, 38);
        let offset = u64::from(field32(header, 42));
        let name = directory
            .get(at + 46..at + 46 + name_length)
            .ok_or_else(|| "the archive is damaged".to_owned())?;
        // Names are UTF-8 when the flag says so; most tools write ASCII.
        let name = String::from_utf8(name.to_vec())
            .map_err(|_| "the archive holds a name that is not UTF-8".to_owned())?;
        if packed == u64::from(u32::MAX)
            || size == u64::from(u32::MAX)
            || offset == u64::from(u32::MAX)
        {
            return Err("ZIP64 archives are not supported".into());
        }
        // A Unix tool keeps the kind of file in the high half of the
        // external attributes; 0o120000 is a symbolic link.
        let link = made_by == 3 && (external >> 16) & 0o170_000 == 0o120_000;
        entries.push(ArchiveEntry {
            name,
            method,
            flags,
            checksum,
            packed,
            size,
            offset,
            link,
        });
        at += 46 + name_length + extra_length + comment_length;
    }
    Ok(entries)
}

/// Where an entry goes in the folder: refused when it would land outside it
/// or is a link.
fn entry_path(entry: &ArchiveEntry) -> Result<Option<PathBuf>, String> {
    if entry.link {
        return Err(format!("{} is a link; links are not allowed", entry.name));
    }
    let name = entry.name.trim_end_matches('/');
    // The resource forks macOS adds beside the files are no part of it.
    if name.is_empty() || name == "__MACOSX" || name.starts_with("__MACOSX/") {
        return Ok(None);
    }
    if entry.name.contains('\\') {
        return Err(format!(
            "{} uses \\ between folders; such archives are not supported",
            entry.name
        ));
    }
    let drive = name.as_bytes().get(1) == Some(&b':');
    if entry.name.starts_with('/') || drive {
        return Err(format!("{} is an absolute path", entry.name));
    }
    manifest::relative_path(name)
        .map(Some)
        .map_err(|reason| format!("{} {reason}", entry.name))
}

/// Unpack an archive into an empty folder. Every name must stay inside it;
/// links, encrypted entries and more than `MAX_TOTAL_BYTES` unpacked are
/// refused. An archive whose files all lie in one folder with the manifest
/// is unpacked from that folder.
pub fn unpack(archive: &Path, target: &Path) -> Result<(), String> {
    let mut file =
        File::open(archive).map_err(|error| format!("{}: {error}", archive.display()))?;
    let length = file.metadata().map_err(|error| error.to_string())?.len();
    if length > MAX_ARCHIVE_BYTES {
        return Err(format!(
            "the archive is larger than {} MiB",
            MAX_ARCHIVE_BYTES / (1024 * 1024)
        ));
    }
    let entries = archive_entries(&mut file, length)?;
    let mut paths = Vec::with_capacity(entries.len());
    for entry in &entries {
        paths.push(entry_path(entry)?);
    }
    // The archive of a folder often holds that folder; it is unpacked from
    // inside it.
    let manifest_at_top = paths
        .iter()
        .flatten()
        .any(|path| path.as_os_str().eq_ignore_ascii_case(MANIFEST));
    let strip = if manifest_at_top {
        None
    } else {
        let tops: BTreeSet<_> = paths
            .iter()
            .flatten()
            .filter_map(|path| path.components().next())
            .map(|part| part.as_os_str().to_owned())
            .collect();
        match tops.into_iter().collect::<Vec<_>>().as_slice() {
            [single] => Some(PathBuf::from(single)),
            _ => None,
        }
    };
    let mut seen = BTreeSet::new();
    let mut files = 0usize;
    let mut unpacked = 0u64;
    for (entry, path) in entries.iter().zip(paths) {
        let Some(path) = path else { continue };
        let path = match &strip {
            Some(top) => match path.strip_prefix(top) {
                Ok(inner) if !inner.as_os_str().is_empty() => inner.to_path_buf(),
                _ => continue,
            },
            None => path,
        };
        // Two names that differ in case only are one file on Windows and
        // macOS.
        if !seen.insert(path.to_string_lossy().to_lowercase()) {
            return Err(format!("{} is in the archive twice", entry.name));
        }
        let skipped = path
            .components()
            .next()
            .is_some_and(|top| NOT_COPIED.iter().any(|name| top.as_os_str() == *name));
        if skipped {
            continue;
        }
        let to = target.join(&path);
        if entry.name.ends_with('/') {
            fs::create_dir_all(&to).map_err(|error| error.to_string())?;
            continue;
        }
        files += 1;
        if files > MAX_FILES {
            return Err(format!("the archive holds more than {MAX_FILES} files"));
        }
        if entry.flags & 0x0001 != 0 {
            return Err(format!("{} is encrypted", entry.name));
        }
        let remaining = MAX_TOTAL_BYTES - unpacked;
        if entry.size > remaining {
            return Err(too_large());
        }
        let data = read_entry(&mut file, entry, remaining)?;
        unpacked += data.len() as u64;
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(&to, data).map_err(|error| format!("{}: {error}", entry.name))?;
    }
    Ok(())
}

fn too_large() -> String {
    format!(
        "its files are larger than {} MiB together",
        MAX_TOTAL_BYTES / (1024 * 1024)
    )
}

/// The content of one entry, at most `limit` bytes, checked against its
/// size and checksum.
fn read_entry(file: &mut File, entry: &ArchiveEntry, limit: u64) -> Result<Vec<u8>, String> {
    let damaged = |_: io::Error| format!("{} is damaged", entry.name);
    file.seek(SeekFrom::Start(entry.offset)).map_err(damaged)?;
    let mut header = [0u8; 30];
    file.read_exact(&mut header).map_err(damaged)?;
    if header[..4] != LOCAL_HEADER {
        return Err(format!("{} is damaged", entry.name));
    }
    let skip = i64::from(field16(&header, 26)) + i64::from(field16(&header, 28));
    file.seek_relative(skip).map_err(damaged)?;
    let packed = file.take(entry.packed);
    let mut data = Vec::new();
    // The declared size is not trusted: reading stops one byte past what
    // may still be unpacked.
    match entry.method {
        0 => packed.take(limit + 1).read_to_end(&mut data),
        8 => DeflateDecoder::new(packed)
            .take(limit + 1)
            .read_to_end(&mut data),
        other => {
            return Err(format!(
            "{} uses compression method {other}; only stored and deflated entries are supported",
            entry.name
        ))
        }
    }
    .map_err(damaged)?;
    if data.len() as u64 > limit {
        return Err(too_large());
    }
    let mut crc = flate2::Crc::new();
    crc.update(&data);
    if data.len() as u64 != entry.size || crc.sum() != entry.checksum {
        return Err(format!("{} is damaged", entry.name));
    }
    Ok(data)
}

/// Put a staged extension in its place, `root/<id>`, over an earlier version
/// whose logs it keeps. Runs of the extension must have ended.
pub fn commit(staged: &Staged, root: &Path) -> Result<PathBuf, String> {
    let target = root.join(&staged.manifest.id);
    let old = root.join(format!("{OLD_PREFIX}{}", uuid::Uuid::new_v4().simple()));
    let replacing = target.exists();
    if replacing {
        fs::rename(&target, &old).map_err(|error| {
            format!(
                "the installed version could not be replaced: {error}; close what uses its folder and try again"
            )
        })?;
    }
    if let Err(error) = fs::rename(&staged.folder, &target) {
        if replacing {
            let _ = fs::rename(&old, &target);
        }
        return Err(format!("the extension could not be put in place: {error}"));
    }
    if replacing {
        let _ = fs::rename(old.join(LOGS), target.join(LOGS));
        let _ = fs::remove_dir_all(&old);
    }
    Ok(target)
}

/// Remove an installed extension with its logs. Runs of it must have ended.
pub fn remove(root: &Path, id: &str) -> Result<(), String> {
    if !manifest::valid_id(id) {
        return Err(format!("{id} is not the id of an extension"));
    }
    let folder = root.join(id);
    match fs::remove_dir_all(&folder) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "its folder could not be removed: {error}; close what uses {} and try again",
            folder.display()
        )),
    }
}
