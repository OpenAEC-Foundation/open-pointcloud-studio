//! LAS/LAZ projection and extra-dimension records can occur before or after the points.
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
    append_extended_records(&mut reader, header.evlr, 0, &mut records, |user_id, _| {
        user_id.eq_ignore_ascii_case(b"LASF_Projection")
    })?;
    Ok(records)
}

/// Retain projection and Extra Bytes EVLRs beyond the records exposed by las-rs when
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
    append_extended_records(
        &mut reader,
        raw.evlr,
        known,
        &mut builder.evlrs,
        |user_id, record_id| {
            user_id.eq_ignore_ascii_case(b"LASF_Projection")
                || (user_id.eq_ignore_ascii_case(b"LASF_Spec") && record_id == 4)
        },
    )?;
    Ok(builder.into_header()?)
}

fn append_extended_records(
    reader: &mut BufReader<File>,
    directory: Option<raw::header::Evlr>,
    known: usize,
    records: &mut Vec<Vlr>,
    keep: impl Fn(&[u8], u16) -> bool,
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
            let record_id = u16::from_le_bytes([prefix[18], prefix[19]]);
            if index as usize >= known && keep(user_id, record_id) {
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
    fn point_exports_keep_extra_dimension_definitions_after_the_first_evlr() {
        use crate::{export_full, export_where, merge_las_map_count, ExportFormat};

        let dir = tempfile::tempdir().unwrap();
        let make_source = |file: &str, name: &[u8]| {
            let path = dir.path().join(file);
            let mut builder = las::Builder::from((1, 4));
            builder.point_format.extra_bytes = 2;
            let mut descriptor = vec![0; 192];
            descriptor[2] = 3; // LAS unsigned short
            descriptor[4..4 + name.len()].copy_from_slice(name);
            builder.evlrs.push(Vlr {
                user_id: "survey".into(),
                record_id: 1,
                ..Vlr::default()
            });
            builder.evlrs.push(Vlr {
                user_id: "LASF_Spec".into(),
                record_id: 4,
                data: descriptor.clone(),
                ..Vlr::default()
            });
            let mut writer = las::Writer::from_path(&path, builder.into_header().unwrap()).unwrap();
            for value in [420u16, 65000] {
                writer
                    .write_point(las::Point {
                        extra_bytes: value.to_le_bytes().to_vec(),
                        ..Default::default()
                    })
                    .unwrap();
            }
            writer.close().unwrap();
            (crate::open(&path, 2).unwrap(), descriptor)
        };
        let (left, descriptor) = make_source("left.las", b"Quality");
        let (right, _) = make_source("right.laz", b"Quality");
        let (other, _) = make_source("other.las", b"Channel");
        let converted = dir.path().join("converted.laz");
        let selected = dir.path().join("selected.las");
        let merged = dir.path().join("merged.laz");
        export_full(&left, &converted, ExportFormat::Laz).unwrap();
        export_where(&right, &selected, ExportFormat::Las, 1, |i, _| i == 0).unwrap();
        let merge = |cloud: &crate::PointCloud| {
            merge_las_map_count(
                &[&left, cloud],
                &merged,
                ExportFormat::Laz,
                Some(4),
                &mut |_, _, point| Some(point),
                &mut |_, _, _| Ok(()),
            )
        };
        assert_eq!(merge(&right).unwrap(), 4);
        for path in [&converted, &selected, &merged] {
            let mut f = File::open(path).unwrap();
            let directory = raw::Header::read_from(&mut f).unwrap().evlr.unwrap();
            assert_eq!(directory.number_of_evlrs, 2);
            f.seek(SeekFrom::Start(directory.start_of_first_evlr))
                .unwrap();
            raw::Vlr::read_from(&mut f, true).unwrap();
            let extra = Vlr::new(raw::Vlr::read_from(&mut f, true).unwrap());
            assert_eq!((extra.user_id.as_str(), extra.record_id), ("LASF_Spec", 4));
            assert_eq!(extra.data, descriptor);
        }
        let previous = std::fs::read(&merged).unwrap();
        assert!(merge(&other)
            .unwrap_err()
            .to_string()
            .contains("extra dimensions"));
        assert_eq!(std::fs::read(&merged).unwrap(), previous);
    }

    #[test]
    fn single_scan_e57_exports_keep_the_las_projection_wkt() {
        let dir = tempfile::tempdir().unwrap();
        let wkt = r#"LOCAL_CS["Survey grid",UNIT["metre",1]]"#;
        let cloud = make_cloud(&dir.path().join("source.laz"), wkt);
        let output = dir.path().join("scan.e57");
        crate::export_full(&cloud, &output, crate::ExportFormat::E57).unwrap();
        assert_eq!(
            crate::e57_points::open_reader(&output)
                .unwrap()
                .coordinate_metadata(),
            Some(wkt)
        );
    }

    #[test]
    fn e57_exports_report_a_geotiff_only_crs_before_replacing_the_destination() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("rd.las");
        let mut builder = las::Builder::from((1, 2));
        // GeoKeyDirectoryTag: projected model, ProjectedCSTypeGeoKey = RD New.
        builder.vlrs.push(Vlr {
            user_id: "LASF_Projection".into(),
            record_id: 34735,
            data: [1u16, 1, 0, 2, 1024, 0, 1, 1, 3072, 0, 1, 28992]
                .into_iter()
                .flat_map(u16::to_le_bytes)
                .collect(),
            ..Default::default()
        });
        let mut writer = las::Writer::from_path(&input, builder.into_header().unwrap()).unwrap();
        writer.write_point(las::Point::default()).unwrap();
        writer.close().unwrap();
        let cloud = crate::open(&input, 1).unwrap();
        let output = dir.path().join("scan.e57");
        std::fs::write(&output, b"previous export").unwrap();
        let single = crate::export_full(&cloud, &output, crate::ExportFormat::E57);
        assert!(single.unwrap_err().to_string().contains("WKT"));
        assert_eq!(std::fs::read(&output).unwrap(), b"previous export");
        let scene = write_e57_scene(
            &[SceneSource {
                cloud: &cloud,
                transform: Default::default(),
            }],
            &output,
            true,
            &mut |_, _, _| true,
            &mut |_, _| Ok(()),
        );
        assert!(scene.unwrap_err().to_string().contains("WKT"));
        assert_eq!(std::fs::read(&output).unwrap(), b"previous export");
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
