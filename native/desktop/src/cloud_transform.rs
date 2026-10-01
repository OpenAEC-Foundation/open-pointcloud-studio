use pointcloud_core::{Bounds, IndexedPoint, Point};

/// An affine edit of source coordinates. The disk source and octree keep their
/// original coordinate system; UI consumers map records when they read them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CloudTransform {
    pub scale: [f64; 3],
    pub offset: [f64; 3],
}

impl Default for CloudTransform {
    fn default() -> Self {
        Self {
            scale: [1.0; 3],
            offset: [0.0; 3],
        }
    }
}

impl CloudTransform {
    pub fn is_identity(self) -> bool {
        self == Self::default()
    }

    pub fn xyz(self, source: [f64; 3]) -> [f64; 3] {
        std::array::from_fn(|axis| source[axis] * self.scale[axis] + self.offset[axis])
    }

    pub fn point(self, mut point: Point) -> Point {
        point.xyz = self.xyz(point.xyz);
        point
    }

    pub fn record(self, mut record: IndexedPoint) -> IndexedPoint {
        record.point = self.point(record.point);
        record
    }

    pub fn bounds(self, source: Bounds) -> Bounds {
        let a = self.xyz(source.min);
        let b = self.xyz(source.max);
        Bounds {
            min: std::array::from_fn(|axis| a[axis].min(b[axis])),
            max: std::array::from_fn(|axis| a[axis].max(b[axis])),
        }
    }

    pub fn axes(self, source: Option<[[f64; 3]; 3]>) -> Option<[[f64; 3]; 3]> {
        let source = source?;
        let mut axes = [[0.0; 3]; 3];
        for (index, direction) in source.into_iter().enumerate() {
            let scaled = std::array::from_fn(|axis| direction[axis] * self.scale[axis]);
            let length = scaled.iter().map(|value| value * value).sum::<f64>().sqrt();
            if !length.is_finite() || length <= f64::EPSILON {
                return None;
            }
            axes[index] = scaled.map(|value| value / length);
        }
        Some(axes)
    }

    pub fn translated(self, delta: [f64; 3], source: Bounds) -> Option<Self> {
        let next = Self {
            offset: std::array::from_fn(|axis| self.offset[axis] + delta[axis]),
            ..self
        };
        next.valid_for(source).then_some(next)
    }

    pub fn scaled_about(self, factors: [f64; 3], pivot: [f64; 3], source: Bounds) -> Option<Self> {
        let next = Self {
            scale: std::array::from_fn(|axis| self.scale[axis] * factors[axis]),
            offset: std::array::from_fn(|axis| {
                pivot[axis] + (self.offset[axis] - pivot[axis]) * factors[axis]
            }),
        };
        next.valid_for(source).then_some(next)
    }

    fn valid_for(self, source: Bounds) -> bool {
        self.scale.iter().all(|value| value.is_finite())
            && self.offset.iter().all(|value| value.is_finite())
            && self
                .bounds(source)
                .min
                .into_iter()
                .chain(self.bounds(source).max)
                .all(|value| value.is_finite())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composed_edits_preserve_world_bounds_even_for_negative_scale() {
        let source = Bounds {
            min: [10.0, 20.0, 30.0],
            max: [20.0, 40.0, 50.0],
        };
        let moved = CloudTransform::default()
            .translated([5.0, -10.0, 0.0], source)
            .unwrap();
        let scaled = moved
            .scaled_about([-2.0, 0.5, 1.0], moved.bounds(source).center(), source)
            .unwrap();
        assert_eq!(scaled.xyz([10.0, 20.0, 30.0]), [30.0, 15.0, 30.0]);
        assert_eq!(scaled.bounds(source).min, [10.0, 15.0, 30.0]);
        assert_eq!(scaled.bounds(source).max, [30.0, 25.0, 50.0]);
    }
}
