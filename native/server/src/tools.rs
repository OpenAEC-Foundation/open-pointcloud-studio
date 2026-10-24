//! The tools of the cloud API: one table, from which `GET /v1/tools`, MCP
//! `tools/list` and the OpenAPI paths are built, and `call_blocking`, which
//! runs one call inside its work folder through `pointcloud_core::ops`.
//!
//! Every tool takes the file in the request (`input`), never a path, and
//! nothing it does outlives the call. The argument names and limits are
//! those of the desktop MCP tools where the meaning is the same, and the
//! limits come from the constants of the core.

use std::borrow::Cow;
use std::fs;
use std::path::{Path, PathBuf};

use pointcloud_core::ops::{self, CachedIndex, Convert, KeepGoing, SectionDrawingOptions};
use pointcloud_core::{
    Bounds, DrawingFormat, DrawingUnits, DrawingView, LoadError, MeshStats, OrientedBox,
    SurfaceMeshConfig, MAX_SLAB_THICKNESS, MIN_SLAB_THICKNESS, SURFACE_MESH_NEIGHBORS,
    SURFACE_MESH_VERTICES,
};
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{json, Map, Value};
use tempfile::TempDir;

use crate::error::ApiError;
use crate::payload;

/// What a call may produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallLimits {
    /// The largest result file, in bytes before base64.
    pub max_output: u64,
}

/// What a call answers: its report, and the file it wrote into its work
/// folder, if any.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    pub report: Value,
    pub file: Option<OutputFile>,
}

/// A result file in the work folder of a call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputFile {
    /// The key of its format: `laz`, `dxf`, `obj`, ...
    pub format: String,
    pub media_type: &'static str,
    /// `<stem>.<ext>`, the stem being that of `input.name`.
    pub file_name: String,
    pub path: PathBuf,
}

/// One tool of the table.
#[derive(Debug)]
pub struct Tool {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    input_schema: fn() -> Value,
    example: fn() -> Value,
    /// Whether the tool answers with a file.
    pub file_result: bool,
}

impl Tool {
    /// The JSON Schema of its arguments.
    pub fn input_schema(&self) -> Value {
        (self.input_schema)()
    }

    /// Valid arguments, which the tests and the OpenAPI document use.
    pub fn example(&self) -> Value {
        (self.example)()
    }

    /// The entry of `GET /v1/tools` and MCP `tools/list`.
    pub fn listing(&self) -> Value {
        json!({
            "name": self.name,
            "title": self.title,
            "description": self.description,
            "inputSchema": self.input_schema(),
            // Nothing a tool does persists on the server.
            "annotations": {"readOnlyHint": true, "openWorldHint": false},
        })
    }
}

/// The tools, in the order they are listed.
pub fn table() -> &'static [Tool] {
    &TOOLS
}

/// The tool of this name.
pub fn find(name: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|tool| tool.name == name)
}

/// `{"tools": [...]}`, as `GET /v1/tools` and MCP `tools/list` answer.
pub fn listing() -> Value {
    json!({"tools": TOOLS.iter().map(Tool::listing).collect::<Vec<_>>()})
}

/// What every description ends with: what the tools cannot do.
macro_rules! no_paths {
    () => {
        " The file travels in the request as base64 and no path on the server can be named; files travel up to the body limit, so the tool suits programs rather than a language model writing out a scan."
    };
}

static TOOLS: [Tool; 5] = [
    Tool {
        name: "inspect",
        title: "Inspect a point cloud",
        description: concat!("Reports what a point-cloud file holds: the number of points, the bounds, whether the points carry colour, intensity and class, and the scanner stations with their positions and axes.", no_paths!()),
        input_schema: inspect_schema,
        example: inspect_example,
        file_result: false,
    },
    Tool {
        name: "convert",
        title: "Convert a point cloud",
        description: concat!("Converts a point-cloud file to LAS, LAZ, E57, binary PLY, XYZ, PTS or CSV, all its points or those inside a box in the units of the scan, turned about the vertical by rotation degrees. The result comes back as base64 with its SHA-256; a result above the output limit is refused, before the work where its size is known.", no_paths!()),
        input_schema: convert_schema,
        example: convert_example,
        file_result: true,
    },
    Tool {
        name: "section_drawing",
        title: "Section drawing",
        description: concat!("Draws the slab behind one face of a box through a point-cloud file as a 2D DXF or DWG drawing: plan takes the slab under the top face, seen from above; front, back, left and right take the slab behind the face at Y min, Y max, X min and X max. The box is in the units of the scan; a plan fills the cut by default. The drawing comes back as base64.", no_paths!()),
        input_schema: section_drawing_schema,
        example: section_drawing_example,
        file_result: true,
    },
    Tool {
        name: "terrain_mesh",
        title: "Terrain mesh",
        description: concat!("Makes a 2.5D terrain mesh from the lowest points of a point-cloud file seen from above, as an OBJ file that comes back as base64; 3D results come in OBJ only.", no_paths!()),
        input_schema: terrain_mesh_schema,
        example: terrain_mesh_example,
        file_result: true,
    },
    Tool {
        name: "surface_mesh",
        title: "Surface mesh",
        description: concat!("Makes a 3D surface mesh through a sample of the points of a point-cloud file, as an OBJ file that comes back as base64; the surface is not watertight, and 3D results come in OBJ only.", no_paths!()),
        input_schema: surface_mesh_schema,
        example: surface_mesh_example,
        file_result: true,
    },
];

// --- Schemas -------------------------------------------------------------

/// An object of exactly these properties, of which `required` must be given.
fn object(properties: Vec<(&str, Value)>, required: &[&str]) -> Value {
    let properties: Map<String, Value> = properties
        .into_iter()
        .map(|(name, schema)| (name.to_owned(), schema))
        .collect();
    let mut schema = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": properties,
    });
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    schema
}

/// The pattern `input.name` must match: a file name, never a path.
pub const NAME_PATTERN: &str = "^[A-Za-z0-9][A-Za-z0-9 ._-]*$";
/// The longest `input.name`.
pub const NAME_MAX: usize = 100;

fn input_schema() -> Value {
    object(
        vec![
            (
                "format",
                json!({"type": "string", "enum": ops::POINT_FORMATS, "description": "The format of the file, by its usual extension"}),
            ),
            (
                "data",
                json!({"type": "string", "contentEncoding": "base64", "description": "The file, base64 (RFC 4648 section 4, padding, no line breaks)"}),
            ),
            (
                "name",
                json!({"type": "string", "maxLength": NAME_MAX, "pattern": NAME_PATTERN, "description": "File name of the scan, not a path: its stem names the result and the point layer of a drawing. Default input"}),
            ),
        ],
        &["format", "data"],
    )
}

fn box_schema(description: &str) -> Value {
    let corner = |description: &str| json!({"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3, "description": description});
    let mut schema = object(
        vec![
            (
                "min",
                corner("Lowest corner [x, y, z] in the units of the scan, before the turn"),
            ),
            (
                "max",
                corner("Highest corner [x, y, z] in the units of the scan, before the turn"),
            ),
            (
                "rotation",
                json!({"type": "number", "minimum": -3_600.0, "maximum": 3_600.0, "description": "Degrees counter-clockwise seen from above, about the vertical through the centre of the box"}),
            ),
        ],
        &["min", "max"],
    );
    schema["description"] = Value::from(description);
    schema
}

fn inspect_schema() -> Value {
    object(vec![("input", input_schema())], &["input"])
}

fn convert_schema() -> Value {
    object(
        vec![
            ("input", input_schema()),
            (
                "output_format",
                json!({"type": "string", "enum": ops::EXPORT_FORMATS, "description": "The format of the result; ply is binary"}),
            ),
            (
                "box",
                box_schema("Only the points inside this box; all points without it"),
            ),
        ],
        &["input", "output_format"],
    )
}

fn section_drawing_schema() -> Value {
    let views = DrawingView::ALL.map(DrawingView::key);
    let units = DrawingUnits::ALL.map(DrawingUnits::key);
    let formats = DrawingFormat::ALL.map(DrawingFormat::extension);
    object(
        vec![
            ("input", input_schema()),
            ("box", box_schema("The section box whose face is drawn")),
            (
                "view",
                json!({"type": "string", "enum": views, "description": "Face of the box that is drawn: plan takes the slab under the top face, seen from above; front, back, left and right take the slab behind the face at Y min, Y max, X min and X max. Default plan"}),
            ),
            (
                "thickness",
                json!({"type": "number", "minimum": MIN_SLAB_THICKNESS, "maximum": MAX_SLAB_THICKNESS, "description": "Depth of the slab behind the cut plane in metres, from 0.005 to 5; it is kept within the box. Default 0.10"}),
            ),
            (
                "units",
                json!({"type": "string", "enum": units, "description": "Unit of the drawing: mm (the default) or m. The units of the scan are taken as metres"}),
            ),
            (
                "fill",
                json!({"type": "boolean", "description": "Whether the material the cut plane goes through is drawn as filled regions with outlines. Default on for plan and off for the other views"}),
            ),
            (
                "output_format",
                json!({"type": "string", "enum": formats, "description": "The format of the drawing. Default dxf"}),
            ),
        ],
        &["input", "box"],
    )
}

fn terrain_mesh_schema() -> Value {
    object(vec![("input", input_schema())], &["input"])
}

fn surface_mesh_schema() -> Value {
    let (vertices, neighbors) = (SURFACE_MESH_VERTICES, SURFACE_MESH_NEIGHBORS);
    object(
        vec![
            ("input", input_schema()),
            (
                "max_vertices",
                json!({"type": "integer", "minimum": *vertices.start() as u64, "maximum": *vertices.end() as u64, "description": "Most vertices, from 3 to 1000000. Default 50000"}),
            ),
            (
                "neighbors",
                json!({"type": "integer", "minimum": *neighbors.start() as u64, "maximum": *neighbors.end() as u64, "description": "Neighbours per vertex, from 3 to 32. Default 12"}),
            ),
            (
                "edge_factor",
                json!({"type": "number", "exclusiveMinimum": 0, "description": "Longest edge relative to the typical point spacing. Default 4"}),
            ),
            (
                "mesh_size",
                json!({"type": "number", "minimum": 0.0, "description": "Width of a voxel in the units of the scan within which one point is kept before the vertices are thinned, from 0. Default 0, which leaves the spacing to max_vertices"}),
            ),
        ],
        &["input"],
    )
}

// --- Examples ------------------------------------------------------------

/// The text of a small XYZ file, made from `point` for every place of a
/// grid of `count` by `count` points 0.1 apart.
fn grid_text(count: u32, point: impl Fn(f64, f64) -> Vec<[f64; 3]>) -> String {
    let mut text = String::new();
    for row in 0..count {
        for column in 0..count {
            for [x, y, z] in point(f64::from(column) * 0.1, f64::from(row) * 0.1) {
                text.push_str(&format!("{x:.3} {y:.3} {z:.3}\n"));
            }
        }
    }
    text
}

fn example_input(text: &str) -> Value {
    json!({"format": "xyz", "data": payload::encode_base64(text.as_bytes()), "name": "example.xyz"})
}

/// A sloping field of 8 by 8 points.
fn field() -> String {
    grid_text(8, |x, y| vec![[x, y, 0.05 * x + 0.02 * y]])
}

fn inspect_example() -> Value {
    json!({"input": example_input(&field())})
}

fn convert_example() -> Value {
    json!({
        "input": example_input(&field()),
        "output_format": "laz",
        "box": {"min": [0.0, 0.0, -1.0], "max": [0.35, 0.7, 1.0], "rotation": 0.0},
    })
}

fn section_drawing_example() -> Value {
    // The walls of a box of 0.7 by 0.7 m at three heights.
    let walls = grid_text(8, |x, y| {
        let on_wall = x < 0.05 || y < 0.05 || x > 0.65 || y > 0.65;
        if on_wall {
            [0.0, 0.5, 1.0].map(|z| [x, y, z]).to_vec()
        } else {
            Vec::new()
        }
    });
    json!({
        "input": example_input(&walls),
        "box": {"min": [-0.2, -0.2, 0.0], "max": [0.9, 0.9, 0.55]},
        "view": "plan",
        "output_format": "dxf",
    })
}

fn terrain_mesh_example() -> Value {
    json!({"input": example_input(&field())})
}

fn surface_mesh_example() -> Value {
    let dome = grid_text(10, |x, y| {
        vec![[x, y, 0.3 - ((x - 0.45).powi(2) + (y - 0.45).powi(2))]]
    });
    json!({"input": example_input(&dome), "max_vertices": 500, "neighbors": 8})
}

// --- Arguments -----------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Input<'a> {
    format: String,
    #[serde(borrow)]
    data: Cow<'a, str>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct BoxArgument {
    min: [f64; 3],
    max: [f64; 3],
    #[serde(default)]
    rotation: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InspectArguments<'a> {
    #[serde(borrow)]
    input: Input<'a>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConvertArguments<'a> {
    #[serde(borrow)]
    input: Input<'a>,
    output_format: String,
    #[serde(default, rename = "box")]
    section: Option<BoxArgument>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SectionDrawingArguments<'a> {
    #[serde(borrow)]
    input: Input<'a>,
    #[serde(rename = "box")]
    section: BoxArgument,
    #[serde(default)]
    view: Option<String>,
    #[serde(default)]
    thickness: Option<f64>,
    #[serde(default)]
    units: Option<String>,
    #[serde(default)]
    fill: Option<bool>,
    #[serde(default)]
    output_format: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MeshArguments<'a> {
    #[serde(borrow)]
    input: Input<'a>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SurfaceMeshArguments<'a> {
    #[serde(borrow)]
    input: Input<'a>,
    #[serde(default)]
    max_vertices: Option<u64>,
    #[serde(default)]
    neighbors: Option<u64>,
    #[serde(default)]
    edge_factor: Option<f64>,
    #[serde(default)]
    mesh_size: Option<f64>,
}

/// The typed arguments of a call, borrowing `data` from the request.
fn parse<'a, T: Deserialize<'a>>(arguments: &'a RawValue) -> Result<T, ApiError> {
    serde_json::from_str(arguments.get())
        .map_err(|error| ApiError::argument(format!("The arguments do not fit the tool: {error}")))
}

impl BoxArgument {
    /// The box, when its corners are finite and in order and its turn is
    /// within the limits.
    fn oriented(self) -> Result<OrientedBox, ApiError> {
        let rotation = self.rotation.unwrap_or(0.0);
        if !rotation.is_finite() || !(-3_600.0..=3_600.0).contains(&rotation) {
            return Err(ApiError::argument(
                "box.rotation must be a number of degrees from -3600 to 3600.",
            ));
        }
        let ordered = (0..3).all(|axis| {
            self.min[axis].is_finite()
                && self.max[axis].is_finite()
                && self.min[axis] <= self.max[axis]
        });
        if !ordered {
            return Err(ApiError::argument(
                "box.min and box.max must be finite and box.min must not exceed box.max on any axis.",
            ));
        }
        Ok(OrientedBox::new(
            Bounds {
                min: self.min,
                max: self.max,
            },
            rotation,
        ))
    }
}

/// The upload of a call, written into its work folder.
struct Received {
    path: PathBuf,
    /// The stem of `input.name`, which names the result and the point layer
    /// of a drawing.
    stem: String,
}

/// Check `input` and decode its file into `<work>/input.<format>`.
fn receive(
    input: &Input<'_>,
    work: &Path,
    keep_going: KeepGoing<'_>,
) -> Result<Received, ApiError> {
    if !ops::POINT_FORMATS.contains(&input.format.as_str()) {
        return Err(ApiError::argument(format!(
            "input.format must be one of {}.",
            ops::POINT_FORMATS.join(", ")
        )));
    }
    let name = input.name.as_deref().unwrap_or("input");
    if !is_file_name(name) {
        return Err(ApiError::argument(format!(
            "input.name must be a file name of at most {NAME_MAX} characters: letters, digits, spaces, dots, dashes and underscores, starting with a letter or a digit, and no path."
        )));
    }
    let stem = Path::new(name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("input")
        .to_owned();
    let path = work.join(format!("input.{}", input.format));
    payload::decode_into(&input.data, &path, keep_going)?;
    Ok(Received { path, stem })
}

/// Whether `name` matches `NAME_PATTERN`, is at most `NAME_MAX` long and
/// holds no `..`.
pub fn is_file_name(name: &str) -> bool {
    let mut characters = name.chars();
    name.chars().count() <= NAME_MAX
        && characters
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        && characters
            .all(|character| character.is_ascii_alphanumeric() || " ._-".contains(character))
        && !name.contains("..")
}

/// Where a call writes its result: `<work>/out/<stem>.<extension>`.
fn destination(work: &Path, stem: &str, extension: &str) -> Result<PathBuf, ApiError> {
    let folder = work.join("out");
    fs::create_dir_all(&folder)
        .map_err(|_| ApiError::internal("The work folder could not be written."))?;
    Ok(folder.join(format!("{stem}.{extension}")))
}

/// The result file, checked against the output limit after the work.
fn result_file(
    path: PathBuf,
    format: &str,
    stem: &str,
    limits: &CallLimits,
) -> Result<OutputFile, ApiError> {
    let bytes = fs::metadata(&path)
        .map_err(|_| ApiError::internal("The result could not be read back."))?
        .len();
    if bytes > limits.max_output {
        return Err(ApiError::output_too_large(limits.max_output));
    }
    Ok(OutputFile {
        format: format.to_owned(),
        media_type: payload::media_type(format),
        file_name: format!("{stem}.{format}"),
        path,
    })
}

fn mesh_report(stats: MeshStats) -> Value {
    json!({
        "source_points": stats.source_points,
        "vertices": stats.vertices,
        "triangles": stats.triangles,
    })
}

/// A failure of the core as the service answers it.
fn failed(error: LoadError) -> ApiError {
    ApiError::tool_failed(&error, None)
}

/// `name` with `arguments` as a JSON value, for callers that hold one: the
/// arguments are written out once and read as `call_blocking` reads them.
pub fn call_value(
    name: &str,
    arguments: &Value,
    limits: &CallLimits,
    work: &TempDir,
    keep_going: KeepGoing<'_>,
) -> Result<ToolOutput, ApiError> {
    let raw = serde_json::value::to_raw_value(arguments)
        .map_err(|error| ApiError::argument(error.to_string()))?;
    call_blocking(name, &raw, limits, work, keep_going)
}

/// Parse the typed arguments (borrowing `data`), decode the input into a fresh
/// work folder, run the `ops` function, and leave the result in the folder.
/// The caller encodes the answer and drops the folder (TempDir).
pub fn call_blocking(
    name: &str,
    arguments: &RawValue,
    limits: &CallLimits,
    work: &TempDir,
    keep_going: KeepGoing<'_>,
) -> Result<ToolOutput, ApiError> {
    let folder = work.path();
    match name {
        "inspect" => {
            let arguments: InspectArguments<'_> = parse(arguments)?;
            let input = receive(&arguments.input, folder, keep_going)?;
            drop(arguments);
            let cloud = ops::open_source(&input.path, keep_going).map_err(failed)?;
            let report = serde_json::to_value(ops::inspect(&cloud))
                .map_err(|_| ApiError::internal("The report could not be written."))?;
            Ok(ToolOutput { report, file: None })
        }
        "convert" => {
            let arguments: ConvertArguments<'_> = parse(arguments)?;
            let key = arguments.output_format.as_str();
            let format = ops::EXPORT_FORMATS
                .contains(&key)
                .then(|| ops::export_format_for_key(key))
                .flatten()
                .ok_or_else(|| {
                    ApiError::argument(format!(
                        "output_format must be one of {}.",
                        ops::EXPORT_FORMATS.join(", ")
                    ))
                })?;
            let key = key.to_owned();
            let section = arguments.section.map(BoxArgument::oriented).transpose()?;
            let input = receive(&arguments.input, folder, keep_going)?;
            drop(arguments);
            let cloud = ops::open_source(&input.path, keep_going).map_err(failed)?;
            let per_point = ops::bytes_per_point(format, &cloud).max(1);
            let convert = Convert {
                format,
                section,
                max_points: Some(limits.max_output / per_point),
            };
            let path = destination(folder, &input.stem, &key)?;
            let points = ops::convert(&cloud, &path, convert, keep_going).map_err(|error| {
                if ops::is_too_many_points(&error) {
                    ApiError::output_too_large(limits.max_output)
                } else {
                    failed(error)
                }
            })?;
            let file = result_file(path, &key, &input.stem, limits)?;
            Ok(ToolOutput {
                report: json!({"points": points}),
                file: Some(file),
            })
        }
        "section_drawing" => {
            let arguments: SectionDrawingArguments<'_> = parse(arguments)?;
            let section = arguments.section.oriented()?;
            let view = match arguments.view.as_deref() {
                None => DrawingView::default(),
                Some(key) => DrawingView::from_key(key).ok_or_else(|| {
                    ApiError::argument("view must be plan, front, back, left or right.")
                })?,
            };
            let units = arguments
                .units
                .as_deref()
                .map(|key| {
                    DrawingUnits::from_key(key)
                        .ok_or_else(|| ApiError::argument("units must be mm or m."))
                })
                .transpose()?;
            if let Some(thickness) = arguments.thickness {
                if !(MIN_SLAB_THICKNESS..=MAX_SLAB_THICKNESS).contains(&thickness) {
                    return Err(ApiError::argument(format!(
                        "thickness must be a number of metres from {MIN_SLAB_THICKNESS} to {MAX_SLAB_THICKNESS}."
                    )));
                }
            }
            let format = match arguments.output_format.as_deref() {
                None => DrawingFormat::Dxf,
                Some(key) => DrawingFormat::ALL
                    .into_iter()
                    .find(|format| format.extension() == key)
                    .ok_or_else(|| ApiError::argument("output_format must be dxf or dwg."))?,
            };
            let options = SectionDrawingOptions {
                view,
                thickness: arguments.thickness,
                units,
                fill: arguments.fill,
            };
            let input = receive(&arguments.input, folder, keep_going)?;
            drop(arguments);
            let path = destination(folder, &input.stem, format.extension())?;
            let drawn = ops::section_drawing_file(
                &input.path,
                &input.stem,
                &path,
                section,
                &options,
                CachedIndex::Never,
                keep_going,
            )
            .map_err(|error| ApiError::tool_failed(&error, Some(view)))?;
            let stats = drawn.stats;
            let file = result_file(path, format.extension(), &input.stem, limits)?;
            Ok(ToolOutput {
                report: json!({
                    "view": drawn.request.view.key(),
                    "slab_thickness": drawn.slab_thickness,
                    "slab_points": stats.slab_points,
                    "read_points": stats.read_points,
                    "drawn_points": stats.drawn_points,
                    "point_spacing": stats.point_spacing,
                    "regions": stats.regions,
                    "vertices": stats.vertices,
                    "dropped_regions": stats.dropped_regions,
                    "grid_cell": stats.grid_cell,
                    "direction_degrees": stats.direction_degrees,
                }),
                file: Some(file),
            })
        }
        "terrain_mesh" => {
            let arguments: MeshArguments<'_> = parse(arguments)?;
            let input = receive(&arguments.input, folder, keep_going)?;
            drop(arguments);
            let path = destination(folder, &input.stem, "obj")?;
            let stats = ops::terrain_mesh_file(&input.path, &path, keep_going).map_err(failed)?;
            let file = result_file(path, "obj", &input.stem, limits)?;
            Ok(ToolOutput {
                report: mesh_report(stats),
                file: Some(file),
            })
        }
        "surface_mesh" => {
            let arguments: SurfaceMeshArguments<'_> = parse(arguments)?;
            let mut config = SurfaceMeshConfig::default();
            if let Some(vertices) = arguments.max_vertices {
                config.max_vertices = usize::try_from(vertices)
                    .ok()
                    .filter(|vertices| SURFACE_MESH_VERTICES.contains(vertices))
                    .ok_or_else(|| {
                        ApiError::argument(format!(
                            "max_vertices must be a whole number from {} to {}.",
                            SURFACE_MESH_VERTICES.start(),
                            SURFACE_MESH_VERTICES.end()
                        ))
                    })?;
            }
            if let Some(neighbors) = arguments.neighbors {
                config.neighbors = usize::try_from(neighbors)
                    .ok()
                    .filter(|neighbors| SURFACE_MESH_NEIGHBORS.contains(neighbors))
                    .ok_or_else(|| {
                        ApiError::argument(format!(
                            "neighbors must be a whole number from {} to {}.",
                            SURFACE_MESH_NEIGHBORS.start(),
                            SURFACE_MESH_NEIGHBORS.end()
                        ))
                    })?;
            }
            if let Some(factor) = arguments.edge_factor {
                if !(factor.is_finite() && factor > 0.0) {
                    return Err(ApiError::argument("edge_factor must be a number above 0."));
                }
                config.max_edge_factor = factor;
            }
            if let Some(size) = arguments.mesh_size {
                if !(size.is_finite() && size >= 0.0) {
                    return Err(ApiError::argument("mesh_size must be a number from 0."));
                }
                config.mesh_size = size;
            }
            let input = receive(&arguments.input, folder, keep_going)?;
            drop(arguments);
            let path = destination(folder, &input.stem, "obj")?;
            let stats =
                ops::surface_mesh_file(&input.path, &path, config, keep_going).map_err(failed)?;
            let file = result_file(path, "obj", &input.stem, limits)?;
            Ok(ToolOutput {
                report: mesh_report(stats),
                file: Some(file),
            })
        }
        _ => Err(ApiError::unknown_tool(name)),
    }
}

/// Whether the arguments of `tool` deserialise into its typed arguments;
/// for the tests of the examples.
#[cfg(test)]
pub(crate) fn arguments_fit(tool: &str, arguments: &RawValue) -> Result<(), ApiError> {
    match tool {
        "inspect" => parse::<InspectArguments<'_>>(arguments).map(drop),
        "convert" => parse::<ConvertArguments<'_>>(arguments).map(drop),
        "section_drawing" => parse::<SectionDrawingArguments<'_>>(arguments).map(drop),
        "terrain_mesh" => parse::<MeshArguments<'_>>(arguments).map(drop),
        "surface_mesh" => parse::<SurfaceMeshArguments<'_>>(arguments).map(drop),
        _ => Err(ApiError::unknown_tool(tool)),
    }
}
