//! e57 0.11.13 discards scale/offset on scalar limit nodes. Resolve those
//! nodes to physical doubles before normalising points or writing metadata.
use e57::{ColorLimits, E57Reader, IntensityLimits, PointCloud, RecordValue};
use roxmltree::Node;
use std::io::{Read, Seek};

use crate::LoadError;

fn invalid(message: &str) -> LoadError {
    LoadError::InvalidData(format!("invalid E57 measurement limits: {message}"))
}

fn child<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.children().find(|n| {
        n.is_element()
            && n.tag_name().name() == name
            && n.tag_name().namespace() == node.tag_name().namespace()
    })
}

fn physical(node: Node<'_, '_>) -> Result<f64, LoadError> {
    let text = node.text().unwrap_or("0").trim();
    let parse_float = |text: &str| {
        text.parse::<f64>()
            .map_err(|_| invalid("non-numeric scalar"))
    };
    let value = match node.attribute("type") {
        Some("ScaledInteger") => {
            let raw = text
                .parse::<i64>()
                .map_err(|_| invalid("invalid scaled integer"))?;
            let scale = parse_float(node.attribute("scale").unwrap_or("1"))?;
            let offset = parse_float(node.attribute("offset").unwrap_or("0"))?;
            let minimum = node
                .attribute("minimum")
                .map(str::parse::<i64>)
                .transpose()
                .map_err(|_| invalid("invalid integer minimum"))?
                .unwrap_or(i64::MIN);
            let maximum = node
                .attribute("maximum")
                .map(str::parse::<i64>)
                .transpose()
                .map_err(|_| invalid("invalid integer maximum"))?
                .unwrap_or(i64::MAX);
            if !scale.is_finite()
                || scale == 0.0
                || !offset.is_finite()
                || raw < minimum
                || raw > maximum
            {
                return Err(invalid("invalid scale, offset or integer range"));
            }
            raw as f64 * scale + offset
        }
        Some("Integer") => text
            .parse::<i64>()
            .map_err(|_| invalid("invalid integer"))? as f64,
        Some("Float") if node.attribute("precision") == Some("single") => f64::from(
            text.parse::<f32>()
                .map_err(|_| invalid("invalid single precision value"))?,
        ),
        Some("Float") => parse_float(text)?,
        _ => return Err(invalid("unsupported limit type")),
    };
    value
        .is_finite()
        .then_some(value)
        .ok_or_else(|| invalid("non-finite physical value"))
}

/// A mixed scalar pair also becomes doubles: the upstream normaliser only
/// recognises pairs of the same scalar variant. Missing bounds stay missing.
fn pair(
    node: Node<'_, '_>,
    names: [&str; 2],
    output: [&mut Option<RecordValue>; 2],
) -> Result<bool, LoadError> {
    let nodes = names.map(|name| child(node, name));
    let variant = |node: Node<'_, '_>| match node.attribute("type") {
        Some("Integer") => 1,
        Some("ScaledInteger") => 2,
        Some("Float") if node.attribute("precision") == Some("single") => 3,
        Some("Float") => 4,
        _ => 0,
    };
    let mixed = match nodes {
        [Some(a), Some(b)] => variant(a) != variant(b),
        _ => false,
    };
    if !mixed
        && !nodes
            .iter()
            .flatten()
            .any(|n| n.attribute("type") == Some("ScaledInteger"))
    {
        return Ok(false);
    }
    let values = nodes
        .map(|n| n.map(physical).transpose())
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
    if let (Some(min), Some(max)) = (values[0], values[1]) {
        if min > max {
            return Err(invalid("minimum exceeds maximum"));
        }
    }
    for (target, value) in output.into_iter().zip(values) {
        *target = value.map(RecordValue::Double);
    }
    Ok(true)
}

pub(crate) fn repaired<T: Read + Seek>(
    reader: &E57Reader<T>,
) -> Result<(Vec<PointCloud>, bool), LoadError> {
    let document = roxmltree::Document::parse(reader.xml()).map_err(|_| invalid("invalid XML"))?;
    let root = document.root_element();
    let mut scans = reader.pointclouds();
    let Some(data) = child(root, "data3D") else {
        return Ok((scans, false));
    };
    let nodes = data
        .children()
        .filter(|n| n.is_element())
        .collect::<Vec<_>>();
    if nodes.len() != scans.len() {
        return Err(invalid("scan metadata count mismatch"));
    }
    let mut changed = false;
    for (scan, node) in scans.iter_mut().zip(nodes) {
        if let Some(node) = child(node, "intensityLimits") {
            let limits = scan.intensity_limits.get_or_insert(IntensityLimits {
                intensity_min: None,
                intensity_max: None,
            });
            changed |= pair(
                node,
                ["intensityMinimum", "intensityMaximum"],
                [&mut limits.intensity_min, &mut limits.intensity_max],
            )?;
        }
        if let Some(node) = child(node, "colorLimits") {
            let limits = scan.color_limits.get_or_insert(ColorLimits {
                red_min: None,
                red_max: None,
                green_min: None,
                green_max: None,
                blue_min: None,
                blue_max: None,
            });
            changed |= pair(
                node,
                ["colorRedMinimum", "colorRedMaximum"],
                [&mut limits.red_min, &mut limits.red_max],
            )?;
            changed |= pair(
                node,
                ["colorGreenMinimum", "colorGreenMaximum"],
                [&mut limits.green_min, &mut limits.green_max],
            )?;
            changed |= pair(
                node,
                ["colorBlueMinimum", "colorBlueMaximum"],
                [&mut limits.blue_min, &mut limits.blue_max],
            )?;
        }
    }
    Ok((scans, changed))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use e57::{E57Writer, Record, RecordDataType, RecordName};
    use std::path::Path;

    pub(crate) fn fixture(path: &Path, intensity: bool, colour: bool) {
        let mut writer = E57Writer::from_file(path, "file").unwrap();
        let mut prototype = vec![
            Record::CARTESIAN_X_F64,
            Record::CARTESIAN_Y_F64,
            Record::CARTESIAN_Z_F64,
            Record {
                name: RecordName::Intensity,
                data_type: RecordDataType::Single {
                    min: None,
                    max: None,
                },
            },
        ];
        for name in [
            RecordName::ColorRed,
            RecordName::ColorGreen,
            RecordName::ColorBlue,
        ] {
            prototype.push(Record {
                name,
                data_type: RecordDataType::Integer { min: 0, max: 65535 },
            });
        }
        let mut scan = writer.add_pointcloud("scan", prototype).unwrap();
        scan.set_intensity_limits(Some(IntensityLimits {
            intensity_min: Some(RecordValue::Single(0.)),
            intensity_max: Some(RecordValue::Single(1.)),
        }));
        scan.set_color_limits(Some(ColorLimits {
            red_min: Some(RecordValue::Double(0.)),
            red_max: Some(RecordValue::Double(4095.)),
            green_min: Some(RecordValue::Double(0.)),
            green_max: Some(RecordValue::Double(4095.)),
            blue_min: Some(RecordValue::Double(0.)),
            blue_max: Some(RecordValue::Double(4095.)),
        }));
        scan.add_point(vec![
            RecordValue::Double(1.),
            RecordValue::Double(2.),
            RecordValue::Double(3.),
            RecordValue::Single(0.25),
            RecordValue::Integer(2048),
            RecordValue::Integer(1024),
            RecordValue::Integer(4095),
        ])
        .unwrap();
        scan.finalize().unwrap();
        writer.finalize_customized_xml(|mut xml| {
            if intensity {
                let old = "<intensityMaximum type=\"Float\" precision=\"single\">1</intensityMaximum>";
                assert!(xml.contains(old));
                xml = xml.replace(old, &format!("<intensityMaximum type=\"ScaledInteger\" scale=\"{}\" offset=\"0\" minimum=\"0\" maximum=\"32767\">32767</intensityMaximum>", 1.0 / 32767.0));
            }
            if colour {
                for channel in ["Red", "Green", "Blue"] {
                    let old = format!("<color{channel}Maximum type=\"Float\">4095</color{channel}Maximum>");
                    assert!(xml.contains(&old));
                    xml = xml.replace(&old, &format!("<color{channel}Maximum type=\"ScaledInteger\" scale=\"0.0625\">65520</color{channel}Maximum>"));
                }
                xml = xml.replace("<colorBlueMinimum type=\"Float\">0</colorBlueMinimum>", "<colorBlueMinimum type=\"ScaledInteger\" scale=\"0.0625\" offset=\"-5\">80</colorBlueMinimum>");
            }
            Ok(xml)
        }).unwrap();
    }

    #[test]
    fn scaled_intensity_limits_read_and_export_in_physical_units() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.e57");
        fixture(&source, true, false);
        let cloud = crate::open(&source, 10).unwrap();
        assert_eq!(cloud.points[0].intensity, Some(16384));
        let output = directory.path().join("copy.e57");
        assert_eq!(
            crate::export_section(
                &cloud,
                &output,
                crate::ExportFormat::E57,
                crate::Bounds {
                    min: [-1e9; 3],
                    max: [1e9; 3]
                }
            )
            .unwrap(),
            1
        );
        let mut external_reader = E57Reader::from_file(&output).unwrap();
        let scan = external_reader.pointclouds().remove(0);
        assert!(
            matches!(scan.intensity_limits.as_ref().unwrap().intensity_max, Some(RecordValue::Double(v)) if v == 1.)
        );
        assert!(
            matches!(external_reader.pointcloud_raw(&scan).unwrap().next().unwrap().unwrap()[3], RecordValue::Single(v) if v == 0.25)
        );
        assert_eq!(
            crate::open(&output, 10).unwrap().points[0].intensity,
            Some(16384)
        );
    }

    #[test]
    fn scaled_colour_limits_keep_twelve_bit_rgb_and_edited_colours() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("colour.e57");
        fixture(&source, false, true);
        let cloud = crate::open(&source, 10).unwrap();
        assert_eq!(cloud.points[0].rgb, Some([128, 64, 255]));
        let output = directory.path().join("edited.e57");
        let mut colours = crate::photo_colour::PointColours::new(1);
        colours.set(0, [30, 60, 90]);
        assert_eq!(
            crate::export_e57_colours_where(
                &cloud,
                &output,
                Some(1),
                1.,
                [0.; 3],
                &colours,
                |_, _| true
            )
            .unwrap(),
            Some(1)
        );
        assert_eq!(
            crate::open(&output, 10).unwrap().points[0].rgb,
            Some([30, 60, 90])
        );
        let external_reader = E57Reader::from_file(&output).unwrap();
        let scan = external_reader.pointclouds().remove(0);
        assert!(
            matches!(scan.color_limits.unwrap().blue_max,Some(RecordValue::Double(v)) if v == 4095.)
        );
        assert!(matches!(
            scan.prototype[4].data_type,
            RecordDataType::Integer { min: 0, max: 65535 }
        ));
    }

    #[test]
    fn mixed_unscaled_limit_types_use_the_same_physical_range() {
        let document = roxmltree::Document::parse("<limits><lo type=\"Float\" precision=\"single\">0</lo><hi type=\"Float\">1</hi></limits>").unwrap();
        let (mut low, mut high) = (None, None);
        assert!(pair(document.root_element(), ["lo", "hi"], [&mut low, &mut high]).unwrap());
        assert!(matches!(low, Some(RecordValue::Double(v)) if v == 0.));
        assert!(matches!(high, Some(RecordValue::Double(v)) if v == 1.));
    }

    #[test]
    fn sensor_grid_frame_survives_filtering_and_scene_export() {
        for explicit in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let source = directory.path().join("grid.e57");
            let mut writer = E57Writer::from_file(&source, "file").unwrap();
            let mut scan = writer
                .add_pointcloud(
                    "scan",
                    vec![
                        Record::CARTESIAN_X_F64,
                        Record::CARTESIAN_Y_F64,
                        Record::CARTESIAN_Z_F64,
                        Record {
                            name: RecordName::RowIndex,
                            data_type: RecordDataType::Integer { min: 0, max: 2047 },
                        },
                        Record {
                            name: RecordName::ColumnIndex,
                            data_type: RecordDataType::Integer { min: 0, max: 1023 },
                        },
                    ],
                )
                .unwrap();
            scan.add_point(vec![
                RecordValue::Double(1.),
                RecordValue::Double(2.),
                RecordValue::Double(3.),
                RecordValue::Integer(5),
                RecordValue::Integer(4),
            ])
            .unwrap();
            let original = explicit.then_some(e57::IndexBounds {
                row_min: Some(0),
                row_max: Some(2047),
                column_min: Some(0),
                column_max: Some(1023),
                return_min: None,
                return_max: None,
            });
            scan.set_index_bounds(original.clone());
            scan.finalize().unwrap();
            writer.finalize().unwrap();
            drop(writer);
            let cloud = crate::open(&source, 10).unwrap();
            let filtered = directory.path().join("filtered.e57");
            crate::export_section(
                &cloud,
                &filtered,
                crate::ExportFormat::E57,
                crate::Bounds {
                    min: [-1e9; 3],
                    max: [1e9; 3],
                },
            )
            .unwrap();
            let scene = directory.path().join("scene.e57");
            crate::scene_export::write_e57_scene(
                &[crate::scene_export::SceneSource {
                    cloud: &cloud,
                    transform: crate::scene_transform::SceneTransform::default(),
                }],
                &scene,
                true,
                &mut |_, _, _| true,
                &mut |_, _| Ok(()),
            )
            .unwrap();
            for path in [filtered, scene] {
                let reader = E57Reader::from_file(path).unwrap();
                let bounds = reader.pointclouds().remove(0).index_bounds;
                let frame = |bounds: &e57::IndexBounds| {
                    [
                        bounds.row_min,
                        bounds.row_max,
                        bounds.column_min,
                        bounds.column_max,
                    ]
                };
                assert_eq!(bounds.as_ref().map(frame), original.as_ref().map(frame));
            }
        }
    }

    #[test]
    fn malformed_scaled_limits_are_refused_before_decoding() {
        for attributes in [
            "scale=\"NaN\"",
            "scale=\"0\"",
            "offset=\"inf\"",
            "minimum=\"10\" maximum=\"20\"",
            "scale=\"1e308\"",
        ] {
            let xml = format!("<limit type=\"ScaledInteger\" {attributes}>5</limit>");
            let document = roxmltree::Document::parse(&xml).unwrap();
            assert!(physical(document.root_element()).is_err(), "{attributes}");
        }
    }
}
