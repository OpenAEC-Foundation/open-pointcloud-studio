//! An optional CAD line layer made from the measured cut contours. It keeps
//! the source contours/fills and never joins separate contours or removes a
//! hole. Work follows the contour vertices, not the point-cloud size.

use serde::{Deserialize, Serialize};

use super::CutRegion;
use crate::{grid2d::simplify_contours, LoadError};

pub const LAYER_STRAIGHT_LINES: &str = "OPS-STRAIGHT-LINES";

/// Lengths in metres. The minimum is a target: shorter edges remain where
/// removing them would exceed the tolerance or change the topology.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StraightLineOptions {
    pub tolerance: f64,
    pub min_length: f64,
}

impl Default for StraightLineOptions {
    fn default() -> Self {
        Self {
            tolerance: 0.01,
            min_length: 0.10,
        }
    }
}

impl StraightLineOptions {
    pub fn validate(&self) -> Result<(), LoadError> {
        if !self.tolerance.is_finite()
            || !(0.0..=1.0).contains(&self.tolerance)
            || !self.min_length.is_finite()
            || !(0.0..=10.0).contains(&self.min_length)
        {
            return Err(LoadError::InvalidData(
                "line tolerance must be between 0 and 1 m and minimum line length between 0 and 10 m".into(),
            ));
        }
        Ok(())
    }
}

/// Deviation is measured at every original contour vertex against the
/// replacement of its own stretch, in metres; RMS is vertex weighted.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
pub struct StraightLineStats {
    pub input_segments: usize,
    pub segments: usize,
    pub short_segments: usize,
    pub max_deviation: f64,
    pub rms_deviation: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StraightLines {
    pub rings: Vec<Vec<[f64; 2]>>,
    pub stats: StraightLineStats,
}

pub fn straighten_cut_regions(
    regions: &[CutRegion],
    options: StraightLineOptions,
    proceed: &mut dyn FnMut() -> Result<(), LoadError>,
) -> Result<StraightLines, LoadError> {
    options.validate()?;
    let rings: Vec<&[[f64; 2]]> = regions
        .iter()
        .flat_map(|region| {
            std::iter::once(region.outer.as_slice()).chain(region.holes.iter().map(Vec::as_slice))
        })
        .collect();
    for ring in &rings {
        if ring.len() < 3 || ring.iter().flatten().any(|value| !value.is_finite()) {
            return Err(LoadError::InvalidData(
                "a cut contour needs at least three finite vertices".into(),
            ));
        }
    }
    if rings.is_empty() {
        return Ok(StraightLines {
            rings: Vec::new(),
            stats: StraightLineStats::default(),
        });
    }
    let kept = simplify_contours(&rings, options.tolerance, options.min_length, proceed)?;
    let mut stats = StraightLineStats::default();
    let mut sum_squared = 0.0;
    let mut output = Vec::with_capacity(rings.len());
    for (ring, indices) in rings.iter().zip(kept) {
        proceed()?;
        stats.input_segments += ring.len();
        stats.segments += indices.len();
        for (edge, &start) in indices.iter().enumerate() {
            let end = indices[(edge + 1) % indices.len()];
            let a = ring[start];
            let b = ring[end];
            let delta = [b[0] - a[0], b[1] - a[1]];
            let length_squared = delta[0] * delta[0] + delta[1] * delta[1];
            stats.short_segments += usize::from(length_squared.sqrt() < options.min_length);
            for step in 0..(end + ring.len() - start) % ring.len() {
                if step % 4096 == 0 {
                    proceed()?;
                }
                let p = ring[(start + step) % ring.len()];
                let p = [p[0] - a[0], p[1] - a[1]];
                let t = if length_squared > 0.0 {
                    ((p[0] * delta[0] + p[1] * delta[1]) / length_squared).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let distance = (p[0] - t * delta[0]).hypot(p[1] - t * delta[1]);
                stats.max_deviation = stats.max_deviation.max(distance);
                sum_squared += distance * distance;
            }
        }
        output.push(indices.iter().map(|index| ring[*index]).collect());
    }
    stats.rms_deviation = (sum_squared / stats.input_segments.max(1) as f64).sqrt();
    Ok(StraightLines {
        rings: output,
        stats,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid2d::{ring_contains, ring_signed_area};

    fn region(ring: Vec<[f64; 2]>) -> CutRegion {
        CutRegion {
            outer: ring,
            holes: Vec::new(),
        }
    }

    fn run(regions: &[CutRegion], tolerance: f64, min_length: f64) -> StraightLines {
        straighten_cut_regions(
            regions,
            StraightLineOptions {
                tolerance,
                min_length,
            },
            &mut || Ok(()),
        )
        .unwrap()
    }

    fn noisy_ring(corners: &[[f64; 2]]) -> Vec<[f64; 2]> {
        let mut ring = Vec::new();
        for (i, &a) in corners.iter().enumerate() {
            let b = corners[(i + 1) % corners.len()];
            let length = (b[0] - a[0]).hypot(b[1] - a[1]);
            for step in 0..50 {
                let t = step as f64 / 50.0;
                let noise = if step == 0 {
                    0.0
                } else {
                    (step as f64 * 1.7).sin() * 0.002
                };
                ring.push([
                    a[0] + t * (b[0] - a[0]) - noise * (b[1] - a[1]) / length,
                    a[1] + t * (b[1] - a[1]) + noise * (b[0] - a[0]) / length,
                ]);
            }
        }
        ring
    }

    #[test]
    fn a_noisy_rotated_room_keeps_its_doorway_and_is_repeatable() {
        // A U-shaped wall with a doorway. Opposite sides of the material
        // remain separate; the diagonal is a genuinely angled wall.
        let corners = [
            [0., 0.],
            [1.5, 0.],
            [1.5, 0.2],
            [0.2, 0.2],
            [0.2, 2.8],
            [3.8, 2.8],
            [3.8, 1.],
            [3.3, 0.2],
            [2.5, 0.2],
            [2.5, 0.],
            [3.4, 0.],
            [4., 1.],
            [4., 3.],
            [0., 3.],
        ];
        let source = noisy_ring(&corners);
        for degrees in [0.0_f64, 27.0, 81.0] {
            let (sin, cos) = degrees.to_radians().sin_cos();
            let turn =
                |[x, y]: [f64; 2]| [1_000_000. + x * cos - y * sin, 500_000. + x * sin + y * cos];
            let input = vec![region(source.iter().copied().map(turn).collect())];
            let result = run(&input, 0.01, 0.10);
            assert!(result.stats.segments < 35, "{result:?}");
            assert!(result.stats.max_deviation <= 0.01001);
            assert!(!ring_contains(&result.rings[0], turn([2., 0.1])));
            assert!(ring_contains(&result.rings[0], turn([0.1, 1.5])));
            assert_eq!(result, run(&input, 0.01, 0.10));
            assert_eq!(
                ring_signed_area(&input[0].outer).signum(),
                ring_signed_area(&result.rings[0]).signum()
            );
        }
    }

    #[test]
    fn holes_and_a_round_column_remain_and_the_original_is_unchanged() {
        let circle: Vec<_> = (0..360)
            .map(|i| {
                let angle = i as f64 * std::f64::consts::TAU / 360.;
                [2. + angle.cos() * 0.3, 2. + angle.sin() * 0.3]
            })
            .collect();
        let mut wall = region(vec![[0., 0.], [4., 0.], [4., 4.], [0., 4.]]);
        wall.holes
            .push(vec![[0.2, 0.2], [0.2, 3.8], [3.8, 3.8], [3.8, 0.2]]);
        let input = vec![wall, region(circle)];
        let before = input.clone();
        let result = run(&input, 0.005, 0.10);
        assert_eq!(input, before);
        assert_eq!(result.rings.len(), 3);
        assert!(
            result.rings[2].len() > 12,
            "a round column must not become a rectangle"
        );
        assert!(ring_contains(&result.rings[1], [2., 2.]));
        assert!(result.stats.max_deviation <= 0.0050001);
        assert!(
            result.stats.short_segments > 0,
            "retain short arcs when the tolerance needs them"
        );
    }

    #[test]
    fn nearby_regions_and_thin_material_do_not_merge() {
        let a = region(noisy_ring(&[[0., 0.], [4., 0.], [4., 0.05], [0., 0.05]]));
        let b = region(noisy_ring(&[
            [0., 0.06],
            [4., 0.06],
            [4., 0.11],
            [0., 0.11],
        ]));
        let result = run(&[a, b], 0.02, 0.2);
        assert_eq!(result.rings.len(), 2);
        for ring in &result.rings {
            assert!(ring.len() >= 4);
            assert!(!ring_contains(ring, [2., 0.055]));
        }
    }

    #[test]
    fn refuses_bad_settings_and_honours_cancellation() {
        let regions = vec![region(noisy_ring(&[
            [0., 0.],
            [4., 0.],
            [4., 4.],
            [0., 4.],
        ]))];
        for tolerance in [f64::NAN, f64::INFINITY, -1.0, 1.01] {
            assert!(straighten_cut_regions(
                &regions,
                StraightLineOptions {
                    tolerance,
                    ..Default::default()
                },
                &mut || Ok(())
            )
            .is_err());
        }
        assert!(matches!(
            straighten_cut_regions(&regions, StraightLineOptions::default(), &mut || Err(
                LoadError::Cancelled
            )),
            Err(LoadError::Cancelled)
        ));
        let zero = run(&regions, 0.0, 0.0);
        assert!(zero.stats.max_deviation < 1e-10);
    }
}
