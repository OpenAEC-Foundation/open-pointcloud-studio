//! Finalization of E57 files rewritten by Studio.
use crate::e57_quick::{logical_offset, physical_offset, Source};
use crate::LoadError;
use std::cell::Cell;
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use tempfile::NamedTempFile;

// e57 0.11.13 writes 1024-byte pages with four bytes of CRC32C.
const PAGE: u64 = 1024;
const BLOB_HEADER: u64 = 16;

pub(crate) fn finalize(
    mut writer: e57::E57Writer<File>,
    temporary: &NamedTempFile,
) -> Result<(), LoadError> {
    let sections = Cell::new(Vec::new());
    writer.finalize_customized_xml(|xml| {
        if xml.contains("type=\"Blob\"") {
            let doc = roxmltree::Document::parse(&xml).map_err(|e| e57::Error::Invalid {
                desc: e.to_string(),
                source: None,
            })?;
            let mut blobs = Vec::new();
            for node in doc
                .descendants()
                .filter(|n| n.attribute("type") == Some("Blob"))
            {
                let number = |key| node.attribute(key).and_then(|s| s.parse::<u64>().ok());
                let (Some(offset), Some(length)) = (number("fileOffset"), number("length")) else {
                    return e57::Error::invalid("invalid blob reference in generated E57 XML");
                };
                blobs.push((offset, length));
            }
            sections.set(blobs);
        }
        // The same writer spells the cylindrical radius element "readius".
        Ok(xml
            .replace("<readius ", "<radius ")
            .replace("<readius>", "<radius>")
            .replace("</readius>", "</radius>"))
    })?;
    // The paged writer flushes its own buffered page on drop.
    drop(writer);
    let blobs = sections.into_inner();
    if !blobs.is_empty() {
        let mut source = Source::open(temporary.path(), PAGE)?;
        let mut file = temporary.reopen()?;
        for (offset, length) in blobs {
            correct_blob_length(&mut source, &mut file, offset, length)?;
        }
    }
    Ok(())
}

fn correct_blob_length(
    source: &mut Source,
    file: &mut File,
    offset: u64,
    length: u64,
) -> Result<(), LoadError> {
    // The section length includes its header and zero padding to four bytes.
    // The XML length counts only the payload; e57 0.11.13 writes that value to both.
    let section_length = (BLOB_HEADER + length).next_multiple_of(4);
    let start = logical_offset(offset, PAGE);
    let mut header = Vec::new();
    source.read(start, BLOB_HEADER as usize, &mut header)?;
    if header[0] != 0 {
        return Err(LoadError::InvalidData(
            "expected an E57 blob section".into(),
        ));
    }
    if header[8..] == section_length.to_le_bytes() {
        return Ok(());
    }
    if header[8..] != length.to_le_bytes() {
        return Err(LoadError::InvalidData(
            "unexpected E57 blob section length".into(),
        ));
    }
    // Section headers can straddle a page checksum.
    for (i, byte) in section_length.to_le_bytes().into_iter().enumerate() {
        let physical = physical_offset(start + 8 + i as u64, PAGE);
        source.raw[(physical - source.raw_start) as usize] = byte;
    }
    for page in source.raw.chunks_exact_mut(PAGE as usize) {
        let (data, crc) = page.split_at_mut(PAGE as usize - 4);
        crc.copy_from_slice(&crc32c::crc32c(data).to_be_bytes());
    }
    file.seek(SeekFrom::Start(source.raw_start))?;
    file.write_all(&source.raw)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_length_spanning_two_pages_preserves_the_payload_and_both_checksums() {
        let mut file = NamedTempFile::new().unwrap();
        let offset = 1008;
        let length = 13u64;
        let mut logical = vec![0x5a; 2040];
        logical[offset..offset + 8].fill(0);
        logical[offset + 8..offset + 16].copy_from_slice(&length.to_le_bytes());
        for page in logical.chunks_exact(1020) {
            file.write_all(page).unwrap();
            file.write_all(&crc32c::crc32c(page).to_be_bytes()).unwrap();
        }
        let mut source = Source::open(file.path(), PAGE).unwrap();
        correct_blob_length(&mut source, file.as_file_mut(), offset as u64, length).unwrap();
        logical[offset + 8..offset + 16].copy_from_slice(&32u64.to_le_bytes());
        let mut actual = Vec::new();
        source.read(0, logical.len(), &mut actual).unwrap();
        assert_eq!(actual, logical);
    }
}
