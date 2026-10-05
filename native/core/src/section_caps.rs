//! Solid caps where a section box cuts a mesh.
//!
//! A wall, floor or ceiling that was meshed from both sides is two surfaces
//! with material between them. Cut by a box, the view looks into the gap
//! between those surfaces. The caps close that gap on the faces of the box,
//! as the cut material of a section drawing is filled.
//!
//! On every face of the box the mesh is cut into segments. Each segment
//! knows the side its triangle faces, the side of the air: the front of a
//! triangle looks away from the material. Along lines in eight directions
//! over the face, a stretch between two segments that lie opposite each
//! other no farther apart than the largest thickness may be material. It is
//! when the line passes into the material through the back of the first one
//! and out through the front of the second; when the fronts of the two look
//! at each other it is the air between two walls. A single surface, such as
//! a facade seen from one side only, has no second surface to pair with and
//! gets no cap.
//!
//! A mesh made without stations faces one point, so one surface of most
//! walls faces the wrong way. When most pairs on a face of the box disagree
//! like that, the fronts are not trusted: air and material take turns at
//! every surface along a line, which begins and ends in the open, and a
//! pair is material where those turns fit the stretches beside it. A
//! surface that stands on its own breaks the turns and is left open.
//!
//! The stretches are gathered on a fine grid over the face, the small gaps
//! where walls meet are closed, and the grid is written as rectangles.

use std::ops::RangeInclusive;

use rayon::prelude::*;

use crate::{MeshGeometry, OrientedBox, DEFAULT_MAX_WALL_THICKNESS, MAX_WALL_THICKNESS};

/// The largest thickness of material that is capped when nothing else is
/// asked: that of a wall in a section drawing.
pub const DEFAULT_CAP_MAX_THICKNESS: f64 = DEFAULT_MAX_WALL_THICKNESS;
/// Two surfaces farther apart than this are never taken as one piece of
/// material.
pub const MAX_CAP_MAX_THICKNESS: f64 = MAX_WALL_THICKNESS;
/// The smallest thickness a request may ask for.
pub const MIN_CAP_MAX_THICKNESS: f64 = 0.01;

/// Directions of the lines that look for material, spread over half a turn.
const DIRECTIONS: usize = 8;
/// The grid over a face has at most this many cells along its longer side.
const MAX_CELLS_PER_SIDE: f64 = 2048.0;
/// Cells are never smaller than this, in scene units.
const MIN_CELL: f64 = 0.005;
/// A line counts a surface only when it crosses it at least this steeply:
/// the cosine between the line and the normal of the surface.
const MIN_FACING: f64 = 0.5;
/// A triangle is cut only when it stands at least this steeply on the face:
/// the sine of the angle between them, here 10 degrees.
const MIN_STEEPNESS: f64 = 0.17;
/// Two surfaces enclose material only when their normals point at least
/// this much against each other.
const OPPOSITE: f64 = -0.5;
/// The mesh is cut and the caps lie this far inside the box, so that the
/// clip of the box keeps them, and a mesh that was made in the same box and
/// ends in its faces is still cut; never more than a thousandth of the size
/// of the box.
const INSET: f64 = 0.0005;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CapOptions {
    /// Two opposite surfaces farther apart than this enclose no cap.
    pub max_thickness: f64,
}

impl Default for CapOptions {
    fn default() -> Self {
        Self {
            max_thickness: DEFAULT_CAP_MAX_THICKNESS,
        }
    }
}

/// One face of the box: the axis it stands across in the frame of the box,
/// whether it is the face at the top of that axis, and where the mesh is
/// cut, just inside the face.
#[derive(Debug, Clone, Copy)]
struct Face {
    axis: usize,
    upper: bool,
    level: f64,
    /// The other two axes, in an order that makes `u` × `v` point along
    /// `axis`.
    u: usize,
    v: usize,
    /// The extent of the face along `u` and `v`.
    min: [f64; 2],
    max: [f64; 2],
}

/// Where a triangle crosses a face, with the unit normal in the face of
/// the side the triangle faces.
#[derive(Debug, Clone, Copy)]
struct Segment {
    a: [f64; 2],
    b: [f64; 2],
    air: [f64; 2],
}

/// Where a line over a face crosses a segment.
#[derive(Debug, Clone, Copy)]
struct Hit {
    /// How far along the line.
    at: f64,
    /// The side of the air along the line: the cosine between the line and
    /// the side the triangle faces.
    facing: f64,
    air: [f64; 2],
}

/// What a line holds between two surfaces it crosses one after the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stretch {
    /// Air, or material too thick to be a wall.
    Open,
    /// The air between two surfaces close together whose fronts look at
    /// each other, as between two walls.
    Gap,
    /// The material of a wall, floor or ceiling: capped.
    Material,
    /// Two surfaces that lie close together and opposite each other, but
    /// that face the same way, so that one of them faces the wrong side, as
    /// in a mesh made without stations. The stretches beside it tell.
    Unknown,
    /// Two surfaces close together that cross the line too slantingly or
    /// stand at an angle to each other: nothing can be said of it, and it
    /// tells nothing about the stretches beside it.
    Unsure,
}

impl Stretch {
    fn between(from: &Hit, to: &Hit, max_thickness: f64) -> Self {
        let length = to.at - from.at;
        // Through the back of the first surface into the material, out
        // through the front of the second.
        let (into, out) = (-from.facing, to.facing);
        let turn = dot(from.air, to.air);
        let steep = into.abs() >= MIN_FACING && out.abs() >= MIN_FACING;
        if !steep || turn.abs() < -OPPOSITE {
            return if length * MIN_FACING > max_thickness {
                Self::Open
            } else {
                Self::Unsure
            };
        }
        if length * into.abs().max(out.abs()) > max_thickness {
            Self::Open
        } else if turn <= OPPOSITE && into > 0.0 && out > 0.0 {
            Self::Material
        } else if turn <= OPPOSITE && into < 0.0 && out < 0.0 {
            Self::Gap
        } else {
            Self::Unknown
        }
    }
}

/// What a line holds between every two surfaces it crosses one after the
/// other.
fn stretches_of(hits: &[Hit], max_thickness: f64, stretches: &mut Vec<Stretch>) {
    stretches.clear();
    stretches.extend(
        hits.windows(2)
            .map(|pair| Stretch::between(&pair[0], &pair[1], max_thickness)),
    );
}

/// The groups of surfaces close together along a line, between open
/// stretches: the hits that bound each run of stretches that are not open.
fn groups(stretches: &[Stretch]) -> impl Iterator<Item = RangeInclusive<usize>> + '_ {
    let mut index = 0;
    std::iter::from_fn(move || {
        while stretches.get(index) == Some(&Stretch::Open) {
            index += 1;
        }
        if index >= stretches.len() {
            return None;
        }
        let first = index;
        while stretches
            .get(index)
            .is_some_and(|stretch| *stretch != Stretch::Open)
        {
            index += 1;
        }
        Some(first..=index)
    })
}

/// Whether a group of surfaces has one between its first and its last that
/// faces against both, which face the same way. A wall thinner than a few
/// voxels comes out so in a mesh made without stations: its two surfaces
/// face one point, and a third one forms inside it.
fn split_inside(group: &[Hit]) -> bool {
    let side = |hit: &Hit| hit.facing > 0.0;
    match group {
        [first, middle @ .., last] if !middle.is_empty() => {
            side(first) == side(last) && middle.iter().any(|hit| side(hit) != side(first))
        }
        _ => false,
    }
}

/// The hits of a line without the surfaces that formed inside thin walls:
/// such a wall lies between the two surfaces around the one inside it.
fn leave_out_inner_surfaces(hits: &[Hit], stretches: &[Stretch], kept: &mut Vec<Hit>) {
    kept.clear();
    let mut next = 0;
    for group in groups(stretches) {
        kept.extend_from_slice(&hits[next..*group.start()]);
        let members = &hits[group.clone()];
        if split_inside(members) {
            let side = members[0].facing > 0.0;
            kept.extend(members.iter().filter(|hit| (hit.facing > 0.0) == side));
        } else {
            kept.extend_from_slice(members);
        }
        next = group.end() + 1;
    }
    kept.extend_from_slice(&hits[next..]);
}

/// Decide the unknown stretches of a line from those beside them, where the
/// fronts of the mesh do not tell the side of the air. Along a line air and
/// material take turns at every surface of a wall that was meshed from both
/// sides, and the line begins and ends in the open. A run
/// of unknown stretches between two known ones, or the ends of the line, is
/// settled when the turns fit at both of its ends. Where they do not, one
/// of the surfaces stands on its own, such as a facade seen from one side in
/// front of a wall, and the run is left open, as it is beside a stretch that
/// tells nothing.
fn settle(stretches: &mut [Stretch]) {
    let mut index = 0;
    while index < stretches.len() {
        if stretches[index] != Stretch::Unknown {
            index += 1;
            continue;
        }
        let first = index;
        while index < stretches.len() && stretches[index] == Stretch::Unknown {
            index += 1;
        }
        let before = first
            .checked_sub(1)
            .map_or(Stretch::Open, |before| stretches[before]);
        let after = stretches.get(index).copied().unwrap_or(Stretch::Open);
        let air = |stretch: Stretch| matches!(stretch, Stretch::Open | Stretch::Gap);
        let run = &mut stretches[first..index];
        // The first of the run turns from what lies before it, and the last
        // must turn into what lies after it.
        let first_material = air(before);
        let last_material = first_material == (run.len() % 2 == 1);
        let fits =
            before != Stretch::Unsure && after != Stretch::Unsure && last_material == air(after);
        for (offset, stretch) in run.iter_mut().enumerate() {
            *stretch = if fits && first_material == (offset % 2 == 0) {
                Stretch::Material
            } else {
                Stretch::Open
            };
        }
    }
}

/// The caps of one mesh on the faces of a section box, in scene
/// coordinates, their fronts looking out of the box. Empty when nothing is
/// capped.
///
/// `place` gives the scene position of a vertex of the mesh.
pub fn section_caps(
    mesh: &MeshGeometry,
    place: impl Fn([f64; 3]) -> [f64; 3] + Sync,
    section: &OrientedBox,
    options: CapOptions,
) -> MeshGeometry {
    let mut caps = MeshGeometry::default();
    if !section.is_valid()
        || !options.max_thickness.is_finite()
        || options.max_thickness <= 0.0
        || mesh.triangles.is_empty()
    {
        return caps;
    }
    let max_thickness = options.max_thickness.min(MAX_CAP_MAX_THICKNESS);
    let faces = box_faces(section);
    if faces.is_empty() {
        return caps;
    }
    let segments = cut_segments(mesh, &place, section, &faces, max_thickness);
    let parts: Vec<MeshGeometry> = faces
        .par_iter()
        .zip(segments.par_iter())
        .map(|(face, segments)| {
            let mut part = MeshGeometry::default();
            if let Some(grid) = Grid::over(face, segments, max_thickness) {
                let mask = grid.material(segments, max_thickness);
                write_rectangles(&mut part, face, &grid, &mask, section);
            }
            part
        })
        .collect();
    for part in parts {
        let Ok(base) = u32::try_from(caps.vertices.len()) else {
            break;
        };
        caps.vertices.extend(part.vertices);
        caps.triangles.extend(
            part.triangles
                .into_iter()
                .map(|triangle| triangle.map(|index| index + base)),
        );
    }
    caps
}

fn box_faces(section: &OrientedBox) -> Vec<Face> {
    let (min, max) = (section.bounds.min, section.bounds.max);
    let mut faces = Vec::with_capacity(6);
    for axis in 0..3 {
        let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
        // A box without depth along an axis shows nothing on those faces,
        // and one without width has no face.
        if max[axis] - min[axis] <= 4.0 * INSET || max[u] <= min[u] || max[v] <= min[v] {
            continue;
        }
        let inset = INSET.min((max[axis] - min[axis]) * 0.001);
        for (upper, level) in [(false, min[axis] + inset), (true, max[axis] - inset)] {
            faces.push(Face {
                axis,
                upper,
                level,
                u,
                v,
                min: [min[u], min[v]],
                max: [max[u], max[v]],
            });
        }
    }
    faces
}

/// The segments of every face, keeping those that lie over the face or
/// within the largest thickness of it.
fn cut_segments(
    mesh: &MeshGeometry,
    place: &(impl Fn([f64; 3]) -> [f64; 3] + Sync),
    section: &OrientedBox,
    faces: &[Face],
    reach: f64,
) -> Vec<Vec<Segment>> {
    let count = mesh.vertices.len();
    mesh.triangles
        .par_iter()
        .fold(
            || vec![Vec::new(); faces.len()],
            |mut found: Vec<Vec<Segment>>, triangle| {
                if triangle.iter().any(|&index| index as usize >= count) {
                    return found;
                }
                let corners =
                    triangle.map(|index| section.to_box(place(mesh.vertices[index as usize])));
                if corners
                    .iter()
                    .any(|corner| corner.iter().any(|value| !value.is_finite()))
                {
                    return found;
                }
                let normal = cross(
                    difference(corners[1], corners[0]),
                    difference(corners[2], corners[0]),
                );
                for (face, found) in faces.iter().zip(found.iter_mut()) {
                    if let Some(segment) = cut(face, triangle, &corners, normal, reach) {
                        found.push(segment);
                    }
                }
                found
            },
        )
        .reduce(
            || vec![Vec::new(); faces.len()],
            |mut left, right| {
                for (left, right) in left.iter_mut().zip(right) {
                    left.extend(right);
                }
                left
            },
        )
}

/// Where one triangle crosses the plane of a face. A corner on the plane
/// counts as above it, so neighbouring triangles agree on what crosses.
fn cut(
    face: &Face,
    triangle: &[u32; 3],
    corners: &[[f64; 3]; 3],
    normal: [f64; 3],
    reach: f64,
) -> Option<Segment> {
    let distance = corners.map(|corner| corner[face.axis] - face.level);
    let below = distance.map(|value| value < 0.0);
    if below.iter().all(|&value| value) || below.iter().all(|&value| !value) {
        return None;
    }
    let along = [normal[face.u], normal[face.v]];
    let length = along[0].hypot(along[1]);
    let size = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
    // A triangle that lies almost in the face, as a floor does in a cut at
    // its height, says nothing about which side of its cut is air: the
    // little of its normal that lies in the face points anywhere.
    if length <= size * MIN_STEEPNESS {
        return None;
    }
    let air = [along[0] / length, along[1] / length];
    let mut points = [[0.0; 2]; 2];
    let mut found = 0;
    for (first, second) in [(0, 1), (1, 2), (2, 0)] {
        if below[first] == below[second] {
            continue;
        }
        // The same edge in the triangle next door is crossed at exactly
        // the same place: measured from its lower vertex.
        let (from, to) = if triangle[first] < triangle[second] {
            (first, second)
        } else {
            (second, first)
        };
        let share = distance[from] / (distance[from] - distance[to]);
        let at =
            |axis: usize| corners[from][axis] + (corners[to][axis] - corners[from][axis]) * share;
        if found < 2 {
            points[found] = [at(face.u), at(face.v)];
        }
        found += 1;
    }
    if found != 2 {
        return None;
    }
    let [a, b] = points;
    let outside = (0..2).any(|axis| {
        a[axis].max(b[axis]) < face.min[axis] - reach
            || a[axis].min(b[axis]) > face.max[axis] + reach
    });
    (!outside).then_some(Segment { a, b, air })
}

/// Parallel lines over a face in one of the directions: `count` lines from
/// `first` across them, `spacing` apart.
#[derive(Debug, Clone, Copy)]
struct Lines {
    step: usize,
    along: [f64; 2],
    across: [f64; 2],
    first: f64,
    spacing: f64,
    count: usize,
}

impl Lines {
    /// Along the rows or the columns of the grid.
    fn straight(&self) -> bool {
        self.step == 0 || 2 * self.step == DIRECTIONS
    }
}

/// A grid over the part of a face where segments lie, aligned to the
/// corner of the face.
#[derive(Debug, Clone, Copy)]
struct Grid {
    origin: [f64; 2],
    cell: f64,
    columns: usize,
    rows: usize,
}

impl Grid {
    fn over(face: &Face, segments: &[Segment], reach: f64) -> Option<Self> {
        let mut low = [f64::INFINITY; 2];
        let mut high = [f64::NEG_INFINITY; 2];
        for segment in segments {
            for point in [segment.a, segment.b] {
                for axis in 0..2 {
                    low[axis] = low[axis].min(point[axis]);
                    high[axis] = high[axis].max(point[axis]);
                }
            }
        }
        // Material may reach past the last segment by no more than the
        // largest thickness; the face bounds the rest.
        for axis in 0..2 {
            low[axis] = (low[axis] - reach).max(face.min[axis]);
            high[axis] = (high[axis] + reach).min(face.max[axis]);
            if low[axis] >= high[axis] {
                return None;
            }
        }
        let longest = (high[0] - low[0]).max(high[1] - low[1]);
        let cell = (longest / MAX_CELLS_PER_SIDE).max(MIN_CELL);
        let origin: [f64; 2] = std::array::from_fn(|axis| {
            face.min[axis] + ((low[axis] - face.min[axis]) / cell).floor() * cell
        });
        let columns = ((high[0] - origin[0]) / cell).ceil().max(1.0) as usize;
        let rows = ((high[1] - origin[1]) / cell).ceil().max(1.0) as usize;
        Some(Self {
            origin,
            cell,
            columns,
            rows,
        })
    }

    /// The cells whose centre lies in material, row after row.
    ///
    /// Where a wall meets another one, as in a T, a small part of the
    /// junction has no pair of opposite faces nearby in any direction. Such
    /// a gap is closed when it lies inside material along a row or a column
    /// and capped cells surround it within half the largest thickness; a
    /// thick block on its own has no capped cells to close between.
    fn material(&self, segments: &[Segment], max_thickness: f64) -> Vec<bool> {
        let mut mask = vec![false; self.columns * self.rows];
        let mut inside = vec![false; self.columns * self.rows];
        let trusted = self.fronts_look_at_the_air(segments, max_thickness);
        // What slanted lines find in a mesh whose fronts do not tell.
        let mut slanted = vec![false; if trusted { 0 } else { mask.len() }];
        let mut stretches: Vec<Stretch> = Vec::new();
        let mut kept: Vec<Hit> = Vec::new();
        for step in 0..DIRECTIONS {
            let lines = self.lines(step);
            self.each_line(segments, &lines, |level, hits| {
                stretches_of(hits, max_thickness, &mut stretches);
                let hits = if trusted {
                    for stretch in &mut stretches {
                        if *stretch == Stretch::Unknown {
                            *stretch = Stretch::Open;
                        }
                    }
                    hits
                } else {
                    leave_out_inner_surfaces(hits, &stretches, &mut kept);
                    stretches_of(&kept, max_thickness, &mut stretches);
                    settle(&mut stretches);
                    &kept
                };
                let found = if trusted || lines.straight() {
                    &mut mask
                } else {
                    &mut slanted
                };
                for (pair, stretch) in hits.windows(2).zip(&stretches) {
                    self.mark_stretch(
                        &lines,
                        level,
                        [&pair[0], &pair[1]],
                        *stretch,
                        found,
                        &mut inside,
                    );
                }
            });
        }
        if !trusted {
            // Where the turns are counted, a slanted line that passes the
            // open end of one of three sheets in a row miscounts: it fills
            // the corners where walls meet, but only next to what the rows
            // and the columns found.
            let near = self.dilate(&mask, (max_thickness / self.cell).ceil() as usize);
            for (cell, set) in mask.iter_mut().enumerate() {
                *set |= slanted[cell] && near[cell];
            }
        }
        let radius = (0.5 * max_thickness / self.cell).ceil() as usize;
        let grown = self.dilate(&mask, radius);
        let outside: Vec<bool> = grown.iter().map(|&set| !set).collect();
        // Cells beyond the grid count as grown: the closing does not shrink
        // at the edges of the face.
        let closed = self.dilate(&outside, radius);
        for (cell, set) in mask.iter_mut().enumerate() {
            *set |= inside[cell] && !closed[cell];
        }
        mask
    }

    /// Whether the fronts of the mesh look at the air, as those of a mesh
    /// made with stations do: the two surfaces of most walls then turn
    /// their backs to each other. In a mesh whose fronts all look at one
    /// point, as one made without stations does, most walls have one
    /// surface that faces the wrong way or a third surface inside, and
    /// their stretches are settled by the turns of air and material
    /// instead. Every group of surfaces close together along the rows and
    /// the columns counts once.
    fn fronts_look_at_the_air(&self, segments: &[Segment], max_thickness: f64) -> bool {
        let (mut agree, mut disagree) = (0usize, 0usize);
        let mut stretches = Vec::new();
        for step in [0, DIRECTIONS / 2] {
            self.each_line(segments, &self.lines(step), |_, hits| {
                stretches_of(hits, max_thickness, &mut stretches);
                for group in groups(&stretches) {
                    let run = &stretches[*group.start()..*group.end()];
                    // A surface on its own beside a wall disagrees with
                    // it, but the wall itself agrees.
                    if split_inside(&hits[group.clone()]) {
                        disagree += 1;
                    } else if run
                        .iter()
                        .any(|stretch| matches!(stretch, Stretch::Material | Stretch::Gap))
                    {
                        agree += 1;
                    } else if run.contains(&Stretch::Unknown) {
                        disagree += 1;
                    }
                }
            });
        }
        agree >= disagree
    }

    /// Mark one stretch of a line. Along rows and columns every stretch
    /// from the back of one face to the front of the next is also marked in
    /// `inside`, however far apart and however turned the faces are.
    fn mark_stretch(
        &self,
        lines: &Lines,
        level: f64,
        [from, to]: [&Hit; 2],
        stretch: Stretch,
        mask: &mut [bool],
        inside: &mut [bool],
    ) {
        let (start, end) = (from.at, to.at);
        // In through the back of the first, out through the front of the
        // second.
        let (into, out) = (-from.facing, to.facing);
        if into > 0.0 && out > 0.0 && lines.straight() {
            self.mark(inside, lines, level, start, end);
        }
        if stretch != Stretch::Material {
            return;
        }
        if lines.straight() {
            self.mark(mask, lines, level, start, end);
        } else {
            // A slanted line marks the cells it passes, also those whose
            // centre lies beyond a face. Kept that far from the faces, it
            // marks none of them; the rows and the columns find the edge of
            // every straight wall exactly.
            let margin = std::f64::consts::FRAC_1_SQRT_2 * self.cell;
            let (start, end) = (start + margin / into.abs(), end - margin / out.abs());
            if start < end {
                self.mark(mask, lines, level, start, end);
            }
        }
    }

    /// Every cell within `radius` cells of a set one, along rows and
    /// columns; cells beyond the grid count as unset.
    fn dilate(&self, mask: &[bool], radius: usize) -> Vec<bool> {
        let along = |values: &mut dyn FnMut(usize) -> bool, count: usize, out: &mut Vec<bool>| {
            out.clear();
            // Set cells in the window from `start` to `end` (exclusive).
            let mut prefix = Vec::with_capacity(count + 1);
            prefix.push(0usize);
            for index in 0..count {
                prefix.push(prefix[index] + usize::from(values(index)));
            }
            for index in 0..count {
                let start = index.saturating_sub(radius);
                let end = (index + radius + 1).min(count);
                out.push(prefix[end] > prefix[start]);
            }
        };
        let (columns, rows) = (self.columns, self.rows);
        let mut across = vec![false; columns * rows];
        let mut line = Vec::new();
        for row in 0..rows {
            along(
                &mut |column| mask[row * columns + column],
                columns,
                &mut line,
            );
            across[row * columns..(row + 1) * columns].copy_from_slice(&line);
        }
        let mut grown = vec![false; columns * rows];
        for column in 0..columns {
            along(&mut |row| across[row * columns + column], rows, &mut line);
            for (row, &set) in line.iter().enumerate() {
                grown[row * columns + column] = set;
            }
        }
        grown
    }

    /// The lines in one of the directions. Along the axes the lines run
    /// through the centres of the cells; in between they lie half a cell
    /// apart.
    fn lines(&self, step: usize) -> Lines {
        let (along, across, first, spacing, count) = match step {
            0 => (
                [1.0, 0.0],
                [0.0, 1.0],
                self.origin[1] + 0.5 * self.cell,
                self.cell,
                self.rows,
            ),
            _ if 2 * step == DIRECTIONS => (
                [0.0, 1.0],
                [-1.0, 0.0],
                -(self.origin[0] + (self.columns as f64 - 0.5) * self.cell),
                self.cell,
                self.columns,
            ),
            _ => {
                let angle = std::f64::consts::PI * step as f64 / DIRECTIONS as f64;
                let (sin, cos) = angle.sin_cos();
                let across = [-sin, cos];
                let corners = [
                    self.origin,
                    [
                        self.origin[0] + self.columns as f64 * self.cell,
                        self.origin[1],
                    ],
                    [
                        self.origin[0],
                        self.origin[1] + self.rows as f64 * self.cell,
                    ],
                    [
                        self.origin[0] + self.columns as f64 * self.cell,
                        self.origin[1] + self.rows as f64 * self.cell,
                    ],
                ];
                let reach = corners.map(|corner| dot(corner, across));
                let low = reach.iter().copied().fold(f64::INFINITY, f64::min);
                let high = reach.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                let spacing = 0.5 * self.cell;
                (
                    [cos, sin],
                    across,
                    low + 0.5 * spacing,
                    spacing,
                    ((high - low) / spacing).ceil() as usize,
                )
            }
        };
        Lines {
            step,
            along,
            across,
            first,
            spacing,
            count,
        }
    }

    /// Visit every line that crosses two segments or more, with where it
    /// lies across the face and where it crosses them, in order along it.
    fn each_line(&self, segments: &[Segment], lines: &Lines, mut visit: impl FnMut(f64, &[Hit])) {
        let Lines {
            along,
            across,
            first,
            spacing,
            count,
            ..
        } = *lines;
        // The segments each line crosses: a segment from one side of a line
        // to the other, its lower end counted and its upper end not.
        let mut crossing: Vec<Vec<u32>> = vec![Vec::new(); count];
        for (index, segment) in segments.iter().enumerate() {
            let (wa, wb) = (dot(segment.a, across), dot(segment.b, across));
            if wa == wb {
                continue;
            }
            let (low, high) = (wa.min(wb), wa.max(wb));
            let start = ((low - first) / spacing).ceil().max(0.0);
            if start >= count as f64 {
                continue;
            }
            let mut line = start as usize;
            while line < count && first + line as f64 * spacing < high {
                crossing[line].push(index as u32);
                line += 1;
            }
        }
        let mut hits: Vec<Hit> = Vec::new();
        for (line, found) in crossing.iter().enumerate() {
            if found.len() < 2 {
                continue;
            }
            let level = first + line as f64 * spacing;
            hits.clear();
            for &index in found {
                let segment = &segments[index as usize];
                let (wa, wb) = (dot(segment.a, across), dot(segment.b, across));
                let share = (level - wa) / (wb - wa);
                let point = [
                    segment.a[0] + (segment.b[0] - segment.a[0]) * share,
                    segment.a[1] + (segment.b[1] - segment.a[1]) * share,
                ];
                hits.push(Hit {
                    at: dot(point, along),
                    facing: dot(segment.air, along),
                    air: segment.air,
                });
            }
            hits.sort_by(|left, right| left.at.total_cmp(&right.at));
            visit(level, &hits);
        }
    }

    /// Mark the cells a line passes from `start` to `end` along it.
    fn mark(&self, mask: &mut [bool], lines: &Lines, level: f64, start: f64, end: f64) {
        let Lines {
            step,
            along,
            across,
            ..
        } = *lines;
        let cell = self.cell;
        // The cells whose centres lie from `start` to `end` along a row or
        // a column with centres at `origin + (i + ½) cell`.
        let centres = |origin: f64, count: usize| {
            let first = ((start - origin) / cell - 0.5).ceil().max(0.0) as usize;
            let last = ((end - origin) / cell - 0.5).floor();
            let last = if last < 0.0 {
                None
            } else {
                Some((last as usize).min(count.saturating_sub(1)))
            };
            last.filter(|last| first <= *last).map(|last| first..=last)
        };
        if step == 0 {
            let row = ((level - self.origin[1]) / cell).floor() as usize;
            if let Some(columns) = centres(self.origin[0], self.columns) {
                for column in columns {
                    mask[row * self.columns + column] = true;
                }
            }
            return;
        }
        if 2 * step == DIRECTIONS {
            let column = ((-level - self.origin[0]) / cell).floor() as usize;
            if let Some(rows) = centres(self.origin[1], self.rows) {
                for row in rows {
                    mask[row * self.columns + column] = true;
                }
            }
            return;
        }
        let samples = ((end - start) / (0.5 * cell)).ceil().max(1.0) as usize;
        for sample in 0..=samples {
            let at = start + (end - start) * sample as f64 / samples as f64;
            let point = [
                along[0] * at + across[0] * level,
                along[1] * at + across[1] * level,
            ];
            let column = ((point[0] - self.origin[0]) / cell).floor();
            let row = ((point[1] - self.origin[1]) / cell).floor();
            if column < 0.0 || row < 0.0 {
                continue;
            }
            let (column, row) = (column as usize, row as usize);
            if column < self.columns && row < self.rows {
                mask[row * self.columns + column] = true;
            }
        }
    }
}

/// Write the marked cells as rectangles: runs along the rows, a run that
/// repeats in the next row grown into it.
fn write_rectangles(
    caps: &mut MeshGeometry,
    face: &Face,
    grid: &Grid,
    mask: &[bool],
    section: &OrientedBox,
) {
    // Open rectangles: (first column, column after the last, first row).
    let mut open: Vec<(usize, usize, usize)> = Vec::new();
    let mut next: Vec<(usize, usize, usize)> = Vec::new();
    for row in 0..=grid.rows {
        next.clear();
        if row < grid.rows {
            let cells = &mask[row * grid.columns..(row + 1) * grid.columns];
            let mut column = 0;
            while column < grid.columns {
                if !cells[column] {
                    column += 1;
                    continue;
                }
                let start = column;
                while column < grid.columns && cells[column] {
                    column += 1;
                }
                let first_row = open
                    .iter()
                    .find(|(from, to, _)| *from == start && *to == column)
                    .map_or(row, |open| open.2);
                next.push((start, column, first_row));
            }
        }
        for &(from, to, first_row) in &open {
            if !next
                .iter()
                .any(|(next_from, next_to, _)| *next_from == from && *next_to == to)
            {
                write_rectangle(caps, face, grid, section, [from, to], [first_row, row]);
            }
        }
        std::mem::swap(&mut open, &mut next);
    }
}

fn write_rectangle(
    caps: &mut MeshGeometry,
    face: &Face,
    grid: &Grid,
    section: &OrientedBox,
    columns: [usize; 2],
    rows: [usize; 2],
) {
    let u = columns
        .map(|column| (grid.origin[0] + column as f64 * grid.cell).clamp(face.min[0], face.max[0]));
    let v =
        rows.map(|row| (grid.origin[1] + row as f64 * grid.cell).clamp(face.min[1], face.max[1]));
    if u[0] >= u[1] || v[0] >= v[1] {
        return;
    }
    let level = face.level;
    let Ok(base) = u32::try_from(caps.vertices.len()) else {
        return;
    };
    for [x, y] in [[u[0], v[0]], [u[1], v[0]], [u[1], v[1]], [u[0], v[1]]] {
        let mut local = [0.0; 3];
        local[face.axis] = level;
        local[face.u] = x;
        local[face.v] = y;
        caps.vertices.push(section.to_scene(local));
    }
    // Counter-clockwise in the face looks along its axis: out of the box at
    // the upper face, into it at the lower one.
    if face.upper {
        caps.triangles.push([base, base + 1, base + 2]);
        caps.triangles.push([base, base + 2, base + 3]);
    } else {
        caps.triangles.push([base, base + 2, base + 1]);
        caps.triangles.push([base, base + 3, base + 2]);
    }
}

fn difference(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Bounds;

    /// A box as a closed mesh; `outward` chooses which way its triangles
    /// face.
    fn add_box(mesh: &mut MeshGeometry, min: [f64; 3], max: [f64; 3], outward: bool) {
        let base = mesh.vertices.len() as u32;
        for corner in 0..8 {
            mesh.vertices.push([
                if corner & 1 == 0 { min[0] } else { max[0] },
                if corner & 2 == 0 { min[1] } else { max[1] },
                if corner & 4 == 0 { min[2] } else { max[2] },
            ]);
        }
        // Each side counter-clockwise seen from outside.
        let sides = [
            [0, 2, 3, 1], // bottom
            [4, 5, 7, 6], // top
            [0, 1, 5, 4], // front, y = min
            [2, 6, 7, 3], // back, y = max
            [0, 4, 6, 2], // left, x = min
            [1, 3, 7, 5], // right, x = max
        ];
        for [a, b, c, d] in sides {
            let quad = [a, b, c, d].map(|corner| base + corner);
            let (first, second) = ([quad[0], quad[1], quad[2]], [quad[0], quad[2], quad[3]]);
            if outward {
                mesh.triangles.push(first);
                mesh.triangles.push(second);
            } else {
                mesh.triangles.push([first[0], first[2], first[1]]);
                mesh.triangles.push([second[0], second[2], second[1]]);
            }
        }
    }

    /// A flat rectangle at y = `y` from x0 to x1 and z0 to z1, facing +y or
    /// -y.
    fn add_sheet(mesh: &mut MeshGeometry, y: f64, x: [f64; 2], z: [f64; 2], toward_plus: bool) {
        let base = mesh.vertices.len() as u32;
        mesh.vertices.extend([
            [x[0], y, z[0]],
            [x[1], y, z[0]],
            [x[1], y, z[1]],
            [x[0], y, z[1]],
        ]);
        // Counter-clockwise from -y faces -y.
        if toward_plus {
            mesh.triangles.push([base, base + 2, base + 1]);
            mesh.triangles.push([base, base + 3, base + 2]);
        } else {
            mesh.triangles.push([base, base + 1, base + 2]);
            mesh.triangles.push([base, base + 2, base + 3]);
        }
    }

    /// A room of 4 by 5 metres inside, with walls, floor and ceiling of
    /// 0.3: the outer box faces out, the inner one faces into the room.
    fn room() -> MeshGeometry {
        let mut mesh = MeshGeometry::default();
        add_box(&mut mesh, [-0.3, -0.3, -0.3], [4.3, 5.3, 3.3], true);
        add_box(&mut mesh, [0.0, 0.0, 0.0], [4.0, 5.0, 3.0], false);
        mesh
    }

    fn area(caps: &MeshGeometry) -> f64 {
        caps.triangles
            .iter()
            .map(|triangle| {
                let [a, b, c] = triangle.map(|index| caps.vertices[index as usize]);
                let normal = cross(difference(b, a), difference(c, a));
                0.5 * (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt()
            })
            .sum()
    }

    fn caps_of(mesh: &MeshGeometry, section: OrientedBox) -> MeshGeometry {
        section_caps(mesh, |xyz| xyz, &section, CapOptions::default())
    }

    fn boxed(min: [f64; 3], max: [f64; 3]) -> OrientedBox {
        OrientedBox::from(Bounds { min, max })
    }

    #[test]
    fn the_walls_of_a_room_cut_at_half_height_are_capped() {
        let caps = caps_of(&room(), boxed([-1.0, -1.0, -1.0], [5.0, 6.0, 1.5]));
        // The ring of walls on the top face of the box.
        let ring = 4.6 * 5.6 - 4.0 * 5.0;
        let found = area(&caps);
        assert!((found - ring).abs() < 0.03 * ring, "{found} against {ring}");
        // Every cap lies on the top face, just inside the box.
        for vertex in &caps.vertices {
            assert!((vertex[2] - 1.5).abs() < 0.001, "{vertex:?}");
        }
        // And faces up, out of the box.
        for triangle in &caps.triangles {
            let [a, b, c] = triangle.map(|index| caps.vertices[index as usize]);
            assert!(cross(difference(b, a), difference(c, a))[2] > 0.0);
        }
        // The open middle of the room stays open.
        let middle = caps.triangles.iter().any(|triangle| {
            let [a, b, c] = triangle.map(|index| caps.vertices[index as usize]);
            let centre = [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0];
            (0.5..3.5).contains(&centre[0]) && (0.5..4.5).contains(&centre[1])
        });
        assert!(!middle);
    }

    /// Whether a point of the plan lies on a cap.
    fn covered(caps: &MeshGeometry, at: [f64; 2]) -> bool {
        caps.triangles.iter().any(|triangle| {
            let [a, b, c] = triangle.map(|index| caps.vertices[index as usize]);
            let side = |p: [f64; 3], q: [f64; 3]| {
                (q[0] - p[0]) * (at[1] - p[1]) - (q[1] - p[1]) * (at[0] - p[0])
            };
            let sides = [side(a, b), side(b, c), side(c, a)];
            sides.iter().all(|&value| value >= 0.0) || sides.iter().all(|&value| value <= 0.0)
        })
    }

    #[test]
    fn the_junction_of_a_partition_and_an_outer_wall_is_capped() {
        // Two rooms beside each other: a partition of 0.2 between them meets
        // the outer walls of 0.3 in a T.
        let mut mesh = MeshGeometry::default();
        add_box(&mut mesh, [-0.3, -0.3, -0.25], [9.3, 5.3, 3.25], true);
        add_box(&mut mesh, [0.0, 0.0, 0.0], [4.0, 5.0, 3.0], false);
        add_box(&mut mesh, [4.2, 0.0, 0.0], [9.0, 5.0, 3.0], false);
        let caps = caps_of(&mesh, boxed([-1.0, -2.5, -0.25], [10.0, 5.3, 1.5]));
        let walls = 9.6 * 5.6 - 4.0 * 5.0 - 4.8 * 5.0;
        let found = area(&caps);
        assert!(
            (found - walls).abs() < 0.01 * walls,
            "{found} against {walls}"
        );
        for at in [
            [4.1, -0.1],
            [4.1, -0.01],
            [4.01, -0.15],
            [4.1, 5.1],
            [-0.15, -0.15],
        ] {
            assert!(covered(&caps, at), "{at:?}");
        }
        for at in [[2.0, 2.5], [4.1, -0.5], [6.0, 0.1]] {
            assert!(!covered(&caps, at), "{at:?}");
        }
        // Across the walls, to within a cell of their faces.
        let across = |from: f64, thickness: f64, step: usize| {
            from + 0.003 + (thickness - 0.006) * step as f64 / 100.0
        };
        let gaps: Vec<_> = (0..=100)
            .flat_map(|step| {
                [
                    [across(-0.3, 0.3, step), 2.5],
                    [2.0, across(-0.3, 0.3, step)],
                    [across(4.0, 0.2, step), 2.5],
                ]
            })
            .filter(|at| !covered(&caps, *at))
            .collect();
        assert!(gaps.is_empty(), "{gaps:?}");
    }

    #[test]
    fn a_floor_cut_by_a_side_of_the_box_is_capped() {
        // The side x = 2 of the box cuts the floor, the ceiling and the
        // two walls along x; it faces -x, out of the box.
        let caps = caps_of(&room(), boxed([2.0, -1.0, -1.0], [6.0, 6.0, 4.0]));
        let expected = 2.0 * 0.3 * 5.6 + 2.0 * 0.3 * 3.0;
        let found = area(&caps);
        assert!(
            (found - expected).abs() < 0.03 * expected,
            "{found} against {expected}"
        );
        for triangle in &caps.triangles {
            let [a, b, c] = triangle.map(|index| caps.vertices[index as usize]);
            assert!(cross(difference(b, a), difference(c, a))[0] < 0.0);
        }
    }

    #[test]
    fn a_single_sheet_gets_no_cap() {
        let mut mesh = MeshGeometry::default();
        add_sheet(&mut mesh, 0.0, [0.0, 10.0], [0.0, 6.0], false);
        assert!(caps_of(&mesh, boxed([-1.0, -1.0, -1.0], [11.0, 1.0, 3.0]))
            .triangles
            .is_empty());
    }

    #[test]
    fn two_sheets_pair_only_within_the_thickness_and_not_front_to_front() {
        let section = boxed([-1.0, -1.0, -1.0], [11.0, 2.0, 3.0]);
        // The outer face of a wall faces -y, the inner +y: material between.
        let mut wall = MeshGeometry::default();
        add_sheet(&mut wall, 0.0, [0.0, 10.0], [0.0, 6.0], false);
        add_sheet(&mut wall, 0.25, [0.0, 10.0], [0.0, 6.0], true);
        let found = area(&caps_of(&wall, section));
        assert!((found - 2.5).abs() < 0.08, "{found}");

        // Both facing the same way, as in a mesh made without stations
        // whose faces all look at one point: one of them faces the wrong
        // way, and with the open on both sides the stretch between them is
        // a wall.
        for toward_plus in [true, false] {
            let mut turned = MeshGeometry::default();
            add_sheet(&mut turned, 0.0, [0.0, 10.0], [0.0, 6.0], toward_plus);
            add_sheet(&mut turned, 0.25, [0.0, 10.0], [0.0, 6.0], toward_plus);
            let found = area(&caps_of(&turned, section));
            assert!((found - 2.5).abs() < 0.08, "{found}");
        }

        // Facing each other: the air between two walls.
        let mut gap = MeshGeometry::default();
        add_sheet(&mut gap, 0.0, [0.0, 10.0], [0.0, 6.0], true);
        add_sheet(&mut gap, 0.25, [0.0, 10.0], [0.0, 6.0], false);
        assert!(caps_of(&gap, section).triangles.is_empty());

        // Farther apart than the thickness.
        let mut far = MeshGeometry::default();
        add_sheet(&mut far, 0.0, [0.0, 10.0], [0.0, 6.0], false);
        add_sheet(&mut far, 0.8, [0.0, 10.0], [0.0, 6.0], true);
        assert!(caps_of(&far, section).triangles.is_empty());
        let wide = section_caps(&far, |xyz| xyz, &section, CapOptions { max_thickness: 1.0 });
        assert!((area(&wide) - 8.0).abs() < 0.2, "{}", area(&wide));
    }

    #[test]
    fn ground_that_wavers_about_a_face_of_the_box_gets_no_cap() {
        // A measured ground of 10 cm triangles, a millimetre above and below
        // the bottom of the box in turn, facing up.
        let mut ground = MeshGeometry::default();
        let count = 60;
        for j in 0..=count {
            for i in 0..=count {
                let lift = if (i * 7 + j * 3) % 2 == 0 {
                    0.001
                } else {
                    -0.001
                };
                ground.vertices.push([i as f64 * 0.1, j as f64 * 0.1, lift]);
            }
        }
        let at = |i: usize, j: usize| (j * (count + 1) + i) as u32;
        for j in 0..count {
            for i in 0..count {
                ground
                    .triangles
                    .push([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
                ground
                    .triangles
                    .push([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
            }
        }
        let caps = caps_of(&ground, boxed([-1.0, -1.0, 0.0], [7.0, 7.0, 2.0]));
        assert!(caps.triangles.is_empty(), "{}", area(&caps));
    }

    #[test]
    fn a_wall_that_ends_in_a_face_of_the_box_is_capped() {
        // As a mesh made in the same box: its faces stop at the bottom, the
        // top and the two ends of it, and each of those is capped.
        let mut wall = MeshGeometry::default();
        add_sheet(&mut wall, 0.0, [0.0, 10.0], [0.0, 1.5], false);
        add_sheet(&mut wall, 0.25, [0.0, 10.0], [0.0, 1.5], true);
        let found = area(&caps_of(&wall, boxed([0.0, -1.0, 0.0], [10.0, 2.0, 1.5])));
        let expected = 2.0 * 10.0 * 0.25 + 2.0 * 1.5 * 0.25;
        assert!((found - expected).abs() < 0.1, "{found}");
    }

    #[test]
    fn stretches_are_settled_by_the_turns_of_air_and_material() {
        use Stretch::{Material as M, Open as O, Unknown as U, Unsure as S};
        let settled = |mut stretches: Vec<Stretch>| {
            settle(&mut stretches);
            stretches
        };
        // A wall between two rooms, or between a room and the open.
        assert_eq!(settled(vec![U]), [M]);
        assert_eq!(settled(vec![O, U, O]), [O, M, O]);
        // A door leaf, the gap behind it and the wall.
        assert_eq!(settled(vec![O, U, U, U, O]), [O, M, O, M, O]);
        // Air between two walls.
        assert_eq!(settled(vec![M, U, M]), [M, O, M]);
        // A sheet in front of a wall, alone on its side: its gap is no
        // wall, and without anything known neither is the wall behind it.
        assert_eq!(settled(vec![U, M, O]), [O, M, O]);
        assert_eq!(settled(vec![O, M, U]), [O, M, O]);
        assert_eq!(settled(vec![U, U]), [O, O]);
        // Beside a stretch that tells nothing.
        assert_eq!(settled(vec![S, U, O]), [S, O, O]);
        assert_eq!(settled(vec![O, U, S]), [O, O, S]);
        // Runs are settled one by one.
        assert_eq!(settled(vec![U, O, U, U, U, M]), [M, O, O, O, O, M]);
        assert_eq!(settled(vec![U, O, U, U, M, U]), [M, O, M, O, M, O]);
    }

    /// Turn every triangle to look at `point`, as a mesh made without
    /// stations faces the middle of its region.
    fn facing(mut mesh: MeshGeometry, point: [f64; 3]) -> MeshGeometry {
        for triangle in &mut mesh.triangles {
            let [a, b, c] = triangle.map(|index| mesh.vertices[index as usize]);
            let normal = cross(difference(b, a), difference(c, a));
            let centre: [f64; 3] = std::array::from_fn(|axis| (a[axis] + b[axis] + c[axis]) / 3.0);
            let towards = difference(point, centre);
            if normal[0] * towards[0] + normal[1] * towards[1] + normal[2] * towards[2] < 0.0 {
                triangle.swap(1, 2);
            }
        }
        mesh
    }

    #[test]
    fn walls_whose_faces_all_look_at_one_point_are_capped() {
        // The room and the facade beside it, all facing the middle of the
        // room or a point outside it.
        let section = boxed([-4.0, -4.0, -2.0], [7.0, 7.0, 1.5]);
        let ring = 4.6 * 5.6 - 4.0 * 5.0;
        for point in [[2.0, 2.5, 1.5], [-6.0, -5.0, 0.0], [2.0, -1.0, 1.0]] {
            let mut mesh = room();
            add_sheet(&mut mesh, -3.0, [-2.0, 6.0], [-1.0, 4.0], false);
            let caps = caps_of(&facing(mesh, point), section);
            let found = area(&caps);
            assert!(
                (found - ring).abs() < 0.03 * ring,
                "{point:?}: {found} against {ring}"
            );
            // The facade, seen from one side only, stays open.
            assert!(caps.vertices.iter().all(|vertex| vertex[1] > -1.0));
        }

        // Two rooms beside each other: the walls and the partition between
        // them, and most of the junction where they meet.
        let mut rooms = MeshGeometry::default();
        add_box(&mut rooms, [-0.3, -0.3, -0.25], [9.3, 5.3, 3.25], true);
        add_box(&mut rooms, [0.0, 0.0, 0.0], [4.0, 5.0, 3.0], false);
        add_box(&mut rooms, [4.2, 0.0, 0.0], [9.0, 5.0, 3.0], false);
        let rooms = facing(rooms, [4.5, 2.5, 1.5]);
        let caps = caps_of(&rooms, boxed([-1.0, -2.5, -0.25], [10.0, 5.3, 1.5]));
        let walls = 9.6 * 5.6 - 4.0 * 5.0 - 4.8 * 5.0;
        let found = area(&caps);
        assert!(
            (found - walls).abs() < 0.01 * walls,
            "{found} against {walls}"
        );
        for at in [[4.1, 2.5], [4.1, -0.15], [-0.15, -0.15], [2.0, -0.15]] {
            assert!(covered(&caps, at), "{at:?}");
        }
        for at in [[2.0, 2.5], [6.0, 2.5], [4.1, -0.5]] {
            assert!(!covered(&caps, at), "{at:?}");
        }
    }

    #[test]
    fn a_thin_wall_with_a_surface_inside_it_is_capped_whole() {
        // Two rooms whose faces look at a point in the second one, and a
        // third surface in the middle of the partition that faces the other
        // way, as a mesh made without stations has in a thin wall.
        let mut rooms = MeshGeometry::default();
        add_box(&mut rooms, [-0.3, -0.3, -0.25], [9.3, 5.3, 3.25], true);
        add_box(&mut rooms, [0.0, 0.0, 0.0], [4.0, 5.0, 3.0], false);
        add_box(&mut rooms, [4.2, 0.0, 0.0], [9.0, 5.0, 3.0], false);
        let mut rooms = facing(rooms, [6.5, 2.5, 1.5]);
        let base = rooms.vertices.len() as u32;
        rooms.vertices.extend([
            [4.08, 0.0, 0.0],
            [4.08, 5.0, 0.0],
            [4.08, 5.0, 3.0],
            [4.08, 0.0, 3.0],
        ]);
        rooms.triangles.push([base, base + 2, base + 1]);
        rooms.triangles.push([base, base + 3, base + 2]);
        let caps = caps_of(&rooms, boxed([-1.0, -2.5, -0.25], [10.0, 5.3, 1.5]));
        let walls = 9.6 * 5.6 - 4.0 * 5.0 - 4.8 * 5.0;
        let found = area(&caps);
        assert!(
            (found - walls).abs() < 0.01 * walls,
            "{found} against {walls}"
        );
        for at in [[4.02, 2.5], [4.1, 2.5], [4.18, 2.5]] {
            assert!(covered(&caps, at), "{at:?}");
        }
    }

    #[test]
    fn a_sheet_in_front_of_a_wall_is_not_capped_to_it() {
        // A wall of 0.25 and a sheet 0.2 in front of it that was seen from
        // its front only.
        let section = boxed([-1.0, -1.0, -1.0], [11.0, 2.0, 3.0]);
        let mut mesh = MeshGeometry::default();
        add_sheet(&mut mesh, 0.0, [0.0, 10.0], [0.0, 6.0], false);
        add_sheet(&mut mesh, 0.25, [0.0, 10.0], [0.0, 6.0], true);
        add_sheet(&mut mesh, -0.2, [0.0, 10.0], [0.0, 6.0], false);
        let caps = caps_of(&mesh, section);
        let found = area(&caps);
        assert!((found - 2.5).abs() < 0.08, "{found}");
        assert!(caps.vertices.iter().all(|vertex| vertex[1] > -0.01));
        // Turned to a point behind the wall, the faces no longer tell the
        // wall from the gap; neither is capped rather than both, also not
        // at the open ends, where a slanted line misses one of the three.
        let turned = caps_of(&facing(mesh, [5.0, 3.0, 1.0]), section);
        assert!(turned.triangles.is_empty(), "{}", area(&turned));
    }

    /// The closed mesh of a scan, at voxels of 4 cm.
    fn closed_mesh(shape: &crate::test_shapes::Shape) -> MeshGeometry {
        use crate::region_source::{RegionSource, SourceTransform};
        use crate::surfels::SurfelSource;
        let cloud = crate::test_shapes::indexed_cloud(&shape.cloud_points(), 4_096);
        let ranges = shape
            .station_ranges()
            .into_iter()
            .map(|(first_ordinal, station)| crate::ScanRange {
                first_ordinal,
                station: (station != u32::MAX).then_some(station),
            })
            .collect();
        let sources = [SurfelSource::with_stations(
            RegionSource::new(&cloud.cloud, Some(&cloud.index), SourceTransform::default()),
            shape.stations.clone(),
            ranges,
        )];
        let config = crate::ClosedMeshConfig {
            voxel: Some(0.04),
            ..crate::ClosedMeshConfig::default()
        };
        let everything = |_: usize, _: u64, _: &crate::Point| true;
        crate::mesh_closed(&sources, None, &everything, &config, &mut |_| Ok(()))
            .unwrap()
            .0
    }

    #[test]
    fn the_closed_mesh_of_a_scanned_room_is_capped_on_its_walls_and_not_on_a_facade() {
        use crate::test_shapes::{box_room, rectangle, RoomSpec};
        // A room of 4 by 3 m inside with walls of 0.3, scanned inside and
        // outside, and a facade south of it scanned from the street only.
        let room = box_room(&RoomSpec {
            wall_thickness: Some(0.3),
            ..RoomSpec::default()
        });
        let facade = rectangle(
            [-1.0, -5.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [6.0, 2.6],
            0.02,
        )
        .with_stations(&[[2.0, -8.0, 1.2]]);
        let scan = room.merged(facade);
        let section = boxed([-2.0, -6.0, -1.0], [6.0, 5.0, 1.3]);
        let ring = 4.6 * 3.6 - 4.0 * 3.0;
        // With its stations the fronts of the mesh look at the air; without
        // them they all look at the middle of the scan, and the walls are
        // capped all the same.
        for shape in [scan.clone(), scan.with_stations(&[])] {
            let caps = caps_of(&closed_mesh(&shape), section);
            let found = area(&caps);
            assert!((found - ring).abs() < 0.03 * ring, "{found} against {ring}");
            assert!(caps.vertices.iter().all(|vertex| vertex[1] > -1.0));
            assert!(!covered(&caps, [2.0, 1.5]));
            for at in [[-0.15, 1.5], [4.15, 1.5], [2.0, -0.15], [2.0, 3.15]] {
                assert!(covered(&caps, at), "{at:?}");
            }
        }
    }

    #[test]
    fn a_thick_block_is_not_capped() {
        let mut mesh = MeshGeometry::default();
        add_box(&mut mesh, [0.0, 0.0, 0.0], [3.0, 2.0, 2.0], true);
        assert!(caps_of(&mesh, boxed([-1.0, -1.0, -1.0], [4.0, 3.0, 1.0]))
            .triangles
            .is_empty());
    }

    #[test]
    fn a_room_beside_a_facade_caps_the_walls_and_not_the_facade() {
        let mut mesh = room();
        add_sheet(&mut mesh, -3.0, [-2.0, 6.0], [-1.0, 4.0], false);
        let caps = caps_of(&mesh, boxed([-4.0, -4.0, -2.0], [7.0, 7.0, 1.5]));
        let ring = 4.6 * 5.6 - 4.0 * 5.0;
        let found = area(&caps);
        assert!((found - ring).abs() < 0.03 * ring, "{found} against {ring}");
        assert!(caps.vertices.iter().all(|vertex| vertex[1] > -1.0));
    }

    #[test]
    fn a_turned_box_caps_in_its_own_frame() {
        let mut mesh = room();
        let (sin, cos) = 30.0_f64.to_radians().sin_cos();
        let center = [2.0, 2.5, 1.5];
        let turn = |xyz: [f64; 3]| {
            let (dx, dy) = (xyz[0] - center[0], xyz[1] - center[1]);
            [
                center[0] + cos * dx - sin * dy,
                center[1] + sin * dx + cos * dy,
                xyz[2],
            ]
        };
        for vertex in &mut mesh.vertices {
            *vertex = turn(*vertex);
        }
        let section = OrientedBox::new(
            Bounds {
                min: [-1.0, -1.5, -1.0],
                max: [5.0, 6.5, 1.5],
            },
            30.0,
        );
        let caps = caps_of(&mesh, section);
        let ring = 4.6 * 5.6 - 4.0 * 5.0;
        let found = area(&caps);
        assert!((found - ring).abs() < 0.03 * ring, "{found} against {ring}");
        assert!(caps.vertices.iter().all(|vertex| {
            let local = section.to_box(*vertex);
            (local[2] - 1.5).abs() < 0.001 && local[0] >= -1.0 - 1e-9 && local[0] <= 5.0 + 1e-9
        }));
    }

    #[test]
    fn a_wall_at_an_angle_is_capped_to_its_thickness() {
        // A wall of 0.2 at 20 degrees to the axes.
        let (sin, cos) = 20.0_f64.to_radians().sin_cos();
        let mut wall = MeshGeometry::default();
        add_sheet(&mut wall, 0.0, [0.0, 8.0], [0.0, 6.0], false);
        add_sheet(&mut wall, 0.2, [0.0, 8.0], [0.0, 6.0], true);
        for vertex in &mut wall.vertices {
            let [x, y, z] = *vertex;
            *vertex = [cos * x - sin * y, sin * x + cos * y, z];
        }
        let found = area(&caps_of(&wall, boxed([-1.0, -1.0, -1.0], [9.0, 4.0, 3.0])));
        assert!((found - 1.6).abs() < 0.1, "{found}");
    }

    #[test]
    fn an_empty_or_invalid_request_has_no_caps() {
        let section = boxed([-1.0, -1.0, -1.0], [5.0, 6.0, 1.5]);
        assert!(caps_of(&MeshGeometry::default(), section)
            .triangles
            .is_empty());
        let none = section_caps(
            &room(),
            |xyz| xyz,
            &section,
            CapOptions {
                max_thickness: f64::NAN,
            },
        );
        assert!(none.triangles.is_empty());
    }
}
