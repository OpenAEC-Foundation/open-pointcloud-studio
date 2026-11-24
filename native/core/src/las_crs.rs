//! Projection records may occur on either side of the LAS/LAZ point data.
use crate::LoadError;
use las::{raw, Vlr};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

pub(super) fn projection_records(path: &Path) -> Result<Vec<Vlr>, LoadError> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let header = raw::Header::read_from(&mut reader)?;
    let mut records = Vec::new();
    for _ in 0..header.number_of_variable_length_records {
        let record = Vlr::new(raw::Vlr::read_from(&mut reader, false)?);
        if record.user_id.eq_ignore_ascii_case("LASF_Projection") {
            records.push(record);
        }
    }
    append_extended_projection_records(&mut reader, header.evlr, 0, &mut records)?;
    Ok(records)
}

/// Retain projection EVLRs beyond the records exposed by las-rs when
/// transcoding, filtering or comparing headers for a layout-preserving merge.
pub(super) fn export_header(path: &Path, header: &las::Header) -> Result<las::Header, LoadError> {
    let known = header.evlrs().len();
    if known == 0 {
        return Ok(header.clone());
    }
    let mut reader = BufReader::new(File::open(path)?);
    let raw = raw::Header::read_from(&mut reader)?;
    if raw
        .evlr
        .is_none_or(|directory| directory.number_of_evlrs as usize <= known)
    {
        return Ok(header.clone());
    }
    let mut builder = las::Builder::from(header.clone());
    append_extended_projection_records(&mut reader, raw.evlr, known, &mut builder.evlrs)?;
    Ok(builder.into_header()?)
}

fn append_extended_projection_records(
    reader: &mut BufReader<File>,
    directory: Option<raw::header::Evlr>,
    known: usize,
    records: &mut Vec<Vlr>,
) -> Result<(), LoadError> {
    // las-rs 0.9.11 exposes just the first EVLR in its high-level Header.
    // Walk the directory, seeking past unrelated records such as waveforms.
    if let Some(directory) = directory {
        let file_len = reader.get_ref().metadata()?.len();
        reader.seek(SeekFrom::Start(directory.start_of_first_evlr))?;
        for index in 0..directory.number_of_evlrs {
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
            if index as usize >= known && user_id.eq_ignore_ascii_case(b"LASF_Projection") {
                let record = raw::Vlr::read_from(prefix.as_slice().chain(&mut *reader), true)?;
                records.push(Vlr::new(record));
            } else {
                reader.seek(SeekFrom::Start(end))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene_export::{write_e57_scene, write_las_scene, SceneSource};

    fn make_cloud(path: &Path, wkt: &str) -> crate::PointCloud {
        let mut builder = las::Builder::from((1, 4));
        builder.has_wkt_crs = true;
        builder.evlrs.push(las::Vlr {
            user_id: "survey".into(),
            record_id: 1,
            data: vec![0; 40],
            ..Default::default()
        });
        builder.evlrs.push(las::Vlr {
            user_id: "LASF_Projection".into(),
            record_id: 2112,
            data: wkt.bytes().chain([0]).collect(),
            ..Default::default()
        });
        let mut writer = las::Writer::from_path(path, builder.into_header().unwrap()).unwrap();
        writer.write_point(las::Point::default()).unwrap();
        writer.close().unwrap();
        crate::open(path, 4).unwrap()
    }

    #[test]
    fn point_exports_preserve_extended_projection_records_and_reject_conflicting_grids() {
        use crate::{export_full, export_where, merge_las_map_count, ExportFormat};
        let directory = tempfile::tempdir().unwrap();
        let wkt = r#"LOCAL_CS["Survey grid",UNIT["metre",1]]"#;
        let first = make_cloud(&directory.path().join("first.las"), wkt);
        let second = make_cloud(&directory.path().join("second.laz"), wkt);
        let other = make_cloud(
            &directory.path().join("other.las"),
            r#"LOCAL_CS["Other grid",UNIT["metre",1]]"#,
        );

        let converted = directory.path().join("converted.laz");
        export_full(&first, &converted, ExportFormat::Laz).unwrap();
        let selected = directory.path().join("selected.las");
        export_where(&second, &selected, ExportFormat::Las, 1, |_, _| true).unwrap();
        let merged = directory.path().join("merged.laz");
        let merge = |second: &crate::PointCloud| {
            merge_las_map_count(
                &[&first, second],
                &merged,
                ExportFormat::Laz,
                Some(2),
                &mut |_, _, point| Some(point),
                &mut |_, _, _| Ok(()),
            )
        };
        assert_eq!(merge(&second).unwrap(), 2);

        for path in [&converted, &selected, &merged] {
            let mut file = File::open(path).unwrap();
            let directory = raw::Header::read_from(&mut file).unwrap().evlr.unwrap();
            assert_eq!(directory.number_of_evlrs, 2);
            file.seek(SeekFrom::Start(directory.start_of_first_evlr))
                .unwrap();
            assert_eq!(
                Vlr::new(raw::Vlr::read_from(&mut file, true).unwrap()).user_id,
                "survey"
            );
            let projection = Vlr::new(raw::Vlr::read_from(&mut file, true).unwrap());
            assert_eq!(projection.user_id, "LASF_Projection");
            assert_eq!(projection.record_id, 2112);
            assert_eq!(projection.data, wkt.bytes().chain([0]).collect::<Vec<_>>());
        }

        let previous = std::fs::read(&merged).unwrap();
        assert!(merge(&other)
            .unwrap_err()
            .to_string()
            .contains("incompatible point formats, coordinate grids or metadata"));
        assert_eq!(std::fs::read(merged).unwrap(), previous);
    }

    #[test]
    fn e57_merge_with_images_accepts_wkt_layout_and_retains_the_first_declaration() {
        let directory = tempfile::tempdir().unwrap();
        let first = r#"LOCAL_CS["Survey ""West"" grid",UNIT["metre",1]]"#;
        let second = "LOCAL_CS[
  \"Survey \"\"West\"\" grid\", UNIT[\"metre\", 1] ]";
        let clouds = [("first", first), ("second", second)].map(|(stem, wkt)| {
            let input = make_cloud(&directory.path().join(format!("{stem}.las")), wkt);
            let e57 = directory.path().join(format!("{stem}.e57"));
            write_e57_scene(
                &[SceneSource {
                    cloud: &input,
                    transform: Default::default(),
                }],
                &e57,
                true,
                &mut |_, _, _| true,
                &mut |_, _| Ok(()),
            )
            .unwrap();
            crate::open(&e57, 4).unwrap()
        });
        let output = directory.path().join("merged.e57");
        let sources = clouds.each_ref().map(|cloud| SceneSource {
            cloud,
            transform: Default::default(),
        });
        assert_eq!(
            write_e57_scene(
                &sources,
                &output,
                true,
                &mut |_, _, _| true,
                &mut |_, _| Ok(())
            )
            .unwrap(),
            2
        );
        assert_eq!(
            crate::e57_points::open_reader(&output)
                .unwrap()
                .coordinate_metadata(),
            Some(first)
        );
    }

    #[test]
    fn scene_exports_use_wkt_after_an_unrelated_extended_las_record() {
        let directory = tempfile::tempdir().unwrap();
        let wkt = r#"LOCAL_CS["Survey grid",UNIT["metre",1]]"#;
        let first = make_cloud(&directory.path().join("first.las"), wkt);
        let second = make_cloud(
            &directory.path().join("second.laz"),
            r#"LOCAL_CS["Surveygrid",UNIT["metre",1]]"#,
        );
        let sources = [&first, &second].map(|cloud| SceneSource {
            cloud,
            transform: Default::default(),
        });

        let laz = directory.path().join("scene.laz");
        write_las_scene(
            &sources[..1],
            &laz,
            true,
            &mut |_, _, _| true,
            &mut |_, _| Ok(()),
        )
        .unwrap();
        let reader = las::Reader::from_path(&laz).unwrap();
        let record = reader
            .header()
            .all_vlrs()
            .find(|record| record.record_id == 2112)
            .unwrap();
        assert_eq!(record.data, wkt.bytes().chain([0]).collect::<Vec<_>>());
        let e57 = directory.path().join("scene.e57");
        write_e57_scene(
            &sources[..1],
            &e57,
            true,
            &mut |_, _, _| true,
            &mut |_, _| Ok(()),
        )
        .unwrap();
        assert_eq!(
            crate::e57_points::open_reader(&e57)
                .unwrap()
                .coordinate_metadata(),
            Some(wkt)
        );

        std::fs::write(&laz, b"existing").unwrap();
        let error = write_las_scene(
            &sources,
            &laz,
            true,
            &mut |_, _, _| true,
            &mut |_, _| Ok(()),
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("different coordinate reference systems"));
        assert_eq!(std::fs::read(laz).unwrap(), b"existing");
    }
}
