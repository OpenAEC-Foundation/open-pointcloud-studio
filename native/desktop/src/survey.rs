//! The `--survey` mode of the command line: the preparation of Mesh to
//! Plans on one scan file, without a window. It finds the box around the
//! building without the stray points far out and the main direction of its
//! walls, reads the scene once into a volume of occupied cells in that
//! direction, proposes the footprint, finds the levels and refines their
//! heights from the points, and writes what it found as JSON.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Instant;

use pointcloud_core::plans::{
    building_frame, detect_levels, refine_levels, robust_bounds, second_direction, survey_scene,
    BuildingFrame, Footprint, FootprintConfig, LevelConfig, LevelDetection, RefineConfig,
    RobustBounds, RobustBoundsConfig, SceneSurvey, SurveyConfig,
};
use pointcloud_core::region_source::{resident_points, RegionSource, SourceTransform};
use pointcloud_core::{Bounds, IndexConfig, IndexedPoint, LoadError, OctreeIndex, OrientedBox};
use serde_json::{json, Value};

use crate::bag_panel::plain_reason;
use crate::camera_views;
use crate::closed_mesh::UNINDEXED_LIMIT;

/// The name of the format of the file `--survey` writes.
const FORMAT: &str = "open-pointcloud-studio-survey";
const VERSION: u32 = 1;

/// What the survey of a scan found, and how long each step took.
struct Found {
    robust: RobustBounds,
    survey: SceneSurvey,
    footprint: Footprint,
    levels: LevelDetection,
    seconds: Vec<(&'static str, f64)>,
}

fn bounds_json(bounds: Bounds) -> Value {
    json!({ "min": bounds.min, "max": bounds.max })
}

/// The steps of the survey on the layers of a scene.
fn survey(sources: &[RegionSource<'_>]) -> Result<Found, LoadError> {
    let mut seconds = Vec::new();
    let mut clock = Instant::now();
    let mut lap = |name: &'static str, seconds: &mut Vec<(&'static str, f64)>| {
        seconds.push((name, clock.elapsed().as_secs_f64()));
        clock = Instant::now();
    };
    let robust = robust_bounds(sources, &RobustBoundsConfig::default())?
        .ok_or_else(|| LoadError::InvalidData("the scan holds no points".into()))?;
    lap("bounds", &mut seconds);
    let core = robust.bounds;
    let frame =
        building_frame(sources, core, &|_, _, _| true, &mut |_| Ok(()))?.unwrap_or_else(|| {
            let center = core.center();
            BuildingFrame::new(0.0, [center[0].round(), center[1].round()])
        });
    lap("frame", &mut seconds);
    let mut survey = survey_scene(
        sources,
        OrientedBox::from(core),
        &frame,
        &|_, _, _| true,
        &SurveyConfig::default(),
        &mut |_| Ok(()),
    )?;
    lap("survey", &mut seconds);
    let config = FootprintConfig::default();
    let walls = survey.wall_columns(config.wall_height);
    survey.frame.second_direction_deg = second_direction(&survey, &walls);
    let footprint = survey.footprint_proposal(&config);
    lap("footprint", &mut seconds);
    let mut levels = detect_levels(&survey, &footprint, &LevelConfig::default());
    refine_levels(
        sources,
        &survey,
        &footprint,
        &mut levels,
        &|_, _, _| true,
        &RefineConfig::default(),
        &mut |_, _| Ok(()),
    )?;
    if let Some(peil) = levels.peil_z {
        survey.frame.peil_z = peil;
    }
    lap("levels", &mut seconds);
    Ok(Found {
        robust,
        survey,
        footprint,
        levels,
        seconds,
    })
}

/// The JSON file of a survey.
fn file_json(source: &Path, points: u64, found: &Found) -> Value {
    let robust = &found.robust;
    let total: f64 = found.seconds.iter().map(|(_, seconds)| seconds).sum();
    let mut seconds: serde_json::Map<String, Value> = found
        .seconds
        .iter()
        .map(|(name, seconds)| ((*name).to_owned(), json!(seconds)))
        .collect();
    seconds.insert("total".to_owned(), json!(total));
    json!({
        "format": FORMAT,
        "version": VERSION,
        "source": source.file_name().map(|name| name.to_string_lossy()),
        "points": points,
        "bounds": {
            "core": bounds_json(robust.bounds),
            "all": bounds_json(robust.all),
            "outside_points": robust.outside_points,
            "outside_groups": robust.outside_groups,
            "below_points": robust.below_points,
            "below_groups": robust.below_groups,
        },
        "frame": found.survey.frame,
        "survey": found.survey.summary(Some(&found.footprint)),
        "levels": found.levels,
        "footprint": {
            "area": found.footprint.area(),
            "scene": found.footprint.regions.iter().map(|region| {
                let scene = |ring: &Vec<[f64; 2]>| -> Vec<[f64; 2]> {
                    ring.iter().map(|uv| found.survey.frame.to_scene_xy(*uv)).collect()
                };
                json!({
                    "outer": scene(&region.outer),
                    "holes": region.holes.iter().map(scene).collect::<Vec<_>>(),
                })
            }).collect::<Vec<_>>(),
        },
        "seconds": seconds,
    })
}

/// The lines `--survey` prints when it is done.
fn summary_lines(found: &Found, destination: &Path) -> String {
    let survey = &found.survey;
    let stats = &survey.stats;
    let total: f64 = found.seconds.iter().map(|(_, seconds)| seconds).sum();
    let mut lines = vec![format!(
        "Survey written: {} of {} points in a grid of {} by {} by {} cells of {:.3} by {:.3} by {:.3} m, \
         {} occupied, {} noise cells in {} groups taken out, {} MB; {} points outside the box in {} groups; \
         {:.1} s -> {}",
        stats.points,
        found.robust.points,
        survey.grid.size[0],
        survey.grid.size[1],
        survey.grid.size[2],
        survey.grid.cell_xy,
        survey.grid.cell_xy,
        survey.grid.cell_z,
        stats.occupied_cells,
        stats.noise_cells,
        stats.noise_groups,
        stats.bytes.div_ceil(1 << 20),
        found.robust.outside_points,
        found.robust.outside_groups,
        total,
        destination.display()
    )];
    let frame = &survey.frame;
    lines.push(match frame.second_direction_deg {
        Some(second) => format!(
            "Main direction {:.2}°, second direction {second:.1}°",
            frame.rotation_deg
        ),
        None => format!("Main direction {:.2}°", frame.rotation_deg),
    });
    let footprint = &found.footprint;
    lines.push(format!(
        "Footprint {:.1} m² in {} {}",
        footprint.area(),
        footprint.regions.len(),
        if footprint.regions.len() == 1 {
            "part"
        } else {
            "parts"
        }
    ));
    let peil = found.levels.peil_z.unwrap_or(0.0);
    for level in &found.levels.levels {
        let mut line = format!(
            "Level {:>4}  {:?}  floor {:+.3} m (scene {:.3} m)",
            level.id,
            level.kind,
            level.floor_z - peil,
            level.floor_z
        );
        if let Some(ceiling) = level.ceiling_z {
            line.push_str(&format!(", ceiling {:+.3} m", ceiling - peil));
        }
        if let Some(thickness) = level.slab_thickness {
            line.push_str(&format!(", slab {thickness:.3} m"));
        }
        if let Some([along_u, along_v]) = level.tilt_mm_per_m {
            line.push_str(&format!(", slope {along_u:.1}/{along_v:.1} mm/m"));
        }
        line.push_str(&format!(
            ", cut at {:.2} m, {:.0}% of the footprint, confidence {:.2}",
            level.cut_height,
            level.share * 100.0,
            level.confidence.score
        ));
        lines.push(line);
    }
    if let Some(ground) = found.levels.ground_z {
        lines.push(format!("Ground {:+.3} m", ground - peil));
    }
    lines.join("\n")
}

/// The `--survey` mode of the command line: survey a scan file and write
/// what was found as JSON. `arguments` are what follows the flag. Returns
/// the line to print, or the exit code with the line that says what is
/// wrong; an empty line stands for the usage line.
pub(crate) fn command_line(arguments: &[OsString]) -> Result<String, (i32, String)> {
    let usage = || (2, String::new());
    let wrong = |line: &str| (2, line.to_owned());
    let [source, destination] = arguments else {
        return Err(usage());
    };
    let (source, destination) = (PathBuf::from(source), PathBuf::from(destination));
    let is_json = destination
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"));
    if !is_json {
        return Err(wrong("The output of --survey is a .json file"));
    }
    if camera_views::source_key(&source) == camera_views::source_key(&destination) {
        return Err(wrong("Choose an output path different from the input"));
    }
    // A bare file name has an empty parent, which is the current folder.
    let folder = destination
        .parent()
        .filter(|folder| !folder.as_os_str().is_empty());
    if folder.is_some_and(|folder| !folder.is_dir()) {
        return Err(wrong("The folder of the output path does not exist"));
    }

    let failed = |error: LoadError| {
        (
            1,
            format!("Survey failed: {}", plain_reason(&error.to_string())),
        )
    };
    let cloud = crate::open_for_export(&source).map_err(failed)?;
    // An index that `--index` or the window left in the cache is used. A
    // file without one is read into memory when it is small enough, and gets
    // an index in a temporary folder otherwise, which goes when the survey
    // is done.
    let scratch;
    let mut index = OctreeIndex::open_cached_if_present(&cloud, IndexConfig::default())
        .ok()
        .flatten();
    if index.is_none() && cloud.total_points > UNINDEXED_LIMIT {
        scratch = tempfile::tempdir().map_err(|error| failed(error.into()))?;
        index = Some(
            OctreeIndex::build_cached(
                &cloud,
                IndexConfig {
                    scratch_dir: Some(scratch.path().to_path_buf()),
                    ..IndexConfig::default()
                },
            )
            .map_err(failed)?,
        );
    }
    let resident: Vec<IndexedPoint> = if index.is_none() {
        resident_points(&cloud, &mut |_| Ok(())).map_err(failed)?
    } else {
        Vec::new()
    };
    let layer = match &index {
        Some(index) => RegionSource::new(&cloud, Some(index), SourceTransform::default()),
        None => RegionSource::resident(&resident, SourceTransform::default()),
    };
    let found = survey(&[layer]).map_err(failed)?;
    let text = serde_json::to_string_pretty(&file_json(&source, cloud.total_points, &found))
        .map_err(|error| failed(LoadError::InvalidData(error.to_string())))?;
    std::fs::write(&destination, text).map_err(|error| failed(error.into()))?;
    Ok(summary_lines(&found, &destination))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A box-shaped building of two storeys of 3 m on a plot, turned by 25
    /// degrees, as the lines of an XYZ file: ground 0.3 m below the ground
    /// floor, facades, two floors and ceilings with slabs of 0.25 m, a roof,
    /// and a few stray points far below. A point every 4 cm: closer than the
    /// columns of a survey, so that a face is one group of cells and no
    /// noise.
    pub(crate) fn building_xyz() -> String {
        let spacing = 0.04;
        let (length, width, wall) = (8.0, 5.0, 0.25);
        let mut points: Vec<[f64; 3]> = Vec::new();
        let steps = |from: f64, to: f64| {
            let count = ((to - from) / spacing).round().max(1.0) as usize;
            let step = (to - from) / count as f64;
            (0..count).map(move |index| from + (index as f64 + 0.5) * step)
        };
        let mut level = |z: f64, min: [f64; 2], max: [f64; 2], skip: &dyn Fn(f64, f64) -> bool| {
            for x in steps(min[0], max[0]) {
                for y in steps(min[1], max[1]) {
                    if !skip(x, y) {
                        points.push([x, y, z]);
                    }
                }
            }
        };
        let inside = |x: f64, y: f64| x > 0.0 && x < length && y > 0.0 && y < width;
        level(-0.3, [-3.0, -3.0], [length + 3.0, width + 3.0], &inside);
        level(6.0, [0.0, 0.0], [length, width], &|_, _| false);
        for storey in 0..2 {
            let floor = storey as f64 * 3.0;
            let inner = ([wall, wall], [length - wall, width - wall]);
            level(floor, inner.0, inner.1, &|_, _| false);
            level(floor + 2.75, inner.0, inner.1, &|_, _| false);
        }
        // Facades from the ground to the roof, and the inner faces.
        let mut faces = |offset: f64, low: f64, high: f64| {
            let (x0, y0, x1, y1) = (offset, offset, length - offset, width - offset);
            for z in steps(low, high) {
                for x in steps(x0, x1) {
                    points.push([x, y0, z]);
                    points.push([x, y1, z]);
                }
                for y in steps(y0, y1) {
                    points.push([x0, y, z]);
                    points.push([x1, y, z]);
                }
            }
        };
        faces(0.0, -0.3, 6.0);
        faces(wall, 0.0, 2.75);
        faces(wall, 3.0, 5.75);
        let (sin, cos) = 25f64.to_radians().sin_cos();
        let mut text = String::new();
        for [x, y, z] in points {
            let (u, v) = (cos * x - sin * y + 1_000.0, sin * x + cos * y + 2_000.0);
            text.push_str(&format!("{u:.4} {v:.4} {z:.4}\n"));
        }
        for step in 0..12 {
            text.push_str(&format!(
                "{:.4} 2002.0000 {:.4}\n",
                1_003.0 + step as f64 * 0.37,
                -9.0 - step as f64 * 0.05
            ));
        }
        text
    }

    #[test]
    fn command_line_surveys_a_scan_file_and_writes_json() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("building.xyz");
        std::fs::write(&source, building_xyz()).unwrap();
        let output = directory.path().join("survey.json");
        let arguments: Vec<OsString> = vec![source.clone().into(), output.clone().into()];
        let line = command_line(&arguments).unwrap();
        assert!(line.starts_with("Survey written: "), "{line}");
        assert!(line.contains("12 points outside the box in "), "{line}");
        assert!(
            line.lines()
                .next()
                .unwrap()
                .ends_with(&format!("-> {}", output.display())),
            "{line}"
        );
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&output).unwrap()).unwrap();
        assert_eq!(written["format"], FORMAT);
        assert_eq!(written["version"], 1);
        assert_eq!(written["source"], "building.xyz");
        let points = written["points"].as_u64().unwrap();
        assert!(points > 300_000, "{points}");
        // The box leaves the strays out and keeps the building.
        let core = &written["bounds"]["core"];
        let low = core["min"][2].as_f64().unwrap();
        let high = core["max"][2].as_f64().unwrap();
        assert!(low > -1.31 && low < -0.3, "{low}");
        assert!((6.0..7.0).contains(&high), "{high}");
        assert!(written["bounds"]["all"]["min"][2].as_f64().unwrap() < -9.0);
        assert_eq!(written["bounds"]["outside_points"], 12);
        assert_eq!(written["bounds"]["below_points"], 12);
        assert_eq!(written["bounds"]["below_groups"], 1);
        let survey = &written["survey"];
        assert_eq!(survey["grid"]["cell_xy"], 0.05);
        assert_eq!(survey["grid"]["cell_z"], 0.02);
        let histogram = survey["area_histogram"].as_array().unwrap();
        assert_eq!(
            histogram.len() as u64,
            survey["grid"]["size"][2].as_u64().unwrap()
        );
        assert!(survey["stats"]["occupied_cells"].as_u64().unwrap() > 10_000);
        assert!(survey["digest"].as_u64().is_some());
        assert!(written["seconds"]["total"].as_f64().unwrap() >= 0.0);
        // Along the walls, turned by 25 degrees, with the footprint of 8 by
        // 5 m.
        let rotation = written["frame"]["rotation_deg"].as_f64().unwrap();
        assert!((rotation - 25.0).abs() < 0.05, "{rotation}");
        assert!(written["frame"]["second_direction_deg"].is_null());
        assert!(line.contains("\nMain direction 25.0"), "{line}");
        assert!(line.contains("\nFootprint 4"), "{line}");
        // Two storeys and the roof, the ground floor at P, and the ground.
        assert!(
            line.contains("\nLevel   00  Ground  floor +0.000 m"),
            "{line}"
        );
        assert!(line.contains("\nLevel   01  Storey  floor +"), "{line}");
        assert!(line.contains("\nLevel    R  Roof  floor +"), "{line}");
        assert!(line.contains("\nGround -0."), "{line}");
        let ground = written["levels"]["ground_z"].as_f64().unwrap();
        assert!((ground + 0.3).abs() < 0.02, "{ground}");
        let levels = written["levels"]["levels"].as_array().unwrap();
        let floors: Vec<f64> = levels
            .iter()
            .map(|level| level["floor_z"].as_f64().unwrap())
            .collect();
        assert_eq!(floors.len(), 3);
        for (found, truth) in floors.iter().zip([0.0, 3.0, 6.0]) {
            assert!((found - truth).abs() <= 0.002, "{floors:?}");
        }
        assert_eq!(levels[0]["is_peil"], true);
        assert_eq!(written["frame"]["peil_z"], written["levels"]["peil_z"]);
        let area = written["footprint"]["area"].as_f64().unwrap();
        assert!(area > 40.0 && area < 41.5, "{area}");
        let outline = written["survey"]["footprint"][0]["outer"]
            .as_array()
            .unwrap();
        assert!(outline.len() >= 4 && outline.len() <= 8, "{outline:?}");
        let corner = &written["footprint"]["scene"][0]["outer"][0];
        assert!((corner[0].as_f64().unwrap() - 1_000.0).abs() < 10.0);
        assert!((corner[1].as_f64().unwrap() - 2_000.0).abs() < 10.0);

        // The same file gives the same survey.
        let again = directory.path().join("again.json");
        command_line(&[source.clone().into(), again.clone().into()]).unwrap();
        let repeated: Value =
            serde_json::from_str(&std::fs::read_to_string(&again).unwrap()).unwrap();
        assert_eq!(repeated["survey"], written["survey"]);
    }

    #[test]
    fn command_line_refuses_what_it_cannot_survey() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("building.xyz");
        std::fs::write(&source, "0 0 0\n1 1 1\n").unwrap();
        let run = |arguments: &[&Path]| {
            command_line(
                &arguments
                    .iter()
                    .map(|path| path.as_os_str().to_owned())
                    .collect::<Vec<_>>(),
            )
        };
        assert_eq!(run(&[&source]), Err((2, String::new())));
        assert_eq!(
            run(&[&source, &directory.path().join("survey.txt")]),
            Err((2, "The output of --survey is a .json file".to_owned()))
        );
        assert_eq!(
            run(&[
                &source,
                &directory.path().join("missing").join("survey.json")
            ]),
            Err((2, "The folder of the output path does not exist".to_owned()))
        );
        let missing = run(&[
            &directory.path().join("absent.xyz"),
            &directory.path().join("survey.json"),
        ])
        .unwrap_err();
        assert_eq!(missing.0, 1);
        assert!(missing.1.starts_with("Survey failed: "), "{}", missing.1);
    }
}
