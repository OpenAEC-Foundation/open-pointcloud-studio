//! Mesh to Plans: from a scan of a building to its plans, sections and
//! model. The steps are deterministic: every accumulation is an integer
//! sum, a minimum, a maximum or a bitwise or, so that the result does not
//! depend on the order in which points arrive or on the number of threads,
//! and every list is sorted completely.
//!
//! - `frame`: the box around what was scanned, without stray points far
//!   away, and the frame that every drawing of the building shares.
//! - `survey`: one read of the whole scene into a volume of occupied cells
//!   in that frame, and the footprint it suggests.
//! - `levels`: the floors, ceilings, slabs and roof found in the survey,
//!   and their heights refined to the millimetre from the points.

use serde::{Deserialize, Serialize};

pub mod frame;
pub mod levels;
pub mod survey;

pub use frame::{
    building_frame, robust_bounds, second_direction, BuildingFrame, RobustBounds,
    RobustBoundsConfig,
};
pub use levels::{
    detect_levels, refine_levels, Facing, Level, LevelConfig, LevelDetection, LevelKind, LevelPeak,
    LevelRefinement, LevelStatus, PeakRole, RefineConfig,
};
pub use survey::{
    survey_scene, Footprint, FootprintConfig, PlanRegion, SceneSurvey, SurveyConfig, SurveyGrid,
    SurveyStats, SurveySummary, DEFAULT_SURVEY_BUDGET,
};

/// From this score on an element is sure.
pub const SURE: f32 = 0.75;
/// From this score on, and below `SURE`, an element is to be checked; below
/// it the measured cut stands instead.
pub const CHECK: f32 = 0.45;

/// What a confidence was judged on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Criterion {
    /// How sharp the peak of a level is: narrow at half its height.
    Sharpness,
    /// How much of the footprint it covers.
    Area,
    /// Whether the height of its storey is a usual one.
    Consistency,
}

/// Where a score falls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Band {
    /// From `SURE` on.
    Sure,
    /// From `CHECK` on: to be looked at.
    Check,
    /// Below `CHECK`: not to be drawn as found.
    Fallback,
}

impl Band {
    pub fn of(score: f32) -> Self {
        if score >= SURE {
            Self::Sure
        } else if score >= CHECK {
            Self::Check
        } else {
            Self::Fallback
        }
    }
}

/// How sure an element is: the product of its parts, from 0 to 1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Confidence {
    pub score: f32,
    pub parts: Vec<(Criterion, f32)>,
    pub band: Band,
}

impl Confidence {
    /// The confidence of these parts, each from 0 to 1.
    pub fn of(parts: Vec<(Criterion, f32)>) -> Self {
        let score = parts
            .iter()
            .map(|(_, part)| part.clamp(0.0, 1.0))
            .product::<f32>();
        Self {
            score,
            parts,
            band: Band::of(score),
        }
    }

    /// Full confidence, for what the user set by hand.
    pub fn certain() -> Self {
        Self::of(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_confidence_is_the_product_of_its_parts_and_falls_in_a_band() {
        let sure = Confidence::of(vec![(Criterion::Sharpness, 1.0), (Criterion::Area, 0.8)]);
        assert!((sure.score - 0.8).abs() < 1e-6);
        assert_eq!(sure.band, Band::Sure);
        let check = Confidence::of(vec![(Criterion::Area, 0.5)]);
        assert_eq!(check.band, Band::Check);
        let fallback = Confidence::of(vec![(Criterion::Area, 0.9), (Criterion::Consistency, 0.4)]);
        assert_eq!(fallback.band, Band::Fallback);
        // A part out of range counts as its nearest end.
        assert_eq!(Confidence::of(vec![(Criterion::Area, 1.5)]).score, 1.0);
        assert_eq!(Confidence::certain().score, 1.0);
        assert_eq!(Band::of(SURE), Band::Sure);
        assert_eq!(Band::of(CHECK), Band::Check);
    }
}
