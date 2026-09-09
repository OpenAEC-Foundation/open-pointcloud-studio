//! View-frustum culling and LOD node selection.
//!
//! The previous selector scored nodes on distance and projected size alone.
//! Distance is direction-blind, so a node directly behind the camera scores
//! exactly like one in front of it and gets loaded and uploaded to the GPU.
//! The camera's `target` and `aspect` were already being sent from the
//! frontend and discarded — rustc reported both as never read. They are what
//! this module needs to build the six planes.

use crate::octree::{Node, Octree};
use crate::types::Bounds;

#[derive(Debug, Clone, Copy)]
pub struct Camera {
    pub position: [f64; 3],
    pub target: [f64; 3],
    pub up: [f64; 3],
    /// Vertical field of view, degrees.
    pub fov: f64,
    pub aspect: f64,
    pub near: f64,
    pub far: f64,
    pub screen_height: f64,
}

/// A plane as ax + by + cz + d = 0, normal pointing into the frustum.
#[derive(Debug, Clone, Copy)]
struct Plane {
    n: [f64; 3],
    d: f64,
}

impl Plane {
    /// Signed distance from the plane to the farthest corner of `b` along the
    /// normal. Negative means the whole box is outside.
    fn max_signed_distance(&self, b: &Bounds) -> f64 {
        // Pick, per axis, whichever bound is farther along the normal.
        let p = [
            if self.n[0] >= 0.0 { b.max[0] } else { b.min[0] },
            if self.n[1] >= 0.0 { b.max[1] } else { b.min[1] },
            if self.n[2] >= 0.0 { b.max[2] } else { b.min[2] },
        ];
        self.n[0] * p[0] + self.n[1] * p[1] + self.n[2] * p[2] + self.d
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn norm(v: [f64; 3]) -> [f64; 3] {
    let l = dot(v, v).sqrt();
    if l <= f64::MIN_POSITIVE {
        [0.0, 0.0, 1.0]
    } else {
        [v[0] / l, v[1] / l, v[2] / l]
    }
}
fn scale(v: [f64; 3], s: f64) -> [f64; 3] {
    [v[0] * s, v[1] * s, v[2] * s]
}
fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub struct Frustum {
    planes: [Plane; 6],
}

impl Frustum {
    pub fn from_camera(cam: &Camera) -> Self {
        let forward = norm(sub(cam.target, cam.position));
        let right = norm(cross(forward, cam.up));
        let up = cross(right, forward);

        let half_v = (cam.fov.to_radians() * 0.5).tan();
        let half_h = half_v * cam.aspect;

        let plane_from = |n: [f64; 3], point: [f64; 3]| Plane {
            n,
            d: -dot(n, point),
        };

        let near_c = add(cam.position, scale(forward, cam.near));
        let far_c = add(cam.position, scale(forward, cam.far));

        // Side planes pass through the eye. Operand order matters: each cross
        // product must come out pointing *into* the frustum, so that
        // `max_signed_distance >= 0` means "at least partly inside".
        let left_edge = add(forward, scale(right, -half_h));
        let right_edge = add(forward, scale(right, half_h));
        let bottom_edge = add(forward, scale(up, -half_v));
        let top_edge = add(forward, scale(up, half_v));

        let left_n = norm(cross(left_edge, up));
        let right_n = norm(cross(up, right_edge));
        let bottom_n = norm(cross(right, bottom_edge));
        let top_n = norm(cross(top_edge, right));

        Self {
            planes: [
                plane_from(forward, near_c),
                plane_from(scale(forward, -1.0), far_c),
                plane_from(left_n, cam.position),
                plane_from(right_n, cam.position),
                plane_from(bottom_n, cam.position),
                plane_from(top_n, cam.position),
            ],
        }
    }

    /// False when the box is entirely outside at least one plane.
    pub fn intersects(&self, b: &Bounds) -> bool {
        self.planes.iter().all(|p| p.max_signed_distance(b) >= 0.0)
    }
}

/// A node chosen for rendering.
#[derive(Debug, Clone)]
pub struct VisibleNode {
    pub index: u32,
    pub screen_size: f64,
    pub render_count: u32,
}

/// Selection statistics, so culling can be shown to be doing something
/// rather than asserted.
#[derive(Debug, Clone, Default)]
pub struct SelectStats {
    pub visited: usize,
    pub culled_frustum: usize,
    pub culled_too_small: usize,
    pub selected: usize,
    pub points: u64,
}

/// Choose nodes to draw: inside the frustum, large enough on screen, and
/// within the point budget, best-first.
pub fn select_visible(
    tree: &Octree,
    cam: &Camera,
    point_budget: u64,
    min_screen_size: f64,
) -> (Vec<VisibleNode>, SelectStats) {
    let mut stats = SelectStats::default();
    if tree.nodes.is_empty() {
        return (Vec::new(), stats);
    }

    let frustum = Frustum::from_camera(cam);
    let proj = cam.screen_height / (2.0 * (cam.fov.to_radians() * 0.5).tan());

    let screen_size_of = |node: &Node| -> f64 {
        let c = node.bounds.center();
        let d = sub(c, cam.position);
        let dist = dot(d, d).sqrt();
        if dist <= f64::MIN_POSITIVE {
            return f64::MAX;
        }
        node.bounds.max_extent() / dist * proj
    };

    // Traverse best-first so the budget is spent on what matters most.
    let mut candidates: Vec<VisibleNode> = Vec::new();
    let mut stack: Vec<u32> = vec![0];

    while let Some(idx) = stack.pop() {
        let node = &tree.nodes[idx as usize];
        stats.visited += 1;

        if !frustum.intersects(&node.bounds) {
            stats.culled_frustum += 1;
            continue;
        }

        let size = screen_size_of(node);
        if size < min_screen_size {
            stats.culled_too_small += 1;
            continue;
        }

        if node.render_count() > 0 {
            candidates.push(VisibleNode {
                index: idx,
                screen_size: size,
                render_count: node.render_count(),
            });
        }

        for &c in &node.children {
            if c != u32::MAX {
                stack.push(c);
            }
        }
    }

    candidates.sort_by(|a, b| {
        b.screen_size
            .partial_cmp(&a.screen_size)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut out = Vec::new();
    let mut total: u64 = 0;
    for c in candidates {
        let next = total + c.render_count as u64;
        if next > point_budget && !out.is_empty() {
            break;
        }
        total = next;
        out.push(c);
    }

    stats.selected = out.len();
    stats.points = total;
    (out, stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam_looking_down_neg_z() -> Camera {
        Camera {
            position: [0.0, 0.0, 10.0],
            target: [0.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            fov: 60.0,
            aspect: 16.0 / 9.0,
            near: 0.1,
            far: 1000.0,
            screen_height: 800.0,
        }
    }

    fn box_at(x: f64, y: f64, z: f64, half: f64) -> Bounds {
        Bounds {
            min: [x - half, y - half, z - half],
            max: [x + half, y + half, z + half],
        }
    }

    #[test]
    fn box_in_front_is_visible() {
        let f = Frustum::from_camera(&cam_looking_down_neg_z());
        assert!(f.intersects(&box_at(0.0, 0.0, 0.0, 1.0)));
    }

    #[test]
    fn box_behind_the_camera_is_culled() {
        // This is the case the old distance-only test could not express: the
        // box is the same distance away as one in front, just the wrong side.
        let f = Frustum::from_camera(&cam_looking_down_neg_z());
        assert!(!f.intersects(&box_at(0.0, 0.0, 20.0, 1.0)));
    }

    #[test]
    fn box_far_off_axis_is_culled() {
        let f = Frustum::from_camera(&cam_looking_down_neg_z());
        assert!(!f.intersects(&box_at(500.0, 0.0, 0.0, 1.0)));
    }

    #[test]
    fn box_beyond_the_far_plane_is_culled() {
        let f = Frustum::from_camera(&cam_looking_down_neg_z());
        assert!(!f.intersects(&box_at(0.0, 0.0, -2000.0, 1.0)));
    }

    #[test]
    fn a_box_straddling_the_edge_is_kept() {
        // Conservative culling: partially visible must stay visible.
        let f = Frustum::from_camera(&cam_looking_down_neg_z());
        assert!(f.intersects(&box_at(0.0, 0.0, 0.0, 1000.0)));
    }
}
