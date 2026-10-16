//! One native shell for OpenAEC applications.
//!
//! Use [`run`] and implement [`Application`]. Domain code supplies commands and
//! content; this crate owns the shell layout, typography, icons and settings.
pub use iced;
pub mod fonts;
pub mod icons;
pub mod ribbon_icons;
pub mod frame;
pub mod find_bar;
pub mod locale;
pub mod model;
pub mod theme;
pub mod tokens;
pub mod widgets;
mod dual_range;
mod modal;
mod popover;
mod shell;
mod runtime;
pub use locale::Language;
pub use model::*;
pub use runtime::{run, Application};
pub use shell::{shell, Slots};
pub use theme::Theme;
