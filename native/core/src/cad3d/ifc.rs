//! A [`Model3d`] as IFC4, written as a STEP physical file.
//!
//! The file holds a project with one site, building and storey, with every
//! object as an `IfcBuildingElementProxy` in the storey. The proxy is
//! honest about what is known: a flat face is a plane of a direction, not a
//! wall or a slab, and a cylinder may be a column or a pipe. The object
//! type names the kind (`Plane (wall)`, `Cylinder`, `Mesh`), and a property
//! set `OPS_ScanGeometry` holds the measured values.
//!
//! Geometry:
//! - a flat face: an `IfcPolygonalFaceSet` with a face per connected part,
//!   its openings as inner loops;
//! - a cylinder seen from outside: an `IfcExtrudedAreaSolid` of an
//!   `IfcCircleProfileDef` along its axis over its scanned length, which a
//!   BIM program can edit as a profile and a depth, with the axis as a
//!   `Curve3D` representation beside it; a cylinder seen from inside, a
//!   shaft, has no solid to fill and is the triangulated scanned surface;
//! - a mesh: an `IfcTriangulatedFaceSet`, closed when every edge has two
//!   triangles.
//!
//! Coordinates: scans are often in national grid coordinates, hundreds of
//! kilometres from zero, where a program that works in single precision
//! loses millimetres. On an axis whose coordinates lie further than
//! [`LOCAL_LIMIT`] from zero, the site is placed at the middle of the model
//! rounded to whole metres, and all geometry is relative to that point. The
//! placement of the site carries the offset, so the objects stand at their
//! scene coordinates; the description of the site names the point. The
//! coordinate system of the scene is not known, so the file holds no map
//! conversion.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use super::step::{guid, length, real, string, time_stamp};
use super::{Kind, Model3d, Object3d, PropertyValue};
use crate::LoadError;

/// The furthest coordinate from zero that is written as it is.
pub const LOCAL_LIMIT: f64 = 1_000.0;
/// Name of the property set of every object.
pub(crate) const PROPERTY_SET: &str = "OPS_ScanGeometry";

/// The point all geometry is relative to: zero on an axis whose coordinates
/// stay within `LOCAL_LIMIT` of zero, the rounded middle on the others.
pub(crate) fn local_origin(model: &Model3d) -> [f64; 3] {
    let Some([min, max]) = model.extents() else {
        return [0.0; 3];
    };
    std::array::from_fn(|axis| {
        if min[axis].abs().max(max[axis].abs()) <= LOCAL_LIMIT {
            0.0
        } else {
            ((min[axis] + max[axis]) / 2.0).round()
        }
    })
}

/// Write the model to `destination` and return the size of the file. The
/// file is written beside it and put in place when complete.
pub(crate) fn write_model_ifc(model: &Model3d, destination: &Path) -> Result<u64, LoadError> {
    super::cad::validate(model)?;
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    {
        let name = destination
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut writer = Step {
            out: BufWriter::new(temporary.as_file_mut()),
            next: 1,
        };
        write_file(model, &name, &mut writer)?;
        writer.out.flush()?;
    }
    temporary.as_file_mut().sync_all()?;
    let bytes = temporary.as_file().metadata()?.len();
    temporary
        .persist(destination)
        .map_err(|error| LoadError::Io(error.error))?;
    Ok(bytes)
}

/// Writes instances with numbers in order.
struct Step<'a> {
    out: BufWriter<&'a mut File>,
    next: u64,
}

impl Step<'_> {
    fn add(&mut self, name: &str, attributes: &str) -> std::io::Result<u64> {
        let id = self.begin(name)?;
        self.out.write_all(attributes.as_bytes())?;
        self.end()?;
        Ok(id)
    }

    /// Start an instance whose attributes the caller writes to `out`.
    fn begin(&mut self, name: &str) -> std::io::Result<u64> {
        let id = self.next;
        self.next += 1;
        write!(self.out, "#{id}={name}(")?;
        Ok(id)
    }

    fn end(&mut self) -> std::io::Result<()> {
        self.out.write_all(b");\n")
    }

    fn point(&mut self, position: [f64; 3]) -> std::io::Result<u64> {
        let [x, y, z] = position.map(length);
        self.add("IFCCARTESIANPOINT", &format!("({x},{y},{z})"))
    }

    fn direction(&mut self, direction: [f64; 3]) -> std::io::Result<u64> {
        let [x, y, z] = direction.map(real);
        self.add("IFCDIRECTION", &format!("({x},{y},{z})"))
    }

    fn placement(&mut self, relative_to: Option<u64>, axes: u64) -> std::io::Result<u64> {
        let relative_to = relative_to.map_or("$".to_owned(), |id| format!("#{id}"));
        self.add("IFCLOCALPLACEMENT", &format!("{relative_to},#{axes}"))
    }
}

fn reference_list(ids: &[u64]) -> String {
    let inner: Vec<String> = ids.iter().map(|id| format!("#{id}")).collect();
    format!("({})", inner.join(","))
}

fn write_file(model: &Model3d, file_name: &str, step: &mut Step) -> Result<(), LoadError> {
    let application = string(&format!(
        "Open Pointcloud Studio {}",
        env!("CARGO_PKG_VERSION")
    ));
    write!(
        step.out,
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(({}),'2;1');\nFILE_NAME({},{},(''),(''),{application},{application},'');\nFILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n",
        string("Geometry detected in a point cloud"),
        string(file_name),
        string(&time_stamp()),
    )?;

    // Units and the representation contexts.
    let units = [
        ".LENGTHUNIT.,$,.METRE.",
        ".AREAUNIT.,$,.SQUARE_METRE.",
        ".VOLUMEUNIT.,$,.CUBIC_METRE.",
        ".PLANEANGLEUNIT.,$,.RADIAN.",
    ]
    .iter()
    .map(|unit| step.add("IFCSIUNIT", &format!("*,{unit}")))
    .collect::<std::io::Result<Vec<u64>>>()?;
    let unit_assignment = step.add("IFCUNITASSIGNMENT", &reference_list(&units))?;
    let zero = step.point([0.0; 3])?;
    let identity = step.add("IFCAXIS2PLACEMENT3D", &format!("#{zero},$,$"))?;
    let context = step.add(
        "IFCGEOMETRICREPRESENTATIONCONTEXT",
        &format!("$,'Model',3,1.E-05,#{identity},$"),
    )?;
    let body_context = step.add(
        "IFCGEOMETRICREPRESENTATIONSUBCONTEXT",
        &format!("'Body','Model',*,*,*,*,#{context},$,.MODEL_VIEW.,$"),
    )?;
    let axis_context = step.add(
        "IFCGEOMETRICREPRESENTATIONSUBCONTEXT",
        &format!("'Axis','Model',*,*,*,*,#{context},$,.GRAPH_VIEW.,$"),
    )?;

    // The spatial structure, with the site at the local origin.
    let mut description = model.notes.clone();
    if !model.source.is_empty() {
        description.insert(0, format!("Source: {}", model.source));
    }
    let project_description = if description.is_empty() {
        "$".to_owned()
    } else {
        string(&description.join("; "))
    };
    let project = step.add(
        "IFCPROJECT",
        &format!(
            "{},$,{},{project_description},$,$,$,(#{context}),#{unit_assignment}",
            string(&guid()),
            string(if model.source.is_empty() {
                "Point cloud"
            } else {
                &model.source
            }),
        ),
    )?;
    let origin = local_origin(model);
    let site_point = step.point(origin)?;
    let site_axes = step.add("IFCAXIS2PLACEMENT3D", &format!("#{site_point},$,$"))?;
    let site_placement = step.placement(None, site_axes)?;
    let site_description = string(&format!(
        "Geometry relative to scene coordinates ({}, {}, {}) m",
        length(origin[0]),
        length(origin[1]),
        length(origin[2])
    ));
    let site = step.add(
        "IFCSITE",
        &format!(
            "{},$,'Site',{site_description},$,#{site_placement},$,$,.ELEMENT.,$,$,$,$,$",
            string(&guid())
        ),
    )?;
    let building_placement = step.placement(Some(site_placement), identity)?;
    let building = step.add(
        "IFCBUILDING",
        &format!(
            "{},$,'Building',$,$,#{building_placement},$,$,.ELEMENT.,$,$,$",
            string(&guid())
        ),
    )?;
    let storey_placement = step.placement(Some(building_placement), identity)?;
    let storey = step.add(
        "IFCBUILDINGSTOREY",
        &format!(
            "{},$,'Scan',$,$,#{storey_placement},$,$,.ELEMENT.,0.",
            string(&guid())
        ),
    )?;
    for (whole, part) in [(project, site), (site, building), (building, storey)] {
        step.add(
            "IFCRELAGGREGATES",
            &format!("{},$,$,$,#{whole},(#{part})", string(&guid())),
        )?;
    }

    let contexts = Contexts {
        body: body_context,
        axis: axis_context,
        placement: storey_placement,
        identity,
    };
    let mut elements = Vec::with_capacity(model.objects.len());
    for object in &model.objects {
        elements.push(write_object(object, origin, &contexts, step)?);
    }
    step.add(
        "IFCRELCONTAINEDINSPATIALSTRUCTURE",
        &format!(
            "{},$,$,$,{},#{storey}",
            string(&guid()),
            reference_list(&elements)
        ),
    )?;
    step.out.write_all(b"ENDSEC;\nEND-ISO-10303-21;\n")?;
    Ok(())
}

struct Contexts {
    body: u64,
    axis: u64,
    /// The placement of the storey, which the objects are placed in.
    placement: u64,
    identity: u64,
}

fn write_object(
    object: &Object3d,
    origin: [f64; 3],
    contexts: &Contexts,
    step: &mut Step,
) -> Result<u64, LoadError> {
    let local = |position: [f64; 3]| -> [f64; 3] {
        std::array::from_fn(|axis| position[axis] - origin[axis])
    };
    let mut representations = Vec::new();
    let solid = object
        .cylinder
        .filter(|shape| object.kind == Kind::Cylinder && !shape.seen_from_inside);
    let (item, representation_type) = if let Some(shape) = solid {
        let profile = step.add(
            "IFCCIRCLEPROFILEDEF",
            &format!(".AREA.,$,$,{}", length(shape.radius)),
        )?;
        let axis = crate::local_fit::unit(crate::local_fit::difference(shape.end, shape.start))
            .unwrap_or([0.0, 0.0, 1.0]);
        let location = step.point(local(shape.start))?;
        let axis_direction = step.direction(axis)?;
        let across = step.direction(shape.across)?;
        let position = step.add(
            "IFCAXIS2PLACEMENT3D",
            &format!("#{location},#{axis_direction},#{across}"),
        )?;
        let up = step.direction([0.0, 0.0, 1.0])?;
        let between = crate::local_fit::difference(shape.end, shape.start);
        let depth = crate::local_fit::dot(between, between).sqrt();
        let solid = step.add(
            "IFCEXTRUDEDAREASOLID",
            &format!("#{profile},#{position},#{up},{}", length(depth)),
        )?;
        (solid, "SweptSolid")
    } else {
        // The corners as one list, relative to the local origin.
        let list = step.begin("IFCCARTESIANPOINTLIST3D")?;
        step.out.write_all(b"(")?;
        for (index, position) in object.vertices.iter().enumerate() {
            let [x, y, z] = local(*position).map(length);
            if index > 0 {
                step.out.write_all(b",")?;
            }
            write!(step.out, "({x},{y},{z})")?;
        }
        step.out.write_all(b")")?;
        step.end()?;
        if object.polygons.is_empty() {
            let closed = match object.closed {
                Some(true) => ".T.",
                Some(false) => ".F.",
                None => "$",
            };
            let set = step.begin("IFCTRIANGULATEDFACESET")?;
            write!(step.out, "#{list},$,{closed},(")?;
            for (index, [a, b, c]) in object.triangles.iter().enumerate() {
                if index > 0 {
                    step.out.write_all(b",")?;
                }
                write!(step.out, "({},{},{})", a + 1, b + 1, c + 1)?;
            }
            step.out.write_all(b"),$")?;
            step.end()?;
            (set, "Tessellation")
        } else {
            let ring = |ring: &[u32]| -> String {
                let indices: Vec<String> =
                    ring.iter().map(|index| (index + 1).to_string()).collect();
                format!("({})", indices.join(","))
            };
            let mut faces = Vec::new();
            for polygon in object
                .polygons
                .iter()
                .filter(|polygon| polygon.outer.len() >= 3)
            {
                let holes: Vec<String> = polygon
                    .holes
                    .iter()
                    .filter(|hole| hole.len() >= 3)
                    .map(|hole| ring(hole))
                    .collect();
                faces.push(if holes.is_empty() {
                    step.add("IFCINDEXEDPOLYGONALFACE", &ring(&polygon.outer))?
                } else {
                    step.add(
                        "IFCINDEXEDPOLYGONALFACEWITHVOIDS",
                        &format!("{},({})", ring(&polygon.outer), holes.join(",")),
                    )?
                });
            }
            let set = step.add(
                "IFCPOLYGONALFACESET",
                &format!("#{list},.F.,{},$", reference_list(&faces)),
            )?;
            (set, "Tessellation")
        }
    };
    // The colour of the object.
    let [r, g, b] = object.rgb.map(|value| real(f64::from(value) / 255.0));
    let colour = step.add("IFCCOLOURRGB", &format!("$,{r},{g},{b}"))?;
    let shading = step.add("IFCSURFACESTYLESHADING", &format!("#{colour},0."))?;
    let style = step.add(
        "IFCSURFACESTYLE",
        &format!("{},.BOTH.,(#{shading})", string(&object.object_type())),
    )?;
    step.add("IFCSTYLEDITEM", &format!("#{item},(#{style}),$"))?;

    if let Some(shape) = object.cylinder {
        let start = step.point(local(shape.start))?;
        let end = step.point(local(shape.end))?;
        let line = step.add("IFCPOLYLINE", &format!("(#{start},#{end})"))?;
        representations.push(step.add(
            "IFCSHAPEREPRESENTATION",
            &format!("#{},'Axis','Curve3D',(#{line})", contexts.axis),
        )?);
    }
    representations.push(step.add(
        "IFCSHAPEREPRESENTATION",
        &format!(
            "#{},'Body','{representation_type}',(#{item})",
            contexts.body
        ),
    )?);
    let shape = step.add(
        "IFCPRODUCTDEFINITIONSHAPE",
        &format!("$,$,{}", reference_list(&representations)),
    )?;
    let placement = step.placement(Some(contexts.placement), contexts.identity)?;
    let element = step.add(
        "IFCBUILDINGELEMENTPROXY",
        &format!(
            "{},$,{},$,{},#{placement},#{shape},{},.USERDEFINED.",
            string(&guid()),
            string(&object.name),
            string(&object.object_type()),
            string(&object.id.to_string()),
        ),
    )?;

    // The measured values.
    let mut properties = Vec::with_capacity(object.properties.len());
    for (name, value) in &object.properties {
        let value = match value {
            PropertyValue::Label(text) => format!("IFCLABEL({})", string(text)),
            PropertyValue::Length(value) => format!("IFCLENGTHMEASURE({})", real(*value)),
            PropertyValue::Area(value) => format!("IFCAREAMEASURE({})", real(*value)),
            PropertyValue::Real(value) => format!("IFCREAL({})", real(*value)),
            PropertyValue::Count(value) => format!("IFCINTEGER({value})"),
            PropertyValue::Bool(value) => {
                format!("IFCBOOLEAN({})", if *value { ".T." } else { ".F." })
            }
        };
        properties.push(step.add(
            "IFCPROPERTYSINGLEVALUE",
            &format!("{},$,{value},$", string(name)),
        )?);
    }
    if !properties.is_empty() {
        let set = step.add(
            "IFCPROPERTYSET",
            &format!(
                "{},$,{},$,{}",
                string(&guid()),
                string(PROPERTY_SET),
                reference_list(&properties)
            ),
        )?;
        step.add(
            "IFCRELDEFINESBYPROPERTIES",
            &format!("{},$,$,$,(#{element}),#{set}", string(&guid())),
        )?;
    }
    Ok(element)
}
