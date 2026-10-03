//! Measured quality of a triangle mesh: how far points lie from its surface
//! and whether the surface is closed.

use std::collections::HashMap;

use rayon::prelude::*;

use super::obj_mesh::MeshGeometry;

/// Triangles in a leaf of the box tree. Fewer make the tree deeper without
/// saving a distance calculation.
const LEAF_TRIANGLES: usize = 8;
/// From this many triangles on, the two halves of a branch are sorted on
/// separate threads.
const PARALLEL_TRIANGLES: usize = 8_192;

/// Distance between points and a mesh surface, in the unit of the mesh.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MeshDeviation {
    pub mean: f64,
    /// 95 % of the points lie at most this far from the surface.
    pub p95: f64,
    pub max: f64,
    /// Points that were measured. Zero when the mesh has no usable triangle.
    pub samples: u64,
}

/// How the triangles of a mesh hang together.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MeshTopology {
    /// Edges with a single triangle: the rims of holes and of open sheets.
    pub open_edges: u64,
    /// Edges shared by more than two triangles.
    pub non_manifold_edges: u64,
    /// Groups of triangles connected through shared vertices.
    pub components: u32,
    /// Vertices minus edges plus triangles: 2 for every closed surface
    /// without handles.
    pub euler: i64,
}

/// Exact distance from every point to the nearest triangle. The triangles sit
/// in a tree of boxes, so a point only meets the triangles near it, whatever
/// the size and the extent of the mesh. Non-finite points and unusable
/// triangles are left out.
pub fn mesh_deviation(mesh: &MeshGeometry, points: &[[f64; 3]]) -> MeshDeviation {
    let Some(index) = TriangleIndex::build(mesh) else {
        return MeshDeviation::default();
    };
    let mut distances = points
        .par_iter()
        .with_min_len(1024)
        .filter(|point| point.iter().all(|value| value.is_finite()))
        // One search stack per worker instead of one per point.
        .map_init(Vec::new, |pending, point| index.distance(*point, pending))
        .collect::<Vec<f64>>();
    if distances.is_empty() {
        return MeshDeviation::default();
    }
    distances.sort_unstable_by(f64::total_cmp);
    let samples = distances.len();
    // Nearest rank: the smallest measured distance that covers 95 %.
    let rank = (samples * 95).div_ceil(100).max(1);
    MeshDeviation {
        mean: distances.iter().sum::<f64>() / samples as f64,
        p95: distances[rank - 1],
        max: distances[samples - 1],
        samples: samples as u64,
    }
}

/// Count open and non-manifold edges, connected components and the Euler
/// characteristic. Vertices are compared by index, so a mesh whose triangles
/// do not share vertices reports every edge as open. Triangles that repeat a
/// vertex or point outside the vertex list are not counted.
pub fn mesh_topology(mesh: &MeshGeometry) -> MeshTopology {
    topology(mesh.vertices.len(), &mesh.triangles)
}

/// The same figures with the vertices that lie at the same position counted
/// as one. A file can hold a vertex more than once, one per face corner or
/// one per material colour, and by index every such seam would count as a
/// rim. Positions are compared exactly, as the STL and DXF readers do when
/// they join the corners of their faces.
pub fn mesh_topology_by_position(mesh: &MeshGeometry) -> MeshTopology {
    let mut places = HashMap::<[u64; 3], u32>::with_capacity(mesh.vertices.len());
    let place = mesh
        .vertices
        .iter()
        .map(|xyz| {
            let next = places.len() as u32;
            // Adding zero makes minus zero and zero the same place.
            *places
                .entry(xyz.map(|value| (value + 0.0).to_bits()))
                .or_insert(next)
        })
        .collect::<Vec<u32>>();
    let triangles = mesh
        .triangles
        .iter()
        .filter(|triangle| triangle.iter().all(|index| (*index as usize) < place.len()))
        .map(|triangle| triangle.map(|index| place[index as usize]))
        .collect::<Vec<_>>();
    topology(places.len(), &triangles)
}

fn topology(vertex_count: usize, triangles: &[[u32; 3]]) -> MeshTopology {
    let edges = sorted_edges(vertex_count, triangles);
    let mut topology = MeshTopology::default();
    let mut distinct_edges = 0_i64;
    for run in edges.chunk_by(|a, b| a == b) {
        distinct_edges += 1;
        match run.len() {
            1 => topology.open_edges += 1,
            2 => {}
            _ => topology.non_manifold_edges += 1,
        }
    }

    // Union-find over the vertices, joined by every triangle.
    let mut parent = (0..vertex_count as u32).collect::<Vec<_>>();
    let mut used = vec![false; vertex_count];
    let mut faces = 0_i64;
    for [a, b, c] in valid_triangles(vertex_count, triangles) {
        faces += 1;
        for vertex in [a, b, c] {
            used[vertex as usize] = true;
        }
        let root = find(&mut parent, a);
        for other in [b, c] {
            let other = find(&mut parent, other);
            parent[other as usize] = root;
        }
    }
    let mut vertices = 0_i64;
    for vertex in 0..vertex_count as u32 {
        if used[vertex as usize] {
            vertices += 1;
            if find(&mut parent, vertex) == vertex {
                topology.components += 1;
            }
        }
    }
    topology.euler = vertices - distinct_edges + faces;
    topology
}

/// Mark the vertices on an edge that does not have exactly two triangles:
/// the rim of the surface. Simplification locks these to keep the outline.
pub fn open_boundary_vertices(mesh: &MeshGeometry) -> Vec<bool> {
    let mut boundary = vec![false; mesh.vertices.len()];
    for run in sorted_edges(mesh.vertices.len(), &mesh.triangles).chunk_by(|a, b| a == b) {
        if run.len() != 2 {
            boundary[(run[0] >> 32) as usize] = true;
            boundary[(run[0] & 0xffff_ffff) as usize] = true;
        }
    }
    boundary
}

fn valid_triangles(
    vertex_count: usize,
    triangles: &[[u32; 3]],
) -> impl Iterator<Item = [u32; 3]> + '_ {
    triangles.iter().copied().filter(move |[a, b, c]| {
        a != b
            && b != c
            && a != c
            && [a, b, c]
                .iter()
                .all(|index| (**index as usize) < vertex_count)
    })
}

/// One key per triangle side, lower index in the high half, sorted so equal
/// edges are neighbours. Sorting keeps this in one allocation where a hash
/// map would need several times the memory on a large mesh.
fn sorted_edges(vertex_count: usize, triangles: &[[u32; 3]]) -> Vec<u64> {
    let mut edges = Vec::with_capacity(triangles.len() * 3);
    for [a, b, c] in valid_triangles(vertex_count, triangles) {
        for (from, to) in [(a, b), (b, c), (c, a)] {
            edges.push((u64::from(from.min(to)) << 32) | u64::from(from.max(to)));
        }
    }
    edges.sort_unstable();
    edges
}

fn find(parent: &mut [u32], mut vertex: u32) -> u32 {
    while parent[vertex as usize] != vertex {
        let next = parent[vertex as usize];
        parent[vertex as usize] = parent[next as usize];
        vertex = next;
    }
    vertex
}

/// Triangles in a tree of boxes. Every node halves its triangles by their
/// place along the longest side of the node, so the depth follows from their
/// number alone: a fragment far away or a few very large triangles do not
/// make the lookup of the others slower. A lookup walks down nearest box
/// first and skips every box that lies farther away than the best triangle
/// found so far.
struct TriangleIndex {
    /// Lower corner of the mesh. Everything is stored relative to it so the
    /// distances keep their precision at survey coordinates.
    origin: [f64; 3],
    vertices: Vec<[f64; 3]>,
    /// In the order of the leaves.
    triangles: Vec<[u32; 3]>,
    /// The root first. The first child of a node follows it directly.
    nodes: Vec<Node>,
}

struct Node {
    low: [f64; 3],
    high: [f64; 3],
    /// The triangles below this node.
    first: u32,
    end: u32,
    /// The second child, zero for a leaf.
    second: u32,
}

/// A node still to be searched and its squared distance to the point.
type Pending = (f64, u32);

impl TriangleIndex {
    fn build(mesh: &MeshGeometry) -> Option<Self> {
        let triangles = mesh
            .triangles
            .iter()
            .copied()
            .filter(|triangle| {
                triangle.iter().all(|index| {
                    mesh.vertices
                        .get(*index as usize)
                        .is_some_and(|vertex| vertex.iter().all(|value| value.is_finite()))
                })
            })
            .collect::<Vec<_>>();
        if triangles.is_empty() || triangles.len() >= u32::MAX as usize / 2 {
            return None;
        }
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        for index in triangles.iter().flatten() {
            let vertex = mesh.vertices[*index as usize];
            for axis in 0..3 {
                min[axis] = min[axis].min(vertex[axis]);
                max[axis] = max[axis].max(vertex[axis]);
            }
        }
        if (0..3).any(|axis| !(max[axis] - min[axis]).is_finite()) {
            return None;
        }
        let vertices = mesh
            .vertices
            .iter()
            .map(|vertex| std::array::from_fn(|axis| vertex[axis] - min[axis]))
            .collect::<Vec<[f64; 3]>>();

        // A triangle is sorted by the centre of its box.
        let mut placed = triangles
            .into_iter()
            .map(|triangle| {
                let (low, high) = triangle_box(&vertices, triangle);
                let centre = std::array::from_fn(|axis| (low[axis] + high[axis]) * 0.5);
                (centre, triangle)
            })
            .collect::<Vec<_>>();
        sort_into_halves(&mut placed);
        let mut index = Self {
            origin: min,
            vertices,
            triangles: placed.into_iter().map(|(_, triangle)| triangle).collect(),
            nodes: Vec::new(),
        };
        index.add_node(0, index.triangles.len());
        Some(index)
    }

    /// Add the node for a run of triangles and everything below it.
    fn add_node(&mut self, first: usize, end: usize) -> usize {
        let node = self.nodes.len();
        let mut low = [f64::INFINITY; 3];
        let mut high = [f64::NEG_INFINITY; 3];
        self.nodes.push(Node {
            low,
            high,
            first: first as u32,
            end: end as u32,
            second: 0,
        });
        let mut grow = |other_low: [f64; 3], other_high: [f64; 3]| {
            for axis in 0..3 {
                low[axis] = low[axis].min(other_low[axis]);
                high[axis] = high[axis].max(other_high[axis]);
            }
        };
        if end - first <= LEAF_TRIANGLES {
            for triangle in &self.triangles[first..end] {
                let (triangle_low, triangle_high) = triangle_box(&self.vertices, *triangle);
                grow(triangle_low, triangle_high);
            }
        } else {
            // The same halves as `sort_into_halves` made.
            let middle = first + (end - first) / 2;
            let children = [self.add_node(first, middle), self.add_node(middle, end)];
            for child in children {
                grow(self.nodes[child].low, self.nodes[child].high);
            }
            self.nodes[node].second = children[1] as u32;
        }
        self.nodes[node].low = low;
        self.nodes[node].high = high;
        node
    }

    fn distance(&self, point: [f64; 3], pending: &mut Vec<Pending>) -> f64 {
        self.nearest(point, pending).0.sqrt()
    }

    /// Squared distance to the nearest triangle, and how many triangles were
    /// measured to find it.
    fn nearest(&self, point: [f64; 3], pending: &mut Vec<Pending>) -> (f64, usize) {
        let local: [f64; 3] = std::array::from_fn(|axis| point[axis] - self.origin[axis]);
        let mut best = f64::INFINITY;
        let mut measured = 0;
        pending.clear();
        pending.push((self.nodes[0].distance_squared(local), 0));
        while let Some((distance, node)) = pending.pop() {
            // Nothing inside a box is nearer than the box itself.
            if distance >= best {
                continue;
            }
            let node_index = node as usize;
            let node = &self.nodes[node_index];
            if node.second == 0 {
                let triangles = &self.triangles[node.first as usize..node.end as usize];
                measured += triangles.len();
                for [a, b, c] in triangles {
                    let distance = point_triangle_distance_squared(
                        local,
                        self.vertices[*a as usize],
                        self.vertices[*b as usize],
                        self.vertices[*c as usize],
                    );
                    if distance < best {
                        best = distance;
                    }
                }
                continue;
            }
            let mut children = [node_index as u32 + 1, node.second]
                .map(|child| (self.nodes[child as usize].distance_squared(local), child));
            // Farthest first onto the stack: the nearest child is searched
            // first, and what it finds rules out most of the other one.
            if children[0].0 < children[1].0 {
                children.swap(0, 1);
            }
            pending.extend(children.into_iter().filter(|child| child.0 < best));
        }
        (best, measured)
    }
}

impl Node {
    fn distance_squared(&self, point: [f64; 3]) -> f64 {
        (0..3)
            .map(|axis| {
                let outside = (self.low[axis] - point[axis]).max(point[axis] - self.high[axis]);
                outside.max(0.0).powi(2)
            })
            .sum()
    }
}

/// Order triangles so that the two halves of the slice lie on either side
/// along the longest side of the box around their centres, and the same for
/// each half, down to the size of a leaf.
fn sort_into_halves(placed: &mut [([f64; 3], [u32; 3])]) {
    let count = placed.len();
    if count <= LEAF_TRIANGLES {
        return;
    }
    let mut low = [f64::INFINITY; 3];
    let mut high = [f64::NEG_INFINITY; 3];
    for (centre, _) in placed.iter() {
        for axis in 0..3 {
            low[axis] = low[axis].min(centre[axis]);
            high[axis] = high[axis].max(centre[axis]);
        }
    }
    let mut axis = 0;
    for other in 1..3 {
        if high[other] - low[other] > high[axis] - low[axis] {
            axis = other;
        }
    }
    let middle = count / 2;
    placed.select_nth_unstable_by(middle, |a, b| a.0[axis].total_cmp(&b.0[axis]));
    let (first, second) = placed.split_at_mut(middle);
    if count >= PARALLEL_TRIANGLES {
        rayon::join(|| sort_into_halves(first), || sort_into_halves(second));
    } else {
        sort_into_halves(first);
        sort_into_halves(second);
    }
}

fn triangle_box(vertices: &[[f64; 3]], triangle: [u32; 3]) -> ([f64; 3], [f64; 3]) {
    let corners = triangle.map(|index| vertices[index as usize]);
    (
        std::array::from_fn(|axis| {
            corners
                .iter()
                .map(|c| c[axis])
                .fold(f64::INFINITY, f64::min)
        }),
        std::array::from_fn(|axis| {
            corners
                .iter()
                .map(|c| c[axis])
                .fold(f64::NEG_INFINITY, f64::max)
        }),
    )
}

fn subtract(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn point_segment_distance_squared(p: [f64; 3], a: [f64; 3], b: [f64; 3]) -> f64 {
    let ab = subtract(b, a);
    let ap = subtract(p, a);
    let length = dot(ab, ab);
    let t = if length > 0.0 {
        (dot(ap, ab) / length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let offset: [f64; 3] = std::array::from_fn(|axis| ap[axis] - ab[axis] * t);
    dot(offset, offset)
}

/// Squared distance to the nearest place on a triangle: its face, one of its
/// sides or one of its corners, whichever region the point projects into.
fn point_triangle_distance_squared(p: [f64; 3], a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> f64 {
    let ab = subtract(b, a);
    let ac = subtract(c, a);
    let ap = subtract(p, a);
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return dot(ap, ap);
    }
    let bp = subtract(p, b);
    let d3 = dot(ab, bp);
    let d4 = dot(ac, bp);
    if d3 >= 0.0 && d4 <= d3 {
        return dot(bp, bp);
    }
    let cp = subtract(p, c);
    let d5 = dot(ab, cp);
    let d6 = dot(ac, cp);
    if d6 >= 0.0 && d5 <= d6 {
        return dot(cp, cp);
    }
    let va = d3 * d6 - d5 * d4;
    let vb = d5 * d2 - d1 * d6;
    let vc = d1 * d4 - d3 * d2;
    let sum = va + vb + vc;
    if vc <= 0.0 || vb <= 0.0 || va <= 0.0 || sum <= 0.0 || !sum.is_finite() {
        // Outside the face, or a triangle without area: one of the sides is
        // nearest. Taking all three avoids dividing by a vanishing area.
        return point_segment_distance_squared(p, a, b)
            .min(point_segment_distance_squared(p, b, c))
            .min(point_segment_distance_squared(p, c, a));
    }
    let v = vb / sum;
    let w = vc / sum;
    let offset: [f64; 3] = std::array::from_fn(|axis| ap[axis] - ab[axis] * v - ac[axis] * w);
    dot(offset, offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mesh(vertices: Vec<[f64; 3]>, triangles: Vec<[u32; 3]>) -> MeshGeometry {
        MeshGeometry {
            vertices,
            triangles,
            ..MeshGeometry::default()
        }
    }

    fn unit_square() -> MeshGeometry {
        mesh(
            vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
            ],
            vec![[0, 1, 2], [0, 2, 3]],
        )
    }

    fn tetrahedron() -> MeshGeometry {
        mesh(
            vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
            ],
            vec![[0, 2, 1], [0, 1, 3], [1, 2, 3], [0, 3, 2]],
        )
    }

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

    /// Deterministic generator so the tests need no data from outside.
    fn xorshift(state: &mut u64) -> f64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        (*state >> 11) as f64 / (1_u64 << 53) as f64
    }

    fn brute_force(mesh: &MeshGeometry, point: [f64; 3]) -> f64 {
        mesh.triangles
            .iter()
            .map(|[a, b, c]| {
                point_triangle_distance_squared(
                    point,
                    mesh.vertices[*a as usize],
                    mesh.vertices[*b as usize],
                    mesh.vertices[*c as usize],
                )
            })
            .fold(f64::INFINITY, f64::min)
            .sqrt()
    }

    #[test]
    fn deviation_of_known_cases() {
        let square = unit_square();
        let above = (0..100)
            .map(|index| {
                [
                    f64::from(index % 10) / 10.0 + 0.05,
                    f64::from(index / 10) / 10.0 + 0.05,
                    0.005,
                ]
            })
            .collect::<Vec<_>>();
        let deviation = mesh_deviation(&square, &above);
        assert_eq!(deviation.samples, 100);
        for value in [deviation.mean, deviation.p95, deviation.max] {
            assert!((value - 0.005).abs() < 1e-12, "{deviation:?}");
        }

        // Beside the square the nearest place is its side, past a corner the
        // corner itself, and below it the face.
        let beside = mesh_deviation(&square, &[[1.25, 0.5, 0.0]]);
        assert!((beside.max - 0.25).abs() < 1e-12);
        let past_corner = mesh_deviation(&square, &[[1.3, 1.4, 0.0]]);
        assert!((past_corner.max - 0.5).abs() < 1e-12);
        let below = mesh_deviation(&square, &[[0.3, 0.6, -2.0]]);
        assert!((below.max - 2.0).abs() < 1e-12);
    }

    #[test]
    fn deviation_statistics_use_the_nearest_rank() {
        let square = unit_square();
        let points = (1..=100)
            .map(|index| [0.5, 0.5, f64::from(index) / 1000.0])
            .collect::<Vec<_>>();
        let deviation = mesh_deviation(&square, &points);
        assert!((deviation.mean - 0.0505).abs() < 1e-12);
        assert!((deviation.p95 - 0.095).abs() < 1e-12);
        assert!((deviation.max - 0.100).abs() < 1e-12);

        // A single outlier moves the maximum, not the 95th percentile.
        let mut with_outlier = vec![[0.5, 0.5, 0.001]; 99];
        with_outlier.push([0.5, 0.5, 3.0]);
        let deviation = mesh_deviation(&square, &with_outlier);
        assert!((deviation.p95 - 0.001).abs() < 1e-12);
        assert!((deviation.max - 3.0).abs() < 1e-12);
    }

    #[test]
    fn deviation_skips_unusable_input() {
        let square = unit_square();
        let deviation = mesh_deviation(
            &square,
            &[
                [0.5, 0.5, 0.25],
                [f64::NAN, 0.0, 0.0],
                [0.0, f64::INFINITY, 0.0],
            ],
        );
        assert_eq!(deviation.samples, 1);
        assert!((deviation.max - 0.25).abs() < 1e-12);
        assert_eq!(mesh_deviation(&square, &[]), MeshDeviation::default());
        assert_eq!(
            mesh_deviation(&MeshGeometry::default(), &[[0.0; 3]]),
            MeshDeviation::default()
        );
        // A triangle that points outside the vertex list is left out.
        let broken = mesh(square.vertices.clone(), vec![[0, 1, 9]]);
        assert_eq!(mesh_deviation(&broken, &[[0.0; 3]]).samples, 0);

        // A triangle without area still measures as the segment it covers.
        let sliver = mesh(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]],
            vec![[0, 1, 2]],
        );
        let deviation = mesh_deviation(&sliver, &[[1.5, 0.5, 0.0], [3.0, 0.0, 0.0]]);
        assert!((deviation.mean - 0.75).abs() < 1e-12, "{deviation:?}");
        let point = mesh(vec![[1.0, 2.0, 3.0]], vec![[0, 0, 0]]);
        assert!((mesh_deviation(&point, &[[1.0, 2.0, 5.0]]).max - 2.0).abs() < 1e-12);
    }

    #[test]
    fn lookup_matches_brute_force_near_and_far() {
        // Survey-sized coordinates: the result must not lose precision there.
        let center = [207_000.0, 474_000.0, 10.0];
        let ball = sphere(center, 1.0, 48, 96);
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut points = Vec::new();
        for _ in 0..4_000 {
            // Most points near the surface, some deep inside and far outside.
            let reach = match points.len() % 10 {
                0 => 0.05,
                1 => 8.0,
                _ => 1.0 + (xorshift(&mut state) - 0.5) * 0.02,
            };
            let polar = (2.0 * xorshift(&mut state) - 1.0).acos();
            let around = std::f64::consts::TAU * xorshift(&mut state);
            points.push([
                center[0] + reach * polar.sin() * around.cos(),
                center[1] + reach * polar.sin() * around.sin(),
                center[2] + reach * polar.cos(),
            ]);
        }
        let index = TriangleIndex::build(&ball).unwrap();
        assert_eq!(index.triangles.len(), ball.triangles.len());
        let mut pending = Vec::new();
        let mut sum = 0.0;
        for point in &points {
            let expected = brute_force(&ball, *point);
            let found = index.distance(*point, &mut pending);
            assert!(
                (found - expected).abs() < 1e-9,
                "{point:?}: {found} instead of {expected}"
            );
            sum += expected;
        }
        let deviation = mesh_deviation(&ball, &points);
        assert_eq!(deviation.samples, points.len() as u64);
        assert!((deviation.mean - sum / points.len() as f64).abs() < 1e-9);
        // The far points sit 7 m outside a sphere of 1 m.
        assert!((deviation.max - 7.0).abs() < 0.01);
    }

    /// Flat sheet of `cells` quads of 2 cm along x and y.
    fn sheet(cells: u32) -> MeshGeometry {
        let mut result = MeshGeometry::default();
        for y in 0..=cells {
            for x in 0..=cells {
                result
                    .vertices
                    .push([f64::from(x) * 0.02, f64::from(y) * 0.02, 0.0]);
            }
        }
        let at = |x: u32, y: u32| y * (cells + 1) + x;
        for y in 0..cells {
            for x in 0..cells {
                result
                    .triangles
                    .push([at(x, y), at(x + 1, y), at(x + 1, y + 1)]);
                result
                    .triangles
                    .push([at(x, y), at(x + 1, y + 1), at(x, y + 1)]);
            }
        }
        result
    }

    /// Mean and largest number of triangles a lookup measures.
    fn measured(index: &TriangleIndex, points: &[[f64; 3]]) -> (f64, usize) {
        let mut pending = Vec::new();
        let counts = points
            .iter()
            .map(|point| index.nearest(*point, &mut pending).1)
            .collect::<Vec<_>>();
        (
            counts.iter().sum::<usize>() as f64 / counts.len() as f64,
            counts.iter().copied().max().unwrap_or_default(),
        )
    }

    #[test]
    fn large_triangles_do_not_slow_the_lookup() {
        // Two wall-sized triangles between thousands of small ones: the
        // lookup stays exact, and a point at the small ones still meets only
        // a handful of them.
        let mut wall = cube([0.0; 3], 0.5, 24);
        let base = wall.vertices.len() as u32;
        wall.vertices.extend([
            [-20.0, -20.0, -1.0],
            [20.0, -20.0, -1.0],
            [20.0, 20.0, 15.0],
            [-20.0, 20.0, 15.0],
        ]);
        wall.triangles
            .extend([[base, base + 1, base + 2], [base, base + 2, base + 3]]);
        let index = TriangleIndex::build(&wall).unwrap();
        let mut pending = Vec::new();
        let mut state = 7_u64;
        for _ in 0..500 {
            let point = [
                xorshift(&mut state) * 44.0 - 22.0,
                xorshift(&mut state) * 44.0 - 22.0,
                xorshift(&mut state) * 20.0 - 3.0,
            ];
            let expected = brute_force(&wall, point);
            assert!((index.distance(point, &mut pending) - expected).abs() < 1e-9);
        }
        let on_top = (0..500)
            .map(|_| {
                [
                    xorshift(&mut state) * 0.5,
                    xorshift(&mut state) * 0.5,
                    0.501,
                ]
            })
            .collect::<Vec<_>>();
        let (mean, most) = measured(&index, &on_top);
        assert!(mean <= 8.0 * LEAF_TRIANGLES as f64, "{mean}");
        assert!(most <= 16 * LEAF_TRIANGLES, "{most}");
    }

    #[test]
    fn far_fragment_does_not_slow_the_lookup() {
        // One stray triangle three kilometres away stretches the box around
        // the mesh more than a thousand times. Points at the sheet must not
        // meet more triangles because of it.
        let alone = sheet(100);
        let mut stretched = sheet(100);
        let base = stretched.vertices.len() as u32;
        stretched.vertices.extend([
            [3_000.0, 0.0, 0.0],
            [3_000.02, 0.0, 0.0],
            [3_000.0, 0.02, 0.0],
        ]);
        stretched.triangles.push([base, base + 1, base + 2]);
        let mut state = 5_u64;
        let points = (0..2_000)
            .map(|_| {
                [
                    xorshift(&mut state) * 2.0,
                    xorshift(&mut state) * 2.0,
                    (xorshift(&mut state) - 0.5) * 0.01,
                ]
            })
            .collect::<Vec<_>>();
        let index = TriangleIndex::build(&stretched).unwrap();
        let (mean, most) = measured(&index, &points);
        let (mean_alone, _) = measured(&TriangleIndex::build(&alone).unwrap(), &points);
        // About one leaf per point, with or without the stray triangle.
        assert!(mean <= 2.0 * LEAF_TRIANGLES as f64, "{mean}");
        assert!(mean <= mean_alone * 1.5, "{mean} against {mean_alone}");
        assert!(most <= 8 * LEAF_TRIANGLES, "{most}");
        let mut pending = Vec::new();
        for point in points.iter().step_by(40) {
            let expected = brute_force(&stretched, *point);
            assert!((index.distance(*point, &mut pending) - expected).abs() < 1e-12);
        }
        // The stray triangle itself is found as well.
        let beside = mesh_deviation(&stretched, &[[3_000.01, 0.005, 0.25]]);
        assert!((beside.max - 0.25).abs() < 1e-9);
    }

    #[test]
    fn topology_of_known_shapes() {
        assert_eq!(
            mesh_topology(&tetrahedron()),
            MeshTopology {
                open_edges: 0,
                non_manifold_edges: 0,
                components: 1,
                euler: 2,
            }
        );
        let single = mesh(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            vec![[0, 1, 2]],
        );
        assert_eq!(
            mesh_topology(&single),
            MeshTopology {
                open_edges: 3,
                non_manifold_edges: 0,
                components: 1,
                euler: 1,
            }
        );
        let closed = mesh_topology(&cube([0.0; 3], 1.0, 4));
        assert_eq!((closed.open_edges, closed.non_manifold_edges), (0, 0));
        assert_eq!((closed.components, closed.euler), (1, 2));
        let ball = mesh_topology(&sphere([0.0; 3], 1.0, 12, 24));
        assert_eq!((ball.open_edges, ball.components, ball.euler), (0, 1, 2));
        assert_eq!(
            mesh_topology(&MeshGeometry::default()),
            MeshTopology::default()
        );
    }

    #[test]
    fn topology_counts_components_holes_and_shared_edges() {
        // Two separate boxes in one mesh.
        let mut pair = cube([0.0; 3], 1.0, 2);
        let second = cube([5.0, 0.0, 0.0], 1.0, 2);
        let base = pair.vertices.len() as u32;
        pair.vertices.extend(second.vertices);
        pair.triangles
            .extend(second.triangles.iter().map(|face| face.map(|v| v + base)));
        // An unused vertex is not a component.
        pair.vertices.push([9.0, 9.0, 9.0]);
        let topology = mesh_topology(&pair);
        assert_eq!((topology.components, topology.euler), (2, 4));
        assert_eq!(topology.open_edges, 0);

        // A box with one quad removed: a hole with four open edges.
        let mut opened = cube([0.0; 3], 1.0, 2);
        opened.triangles.truncate(opened.triangles.len() - 2);
        let topology = mesh_topology(&opened);
        assert_eq!((topology.open_edges, topology.euler), (4, 1));
        let rim = open_boundary_vertices(&opened);
        assert_eq!(rim.iter().filter(|on_rim| **on_rim).count(), 4);
        assert!(open_boundary_vertices(&cube([0.0; 3], 1.0, 2))
            .iter()
            .all(|on_rim| !on_rim));

        // Three triangles on one edge, plus two triangles that are skipped:
        // one repeats a vertex, one points outside the vertex list.
        let fins = mesh(
            vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, -1.0, 0.0],
                [0.0, 0.0, 1.0],
            ],
            vec![[0, 1, 2], [0, 1, 3], [0, 1, 4], [2, 2, 3], [0, 1, 7]],
        );
        let topology = mesh_topology(&fins);
        assert_eq!(topology.non_manifold_edges, 1);
        assert_eq!(topology.open_edges, 6);
        assert_eq!(topology.components, 1);
        assert_eq!(topology.euler, 5 - 7 + 3);
        assert!(open_boundary_vertices(&fins).iter().all(|on_rim| *on_rim));
    }

    #[test]
    fn vertices_at_one_position_count_as_one_when_asked() {
        let closed = MeshTopology {
            open_edges: 0,
            non_manifold_edges: 0,
            components: 1,
            euler: 2,
        };
        // A mesh that shares its vertices gives the same figures both ways.
        let shared = cube([0.0; 3], 1.0, 2);
        assert_eq!(mesh_topology(&shared), closed);
        assert_eq!(mesh_topology_by_position(&shared), closed);

        // The upper face gets vertices of its own, as a file reader does for
        // a face with another material colour: by index it is a loose lid
        // over an open box.
        let mut lid = cube([0.0; 3], 1.0, 1);
        let top = lid.triangles.len() - 2;
        let mut copies = std::collections::BTreeMap::new();
        for corner in 0..6 {
            let original = lid.triangles[top + corner / 3][corner % 3];
            assert_eq!(lid.vertices[original as usize][2], 1.0);
            let copy = *copies.entry(original).or_insert_with(|| {
                lid.vertices.push(lid.vertices[original as usize]);
                lid.vertices.len() as u32 - 1
            });
            lid.triangles[top + corner / 3][corner % 3] = copy;
        }
        assert_eq!(lid.vertices.len(), 12);
        let by_index = mesh_topology(&lid);
        assert_eq!((by_index.open_edges, by_index.components), (8, 2));
        assert_eq!(mesh_topology_by_position(&lid), closed);

        // One vertex per face corner: by index every triangle stands alone.
        let whole = tetrahedron();
        let mut loose = MeshGeometry::default();
        for triangle in &whole.triangles {
            let base = loose.vertices.len() as u32;
            loose
                .vertices
                .extend(triangle.map(|index| whole.vertices[index as usize]));
            loose.triangles.push([base, base + 1, base + 2]);
        }
        // Minus zero is the same place as zero.
        loose.vertices[0][0] = -0.0;
        let by_index = mesh_topology(&loose);
        assert_eq!((by_index.open_edges, by_index.components), (12, 4));
        assert_eq!(mesh_topology_by_position(&loose), closed);

        // What is open stays open: a box without its lid has four open edges
        // however its vertices are numbered. A triangle that points outside
        // the vertex list, and one whose corners fall together, do not count.
        let (&original, &copy) = copies.iter().next().unwrap();
        let mut opened = lid;
        opened.triangles.truncate(top);
        opened.triangles.extend([[0, 1, 99], [original, copy, 0]]);
        let topology = mesh_topology_by_position(&opened);
        assert_eq!((topology.open_edges, topology.components), (4, 1));
        assert_eq!(
            mesh_topology_by_position(&MeshGeometry::default()),
            MeshTopology::default()
        );
    }
}
