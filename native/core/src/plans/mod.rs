//! Mesh to Plans: from a scan of a building to its plans, sections and
//! model. The steps are deterministic: every accumulation is an integer
//! sum, a minimum, a maximum or a bitwise or, so that the result does not
//! depend on the order in which points arrive or on the number of threads,
//! and every list is sorted completely.
//!
//! - `frame`: the box around what was scanned, without stray points far
//!   away, and the frame that every drawing of the building shares.
//! - `survey`: one read of the whole scene into a volume of occupied cells
//!   in that frame.

pub mod frame;
pub mod survey;

pub use frame::{robust_bounds, BuildingFrame, RobustBounds, RobustBoundsConfig};
pub use survey::{
    survey_scene, SceneSurvey, SurveyConfig, SurveyGrid, SurveyStats, SurveySummary,
    DEFAULT_SURVEY_BUDGET,
};
