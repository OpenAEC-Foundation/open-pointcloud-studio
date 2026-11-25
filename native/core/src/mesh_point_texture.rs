//! Measured RGB near a surface texel, with bounded k-nearest queries.
use crate::local_fit::{difference, dot, Moments};
use crate::point_index::PointIndex;
use crate::{LoadError, Point};
use rayon::prelude::*;

pub const MAX_TEXTURE_POINTS: usize = 262_144;

pub(crate) struct PointSampler {
    positions: Vec<[f64; 3]>,
    colours: Vec<Option<[u8; 3]>>,
    normals: Vec<Option<[f64; 3]>>,
    index: PointIndex,
    radius: f64,
}

impl PointSampler {
    pub(crate) fn positions(&self) -> &[[f64; 3]] {
        &self.positions
    }

    pub(crate) fn new(
        points: &[Point],
        radius: f64,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<Self, LoadError> {
        if points.len() > MAX_TEXTURE_POINTS
            || !radius.is_finite()
            || radius <= 0.0
            || points.iter().any(|p| p.xyz.iter().any(|x| !x.is_finite()))
        {
            return Err(LoadError::InvalidData(
                "invalid measured texture samples or search radius".into(),
            ));
        }
        if cancelled() {
            return Err(LoadError::Cancelled);
        }
        let positions = points.iter().map(|p| p.xyz).collect::<Vec<_>>();
        let index = PointIndex::new(&positions);
        let normals = positions
            .par_iter()
            .map_init(
                || Vec::with_capacity(17),
                |near, &p| {
                    if cancelled() {
                        return Err(LoadError::Cancelled);
                    }
                    index.nearest_into(&positions, p, 16, radius * radius, near);
                    let fit = Moments::from_points(near.iter().map(|&(_, i)| positions[i])).plane();
                    Ok(fit
                        .filter(|f| f.eigenvalues[1] > 1e-12 && f.surface_variation() < 0.04)
                        .map(|f| f.normal))
                },
            )
            .collect::<Result<Vec<_>, LoadError>>()?;
        Ok(Self {
            positions,
            colours: points.iter().map(|p| p.rgb).collect(),
            normals,
            index,
            radius,
        })
    }

    pub(crate) fn sample(
        &self,
        p: [f64; 3],
        normal: [f64; 3],
        near: &mut Vec<(f64, usize)>,
    ) -> Option<[u8; 3]> {
        self.index
            .nearest_into(&self.positions, p, 32, self.radius * self.radius, near);
        // Reject another side of a fold, and points on a nearby parallel wall.
        near.retain(|&(_, i)| {
            self.colours[i].is_some()
                && self.normals[i].is_some_and(|n| dot(n, normal).abs() >= 0.75)
                && dot(difference(self.positions[i], p), normal).abs() <= self.radius * 0.2
        });
        if near.len() < 3 {
            return None;
        }
        // The nearest five observations define a robust local colour, keeping
        // isolated bright/dark scan noise from dominating inverse-distance weights.
        let mut seed = [[0u8; 5]; 3];
        let count = near.len().min(5);
        for (j, &(_, i)) in near.iter().take(count).enumerate() {
            for (channel, values) in seed.iter_mut().enumerate() {
                values[j] = self.colours[i].unwrap()[channel];
            }
        }
        let reference = seed.map(|mut values| {
            values[..count].sort_unstable();
            values[count / 2]
        });
        let mut sum = [0.0; 3];
        let mut total = 0.0;
        let mut accepted = 0;
        for &(squared, i) in near.iter() {
            let rgb = self.colours[i].unwrap();
            if (0..3).any(|c| rgb[c].abs_diff(reference[c]) > 48) {
                continue;
            }
            let weight = (1.0 - squared.sqrt() / self.radius).max(0.0).powi(2)
                / squared.max(self.radius * self.radius * 0.0025);
            for c in 0..3 {
                sum[c] += f64::from(rgb[c]) * weight;
            }
            total += weight;
            accepted += 1;
        }
        (accepted >= 3 && total > 0.0)
            .then(|| sum.map(|v| (v / total).round().clamp(0.0, 255.0) as u8))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn plane(z: f64, colour: [u8; 3]) -> Vec<Point> {
        (-5..=5)
            .flat_map(|x| {
                (-5..=5).map(move |y| Point {
                    xyz: [x as f64 * 0.02, y as f64 * 0.02, z],
                    rgb: Some(colour),
                    intensity: None,
                    classification: None,
                })
            })
            .collect()
    }
    #[test]
    fn a_measured_colour_ramp_is_preserved_within_three_rgb_levels() {
        let mut points = plane(0., [0; 3]);
        for p in &mut points {
            p.rgb = Some([(120. + p.xyz[0] * 300.).round() as u8, 80, 160]);
        }
        let sampler = PointSampler::new(&points, 0.08, &|| false).unwrap();
        for x in [-0.04, -0.01, 0., 0.01, 0.04] {
            let rgb = sampler
                .sample([x, 0., 0.], [0., 0., 1.], &mut Vec::new())
                .unwrap();
            assert!((f64::from(rgb[0]) - (120. + x * 300.)).abs() <= 3.0);
            assert_eq!([rgb[1], rgb[2]], [80, 160]);
        }
    }

    #[test]
    fn measured_colour_rejects_nearest_outlier_and_missing_surfaces() {
        let mut points = plane(0.0, [110, 120, 130]);
        points[60].rgb = Some([255, 0, 255]);
        let sampler = PointSampler::new(&points, 0.08, &|| false).unwrap();
        let mut near = Vec::new();
        assert_eq!(
            sampler.sample([0.0; 3], [0., 0., 1.], &mut near),
            Some([110, 120, 130])
        );
        assert_eq!(sampler.sample([1., 0., 0.], [0., 0., 1.], &mut near), None);
        assert_eq!(sampler.sample([0.; 3], [1., 0., 0.], &mut near), None);
    }
    #[test]
    fn a_nearby_parallel_wall_does_not_supply_colour() {
        let points = plane(0.04, [200, 0, 0]);
        let sampler = PointSampler::new(&points, 0.1, &|| false).unwrap();
        assert_eq!(sampler.sample([0.; 3], [0., 0., 1.], &mut Vec::new()), None);
    }
    #[test]
    fn cancellation_and_nonfinite_samples_are_rejected() {
        assert!(matches!(
            PointSampler::new(&plane(0., [1; 3]), 0.1, &|| true),
            Err(LoadError::Cancelled)
        ));
        let mut points = plane(0., [1; 3]);
        points[0].xyz[0] = f64::NAN;
        assert!(PointSampler::new(&points, 0.1, &|| false).is_err());
    }
}
