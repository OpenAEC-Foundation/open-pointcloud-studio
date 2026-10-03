//! Quadric error edge collapse: fewer triangles where the surface is flat,
//! within a stated distance of the original surface.
//!
//! Every vertex carries the planes of the triangles it stands for. An edge
//! collapses while the summed squared distance from the merged vertex to all
//! those planes stays within the tolerance squared, so none of them ends up
//! farther away than the tolerance. The sum makes this a cautious bound: a
//! flat area collapses completely, a curved or rough one stops well before
//! the tolerance is used up, and sooner the finer its triangles are.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use super::obj_mesh::MeshGeometry;
use super::LoadError;

/// A collapse may turn the normal of a neighbouring triangle by about 78
/// degrees at most. Beyond that the surface folds over itself.
const MIN_NORMAL_COSINE: f64 = 0.2;
/// All collapses together may turn a triangle 60 degrees away from the
/// normal it had in the input. Noise already tilts the small triangles of
/// the input by ten degrees and more, so a wider limit lets a triangle end
/// up facing the other side of the surface.
const MIN_TOTAL_COSINE: f64 = 0.5;
/// Squared sine of the sharpest corner a collapse may leave a triangle with:
/// a sine of 0.02, a good degree. A thinner triangle is a needle whose normal
/// follows the noise of the surface, not its shape.
const MIN_CORNER_SINE_SQUARED: f64 = 4e-4;
/// A triangle that was thinner than that before the collapse may stay as
/// thin, down to this: below it the corners lie on one line and the triangle
/// has no normal.
const MIN_AREA_SINE_SQUARED: f64 = 1e-12;
/// Share of the allowed error below which a collapse counts as free. On a
/// flat surface the costs are rounding, and ordering by them would be
/// ordering by chance.
const FLAT_COST: f64 = 1e-6;
/// A merged vertex only leaves the original vertex positions when that wins
/// this share of the allowed error. Flat areas keep exact input positions.
const MOVE_GAIN: f64 = 1e-3;
/// The unconstrained best position is only trusted where three clearly
/// different planes meet; elsewhere it is free to slide along the surface.
const MIN_CORNER_DETERMINANT: f64 = 1e-3;
const PROGRESS_INTERVAL: u64 = 4_096;
/// Passes over all remaining edges. Each one after the first only picks up
/// what the pass before it made possible, so they shrink quickly.
const MAX_PASSES: usize = 8;
const REMOVED: u32 = u32::MAX;

#[derive(Debug, Default)]
pub struct SimplifiedMesh {
    pub mesh: MeshGeometry,
    /// For every vertex of `mesh`, the input vertex it continues. A vertex
    /// that was not moved has exactly the position of that input vertex.
    pub source_vertices: Vec<u32>,
    pub collapses: u64,
}

/// Simplify without progress reports. See `simplify_mesh_progress`.
pub fn simplify_mesh(
    mesh: &MeshGeometry,
    tolerance: f64,
    locked: &[bool],
) -> Result<SimplifiedMesh, LoadError> {
    simplify_mesh_progress(mesh, tolerance, locked, &mut |_, _| Ok(()))
}

/// Collapse edges, cheapest first, while the result stays within `tolerance`
/// (in the unit of the mesh) of the planes of the original triangles.
///
/// `locked` is empty or holds one flag per vertex; a locked vertex keeps its
/// position, colour and normal, which lets separately simplified pieces still
/// fit together. Open rims keep their outline without being locked. Colours
/// and normals of merged vertices are averaged by the area they stand for.
///
/// A tolerance of zero returns the input unchanged. Otherwise vertices that
/// no triangle uses and triangles that repeat a vertex are left out, and an
/// open piece thinner than the tolerance may disappear altogether.
///
/// A collapse that would fold the surface, turn a triangle towards its other
/// side or leave a needle of a triangle is skipped, so a locked rim of short
/// edges keeps a few vertices inside it.
///
/// `progress` receives the candidates handled so far and an estimate of the
/// total. Collapses add candidates, so the estimate grows, but the share the
/// two numbers give never goes back. An error from it stops the work.
pub fn simplify_mesh_progress(
    mesh: &MeshGeometry,
    tolerance: f64,
    locked: &[bool],
    progress: &mut dyn FnMut(u64, u64) -> Result<(), LoadError>,
) -> Result<SimplifiedMesh, LoadError> {
    let count = mesh.vertices.len();
    let valid = tolerance.is_finite()
        && tolerance >= 0.0
        && count < REMOVED as usize
        && mesh.triangles.len() < REMOVED as usize
        && (locked.is_empty() || locked.len() == count)
        && mesh
            .vertices
            .iter()
            .all(|vertex| vertex.iter().all(|value| value.is_finite()))
        && mesh
            .triangles
            .iter()
            .all(|face| face.iter().all(|index| (*index as usize) < count))
        && mesh
            .colors
            .as_ref()
            .is_none_or(|colors| colors.len() == count)
        && mesh.normals.as_ref().is_none_or(|normals| {
            normals.len() == count
                && normals
                    .iter()
                    .all(|normal| normal.iter().all(|value| value.is_finite()))
        });
    if !valid {
        return Err(LoadError::InvalidData(
            "invalid mesh simplification input".into(),
        ));
    }
    if tolerance == 0.0 {
        return Ok(SimplifiedMesh {
            mesh: MeshGeometry {
                vertices: mesh.vertices.clone(),
                triangles: mesh.triangles.clone(),
                colors: mesh.colors.clone(),
                normals: mesh.normals.clone(),
            },
            source_vertices: (0..count as u32).collect(),
            collapses: 0,
        });
    }

    let mut state = Simplifier::new(mesh, tolerance, locked);
    let mut handled = 0_u64;
    let mut reported = (0_u64, state.heap.len() as u64);
    progress(reported.0, reported.1)?;
    for _ in 0..MAX_PASSES {
        let before = state.collapses;
        while let Some(candidate) = state.heap.pop() {
            handled += 1;
            if handled.is_multiple_of(PROGRESS_INTERVAL) {
                let mut total = handled + state.heap.len() as u64;
                // A burst of new candidates must not take back progress
                // that was reported: the total grows no faster than the
                // handled count did.
                if reported.0 > 0 {
                    let limit =
                        u128::from(handled) * u128::from(reported.1) / u128::from(reported.0);
                    total = u128::from(total).min(limit) as u64;
                }
                reported = (handled, total);
                progress(handled, total)?;
            }
            // Either end changed since this candidate was priced: a newer
            // candidate for the same edge is in the heap, or the edge is gone.
            if state.version[candidate.a as usize] != candidate.version_a
                || state.version[candidate.b as usize] != candidate.version_b
            {
                continue;
            }
            state.collapse(candidate.a, candidate.b);
        }
        if state.collapses == before {
            break;
        }
        // A candidate that would have folded the surface is dropped, yet a
        // later collapse next to it can make it harmless. Offer every edge
        // again until a whole pass changes nothing.
        state.queue_all_edges();
    }
    progress(handled, handled)?;
    Ok(state.finish(mesh))
}

/// Sum of squared distances to a set of planes, as the coefficients of
/// `v.A.v + 2 b.v + c`.
#[derive(Clone, Copy, Default)]
struct Quadric {
    /// Upper triangle of the symmetric matrix: xx, xy, xz, yy, yz, zz.
    a: [f64; 6],
    b: [f64; 3],
    c: f64,
}

impl Quadric {
    /// Add the plane `normal.x + offset = 0` with a unit normal.
    fn add_plane(&mut self, normal: [f64; 3], offset: f64) {
        let [x, y, z] = normal;
        for (sum, value) in self
            .a
            .iter_mut()
            .zip([x * x, x * y, x * z, y * y, y * z, z * z])
        {
            *sum += value;
        }
        for (sum, value) in self.b.iter_mut().zip(normal) {
            *sum += value * offset;
        }
        self.c += offset * offset;
    }

    fn add(&mut self, other: &Self) {
        for (sum, value) in self.a.iter_mut().zip(other.a) {
            *sum += value;
        }
        for (sum, value) in self.b.iter_mut().zip(other.b) {
            *sum += value;
        }
        self.c += other.c;
    }

    fn apply(&self, v: [f64; 3]) -> [f64; 3] {
        let a = &self.a;
        [
            a[0] * v[0] + a[1] * v[1] + a[2] * v[2],
            a[1] * v[0] + a[3] * v[1] + a[4] * v[2],
            a[2] * v[0] + a[4] * v[1] + a[5] * v[2],
        ]
    }

    fn error(&self, v: [f64; 3]) -> f64 {
        dot(v, self.apply(v)) + 2.0 * dot(self.b, v) + self.c
    }

    /// The position with the least error, where the planes pin one down.
    fn minimum(&self) -> Option<[f64; 3]> {
        let a = &self.a;
        let minors = [
            a[3] * a[5] - a[4] * a[4],
            a[1] * a[5] - a[4] * a[2],
            a[1] * a[4] - a[3] * a[2],
        ];
        let determinant = a[0] * minors[0] - a[1] * minors[1] + a[2] * minors[2];
        // Unit normals: the trace is the number of planes, and three planes
        // at right angles give a determinant of (trace / 3) cubed.
        let scale = (a[0] + a[3] + a[5]) / 3.0;
        let pinned = determinant > MIN_CORNER_DETERMINANT * scale * scale * scale;
        if !pinned {
            return None;
        }
        let r = self.b.map(|value| -value);
        let x = r[0] * minors[0] - a[1] * (r[1] * a[5] - a[4] * r[2])
            + a[2] * (r[1] * a[4] - a[3] * r[2]);
        let y = a[0] * (r[1] * a[5] - a[4] * r[2]) - r[0] * minors[1]
            + a[2] * (a[1] * r[2] - r[1] * a[2]);
        let z = a[0] * (a[3] * r[2] - r[1] * a[4]) - a[1] * (a[1] * r[2] - r[1] * a[2])
            + r[0] * minors[2];
        let position = [x / determinant, y / determinant, z / determinant];
        position
            .iter()
            .all(|value| value.is_finite())
            .then_some(position)
    }
}

/// Where the vertex that survives a collapse ends up.
#[derive(Clone, Copy)]
enum Placement {
    /// At the unchanged position of this end of the edge.
    Keep(u32),
    Move([f64; 3]),
}

struct Candidate {
    /// Error of the collapse, zero for every one that counts as free.
    cost: f64,
    /// Input vertices the two ends stand for together.
    size: u32,
    /// Squared length of the edge.
    length: f32,
    a: u32,
    b: u32,
    version_a: u32,
    version_b: u32,
}

impl PartialEq for Candidate {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Candidate {}

impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Candidate {
    /// The heap hands out its greatest element, so the cheapest collapse
    /// ranks highest. A flat area is all ties: there the ends that took in
    /// the fewest vertices go first, and of those the shortest edge. Without
    /// that one vertex would swallow the whole area, and every collapse walks
    /// all triangles of its vertex. The vertex numbers settle what is left,
    /// which keeps the result the same from run to run.
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .cost
            .total_cmp(&self.cost)
            .then_with(|| other.size.cmp(&self.size))
            .then_with(|| other.length.total_cmp(&self.length))
            .then_with(|| other.a.cmp(&self.a))
            .then_with(|| other.b.cmp(&self.b))
            .then_with(|| other.version_a.cmp(&self.version_a))
            .then_with(|| other.version_b.cmp(&self.version_b))
    }
}

struct Simplifier {
    tolerance_squared: f64,
    /// Centre of the mesh. The quadrics are built around it: at survey
    /// coordinates their terms would cancel away every significant digit.
    origin: [f64; 3],
    /// Positions relative to `origin`.
    position: Vec<[f64; 3]>,
    /// The same positions as they go into the result. Kept apart so a vertex
    /// that never moved comes back bit for bit.
    world: Vec<[f64; 3]>,
    quadric: Vec<Quadric>,
    triangles: Vec<[u32; 3]>,
    /// Unit normal every triangle had in the input, zero where it had none.
    reference: Vec<[f32; 3]>,
    alive: Vec<bool>,
    /// Triangles per vertex. Lists of other vertices are not updated when a
    /// triangle dies, so readers skip dead entries.
    incident: Vec<Vec<u32>>,
    /// Counts the changes of a vertex; `REMOVED` once it is merged away.
    version: Vec<u32>,
    locked: Vec<bool>,
    /// On an edge that does not have exactly two triangles.
    rim: Vec<bool>,
    /// Surface area a vertex stands for, the weight of its colour and normal.
    weight: Vec<f64>,
    /// Input vertices a vertex stands for, itself included.
    absorbed: Vec<u32>,
    colors: Option<Vec<[f64; 3]>>,
    normals: Option<Vec<[f64; 3]>>,
    merged: Vec<bool>,
    mark: Vec<u32>,
    stamp: u32,
    neighbours: Vec<u32>,
    heap: BinaryHeap<Candidate>,
    collapses: u64,
}

impl Simplifier {
    fn new(mesh: &MeshGeometry, tolerance: f64, locked: &[bool]) -> Self {
        let count = mesh.vertices.len();
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        for vertex in &mesh.vertices {
            for axis in 0..3 {
                min[axis] = min[axis].min(vertex[axis]);
                max[axis] = max[axis].max(vertex[axis]);
            }
        }
        let origin: [f64; 3] = if count == 0 {
            [0.0; 3]
        } else {
            std::array::from_fn(|axis| min[axis] + (max[axis] - min[axis]) * 0.5)
        };
        let position = mesh
            .vertices
            .iter()
            .map(|vertex| subtract(*vertex, origin))
            .collect::<Vec<_>>();
        let triangles = mesh
            .triangles
            .iter()
            .copied()
            .filter(|[a, b, c]| a != b && b != c && a != c)
            .collect::<Vec<_>>();

        let mut quadric = vec![Quadric::default(); count];
        let mut weight = vec![0.0; count];
        let mut incident = vec![Vec::new(); count];
        let mut face_normals = Vec::with_capacity(triangles.len());
        let mut edges = Vec::with_capacity(triangles.len() * 3);
        for (triangle, corners) in triangles.iter().enumerate() {
            let [a, b, c] = corners.map(|index| position[index as usize]);
            let normal = cross(subtract(b, a), subtract(c, a));
            let doubled_area = dot(normal, normal).sqrt();
            let unit_normal =
                (doubled_area > 0.0).then(|| normal.map(|value| value / doubled_area));
            for (corner, vertex) in corners.iter().enumerate() {
                let vertex = *vertex as usize;
                incident[vertex].push(triangle as u32);
                weight[vertex] += doubled_area / 6.0;
                if let Some(normal) = unit_normal {
                    quadric[vertex].add_plane(normal, -dot(normal, a));
                }
                let next = corners[(corner + 1) % 3];
                let key = (u64::from(corners[corner].min(next)) << 32)
                    | u64::from(corners[corner].max(next));
                edges.push((key, triangle as u32));
            }
            face_normals.push(unit_normal);
        }

        // Sorting puts the triangles of one edge side by side.
        edges.sort_unstable();
        let mut rim = vec![false; count];
        let mut unique = Vec::with_capacity(edges.len() / 2);
        for run in edges.chunk_by(|a, b| a.0 == b.0) {
            let a = (run[0].0 >> 32) as usize;
            let b = (run[0].0 & 0xffff_ffff) as usize;
            unique.push((a as u32, b as u32));
            if run.len() == 2 {
                continue;
            }
            rim[a] = true;
            rim[b] = true;
            // An open edge adds a plane through itself, square to its
            // triangle. Sliding along the rim stays free, leaving it does not.
            if let (1, Some(normal)) = (run.len(), face_normals[run[0].1 as usize]) {
                let across = cross(subtract(position[b], position[a]), normal);
                let length = dot(across, across).sqrt();
                if length > 0.0 {
                    let across = across.map(|value| value / length);
                    let offset = -dot(across, position[a]);
                    quadric[a].add_plane(across, offset);
                    quadric[b].add_plane(across, offset);
                }
            }
        }
        drop(edges);

        let as_f64 = |values: [f32; 3]| values.map(f64::from);
        let mut state = Self {
            tolerance_squared: tolerance * tolerance,
            origin,
            position,
            world: mesh.vertices.clone(),
            quadric,
            alive: vec![true; triangles.len()],
            triangles,
            reference: face_normals
                .iter()
                .map(|normal| normal.unwrap_or_default().map(|value| value as f32))
                .collect(),
            incident,
            version: vec![0; count],
            locked: if locked.is_empty() {
                vec![false; count]
            } else {
                locked.to_vec()
            },
            rim,
            weight,
            absorbed: vec![1; count],
            colors: mesh.colors.as_ref().map(|colors| {
                colors
                    .iter()
                    .map(|rgb| rgb.map(f64::from))
                    .collect::<Vec<_>>()
            }),
            normals: mesh
                .normals
                .as_ref()
                .map(|normals| normals.iter().map(|normal| as_f64(*normal)).collect()),
            merged: vec![false; count],
            mark: vec![0; count],
            stamp: 0,
            neighbours: Vec::new(),
            heap: BinaryHeap::with_capacity(unique.len()),
            collapses: 0,
        };
        for (a, b) in unique {
            state.push(a, b);
        }
        state
    }

    /// Cost and placement of merging the two ends of an edge, or `None` when
    /// that is not allowed: both ends locked, or the error past the tolerance.
    fn evaluate(&self, a: u32, b: u32) -> Option<(f64, Placement)> {
        let (locked_a, locked_b) = (self.locked[a as usize], self.locked[b as usize]);
        if locked_a && locked_b {
            return None;
        }
        let mut quadric = self.quadric[a as usize];
        quadric.add(&self.quadric[b as usize]);
        let (at_a, at_b) = (self.position[a as usize], self.position[b as usize]);
        let (cost, placement) = if locked_a {
            (quadric.error(at_a), Placement::Keep(a))
        } else if locked_b {
            (quadric.error(at_b), Placement::Keep(b))
        } else {
            let (error_a, error_b) = (quadric.error(at_a), quadric.error(at_b));
            let mut best = if error_a < error_b {
                (error_a, Placement::Keep(a))
            } else {
                (error_b, Placement::Keep(b))
            };
            let gain = MOVE_GAIN * self.tolerance_squared;
            let edge = subtract(at_b, at_a);
            // Best place on the edge itself: always well defined, and enough
            // to follow a curved surface.
            let bend = dot(edge, quadric.apply(edge));
            if bend > 0.0 {
                let slope = dot(edge, quadric.apply(at_a)) + dot(edge, quadric.b);
                let along = -slope / bend;
                if along > 0.0 && along < 1.0 {
                    let position = std::array::from_fn(|axis| at_a[axis] + edge[axis] * along);
                    let error = quadric.error(position);
                    if error < best.0 - gain {
                        best = (error, Placement::Move(position));
                    }
                }
            }
            // Best place anywhere, accepted near the edge only: it rebuilds
            // a corner that the edge cuts off.
            if let Some(position) = quadric.minimum() {
                let from_middle: [f64; 3] =
                    std::array::from_fn(|axis| position[axis] - (at_a[axis] + at_b[axis]) * 0.5);
                if dot(from_middle, from_middle) <= dot(edge, edge) {
                    let error = quadric.error(position);
                    if error < best.0 - gain {
                        best = (error, Placement::Move(position));
                    }
                }
            }
            best
        };
        // Rounding can leave a tiny negative sum on a flat surface.
        let cost = cost.max(0.0);
        (cost <= self.tolerance_squared).then_some((cost, placement))
    }

    fn push(&mut self, a: u32, b: u32) {
        if let Some((cost, _)) = self.evaluate(a, b) {
            let (a, b) = (a.min(b), a.max(b));
            let edge = subtract(self.position[b as usize], self.position[a as usize]);
            self.heap.push(Candidate {
                cost: if cost <= FLAT_COST * self.tolerance_squared {
                    0.0
                } else {
                    cost
                },
                size: self.absorbed[a as usize].saturating_add(self.absorbed[b as usize]),
                length: dot(edge, edge) as f32,
                a,
                b,
                version_a: self.version[a as usize],
                version_b: self.version[b as usize],
            });
        }
    }

    fn queue_all_edges(&mut self) {
        for vertex in 0..self.version.len() as u32 {
            if self.version[vertex as usize] == REMOVED {
                continue;
            }
            let stamp = self.next_stamp();
            let mut neighbours = std::mem::take(&mut self.neighbours);
            neighbours.clear();
            for triangle in &self.incident[vertex as usize] {
                if !self.alive[*triangle as usize] {
                    continue;
                }
                for other in self.triangles[*triangle as usize] {
                    // Each edge once, from its lower end.
                    if other > vertex && self.mark[other as usize] != stamp {
                        self.mark[other as usize] = stamp;
                        neighbours.push(other);
                    }
                }
            }
            for neighbour in &neighbours {
                self.push(vertex, *neighbour);
            }
            self.neighbours = neighbours;
        }
    }

    fn next_stamp(&mut self) -> u32 {
        // Two values per use: "seen" and "seen from both sides".
        if self.stamp >= u32::MAX - 4 {
            self.mark.fill(0);
            self.stamp = 0;
        }
        self.stamp += 2;
        self.stamp
    }

    fn has_triangle(&self, vertex: u32, first: u32, second: u32) -> bool {
        self.incident[vertex as usize].iter().any(|triangle| {
            let corners = self.triangles[*triangle as usize];
            self.alive[*triangle as usize] && corners.contains(&first) && corners.contains(&second)
        })
    }

    /// Whether merging `remove` into `keep` leaves a surface: no edge with
    /// more than two triangles, no two triangles on the same corners, no
    /// sheet pinched together in one vertex.
    fn keeps_surface(&mut self, keep: u32, remove: u32) -> bool {
        let near = self.next_stamp();
        let both = near + 1;
        for triangle in &self.incident[keep as usize] {
            if self.alive[*triangle as usize] {
                for vertex in self.triangles[*triangle as usize] {
                    self.mark[vertex as usize] = near;
                }
            }
        }
        let mut shared = 0;
        let mut opposite = [REMOVED; 2];
        let mut common = 0;
        for triangle in &self.incident[remove as usize] {
            if !self.alive[*triangle as usize] {
                continue;
            }
            let corners = self.triangles[*triangle as usize];
            let on_edge = corners.contains(&keep);
            if on_edge && shared == 2 {
                return false;
            }
            for vertex in corners {
                if vertex == keep || vertex == remove {
                    continue;
                }
                if on_edge {
                    opposite[shared] = vertex;
                }
                if self.mark[vertex as usize] == near {
                    self.mark[vertex as usize] = both;
                    common += 1;
                }
            }
            shared += usize::from(on_edge);
        }
        // Every vertex next to both ends must sit on a triangle of the edge;
        // any other one would get an edge with too many triangles.
        if shared == 0 || common != shared {
            return false;
        }
        if shared == 2 {
            // An inner edge between two rim vertices would join the rims.
            if self.rim[keep as usize] && self.rim[remove as usize] {
                return false;
            }
            // Both ends already span a triangle with the two opposite
            // vertices: the merge would lay those two on top of each other.
            if self.has_triangle(remove, opposite[0], opposite[1])
                && self.has_triangle(keep, opposite[0], opposite[1])
            {
                return false;
            }
        }
        true
    }

    /// Whether every triangle that stays keeps a usable shape and roughly
    /// its direction when `vertex` moves to `target`.
    fn keeps_normals(&self, vertex: u32, other: u32, target: [f64; 3]) -> bool {
        self.incident[vertex as usize].iter().all(|triangle| {
            let corners = self.triangles[*triangle as usize];
            // Triangles on the collapsing edge disappear.
            if !self.alive[*triangle as usize] || corners.contains(&other) {
                return true;
            }
            let before = corners.map(|index| self.position[index as usize]);
            let mut after = before;
            for (corner, index) in corners.iter().enumerate() {
                if *index == vertex {
                    after[corner] = target;
                }
            }
            let (old, new) = (doubled_area_normal(before), doubled_area_normal(after));
            let (old_squared, new_squared) = (dot(old, old), dot(new, new));
            // A needle may only stay where there was one, and not get thinner.
            let old_sine = corner_sine_squared(before, old_squared);
            let new_sine = corner_sine_squared(after, new_squared);
            if new_sine < MIN_CORNER_SINE_SQUARED
                && (new_sine < old_sine || new_sine <= MIN_AREA_SINE_SQUARED)
            {
                return false;
            }
            // Against the normal the triangle had in the input as well as
            // the one it has now: many small turns must not add up to a
            // triangle that faces the other way.
            let reference = self.reference[*triangle as usize].map(f64::from);
            let length = new_squared.sqrt();
            (old_squared == 0.0 || dot(old, new) >= MIN_NORMAL_COSINE * old_squared.sqrt() * length)
                && (reference == [0.0; 3] || dot(reference, new) >= MIN_TOTAL_COSINE * length)
        })
    }

    /// Merge the ends of an edge when the surface allows it.
    fn collapse(&mut self, a: u32, b: u32) {
        let Some((_, placement)) = self.evaluate(a, b) else {
            return;
        };
        let (keep, remove, target) = match placement {
            Placement::Keep(vertex) if vertex == a => (a, b, self.position[a as usize]),
            Placement::Keep(_) => (b, a, self.position[b as usize]),
            Placement::Move(position) => (b, a, position),
        };
        if !self.keeps_surface(keep, remove)
            || !self.keeps_normals(remove, keep, target)
            || (matches!(placement, Placement::Move(_))
                && !self.keeps_normals(keep, remove, target))
        {
            return;
        }

        let (keep_index, remove_index) = (keep as usize, remove as usize);
        if matches!(placement, Placement::Move(_)) {
            self.position[keep_index] = target;
            self.world[keep_index] = std::array::from_fn(|axis| target[axis] + self.origin[axis]);
        }
        let absorbed = self.quadric[remove_index];
        self.quadric[keep_index].add(&absorbed);
        if !self.locked[keep_index] {
            let total = self.weight[keep_index] + self.weight[remove_index];
            let share = if total > 0.0 {
                self.weight[remove_index] / total
            } else {
                0.5
            };
            for values in [&mut self.colors, &mut self.normals].into_iter().flatten() {
                let absorbed = values[remove_index];
                for (value, other) in values[keep_index].iter_mut().zip(absorbed) {
                    *value += (other - *value) * share;
                }
            }
            self.merged[keep_index] = true;
        }
        self.weight[keep_index] += self.weight[remove_index];
        self.absorbed[keep_index] =
            self.absorbed[keep_index].saturating_add(self.absorbed[remove_index]);
        self.rim[keep_index] |= self.rim[remove_index];

        for triangle in std::mem::take(&mut self.incident[remove_index]) {
            if !self.alive[triangle as usize] {
                continue;
            }
            let corners = &mut self.triangles[triangle as usize];
            if corners.contains(&keep) {
                self.alive[triangle as usize] = false;
            } else {
                for index in corners.iter_mut() {
                    if *index == remove {
                        *index = keep;
                    }
                }
                self.incident[keep_index].push(triangle);
            }
        }
        let alive = &self.alive;
        self.incident[keep_index].retain(|triangle| alive[*triangle as usize]);
        self.version[remove_index] = REMOVED;
        self.version[keep_index] += 1;
        self.collapses += 1;

        // Every edge of the merged vertex has a new cost.
        let stamp = self.next_stamp();
        let mut neighbours = std::mem::take(&mut self.neighbours);
        neighbours.clear();
        for triangle in &self.incident[keep_index] {
            for vertex in self.triangles[*triangle as usize] {
                if vertex != keep && self.mark[vertex as usize] != stamp {
                    self.mark[vertex as usize] = stamp;
                    neighbours.push(vertex);
                }
            }
        }
        for neighbour in &neighbours {
            self.push(keep, *neighbour);
        }
        self.neighbours = neighbours;
    }

    fn finish(self, mesh: &MeshGeometry) -> SimplifiedMesh {
        let mut used = vec![false; self.position.len()];
        for (triangle, corners) in self.triangles.iter().enumerate() {
            if self.alive[triangle] {
                for index in corners {
                    used[*index as usize] = true;
                }
            }
        }
        // Input order, so the result does not depend on the collapse order.
        let mut renumbered = vec![REMOVED; used.len()];
        let mut result = SimplifiedMesh {
            mesh: MeshGeometry {
                colors: mesh.colors.as_ref().map(|_| Vec::new()),
                normals: mesh.normals.as_ref().map(|_| Vec::new()),
                ..MeshGeometry::default()
            },
            collapses: self.collapses,
            ..SimplifiedMesh::default()
        };
        for vertex in (0..used.len()).filter(|vertex| used[*vertex]) {
            renumbered[vertex] = result.mesh.vertices.len() as u32;
            result.source_vertices.push(vertex as u32);
            result.mesh.vertices.push(self.world[vertex]);
            if let (Some(colors), Some(mixed)) = (&mut result.mesh.colors, &self.colors) {
                colors.push(mixed[vertex].map(|value| value.round().clamp(0.0, 255.0) as u8));
            }
            if let (Some(normals), Some(mixed), Some(input)) =
                (&mut result.mesh.normals, &self.normals, &mesh.normals)
            {
                let length = dot(mixed[vertex], mixed[vertex]).sqrt();
                // Opposite normals can cancel; the surviving vertex then
                // keeps its own.
                normals.push(if self.merged[vertex] && length > 1e-9 {
                    mixed[vertex].map(|value| (value / length) as f32)
                } else {
                    input[vertex]
                });
            }
        }
        result.mesh.triangles = self
            .triangles
            .iter()
            .enumerate()
            .filter(|(triangle, _)| self.alive[*triangle])
            .map(|(_, corners)| corners.map(|index| renumbered[index as usize]))
            .collect();
        result
    }
}

fn subtract(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Normal of a triangle with the length of twice its area.
fn doubled_area_normal([a, b, c]: [[f64; 3]; 3]) -> [f64; 3] {
    cross(subtract(b, a), subtract(c, a))
}

/// Squared sine of the sharpest corner of a triangle, the one between its
/// two longest sides, from the squared length of `doubled_area_normal`.
fn corner_sine_squared([a, b, c]: [[f64; 3]; 3], doubled_area_squared: f64) -> f64 {
    let sides = [subtract(b, a), subtract(c, b), subtract(a, c)].map(|side| dot(side, side));
    let shortest = sides.iter().copied().fold(f64::INFINITY, f64::min);
    let longest = sides.iter().copied().fold(0.0, f64::max);
    let middle = sides.iter().sum::<f64>() - shortest - longest;
    if middle > 0.0 {
        doubled_area_squared / (longest * middle)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{mesh_deviation, mesh_topology, open_boundary_vertices};

    /// Closed axis-aligned box of `cells` quads along every side, with
    /// welded vertices and outward faces.
    fn cube(origin: [f64; 3], size: f64, cells: u32) -> MeshGeometry {
        let mut result = MeshGeometry::default();
        let mut index = std::collections::HashMap::<[u32; 3], u32>::new();
        let mut vertex = |result: &mut MeshGeometry, lattice: [u32; 3]| {
            *index.entry(lattice).or_insert_with(|| {
                result.vertices.push(std::array::from_fn(|axis| {
                    origin[axis] + size * f64::from(lattice[axis]) / f64::from(cells)
                }));
                result.vertices.len() as u32 - 1
            })
        };
        for axis in 0..3 {
            let (u_axis, v_axis) = ((axis + 1) % 3, (axis + 2) % 3);
            for side in [0, cells] {
                for u in 0..cells {
                    for v in 0..cells {
                        let corner = |du: u32, dv: u32| {
                            let mut lattice = [0; 3];
                            lattice[axis] = side;
                            lattice[u_axis] = u + du;
                            lattice[v_axis] = v + dv;
                            lattice
                        };
                        let quad = [corner(0, 0), corner(1, 0), corner(1, 1), corner(0, 1)]
                            .map(|lattice| vertex(&mut result, lattice));
                        // u x v points along +axis; the low side faces the other way.
                        if side == 0 {
                            result.triangles.push([quad[0], quad[2], quad[1]]);
                            result.triangles.push([quad[0], quad[3], quad[2]]);
                        } else {
                            result.triangles.push([quad[0], quad[1], quad[2]]);
                            result.triangles.push([quad[0], quad[2], quad[3]]);
                        }
                    }
                }
            }
        }
        result
    }

    /// Closed sphere from rings of latitude, with one vertex at each pole.
    fn sphere(center: [f64; 3], radius: f64, rings: u32, segments: u32) -> MeshGeometry {
        let mut result = MeshGeometry::default();
        result
            .vertices
            .push([center[0], center[1], center[2] + radius]);
        for ring in 1..rings {
            let polar = std::f64::consts::PI * f64::from(ring) / f64::from(rings);
            for segment in 0..segments {
                let around = std::f64::consts::TAU * f64::from(segment) / f64::from(segments);
                result.vertices.push([
                    center[0] + radius * polar.sin() * around.cos(),
                    center[1] + radius * polar.sin() * around.sin(),
                    center[2] + radius * polar.cos(),
                ]);
            }
        }
        result
            .vertices
            .push([center[0], center[1], center[2] - radius]);
        let south = result.vertices.len() as u32 - 1;
        let at = |ring: u32, segment: u32| 1 + (ring - 1) * segments + segment % segments;
        for segment in 0..segments {
            result
                .triangles
                .push([0, at(1, segment), at(1, segment + 1)]);
            result
                .triangles
                .push([south, at(rings - 1, segment + 1), at(rings - 1, segment)]);
            for ring in 1..rings - 1 {
                let quad = [
                    at(ring, segment),
                    at(ring + 1, segment),
                    at(ring + 1, segment + 1),
                    at(ring, segment + 1),
                ];
                result.triangles.push([quad[0], quad[1], quad[2]]);
                result.triangles.push([quad[0], quad[2], quad[3]]);
            }
        }
        result
    }

    /// Flat sheet of `cells` quads along x and y at height `origin[2]`,
    /// facing up. `keep` decides per quad whether it is part of the sheet.
    fn plane(
        origin: [f64; 3],
        size: f64,
        cells: u32,
        keep: impl Fn(u32, u32) -> bool,
    ) -> MeshGeometry {
        let mut result = MeshGeometry::default();
        for y in 0..=cells {
            for x in 0..=cells {
                result.vertices.push([
                    origin[0] + size * f64::from(x) / f64::from(cells),
                    origin[1] + size * f64::from(y) / f64::from(cells),
                    origin[2],
                ]);
            }
        }
        let at = |x: u32, y: u32| y * (cells + 1) + x;
        for y in 0..cells {
            for x in 0..cells {
                if keep(x, y) {
                    result
                        .triangles
                        .push([at(x, y), at(x + 1, y), at(x + 1, y + 1)]);
                    result
                        .triangles
                        .push([at(x, y), at(x + 1, y + 1), at(x, y + 1)]);
                }
            }
        }
        result
    }

    fn face_normal(mesh: &MeshGeometry, face: [u32; 3]) -> [f64; 3] {
        let [a, b, c] = face.map(|index| mesh.vertices[index as usize]);
        cross(subtract(b, a), subtract(c, a))
    }

    fn area(mesh: &MeshGeometry) -> f64 {
        mesh.triangles
            .iter()
            .map(|face| {
                let normal = face_normal(mesh, *face);
                dot(normal, normal).sqrt() * 0.5
            })
            .sum()
    }

    /// Volume enclosed by a closed mesh with outward faces.
    fn volume(mesh: &MeshGeometry) -> f64 {
        let base = mesh.vertices[0];
        mesh.triangles
            .iter()
            .map(|face| {
                let [a, b, c] = face.map(|index| subtract(mesh.vertices[index as usize], base));
                dot(a, cross(b, c)) / 6.0
            })
            .sum()
    }

    /// Whether the triangles around every vertex form one fan: no two sheets
    /// that only touch in a point.
    fn fans_are_single(mesh: &MeshGeometry) -> bool {
        let mut links = vec![Vec::<[u32; 2]>::new(); mesh.vertices.len()];
        for face in &mesh.triangles {
            for corner in 0..3 {
                links[face[corner] as usize].push([face[(corner + 1) % 3], face[(corner + 2) % 3]]);
            }
        }
        links.iter().all(|link| {
            // Walk from the first triangle to every one that shares a spoke.
            let mut reached = vec![false; link.len()];
            let mut pending = vec![0];
            while let Some(current) = pending.pop() {
                if current >= link.len() || std::mem::replace(&mut reached[current], true) {
                    continue;
                }
                for (other, spokes) in link.iter().enumerate() {
                    if !reached[other] && spokes.iter().any(|spoke| link[current].contains(spoke)) {
                        pending.push(other);
                    }
                }
            }
            reached.iter().all(|flag| *flag)
        })
    }

    #[test]
    fn narrow_band_is_not_cut_through() {
        // A band of 1 mm wide along half a circle. Its short cross edges are
        // the only cheap ones, and collapsing one in the middle would leave
        // two pieces hanging together in a single vertex.
        let segments = 18_u32;
        let mut input = MeshGeometry::default();
        for step in 0..=segments {
            let angle = std::f64::consts::PI * f64::from(step) / f64::from(segments);
            for radius in [1.0, 1.001] {
                input
                    .vertices
                    .push([radius * angle.cos(), radius * angle.sin(), 0.0]);
            }
        }
        for step in 0..segments {
            let (inner, outer) = (step * 2, step * 2 + 1);
            input.triangles.push([inner, outer, outer + 2]);
            input.triangles.push([inner, outer + 2, inner + 2]);
        }
        let result = simplify_mesh(&input, 0.01, &[]).unwrap();
        let output = &result.mesh;
        // At most the two ends lose their cross edge.
        assert!(output.triangles.len() >= input.triangles.len() - 2);
        assert!(fans_are_single(output));
        let topology = mesh_topology(output);
        assert_eq!((topology.components, topology.euler), (1, 1));
        assert_eq!(topology.non_manifold_edges, 0);
    }

    #[test]
    fn flat_faces_collapse_and_shape_stays() {
        // Survey-sized coordinates: the quadrics must not lose their digits.
        let origin = [207_000.25, 474_000.5, 10.0];
        let input = cube(origin, 2.0, 16);
        assert_eq!(input.triangles.len(), 6 * 16 * 16 * 2);
        let result = simplify_mesh(&input, 0.003, &[]).unwrap();
        let output = &result.mesh;
        assert!(
            output.triangles.len() * 20 <= input.triangles.len(),
            "{} triangles left",
            output.triangles.len()
        );
        assert_eq!(
            result.collapses as usize,
            input.vertices.len() - output.vertices.len()
        );

        let topology = mesh_topology(output);
        assert_eq!((topology.open_edges, topology.non_manifold_edges), (0, 0));
        assert_eq!((topology.components, topology.euler), (1, 2));
        assert!(fans_are_single(output));
        // The box is still the same box: its faces, its volume, its corners.
        assert!((area(output) - 24.0).abs() < 1e-6);
        assert!((volume(output) - 8.0).abs() < 1e-6);
        let deviation = mesh_deviation(output, &input.vertices);
        assert!(deviation.max < 1e-6, "{deviation:?}");
        for corner in 0..8_u32 {
            let expected: [f64; 3] =
                std::array::from_fn(|axis| origin[axis] + 2.0 * f64::from((corner >> axis) & 1));
            assert!(output.vertices.contains(&expected), "{expected:?}");
        }
        // Flat areas keep input positions exactly.
        for (vertex, source) in output.vertices.iter().zip(&result.source_vertices) {
            assert_eq!(*vertex, input.vertices[*source as usize]);
        }
    }

    #[test]
    fn tolerance_zero_returns_the_input() {
        let mut input = cube([0.0; 3], 1.0, 3);
        input.vertices.push([9.0, 9.0, 9.0]);
        input.colors = Some(vec![[1, 2, 3]; input.vertices.len()]);
        let result = simplify_mesh(&input, 0.0, &[]).unwrap();
        assert_eq!(result.mesh.vertices, input.vertices);
        assert_eq!(result.mesh.triangles, input.triangles);
        assert_eq!(result.mesh.colors, input.colors);
        assert_eq!(result.collapses, 0);
        assert_eq!(
            result.source_vertices,
            (0..input.vertices.len() as u32).collect::<Vec<_>>()
        );

        // A tolerance too small for any collapse keeps every used vertex and
        // drops the one no triangle uses.
        let mut curved = sphere([0.0; 3], 1.0, 8, 12);
        let used = curved.vertices.len();
        curved.vertices.push([9.0, 9.0, 9.0]);
        let result = simplify_mesh(&curved, 1e-9, &[]).unwrap();
        assert_eq!(result.collapses, 0);
        assert_eq!(result.mesh.vertices, curved.vertices[..used]);
        assert_eq!(result.mesh.triangles, curved.triangles);
    }

    #[test]
    fn locked_vertices_do_not_move() {
        let mut input = cube([-1.0, -1.0, 0.0], 2.0, 10);
        let count = input.vertices.len();
        input.colors = Some(
            (0..count)
                .map(|index| [(index % 251) as u8, (index % 7) as u8 * 30, 200])
                .collect(),
        );
        // Lock the top face, as a tile locks the layer it shares.
        let locked = input
            .vertices
            .iter()
            .map(|vertex| vertex[2] == 2.0)
            .collect::<Vec<_>>();
        let locked_count = locked.iter().filter(|flag| **flag).count();
        assert_eq!(locked_count, 11 * 11);
        let result = simplify_mesh(&input, 0.002, &locked).unwrap();
        let output = &result.mesh;
        assert!(output.vertices.len() < count / 2);

        let mut found = 0;
        for (index, source) in result.source_vertices.iter().enumerate() {
            if locked[*source as usize] {
                found += 1;
                assert_eq!(output.vertices[index], input.vertices[*source as usize]);
                assert_eq!(
                    output.colors.as_ref().unwrap()[index],
                    input.colors.as_ref().unwrap()[*source as usize]
                );
            }
        }
        assert_eq!(found, locked_count);
        // The locked face keeps all of its triangles, the rest is reduced.
        let on_top = output
            .triangles
            .iter()
            .filter(|face| {
                face.iter()
                    .all(|index| output.vertices[*index as usize][2] == 2.0)
            })
            .count();
        assert_eq!(on_top, 10 * 10 * 2);
        let topology = mesh_topology(output);
        assert_eq!((topology.open_edges, topology.euler), (0, 2));
        assert!((volume(output) - 8.0).abs() < 1e-9);

        // With everything locked nothing can collapse.
        let all = vec![true; count];
        let result = simplify_mesh(&input, 0.5, &all).unwrap();
        assert_eq!(result.collapses, 0);
        assert_eq!(result.mesh.triangles, input.triangles);
    }

    #[test]
    fn curved_surface_stays_within_the_tolerance() {
        let input = sphere([207_000.0, 474_000.0, 10.0], 1.0, 64, 128);
        let tolerance = 0.004;
        let result = simplify_mesh(&input, tolerance, &[]).unwrap();
        let output = &result.mesh;
        assert!(
            output.triangles.len() * 2 <= input.triangles.len(),
            "{} of {} triangles left",
            output.triangles.len(),
            input.triangles.len()
        );
        let topology = mesh_topology(output);
        assert_eq!((topology.open_edges, topology.non_manifold_edges), (0, 0));
        assert_eq!((topology.components, topology.euler), (1, 2));
        assert!(fans_are_single(output));
        // Input vertices measured against the result, and result vertices
        // against the true sphere.
        let deviation = mesh_deviation(output, &input.vertices);
        assert!(deviation.max <= tolerance, "{deviation:?}");
        for vertex in &output.vertices {
            let offset = subtract(*vertex, [207_000.0, 474_000.0, 10.0]);
            let radius = dot(offset, offset).sqrt();
            assert!((radius - 1.0).abs() <= tolerance, "{radius}");
        }
        // No triangle was folded inward.
        for face in &output.triangles {
            let outward = subtract(
                output.vertices[face[0] as usize],
                [207_000.0, 474_000.0, 10.0],
            );
            assert!(dot(face_normal(output, *face), outward) > 0.0);
        }

        // A tighter tolerance keeps more triangles.
        let finer = simplify_mesh(&input, 0.0005, &[]).unwrap();
        assert!(finer.mesh.triangles.len() > output.triangles.len());
        assert!(mesh_deviation(&finer.mesh, &input.vertices).max <= 0.0005);
    }

    /// Deterministic generator so the tests need no data from outside.
    fn xorshift(state: &mut u64) -> f64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        (*state >> 11) as f64 / (1_u64 << 53) as f64
    }

    #[test]
    fn rough_wall_is_reduced_within_the_tolerance() {
        // A wall as a reconstruction delivers it: 2 cm triangles with a
        // ripple of a few tenths of a millimetre, not exactly flat.
        let mut input = plane([207_000.0, 474_000.0, 0.0], 1.2, 60, |_, _| true);
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        for vertex in &mut input.vertices {
            vertex[2] += (xorshift(&mut state) - 0.5) * 0.0004;
        }
        let tolerance = 0.003;
        let result = simplify_mesh(&input, tolerance, &[]).unwrap();
        let output = &result.mesh;
        // Less than on an exactly flat wall: the tilted planes of the small
        // triangles count against every vertex that takes them over.
        assert!(
            output.triangles.len() * 10 <= input.triangles.len(),
            "{} triangles left",
            output.triangles.len()
        );
        let deviation = mesh_deviation(output, &input.vertices);
        assert!(deviation.max <= tolerance, "{deviation:?}");
        assert!((area(output) - 1.44).abs() < 1e-3);
        for face in &output.triangles {
            assert!(face_normal(output, *face)[2] > 0.0);
        }
        let topology = mesh_topology(output);
        assert_eq!((topology.components, topology.euler), (1, 1));
    }

    #[test]
    fn open_rim_keeps_its_outline_without_locks() {
        let input = plane([5.0, -3.0, 1.5], 4.0, 20, |_, _| true);
        let result = simplify_mesh(&input, 0.002, &[]).unwrap();
        let output = &result.mesh;
        assert!(output.triangles.len() <= 8, "{}", output.triangles.len());
        assert!((area(output) - 16.0).abs() < 1e-9);
        for corner in [
            [5.0, -3.0, 1.5],
            [9.0, -3.0, 1.5],
            [9.0, 1.0, 1.5],
            [5.0, 1.0, 1.5],
        ] {
            assert!(output.vertices.contains(&corner));
        }
        let topology = mesh_topology(output);
        assert_eq!((topology.components, topology.euler), (1, 1));
        assert_eq!(topology.non_manifold_edges, 0);

        // Locking the rim, as the pass over a whole mesh does, keeps every
        // rim vertex and still empties the inside, but for the odd vertex
        // that keeps the triangles on the short rim edges from becoming
        // needles.
        let rim = open_boundary_vertices(&input);
        let result = simplify_mesh(&input, 0.002, &rim).unwrap();
        let kept_rim = result
            .source_vertices
            .iter()
            .filter(|source| rim[**source as usize])
            .count();
        assert_eq!(kept_rim, 80);
        assert!(
            result.mesh.vertices.len() <= 82,
            "{}",
            result.mesh.vertices.len()
        );
        assert!(sharpest_corner_sine(&result.mesh) >= MIN_CORNER_SINE_SQUARED.sqrt());
        assert!((area(&result.mesh) - 16.0).abs() < 1e-9);
    }

    #[test]
    fn collapses_never_fold_a_flat_sheet() {
        // An L-shaped sheet: on a flat surface every collapse is free, so
        // only the normal guard keeps triangles from flipping over the
        // inner corner.
        let input = plane([0.0; 3], 3.0, 18, |x, y| x < 9 || y < 9);
        let expected = 9.0 * 0.75;
        assert!((area(&input) - expected).abs() < 1e-9);
        let result = simplify_mesh(&input, 0.005, &[]).unwrap();
        let output = &result.mesh;
        assert!(output.triangles.len() <= 16, "{}", output.triangles.len());
        for face in &output.triangles {
            let normal = face_normal(output, *face);
            assert!(normal[2] > 0.0 && normal[0] == 0.0 && normal[1] == 0.0);
        }
        // Same area with every triangle facing up: nothing overlaps and
        // nothing reaches into the missing quarter.
        assert!((area(output) - expected).abs() < 1e-9);
        assert!(output.vertices.contains(&[1.5, 1.5, 0.0]));
        assert!(!output
            .vertices
            .iter()
            .any(|vertex| vertex[0] > 1.5 && vertex[1] > 1.5));
    }

    #[test]
    fn colours_and_normals_are_averaged() {
        // Blue rim, red inside, normals all up.
        let mut input = plane([0.0; 3], 2.0, 8, |_, _| true);
        let on_rim = |vertex: &[f64; 3]| {
            vertex[0] == 0.0 || vertex[0] == 2.0 || vertex[1] == 0.0 || vertex[1] == 2.0
        };
        input.colors = Some(
            input
                .vertices
                .iter()
                .map(|vertex| {
                    if on_rim(vertex) {
                        [0, 0, 255]
                    } else {
                        [255, 0, 0]
                    }
                })
                .collect(),
        );
        input.normals = Some(vec![[0.0, 0.0, 1.0]; input.vertices.len()]);
        let result = simplify_mesh(&input, 0.001, &[]).unwrap();
        let colors = result.mesh.colors.as_ref().unwrap();
        let normals = result.mesh.normals.as_ref().unwrap();
        assert_eq!(colors.len(), result.mesh.vertices.len());
        assert_eq!(normals.len(), result.mesh.vertices.len());
        // Only rim vertices are left, so the red of the inside went into them.
        assert!(result.mesh.vertices.iter().all(on_rim));
        assert!(colors.iter().any(|rgb| rgb[0] > 0 && rgb[2] > 0));
        // Mixing red and blue keeps their sum and leaves green empty.
        for rgb in colors {
            assert_eq!(rgb[1], 0);
            assert!((i32::from(rgb[0]) + i32::from(rgb[2]) - 255).abs() <= 1);
        }
        for normal in normals {
            assert_eq!(*normal, [0.0, 0.0, 1.0]);
        }

        // One colour stays that colour, and normals that turn with the
        // surface come back with unit length.
        let mut ball = sphere([0.0; 3], 1.0, 32, 64);
        ball.colors = Some(vec![[90, 120, 150]; ball.vertices.len()]);
        ball.normals = Some(
            ball.vertices
                .iter()
                .map(|vertex| vertex.map(|value| value as f32))
                .collect(),
        );
        let result = simplify_mesh(&ball, 0.01, &[]).unwrap();
        assert!(result.collapses > 0);
        for rgb in result.mesh.colors.as_ref().unwrap() {
            assert_eq!(*rgb, [90, 120, 150]);
        }
        for (vertex, normal) in result
            .mesh
            .vertices
            .iter()
            .zip(result.mesh.normals.as_ref().unwrap())
        {
            let normal = normal.map(f64::from);
            assert!((dot(normal, normal).sqrt() - 1.0).abs() < 1e-5);
            assert!(dot(normal, *vertex) > 0.95);
        }
    }

    #[test]
    fn smallest_closed_shapes_are_left_alone() {
        // A tetrahedron has no edge that can go without flattening it.
        let tetrahedron = MeshGeometry {
            vertices: vec![
                [0.0, 0.0, 0.0],
                [0.001, 0.0, 0.0],
                [0.0, 0.001, 0.0],
                [0.0, 0.0, 0.001],
            ],
            triangles: vec![[0, 2, 1], [0, 1, 3], [1, 2, 3], [0, 3, 2]],
            ..MeshGeometry::default()
        };
        let result = simplify_mesh(&tetrahedron, 1.0, &[]).unwrap();
        assert_eq!(result.collapses, 0);
        assert_eq!(result.mesh.triangles.len(), 4);

        // Three large fins on one very short edge, with only one end of that
        // edge free to move. Collapsing it is cheap, and it is the one edge
        // that may not go: it has more than two triangles.
        let fins = MeshGeometry {
            vertices: vec![
                [0.0, 0.0, 0.0],
                [0.001, 0.0, 0.0],
                [0.0005, 1.0, 0.0],
                [0.0005, -1.0, 0.0],
                [0.0005, 0.0, 1.0],
            ],
            triangles: vec![[0, 1, 2], [1, 0, 3], [0, 1, 4]],
            ..MeshGeometry::default()
        };
        let result = simplify_mesh(&fins, 0.01, &[false, true, true, true, true]).unwrap();
        assert_eq!(result.collapses, 0);
        assert_eq!(mesh_topology(&result.mesh).non_manifold_edges, 1);

        // Triangles that repeat a vertex are dropped, an empty mesh passes.
        let repeated = MeshGeometry {
            vertices: tetrahedron.vertices.clone(),
            triangles: vec![[0, 1, 1], [0, 2, 1]],
            ..MeshGeometry::default()
        };
        let result = simplify_mesh(&repeated, 1e-6, &[]).unwrap();
        assert_eq!(result.mesh.triangles, vec![[0, 2, 1]]);
        assert_eq!(result.source_vertices, vec![0, 1, 2]);
        let empty = simplify_mesh(&MeshGeometry::default(), 0.01, &[]).unwrap();
        assert!(empty.mesh.vertices.is_empty() && empty.mesh.triangles.is_empty());
    }

    #[test]
    fn progress_reports_and_cancels() {
        let input = cube([0.0; 3], 1.0, 24);
        let mut calls = Vec::new();
        let result = simplify_mesh_progress(&input, 0.001, &[], &mut |done, total| {
            calls.push((done, total));
            Ok(())
        })
        .unwrap();
        assert!(calls.len() >= 3);
        assert_eq!(calls[0].0, 0);
        assert!(calls.iter().all(|(done, total)| done <= total));
        assert!(calls.windows(2).all(|pair| pair[0].0 <= pair[1].0));
        let last = calls[calls.len() - 1];
        assert_eq!(last.0, last.1);

        // The same input gives the same result, run after run.
        let again = simplify_mesh(&input, 0.001, &[]).unwrap();
        assert_eq!(again.mesh.vertices, result.mesh.vertices);
        assert_eq!(again.mesh.triangles, result.mesh.triangles);

        let mut seen = 0;
        let cancelled = simplify_mesh_progress(&input, 0.001, &[], &mut |_, _| {
            seen += 1;
            if seen == 2 {
                Err(LoadError::Cancelled)
            } else {
                Ok(())
            }
        });
        assert!(matches!(cancelled, Err(LoadError::Cancelled)));
    }

    #[test]
    fn invalid_input_is_refused() {
        let good = cube([0.0; 3], 1.0, 2);
        let count = good.vertices.len();
        let copy = |change: &dyn Fn(&mut MeshGeometry)| {
            let mut mesh = cube([0.0; 3], 1.0, 2);
            change(&mut mesh);
            mesh
        };
        for tolerance in [-0.001, f64::NAN, f64::INFINITY] {
            assert!(simplify_mesh(&good, tolerance, &[]).is_err());
        }
        assert!(simplify_mesh(&good, 0.01, &vec![false; count - 1]).is_err());
        let invalid = [
            copy(&|mesh| mesh.triangles.push([0, 1, count as u32])),
            copy(&|mesh| mesh.vertices[3][1] = f64::NAN),
            copy(&|mesh| mesh.colors = Some(vec![[0, 0, 0]; count - 1])),
            copy(&|mesh| mesh.normals = Some(vec![[0.0, 0.0, 1.0]; count + 1])),
            copy(&|mesh| mesh.normals = Some(vec![[0.0, f32::NAN, 1.0]; count])),
        ];
        for mesh in &invalid {
            // Also with tolerance zero: nothing invalid is handed back.
            for tolerance in [0.0, 0.01] {
                assert!(matches!(
                    simplify_mesh(mesh, tolerance, &[]),
                    Err(LoadError::InvalidData(_))
                ));
            }
        }
    }

    /// Closed ball made by pushing a tessellated cube out to a sphere, with
    /// every vertex moved along its radius by up to `noise`. Its triangles
    /// have one size all over, where the rings of `sphere` get thin at the
    /// poles.
    fn noisy_ball(cells: u32, noise: f64, seed: u64) -> MeshGeometry {
        let mut ball = cube([-1.0; 3], 2.0, cells);
        let mut state = seed;
        for vertex in &mut ball.vertices {
            let radius = 1.0 + (xorshift(&mut state) * 2.0 - 1.0) * noise;
            let length = dot(*vertex, *vertex).sqrt();
            *vertex = vertex.map(|value| value * radius / length);
        }
        ball
    }

    /// Sine of the sharpest corner of any triangle.
    fn sharpest_corner_sine(mesh: &MeshGeometry) -> f64 {
        mesh.triangles
            .iter()
            .map(|face| {
                let corners = face.map(|index| mesh.vertices[index as usize]);
                let normal = doubled_area_normal(corners);
                corner_sine_squared(corners, dot(normal, normal)).sqrt()
            })
            .fold(f64::INFINITY, f64::min)
    }

    #[test]
    fn corner_sine_of_known_triangles() {
        let sine = |corners: [[f64; 3]; 3]| {
            let normal = doubled_area_normal(corners);
            corner_sine_squared(corners, dot(normal, normal)).sqrt()
        };
        // Sides 3, 4 and 5: the sharpest corner lies opposite the 3.
        let right = [[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [0.0, 3.0, 0.0]];
        assert!((sine(right) - 0.6).abs() < 1e-12);
        // The same triangle from each of its corners.
        assert!((sine([right[1], right[2], right[0]]) - 0.6).abs() < 1e-12);
        assert!((sine([right[2], right[0], right[1]]) - 0.6).abs() < 1e-12);
        // A needle of 1 mm on a base of 1 m, and corners on one line.
        let needle = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.5, 0.001, 0.0]];
        assert!((sine(needle) - 0.002).abs() < 1e-5);
        assert_eq!(sine([[0.0; 3], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]]), 0.0);
        assert_eq!(sine([[1.0; 3], [1.0; 3], [1.0; 3]]), 0.0);
    }

    #[test]
    fn noisy_surface_keeps_its_side_and_gets_no_needles() {
        // 2 mm of noise on triangles of 2.5 cm, simplified well past the
        // noise. Many small turns of one triangle used to add up until it
        // faced inward, mostly as a needle a millimetre high.
        for seed in [0x2545_f491_4f6c_dd1d_u64, 99, 12_345] {
            let input = noisy_ball(64, 0.002, seed);
            let tolerance = 0.01;
            let result = simplify_mesh(&input, tolerance, &[]).unwrap();
            let output = &result.mesh;
            assert!(
                output.triangles.len() * 3 <= input.triangles.len(),
                "{} triangles left",
                output.triangles.len()
            );
            for face in &output.triangles {
                let normal = face_normal(output, *face);
                // The ball is centred on zero: its corners point outward.
                let [a, b, c] = face.map(|index| output.vertices[index as usize]);
                let outward: [f64; 3] = std::array::from_fn(|axis| a[axis] + b[axis] + c[axis]);
                let cosine =
                    dot(normal, outward) / (dot(normal, normal) * dot(outward, outward)).sqrt();
                assert!(cosine > 0.0, "triangle {face:?} faces inward: {cosine}");
            }
            assert!(sharpest_corner_sine(&input) > 0.4);
            assert!(sharpest_corner_sine(output) >= MIN_CORNER_SINE_SQUARED.sqrt() * (1.0 - 1e-9));
            let topology = mesh_topology(output);
            assert_eq!((topology.open_edges, topology.non_manifold_edges), (0, 0));
            assert_eq!((topology.components, topology.euler), (1, 2));
            let deviation = mesh_deviation(output, &input.vertices);
            assert!(deviation.max <= tolerance, "{deviation:?}");
        }
    }

    /// Simplify and return every progress report with the result.
    fn with_reports(
        input: &MeshGeometry,
        tolerance: f64,
        locked: &[bool],
    ) -> (Vec<(u64, u64)>, SimplifiedMesh) {
        let mut reports = Vec::new();
        let result = simplify_mesh_progress(input, tolerance, locked, &mut |done, total| {
            reports.push((done, total));
            Ok(())
        })
        .unwrap();
        (reports, result)
    }

    #[test]
    fn flat_areas_take_work_in_proportion_to_their_size() {
        // On a flat surface every collapse is free. Handed out by vertex
        // number, one vertex took in the whole area and each of its
        // collapses walked all its triangles: sixteen times the triangles
        // cost sixty times the work.
        let small = plane([0.0; 3], 0.8, 40, |_, _| true);
        let large = plane([0.0; 3], 3.2, 160, |_, _| true);
        // A wall that is flat but for rounding: upright, turned 30 degrees,
        // at survey coordinates.
        let mut wall = plane([0.0; 3], 3.2, 160, |_, _| true);
        let (sine, cosine) = 30.0_f64.to_radians().sin_cos();
        for vertex in &mut wall.vertices {
            *vertex = [
                207_000.25 + vertex[0] * cosine,
                474_000.5 + vertex[0] * sine,
                10.0 + vertex[1],
            ];
        }
        let rim = open_boundary_vertices(&wall);
        let closed = cube([207_000.25, 474_000.5, 10.0], 2.0, 64);
        let free: &[bool] = &[];
        let mut work = Vec::new();
        for (input, locked, most_left) in [
            (&small, free, 2),
            (&large, free, 2),
            (&wall, free, 8),
            (&wall, &rim[..], 800),
            (&closed, free, 12),
        ] {
            let (reports, result) = with_reports(input, 0.003, locked);
            let left = result.mesh.triangles.len();
            assert!(left <= most_left, "{left} triangles left");
            let handled = reports[reports.len() - 1].0;
            // A handful of candidates per triangle, whatever the size.
            assert!(
                handled <= input.triangles.len() as u64 * 8,
                "{handled} candidates for {} triangles",
                input.triangles.len()
            );
            // The first estimate is of the right order, and the share that
            // is reported never goes back.
            assert!(handled <= reports[0].1 * 8, "{:?}", reports[0]);
            for pair in reports.windows(2) {
                assert!(
                    u128::from(pair[0].0) * u128::from(pair[1].1)
                        <= u128::from(pair[1].0) * u128::from(pair[0].1),
                    "{pair:?}"
                );
            }
            work.push(handled);
        }
        // Sixteen times the triangles, about sixteen times the work.
        assert!(work[1] <= work[0] * 20, "{work:?}");
    }
}
