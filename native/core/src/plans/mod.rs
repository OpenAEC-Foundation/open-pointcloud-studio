//! Mesh to Plans: from a scan of a building to its plans, sections and
//! model. The steps are deterministic: every accumulation is an integer
//! sum, a minimum, a maximum or a bitwise or, so that the result does not
//! depend on the order in which points arrive or on the number of threads,
//! and every list is sorted completely.
//!
//! - `frame`: the box around what was scanned, without stray points far
//!   away, and the frame that every drawing of the building shares.

pub mod frame;

pub use frame::{robust_bounds, BuildingFrame, RobustBounds, RobustBoundsConfig};
