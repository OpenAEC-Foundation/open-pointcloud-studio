//! One native shell for OpenAEC applications.
//!
//! Use [`run`] and implement [`Application`]. Domain code supplies commands and
//! content; this crate owns the shell layout, typography, icons and settings.
pub use iced;
pub mod design;
mod dual_range;
pub mod find_bar;
pub mod fonts;
pub mod frame;
pub mod icons;
pub mod locale;
mod modal;
pub mod model;
mod popover;
pub mod ribbon_icons;
mod runtime;
mod shell;
pub mod theme;
pub mod tokens;
pub mod widgets;
pub use locale::Language;
pub use model::*;
pub use runtime::{run, Application};
pub use shell::{shell, Slots};
pub use theme::Theme;
