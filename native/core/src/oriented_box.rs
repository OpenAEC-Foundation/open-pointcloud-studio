//! A box turned about the vertical through its centre: the section box, so
//! that it can follow the walls of a building that does not stand along the
//! model axes.

use crate::Bounds;

/// A box given by its limits before it is turned and by a turn about the
/// vertical line through its centre. Its own X and Y axes follow the turn;
/// Z stays vertical. Without a turn it is the axis-aligned box `bounds`, and
/// every test on it is the plain test on those limits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrientedBox {
    /// The box before it is turned, in scene coordinates.
    pub bounds: Bounds,
    /// The turn in degrees, counter-clockwise as seen from above.
    pub rotation_degrees: f64,
}

impl From<Bounds> for OrientedBox {
    fn from(bounds: Bounds) -> Self {
        Self {
            bounds,
            rotation_degrees: 0.0,
        }
    }
}

/// An angle in degrees brought within -180 (not included) and 180.
pub fn normalized_degrees(degrees: f64) -> f64 {
    let turned = degrees.rem_euclid(360.0);
    if turned > 180.0 {
        turned - 360.0
    } else {
        turned
    }
}

impl OrientedBox {
    pub fn new(bounds: Bounds, rotation_degrees: f64) -> Self {
        Self {
            bounds,
            rotation_degrees,
        }
    }

    /// Whether the box is turned at all. A whole number of turns is none.
    pub fn is_turned(&self) -> bool {
        self.rotation_degrees.rem_euclid(360.0) != 0.0
    }

    /// Finite limits that are in order, and a finite turn.
    pub fn is_valid(&self) -> bool {
        self.rotation_degrees.is_finite()
            && (0..3).all(|axis| {
                self.bounds.min[axis].is_finite()
                    && self.bounds.max[axis].is_finite()
                    && self.bounds.min[axis] <= self.bounds.max[axis]
            })
    }

    pub fn center(&self) -> [f64; 3] {
        self.bounds.center()
    }

    /// Length, width and height of the box along its own axes.
    pub fn size(&self) -> [f64; 3] {
        std::array::from_fn(|axis| self.bounds.max[axis] - self.bounds.min[axis])
    }

    fn sin_cos(&self) -> (f64, f64) {
        if self.is_turned() {
            self.rotation_degrees.to_radians().sin_cos()
        } else {
            (0.0, 1.0)
        }
    }

    /// The X and Y axes of the box in the scene, as unit vectors.
    pub fn axes(&self) -> [[f64; 3]; 2] {
        let (sin, cos) = self.sin_cos();
        [[cos, sin, 0.0], [-sin, cos, 0.0]]
    }

    /// A scene position in the frame of the box before it was turned: the
    /// position that `bounds` is tested against.
    pub fn to_box(&self, xyz: [f64; 3]) -> [f64; 3] {
        if !self.is_turned() {
            return xyz;
        }
        let (sin, cos) = self.sin_cos();
        let center = self.center();
        let (dx, dy) = (xyz[0] - center[0], xyz[1] - center[1]);
        [
            center[0] + cos * dx + sin * dy,
            center[1] - sin * dx + cos * dy,
            xyz[2],
        ]
    }

    /// The scene position of a position in the frame of the box.
    pub fn to_scene(&self, local: [f64; 3]) -> [f64; 3] {
        if !self.is_turned() {
            return local;
        }
        let (sin, cos) = self.sin_cos();
        let center = self.center();
        let (dx, dy) = (local[0] - center[0], local[1] - center[1]);
        [
            center[0] + cos * dx - sin * dy,
            center[1] + sin * dx + cos * dy,
            local[2],
        ]
    }

    /// Whether a scene position lies in the box, its faces included.
    pub fn contains(&self, xyz: [f64; 3]) -> bool {
        let at = self.to_box(xyz);
        (0..3).all(|axis| at[axis] >= self.bounds.min[axis] && at[axis] <= self.bounds.max[axis])
    }

    /// The eight corners in the scene: the four at the bottom and then the
    /// four at the top, each four counter-clockwise from the corner at the
    /// minimum of both own axes.
    pub fn corners(&self) -> [[f64; 3]; 8] {
        let (min, max) = (self.bounds.min, self.bounds.max);
        let ring = [
            [min[0], min[1]],
            [max[0], min[1]],
            [max[0], max[1]],
            [min[0], max[1]],
        ];
        std::array::from_fn(|corner| {
            let [x, y] = ring[corner % 4];
            let z = if corner < 4 { min[2] } else { max[2] };
            self.to_scene([x, y, z])
        })
    }

    /// The axis-aligned box around the turned box; the box itself when it is
    /// not turned.
    pub fn aabb(&self) -> Bounds {
        if !self.is_turned() {
            return self.bounds;
        }
        let corners = self.corners();
        let mut around = Bounds {
            min: corners[0],
            max: corners[0],
        };
        for corner in &corners[1..] {
            for axis in 0..3 {
                around.min[axis] = around.min[axis].min(corner[axis]);
                around.max[axis] = around.max[axis].max(corner[axis]);
            }
        }
        around
    }

    /// A part of the box, given by limits in the frame of the box, as a box
    /// of its own that is turned the same way.
    pub fn part(&self, local: Bounds) -> Self {
        if !self.is_turned() {
            return local.into();
        }
        let center = self.to_scene(local.center());
        let half: [f64; 3] = std::array::from_fn(|axis| (local.max[axis] - local.min[axis]) * 0.5);
        Self {
            bounds: Bounds {
                min: std::array::from_fn(|axis| center[axis] - half[axis]),
                max: std::array::from_fn(|axis| center[axis] + half[axis]),
            },
            rotation_degrees: self.rotation_degrees,
        }
    }

    /// The axis-aligned box, in the frame of a turn of `rotation_degrees`
    /// about the vertical through `pivot`, around the given scene corners.
    pub fn frame_bounds(
        corners: impl IntoIterator<Item = [f64; 3]>,
        rotation_degrees: f64,
        pivot: [f64; 2],
    ) -> Option<Bounds> {
        let frame = Self::new(
            Bounds {
                min: [pivot[0], pivot[1], 0.0],
                max: [pivot[0], pivot[1], 0.0],
            },
            rotation_degrees,
        );
        let mut around: Option<Bounds> = None;
        for corner in corners {
            let at = frame.to_box(corner);
            match &mut around {
                Some(around) => {
                    for axis in 0..3 {
                        around.min[axis] = around.min[axis].min(at[axis]);
                        around.max[axis] = around.max[axis].max(at[axis]);
                    }
                }
                None => around = Some(Bounds { min: at, max: at }),
            }
        }
        around
    }
}

/// The eight corners of an axis-aligned box.
pub fn bounds_corners(bounds: Bounds) -> [[f64; 3]; 8] {
    OrientedBox::from(bounds).corners()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOM: Bounds = Bounds {
        min: [10.0, 20.0, 0.0],
        max: [14.0, 23.0, 2.5],
    };

    fn near(a: [f64; 3], b: [f64; 3]) -> bool {
        (0..3).all(|axis| (a[axis] - b[axis]).abs() < 1e-9)
    }

    #[test]
    fn a_box_that_is_not_turned_is_its_limits() {
        for degrees in [0.0, 360.0, -720.0] {
            let plain = OrientedBox::new(ROOM, degrees);
            assert!(!plain.is_turned());
            assert_eq!(plain.aabb(), ROOM);
            assert_eq!(plain.to_box([1.0, 2.0, 3.0]), [1.0, 2.0, 3.0]);
            assert!(plain.contains(ROOM.min) && plain.contains(ROOM.max));
            assert!(!plain.contains([9.999_999, 21.0, 1.0]));
            assert!(!plain.contains([12.0, 21.0, 2.500_001]));
        }
        assert_eq!(OrientedBox::from(ROOM).rotation_degrees, 0.0);
    }

    #[test]
    fn a_point_is_inside_when_it_lies_inside_in_the_frame_of_the_box() {
        // Turned a quarter: 4 m along Y and 3 m along X, about (12, 21.5).
        let quarter = OrientedBox::new(ROOM, 90.0);
        assert!(quarter.is_turned());
        let around = quarter.aabb();
        assert!(near(around.min, [10.5, 19.5, 0.0]));
        assert!(near(around.max, [13.5, 23.5, 2.5]));
        assert!(quarter.contains([12.0, 23.4, 1.0]));
        assert!(!quarter.contains([13.9, 21.5, 1.0]));

        // Turned 30 degrees: a point along the own X axis of the box is in
        // up to half the length from the centre, and out just past it.
        let turned = OrientedBox::new(ROOM, 30.0);
        let [along, across] = turned.axes();
        let center = turned.center();
        let at = |x: f64, y: f64, z: f64| -> [f64; 3] {
            [
                center[0] + x * along[0] + y * across[0],
                center[1] + x * along[1] + y * across[1],
                z,
            ]
        };
        assert!(turned.contains(at(1.999, 0.0, 1.0)));
        assert!(!turned.contains(at(2.001, 0.0, 1.0)));
        assert!(turned.contains(at(0.0, 1.499, 1.0)));
        assert!(!turned.contains(at(0.0, 1.501, 1.0)));
        assert!(turned.contains(at(1.99, 1.49, 2.5)));
        assert!(!turned.contains(at(1.99, 1.49, 2.51)));
        // The corner of the unturned box that sticks out is not inside.
        assert!(!turned.contains([14.0, 23.0, 1.0]));
        // Into the frame of the box and back.
        let point = [11.3, 22.7, 0.4];
        assert!(near(turned.to_scene(turned.to_box(point)), point));
        // The corners lie on the limits in the frame of the box.
        for corner in turned.corners() {
            let local = turned.to_box(corner);
            assert!((0..2).all(|axis| {
                (local[axis] - ROOM.min[axis]).abs() < 1e-9
                    || (local[axis] - ROOM.max[axis]).abs() < 1e-9
            }));
        }
    }

    #[test]
    fn a_part_of_a_turned_box_is_turned_the_same_way_in_place() {
        let turned = OrientedBox::new(ROOM, 30.0);
        // The slab of 0.1 m at the face at X min of the box.
        let local = Bounds {
            min: ROOM.min,
            max: [10.1, ROOM.max[1], ROOM.max[2]],
        };
        let part = turned.part(local);
        assert_eq!(part.rotation_degrees, 30.0);
        assert!(near(part.size(), [0.1, 3.0, 2.5]));
        for corner in part.corners() {
            let local_corner = turned.to_box(corner);
            assert!((0..3).all(|axis| {
                local_corner[axis] >= local.min[axis] - 1e-9
                    && local_corner[axis] <= local.max[axis] + 1e-9
            }));
        }
        assert_eq!(OrientedBox::from(ROOM).part(local), local.into());
    }

    #[test]
    fn angles_are_brought_within_a_half_turn_either_way() {
        assert_eq!(normalized_degrees(190.0), -170.0);
        assert_eq!(normalized_degrees(-190.0), 170.0);
        assert_eq!(normalized_degrees(180.0), 180.0);
        assert_eq!(normalized_degrees(-180.0), 180.0);
        assert_eq!(normalized_degrees(30.0), 30.0);
    }

    #[test]
    fn frame_bounds_hold_the_corners_in_a_turned_frame() {
        let corners = bounds_corners(ROOM);
        let same = OrientedBox::frame_bounds(corners, 0.0, [0.0, 0.0]).unwrap();
        assert_eq!(same, ROOM);
        let center = ROOM.center();
        let quarter = OrientedBox::frame_bounds(corners, 90.0, [center[0], center[1]]).unwrap();
        assert!(near(quarter.min, [10.5, 19.5, 0.0]));
        assert!(near(quarter.max, [13.5, 23.5, 2.5]));
        assert_eq!(OrientedBox::frame_bounds([], 10.0, [0.0, 0.0]), None);
    }
}
