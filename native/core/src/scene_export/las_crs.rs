//! Projection records may occur on either side of the LAS/LAZ point data.
use crate::LoadError;
use las::{raw, Vlr};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

pub(super) fn projection_records(path: &Path) -> Result<Vec<Vlr>, LoadError> {
    let file = File::open(path)?;
    let file_len = file.metadata()?.len();
    let mut reader = BufReader::new(file);
    let header = raw::Header::read_from(&mut reader)?;
    let mut records = Vec::new();
    for _ in 0..header.number_of_variable_length_records {
        let record = Vlr::new(raw::Vlr::read_from(&mut reader, false)?);
        if record.user_id.eq_ignore_ascii_case("LASF_Projection") {
            records.push(record);
        }
    }
    // las-rs 0.9.11 exposes just the first EVLR in its high-level Header.
    // Walk the directory, seeking past unrelated records such as waveforms.
    if let Some(directory) = header.evlr {
        reader.seek(SeekFrom::Start(directory.start_of_first_evlr))?;
        for _ in 0..directory.number_of_evlrs {
            let mut prefix = [0; 60];
            reader.read_exact(&mut prefix)?;
            let length = u64::from_le_bytes(prefix[20..28].try_into().unwrap());
            let end = reader
                .stream_position()?
                .checked_add(length)
                .filter(|end| *end <= file_len)
                .ok_or_else(|| {
                    LoadError::InvalidData("extended LAS record exceeds file size".into())
                })?;
            let user_id = prefix[2..18]
                .split(|&byte| byte == 0)
                .next()
                .unwrap_or_default();
            if user_id.eq_ignore_ascii_case(b"LASF_Projection") {
                let record = raw::Vlr::read_from(prefix.as_slice().chain(&mut reader), true)?;
                records.push(Vlr::new(record));
            } else {
                reader.seek(SeekFrom::Start(end))?;
            }
        }
    }
    Ok(records)
}
