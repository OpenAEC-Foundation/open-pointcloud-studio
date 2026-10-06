//! The crop region of a 2D drawing of the Project Browser, and the
//! two-letter command RO that turns it.
//!
//! The crop region is the face of the box of a drawing as the drawing shows
//! it: for a plan the box along its own two horizontal axes, for an
//! elevation or a section its width along the view and its height. Its
//! handles in the Drawing view move its sides once a click on its outline
//! selected it, the Crop region section of Properties sets its figures while
//! it is selected, and the drawing is made again in place from the box that
//! follows, from the points it read before as long as its cut, its depth and
//! the points it uses stay. RO turns the crop region of a plan: the box turns
//! about the vertical through the centre of the region, and the plan made
//! again stands upright in it with the model turned the other way. In the 3D
//! view RO turns the section box in the same way.

use std::time::{Duration, Instant};

use iced::widget::{column, container, text, text_input};
use iced::{Element, Fill, Size, Task};
use pointcloud_core::{
    normalized_degrees, Bounds, DrawingFrame, DrawingOrigin, DrawingView, OrientedBox,
    MAX_SLAB_THICKNESS, MIN_DRAWING_SAMPLE_PERCENT, MIN_SLAB_THICKNESS,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::drawing_view::ViewCamera;
use crate::i18n::key;
use crate::saved_drawings::SavedDrawing;
use crate::selection::Projection;
use crate::{
    combined_bounds, format_rotation, opencad_properties, parse_rotation, section_turn_delta,
    Message, Studio,
};

/// The smallest side of a crop region, in metres.
pub const MIN_SIDE: f64 = 0.10;
/// A dragged side makes the crop region a whole number of these long, in
/// metres.
pub const SNAP: f64 = 0.01;
/// R and then O typed within this time start a turn.
pub const SEQUENCE_TIME: Duration = Duration::from_millis(1500);
/// With Shift the pointer turns in steps of this many degrees.
pub const SHIFT_STEP: f64 = 15.0;
/// The longest number of degrees that can be typed while turning.
const MAX_TYPED: usize = 9;

/// The face of the box a drawing shows: the cut plane as the coordinate
/// system of the drawing, and the rectangle of the face in it, lower left
/// and upper right, in metres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CropFrame {
    pub frame: DrawingFrame,
    pub rect: [[f64; 2]; 2],
}

impl CropFrame {
    /// The middle of the crop region on the cut plane, in the model.
    pub fn centre(&self) -> [f64; 3] {
        self.frame.to_world(middle(self.rect))
    }
}

/// The crop region of a box seen from a view, with the origin a drawing
/// measures from; nothing for a box without size in the view.
pub fn crop_frame(
    section: OrientedBox,
    view: DrawingView,
    origin: DrawingOrigin,
) -> Option<CropFrame> {
    let slab = pointcloud_core::slab_from_section(section, view, None, origin).ok()?;
    Some(CropFrame {
        frame: slab.frame,
        rect: slab.extent,
    })
}

/// The middle of a rectangle.
pub fn middle(rect: [[f64; 2]; 2]) -> [f64; 2] {
    [
        (rect[0][0] + rect[1][0]) / 2.0,
        (rect[0][1] + rect[1][1]) / 2.0,
    ]
}

/// A rectangle with every coordinate multiplied by `factor`.
pub fn scaled(rect: [[f64; 2]; 2], factor: f64) -> [[f64; 2]; 2] {
    rect.map(|corner| corner.map(|value| value * factor))
}

/// What each side of the crop region moves, in the order left, right,
/// bottom and top as the drawing shows them: the axis of the box before its
/// turn, whether the side is the face at the maximum of that axis, and the
/// sign that makes a move of the side to the right or up a move along that
/// axis. A view from behind has the box's own X or Y running to the left.
fn sides(view: DrawingView) -> [(usize, bool, f64); 4] {
    let (bottom, top) = ((2, false, 1.0), (2, true, 1.0));
    match view {
        DrawingView::Plan => [
            (0, false, 1.0),
            (0, true, 1.0),
            (1, false, 1.0),
            (1, true, 1.0),
        ],
        DrawingView::Front => [(0, false, 1.0), (0, true, 1.0), bottom, top],
        DrawingView::Back => [(0, true, -1.0), (0, false, -1.0), bottom, top],
        DrawingView::Left => [(1, true, -1.0), (1, false, -1.0), bottom, top],
        DrawingView::Right => [(1, false, 1.0), (1, true, 1.0), bottom, top],
    }
}

/// The box after its crop region went from `old` to `new`, both in metres in
/// the frame of the drawing. Only the faces in the plane of the drawing
/// move; the faces the drawing looks through stay, and so do the faces of a
/// turned box that did not move.
pub fn box_for_crop(
    section: OrientedBox,
    view: DrawingView,
    old: [[f64; 2]; 2],
    new: [[f64; 2]; 2],
) -> OrientedBox {
    let moves = [
        new[0][0] - old[0][0],
        new[1][0] - old[1][0],
        new[0][1] - old[0][1],
        new[1][1] - old[1][1],
    ];
    let mut local = section.bounds;
    for ((axis, at_max, sign), moved) in sides(view).into_iter().zip(moves) {
        if at_max {
            local.max[axis] += sign * moved;
        } else {
            local.min[axis] += sign * moved;
        }
    }
    section.part(local)
}

/// A handle of the crop region: one in the middle of each side and one at
/// each corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    Left,
    Right,
    Bottom,
    Top,
    BottomLeft,
    BottomRight,
    TopLeft,
    TopRight,
}

impl Handle {
    pub const ALL: [Self; 8] = [
        Self::Left,
        Self::Right,
        Self::Bottom,
        Self::Top,
        Self::BottomLeft,
        Self::BottomRight,
        Self::TopLeft,
        Self::TopRight,
    ];

    /// The side it moves across and the one it moves up: the high side
    /// (right or top), the low side, or none.
    fn sides(self) -> [Option<bool>; 2] {
        match self {
            Self::Left => [Some(false), None],
            Self::Right => [Some(true), None],
            Self::Bottom => [None, Some(false)],
            Self::Top => [None, Some(true)],
            Self::BottomLeft => [Some(false), Some(false)],
            Self::BottomRight => [Some(true), Some(false)],
            Self::TopLeft => [Some(false), Some(true)],
            Self::TopRight => [Some(true), Some(true)],
        }
    }

    /// The name the local API gives it.
    pub fn key(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Bottom => "bottom",
            Self::Top => "top",
            Self::BottomLeft => "bottom_left",
            Self::BottomRight => "bottom_right",
            Self::TopLeft => "top_left",
            Self::TopRight => "top_right",
        }
    }

    pub fn from_key(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|handle| handle.key() == value)
    }

    /// Where it lies on a rectangle.
    pub fn at(self, rect: [[f64; 2]; 2]) -> [f64; 2] {
        let sides = self.sides();
        std::array::from_fn(|axis| match sides[axis] {
            Some(high) => rect[usize::from(high)][axis],
            None => (rect[0][axis] + rect[1][axis]) / 2.0,
        })
    }

    /// The pointer over it: arrows along the way it moves the region.
    pub fn cursor(self) -> iced::mouse::Interaction {
        use iced::mouse::Interaction;
        match self {
            Self::Left | Self::Right => Interaction::ResizingHorizontally,
            Self::Bottom | Self::Top => Interaction::ResizingVertically,
            // The sheet has its Y axis down: the lower left and upper right
            // corners move along the rising diagonal.
            Self::BottomLeft | Self::TopRight => Interaction::ResizingDiagonallyUp,
            Self::TopLeft | Self::BottomRight => Interaction::ResizingDiagonallyDown,
        }
    }
}

/// The rectangle after a handle was dragged to `pointer`: the sides it
/// moves go to the pointer, with the size a whole number of centimetres and
/// at least the smallest side. `unit` is how many drawing units make a
/// metre.
pub fn dragged(rect: [[f64; 2]; 2], handle: Handle, pointer: [f64; 2], unit: f64) -> [[f64; 2]; 2] {
    let step = SNAP * unit;
    let least = MIN_SIDE * unit;
    let snap = |size: f64| ((size / step).round() * step).max(least);
    let mut moved = rect;
    for (axis, side) in handle.sides().into_iter().enumerate() {
        match side {
            Some(true) => moved[1][axis] = rect[0][axis] + snap(pointer[axis] - rect[0][axis]),
            Some(false) => moved[0][axis] = rect[1][axis] - snap(rect[1][axis] - pointer[axis]),
            None => {}
        }
    }
    moved
}

/// The box of a plan after its crop region turned `degrees`
/// counter-clockwise on the sheet: the box turns as far, counter-clockwise
/// seen from above, about the vertical through its centre, which is the
/// centre of the crop region.
pub fn turned(section: OrientedBox, degrees: f64) -> OrientedBox {
    OrientedBox::new(
        section.bounds,
        normalized_degrees(section.rotation_degrees + degrees) + 0.0,
    )
}

/// The axes of the box a view uses: the one along its width, the one along
/// its height and the one it looks along, and whether the cut is the face at
/// the maximum of that last one.
#[derive(Debug, Clone, Copy)]
struct Layout {
    across: usize,
    up: usize,
    look: usize,
    cut_at_max: bool,
}

impl Layout {
    fn of(view: DrawingView) -> Self {
        let (across, up, look, cut_at_max) = match view {
            DrawingView::Plan => (0, 1, 2, true),
            DrawingView::Front => (0, 2, 1, false),
            DrawingView::Back => (0, 2, 1, true),
            DrawingView::Left => (1, 2, 0, false),
            DrawingView::Right => (1, 2, 0, true),
        };
        Self {
            across,
            up,
            look,
            cut_at_max,
        }
    }
}

/// Where a model position lies along an axis of the box: its height for the
/// vertical, its distance from the model origin along the axis otherwise.
fn along(section: OrientedBox, axis: usize, point: [f64; 3]) -> f64 {
    if axis == 2 {
        return point[2];
    }
    let direction = section.axes()[axis];
    direction[0] * point[0] + direction[1] * point[1]
}

/// The figures of a crop region as Properties shows them, in metres and
/// degrees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Figures {
    pub width: f64,
    pub height: f64,
    /// A plan: the model X and Y of its centre. An elevation or a section:
    /// where its centre lies along the box, and its height.
    pub centre: [f64; 2],
    /// The turn of the box, counter-clockwise seen from above.
    pub rotation: f64,
    /// A plan: the height of the cut. An elevation or a section: where the
    /// cut lies along the direction it looks, measured along the box.
    pub cut: f64,
    /// How deep the drawing sees behind the cut: the slab that is drawn.
    pub depth: f64,
}

/// The figures of the crop region of a box seen from a view with a slab of
/// `thickness`; an elevation has none and sees through the whole box.
pub fn figures(section: OrientedBox, view: DrawingView, thickness: Option<f64>) -> Figures {
    let layout = Layout::of(view);
    let size = section.size();
    let centre = section.center();
    let mut face = section.bounds.center();
    face[layout.look] = if layout.cut_at_max {
        section.bounds.max[layout.look]
    } else {
        section.bounds.min[layout.look]
    };
    let face = section.to_scene(face);
    Figures {
        width: size[layout.across],
        height: size[layout.up],
        centre: match view {
            DrawingView::Plan => [centre[0], centre[1]],
            _ => [along(section, layout.across, centre), centre[2]],
        },
        rotation: section.rotation_degrees,
        cut: along(section, layout.look, face),
        depth: thickness.map_or(size[layout.look], |slab| slab.min(size[layout.look])),
    }
}

/// A figure of the Crop region section of Properties.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Width,
    Height,
    /// A plan: the model X of the centre. Otherwise its place along the box.
    CentreA,
    /// A plan: the model Y of the centre. Otherwise its height.
    CentreB,
    /// The turn of a plan.
    Rotation,
    Cut,
    Depth,
    /// The share of the points of the scans the drawing is made from.
    Points,
}

impl Field {
    /// The fields of a view, in the order Properties shows them.
    pub fn of(view: DrawingView) -> &'static [Self] {
        match view {
            DrawingView::Plan => &[
                Self::Width,
                Self::Height,
                Self::CentreA,
                Self::CentreB,
                Self::Rotation,
                Self::Cut,
                Self::Depth,
                Self::Points,
            ],
            _ => &[
                Self::Width,
                Self::Height,
                Self::CentreA,
                Self::CentreB,
                Self::Cut,
                Self::Depth,
                Self::Points,
            ],
        }
    }

    fn label(self, view: DrawingView) -> &'static str {
        let plan = view == DrawingView::Plan;
        match self {
            Self::Width => key("Width (m)"),
            Self::Height => key("Height (m)"),
            Self::CentreA if plan => key("Centre X"),
            Self::CentreA => key("Centre along"),
            Self::CentreB if plan => key("Centre Y"),
            Self::CentreB => key("Centre height"),
            Self::Rotation => key("Rotation (°)"),
            Self::Cut if plan => key("Cut height"),
            Self::Cut => key("Cut position"),
            Self::Depth => key("View depth (m)"),
            Self::Points => key("Points used (%)"),
        }
    }

    /// The figure as the field shows it, with `percent` the points used.
    fn text(self, figures: &Figures, percent: f64) -> String {
        match self {
            Self::Width => format!("{:.3}", figures.width),
            Self::Height => format!("{:.3}", figures.height),
            Self::CentreA => format!("{:.3}", figures.centre[0]),
            Self::CentreB => format!("{:.3}", figures.centre[1]),
            Self::Rotation => format_rotation(figures.rotation),
            Self::Cut => format!("{:.3}", figures.cut),
            Self::Depth => format!("{:.3}", figures.depth),
            Self::Points => percent_text(percent),
        }
    }
}

/// A share in percent as a field shows it: without the decimals it does not
/// need.
pub fn percent_text(percent: f64) -> String {
    let text = format!("{percent:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// A share in percent as it is typed, with a point or a comma and with or
/// without the percent sign; nothing for what is no number.
pub fn parse_percent(typed: &str) -> Option<f64> {
    typed
        .trim()
        .trim_end_matches('%')
        .trim()
        .replace(',', ".")
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

/// Why a share of the points cannot be used, if it cannot.
pub fn percent_problem(percent: f64) -> Option<String> {
    (!(MIN_DRAWING_SAMPLE_PERCENT..=100.0).contains(&percent))
        .then(|| format!("The points used must lie between {MIN_DRAWING_SAMPLE_PERCENT} and 100 %"))
}

/// The box and the slab after one figure of the crop region was set. Width
/// and height change about the centre; a centre moves the box; the cut moves
/// the face the drawing cuts at, the box keeping at least the depth the
/// drawing sees; the view depth is the slab, for which the box grows when it
/// is shallower, and for an elevation the depth of the box itself.
pub fn with_figure(
    section: OrientedBox,
    view: DrawingView,
    thickness: Option<f64>,
    field: Field,
    value: f64,
) -> Result<(OrientedBox, Option<f64>), String> {
    if !value.is_finite() {
        return Err("The value must be a number".into());
    }
    let layout = Layout::of(view);
    let now = figures(section, view, thickness);
    let mut local = section.bounds;
    let moved = |by: [f64; 3]| {
        let shifted = Bounds {
            min: std::array::from_fn(|axis| section.bounds.min[axis] + by[axis]),
            max: std::array::from_fn(|axis| section.bounds.max[axis] + by[axis]),
        };
        OrientedBox::new(shifted, section.rotation_degrees)
    };
    let plan = view == DrawingView::Plan;
    let changed = match field {
        Field::Width | Field::Height => {
            if value < MIN_SIDE - 1e-9 {
                return Err(format!(
                    "The crop region must be at least {MIN_SIDE:.2} m wide and high"
                ));
            }
            let axis = if field == Field::Width {
                layout.across
            } else {
                layout.up
            };
            let centre = (local.min[axis] + local.max[axis]) / 2.0;
            local.min[axis] = centre - value / 2.0;
            local.max[axis] = centre + value / 2.0;
            section.part(local)
        }
        Field::CentreA if plan => moved([value - now.centre[0], 0.0, 0.0]),
        Field::CentreB if plan => moved([0.0, value - now.centre[1], 0.0]),
        Field::CentreA => {
            let direction = section.axes()[layout.across];
            let shift = value - now.centre[0];
            moved([direction[0] * shift, direction[1] * shift, 0.0])
        }
        Field::CentreB => moved([0.0, 0.0, value - now.centre[1]]),
        Field::Rotation if plan => {
            OrientedBox::new(section.bounds, normalized_degrees(value) + 0.0)
        }
        Field::Rotation => return Err("Only the crop region of a plan turns".into()),
        Field::Points => return Err("The points used are no figure of the box".into()),
        Field::Cut => {
            let shift = value - now.cut;
            let look = layout.look;
            if layout.cut_at_max {
                local.max[look] += shift;
                local.min[look] = local.min[look].min(local.max[look] - now.depth);
            } else {
                local.min[look] += shift;
                local.max[look] = local.max[look].max(local.min[look] + now.depth);
            }
            section.part(local)
        }
        Field::Depth => {
            let look = layout.look;
            let depth = local.max[look] - local.min[look];
            let wanted = match thickness {
                Some(_) => {
                    if !(MIN_SLAB_THICKNESS..=MAX_SLAB_THICKNESS).contains(&value) {
                        return Err(format!(
                            "The view depth must lie between {MIN_SLAB_THICKNESS} and {MAX_SLAB_THICKNESS} m"
                        ));
                    }
                    depth.max(value)
                }
                None if value >= MIN_SLAB_THICKNESS => value,
                None => {
                    return Err(format!(
                        "The view depth must be at least {MIN_SLAB_THICKNESS} m"
                    ))
                }
            };
            if layout.cut_at_max {
                local.min[look] = local.max[look] - wanted;
            } else {
                local.max[look] = local.min[look] + wanted;
            }
            let slab = thickness.map(|_| value);
            return Ok((section.part(local), slab));
        }
    };
    Ok((changed, thickness))
}

/// The two-letter command RO: R and then O within `SEQUENCE_TIME`, both
/// typed while no text field has the keyboard. Any other key in between,
/// also one without a character such as Space or Escape, a key a text field
/// takes or a pause too long starts it over.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct KeySequence {
    r_at: Option<Instant>,
}

impl KeySequence {
    /// A key with this character was typed at `now`, taken by a text field
    /// when `in_text`. Answers whether it completes RO.
    pub fn typed(&mut self, character: &str, now: Instant, in_text: bool) -> bool {
        let earlier = self.r_at.take();
        if in_text {
            return false;
        }
        if character.eq_ignore_ascii_case("o") {
            return earlier.is_some_and(|at| now.saturating_duration_since(at) <= SEQUENCE_TIME);
        }
        if character.eq_ignore_ascii_case("r") {
            self.r_at = Some(now);
        }
        false
    }

    /// A key without a character, such as Space, Enter, Tab, an arrow or
    /// Escape, was pressed: RO starts over. Shift and the other modifiers
    /// do not count.
    pub fn interrupt(&mut self) {
        self.r_at = None;
    }
}

/// Whether a key without a character only changes the keys typed with it:
/// Shift, Control, Alt and the like, and the lock keys.
pub fn is_modifier_key(named: iced::keyboard::key::Named) -> bool {
    use iced::keyboard::key::Named;
    matches!(
        named,
        Named::Shift
            | Named::Control
            | Named::Alt
            | Named::AltGraph
            | Named::Super
            | Named::Meta
            | Named::Hyper
            | Named::Fn
            | Named::FnLock
            | Named::Symbol
            | Named::SymbolLock
            | Named::CapsLock
            | Named::NumLock
            | Named::ScrollLock
    )
}

/// What RO turns, by what the window shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnStart {
    /// The crop region of the plan of the Project Browser with this
    /// identifier, shown in the Drawing view.
    Plan(String),
    /// The Drawing view shows an elevation or a section.
    NotAPlan,
    /// The Drawing view shows a drawing without a crop region: a preview,
    /// an export, a file, or nothing.
    NoCropRegion,
    /// The section box in the 3D view.
    SectionBox,
    /// The 3D view without a section box.
    NoSectionBox,
}

/// What RO turns: in the Drawing view the crop region of a plan of the
/// Project Browser, `sheet` being the drawing shown with its view; in the 3D
/// view the section box while it is on.
pub fn turn_start(
    drawing_shown: bool,
    sheet: Option<(&str, DrawingView)>,
    section_on: bool,
) -> TurnStart {
    if drawing_shown {
        return match sheet {
            Some((guid, DrawingView::Plan)) => TurnStart::Plan(guid.to_owned()),
            Some(_) => TurnStart::NotAPlan,
            None => TurnStart::NoCropRegion,
        };
    }
    if section_on {
        TurnStart::SectionBox
    } else {
        TurnStart::NoSectionBox
    }
}

/// What a turn under way turns.
#[derive(Debug, Clone, PartialEq)]
pub enum TurnTarget {
    /// The crop region of a plan, by the identifier of the drawing, with its
    /// centre in the units of the drawing.
    Plan { guid: String, centre: [f64; 2] },
    /// The section box as it was when the turn started.
    SectionBox(OrientedBox),
}

/// A turn under way, started with RO.
#[derive(Debug, Clone, PartialEq)]
pub struct Turning {
    pub target: TurnTarget,
    /// Where the pointer was when the turn started: a point of the drawing
    /// over the Drawing view, a pixel over the 3D view; nothing until the
    /// pointer is known.
    start: Option<[f64; 2]>,
    /// The turn the pointer gives, in degrees, not rounded.
    pointer: f64,
    /// A number of degrees typed.
    typed: String,
    /// The section box as the turn last left it, also while it is off: a
    /// box set from elsewhere meanwhile, by a view restored or limits
    /// typed, ends the turn and stays.
    pub placed: Option<OrientedBox>,
}

impl Turning {
    pub fn new(target: TurnTarget, pointer: Option<[f64; 2]>) -> Self {
        let placed = match target {
            TurnTarget::SectionBox(original) => Some(original),
            TurnTarget::Plan { .. } => None,
        };
        Self {
            target,
            start: pointer,
            pointer: 0.0,
            typed: String::new(),
            placed,
        }
    }

    /// Where the pointer was when the turn started; `at` when it was not
    /// known yet.
    pub fn start_at(&mut self, at: [f64; 2]) -> [f64; 2] {
        *self.start.get_or_insert(at)
    }

    /// The turn the pointer now gives, in degrees.
    pub fn set_pointer(&mut self, degrees: f64) {
        if degrees.is_finite() {
            self.pointer = normalized_degrees(degrees);
        }
    }

    pub fn typed(&self) -> &str {
        &self.typed
    }

    /// A typed character: a digit, a minus sign, or a decimal point or
    /// comma. Answers whether it belongs to the number.
    pub fn type_char(&mut self, character: char) -> bool {
        let fits = character.is_ascii_digit() || matches!(character, '-' | '.' | ',');
        if fits && self.typed.chars().count() < MAX_TYPED {
            self.typed.push(character);
        }
        fits
    }

    /// Take the last typed character away; false when nothing was typed.
    pub fn backspace(&mut self) -> bool {
        self.typed.pop().is_some()
    }

    /// The turn in degrees, counter-clockwise: the number typed, else what
    /// the pointer gives in whole degrees, or with Shift in steps of 15.
    /// Nothing while what is typed is no number yet.
    pub fn degrees(&self, shift: bool) -> Option<f64> {
        if !self.typed.is_empty() {
            return parse_rotation(&self.typed).map(|degrees| normalized_degrees(degrees) + 0.0);
        }
        let step = if shift { SHIFT_STEP } else { 1.0 };
        Some(normalized_degrees((self.pointer / step).round() * step) + 0.0)
    }

    /// The angle as it is shown beside the centre.
    pub fn label(&self, shift: bool) -> String {
        if !self.typed.is_empty() {
            return format!("{}°", self.typed);
        }
        format!(
            "{}°",
            format_rotation(self.degrees(shift).unwrap_or_default())
        )
    }
}

/// The direction of a point seen from a centre, in degrees counter-clockwise
/// from the X axis; nothing for the centre itself.
pub fn angle_about(centre: [f64; 2], point: [f64; 2]) -> Option<f64> {
    let (dx, dy) = (point[0] - centre[0], point[1] - centre[1]);
    (dx.hypot(dy) > 0.0).then(|| dy.atan2(dx).to_degrees())
}

/// The corners of a rectangle turned `degrees` counter-clockwise about its
/// middle, from the lower left one round.
pub fn turned_corners(rect: [[f64; 2]; 2], degrees: f64) -> [[f64; 2]; 4] {
    let centre = middle(rect);
    let (sin, cos) = degrees.to_radians().sin_cos();
    [
        [rect[0][0], rect[0][1]],
        [rect[1][0], rect[0][1]],
        [rect[1][0], rect[1][1]],
        [rect[0][0], rect[1][1]],
    ]
    .map(|[x, y]| {
        let (dx, dy) = (x - centre[0], y - centre[1]);
        [
            centre[0] + cos * dx - sin * dy,
            centre[1] + sin * dx + cos * dy,
        ]
    })
}

/// A drawing being made again in place: after a change of its crop region
/// the view keeps where it looks and which layers it shows.
#[derive(Debug, Clone)]
pub(crate) struct Remake {
    pub guid: String,
    /// How the drawing is made again.
    pub definition: SavedDrawing,
    /// The camera that keeps the drawing where it was on the sheet.
    pub camera: Option<ViewCamera>,
    /// The command of the local API that asked for it.
    pub operation: &'static str,
    /// Which layers were shown, by name.
    pub layers: Vec<(String, bool)>,
}

/// The crop region of the drawing shown, as the Drawing view draws it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CropOverlay {
    pub guid: String,
    /// The region in the units of the drawing: lower left and upper right.
    pub rect: [[f64; 2]; 2],
    /// Drawing units in a metre.
    pub unit: f64,
    /// Its four corners as they are drawn: turned while RO turns it, the new
    /// region while the drawing is made again.
    pub corners: [[f64; 2]; 4],
    /// While RO turns it: the angle as it is shown.
    pub turn: Option<String>,
    /// Its handles can be dragged, once it is selected.
    pub editable: bool,
    /// A click on its outline selected it: it is drawn thicker, with its
    /// handles, and Properties shows its figures.
    pub selected: bool,
}

/// What the crop region and RO react to.
#[derive(Debug, Clone)]
pub enum CropAction {
    /// Show or hide the crop region in the Drawing view.
    Show(bool),
    /// Select the crop region of the drawing shown, or deselect it: a click
    /// on its outline, or elsewhere on the sheet.
    Select(bool),
    /// A handle was let go with the crop region at this rectangle, in the
    /// units of the drawing with this identifier.
    Set(String, [[f64; 2]; 2]),
    /// The text of a field of the Crop region section.
    Field(Field, String),
    /// Enter in a field.
    Apply(Field),
    /// The pointer moved over the Drawing view while turning: a point of the
    /// drawing.
    TurnPointer([f64; 2]),
    /// The pointer moved over the 3D view while turning: a pixel of a
    /// viewport of this size.
    TurnPointer3d([f32; 2], Size),
    TurnApply,
    TurnCancel,
}

/// The choices of `set_sheet_crop`, each one left out keeping what the
/// drawing has.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SheetCropOptions {
    /// The drawing; without it the one the Drawing view shows.
    #[serde(default)]
    pub name: Option<String>,
    /// The crop region as a handle drag leaves it: lower left and upper
    /// right in the units of the drawing.
    #[serde(default)]
    pub rect: Option<[[f64; 2]; 2]>,
    #[serde(default)]
    pub width: Option<f64>,
    #[serde(default)]
    pub height: Option<f64>,
    /// A plan: model X and Y. An elevation or a section: the place along it
    /// and the height.
    #[serde(default)]
    pub center: Option<[f64; 2]>,
    #[serde(default)]
    pub rotation: Option<f64>,
    #[serde(default)]
    pub cut: Option<f64>,
    #[serde(default)]
    pub depth: Option<f64>,
    /// The points used, in percent.
    #[serde(default)]
    pub sample_percent: Option<f64>,
}

/// The figures and the rectangle of the crop region of a drawing, for the
/// local API.
pub(crate) fn crop_value(definition: &SavedDrawing) -> Value {
    let Some(request) = definition.request() else {
        return Value::Null;
    };
    let section = definition.oriented();
    let shown = figures(section, request.view, request.thickness);
    let rect = crop_frame(section, request.view, request.origin)
        .map(|crop| scaled(crop.rect, request.units.factor()));
    json!({
        "width": shown.width,
        "height": shown.height,
        "center": shown.centre,
        "rotation": shown.rotation,
        "cut": shown.cut,
        "depth": shown.depth,
        "rect": rect,
        "units": request.units.key(),
        "sample_percent": request.sample_percent,
    })
}

impl Studio {
    /// How the drawing of the Project Browser in the Drawing view was made,
    /// while it is shown.
    pub(crate) fn shown_sheet(&self) -> Option<&SavedDrawing> {
        let guid = self.drawing_view.shown_guid()?;
        self.drawing_view
            .saved
            .iter()
            .find(|drawing| drawing.guid == guid)
    }

    /// The crop region of the drawing shown, as the Drawing view draws it:
    /// while it is switched on, and always while RO turns it.
    pub(crate) fn crop_overlay(&self) -> Option<CropOverlay> {
        let definition = self.shown_sheet()?;
        let request = definition.request()?;
        let unit = request.units.factor();
        let crop = crop_frame(definition.oriented(), request.view, request.origin)?;
        let rect = scaled(crop.rect, unit);
        let turning = self.turn.as_ref().filter(|turn| {
            matches!(&turn.target, TurnTarget::Plan { guid, .. } if *guid == definition.guid)
        });
        if !self.drawing_view.crop_shown && turning.is_none() {
            return None;
        }
        let shift = self.modifiers.shift();
        let mut corners = turned_corners(rect, 0.0);
        // A locked drawing keeps its crop region.
        let mut editable = !self.drawing.busy() && !definition.locked;
        if let Some(turn) = turning {
            corners = turned_corners(rect, turn.degrees(shift).unwrap_or_default());
            editable = false;
        }
        // While it is made again, the region it is made with, on the drawing
        // that is still shown.
        if let Some(remake) = self
            .drawing_view
            .remake
            .as_ref()
            .filter(|remake| remake.guid == definition.guid)
        {
            editable = false;
            if let Some(new) =
                crop_frame(remake.definition.oriented(), request.view, request.origin)
            {
                let [low, high] = new.rect;
                corners = [low, [high[0], low[1]], high, [low[0], high[1]]].map(|corner| {
                    let at = crop.frame.to_uv(new.frame.to_world(corner));
                    [at[0] * unit, at[1] * unit]
                });
            }
        }
        Some(CropOverlay {
            selected: self.drawing_view.crop_selected.as_deref() == Some(definition.guid.as_str()),
            guid: definition.guid.clone(),
            rect,
            unit,
            corners,
            turn: turning.map(|turn| turn.label(shift)),
            editable,
        })
    }

    /// Select the crop region of the drawing shown, or deselect it; false
    /// when nothing changed.
    pub(crate) fn select_crop(&mut self, on: bool) -> bool {
        if !on {
            let was = self.drawing_view.crop_selected.take().is_some();
            if was {
                self.status = "Crop region deselected".into();
            }
            return was;
        }
        let Some(crop) = self.crop_overlay() else {
            return false;
        };
        if crop.selected {
            return false;
        }
        let name = self
            .shown_sheet()
            .map(|definition| definition.name.clone())
            .unwrap_or_default();
        self.drawing_view.crop_selected = Some(crop.guid);
        self.status = format!(
            "Crop region of {name} selected: set its figures in Properties or drag a handle; Escape deselects it"
        );
        true
    }

    pub(crate) fn update_crop(&mut self, action: CropAction) -> Task<Message> {
        match action {
            CropAction::Select(on) => {
                self.select_crop(on);
            }
            CropAction::Show(on) => {
                if !on {
                    self.drawing_view.crop_selected = None;
                }
                self.drawing_view.crop_shown = on;
                self.status = if on {
                    "Crop region shown; click its outline to select it and change it"
                } else {
                    "Crop region hidden"
                }
                .into();
            }
            CropAction::Set(guid, rect) => {
                self.drawing_view.held = None;
                match self.set_crop_rect(&guid, rect, None) {
                    Ok(task) => return task,
                    Err(reason) => self.status = reason,
                }
            }
            CropAction::Field(field, value) => {
                let guid = self.drawing_view.shown_guid().map(str::to_owned);
                let edits = &mut self.drawing_view.crop_edits;
                if edits.0 != guid {
                    *edits = (guid, Vec::new());
                }
                match edits.1.iter_mut().find(|(known, _)| *known == field) {
                    Some((_, text)) => *text = value,
                    None => edits.1.push((field, value)),
                }
            }
            CropAction::Apply(field) => {
                let typed = self
                    .drawing_view
                    .crop_edits
                    .1
                    .iter()
                    .find(|(known, _)| *known == field)
                    .map(|(_, text)| text.clone());
                let Some(typed) = typed else {
                    return Task::none();
                };
                let Some(guid) = self.drawing_view.shown_guid().map(str::to_owned) else {
                    return Task::none();
                };
                let value = match field {
                    Field::Rotation => parse_rotation(&typed),
                    Field::Points => parse_percent(&typed),
                    _ => typed.trim().replace(',', ".").parse::<f64>().ok(),
                };
                let Some(value) = value.filter(|value| value.is_finite()) else {
                    self.status = "Type a number".into();
                    return Task::none();
                };
                match self.set_crop_figures(&guid, &[(field, value)], None) {
                    Ok(task) => {
                        self.drawing_view
                            .crop_edits
                            .1
                            .retain(|(known, _)| *known != field);
                        return task;
                    }
                    Err(reason) => self.status = reason,
                }
            }
            CropAction::TurnPointer(at) => self.turn_pointer(at),
            CropAction::TurnPointer3d(pixel, size) => return self.turn_pointer_3d(pixel, size),
            CropAction::TurnApply => return self.apply_turn(),
            CropAction::TurnCancel => return self.cancel_turn(),
        }
        Task::none()
    }

    /// A drawing of the Project Browser by its identifier, with how it was
    /// made, or why it cannot change.
    fn crop_definition(&self, guid: &str) -> Result<SavedDrawing, String> {
        self.drawing_view
            .saved
            .iter()
            .find(|drawing| drawing.guid == guid)
            .cloned()
            .ok_or_else(|| "That drawing is no longer kept".to_owned())
    }

    /// A drawing whose crop region may change now: one that is not being
    /// made again already.
    fn editable_definition(&self, guid: &str) -> Result<SavedDrawing, String> {
        let definition = self.crop_definition(guid)?;
        if definition.locked {
            return Err(format!("{} is locked", definition.name));
        }
        if self
            .drawing_view
            .remake
            .as_ref()
            .is_some_and(|remake| remake.guid == guid)
        {
            return Err(format!(
                "The drawing {} is being made again; wait for it",
                definition.name
            ));
        }
        Ok(definition)
    }

    /// The crop region of a drawing set to a rectangle in its units, as a
    /// handle drag leaves it: the box follows and the drawing is made again.
    pub(crate) fn set_crop_rect(
        &mut self,
        guid: &str,
        rect: [[f64; 2]; 2],
        api_job_id: Option<String>,
    ) -> Result<Task<Message>, String> {
        let definition = self.editable_definition(guid)?;
        let request = definition
            .request()
            .ok_or_else(|| "The settings of the drawing cannot be used".to_owned())?;
        let unit = request.units.factor();
        let crop = crop_frame(definition.oriented(), request.view, request.origin)
            .ok_or_else(|| "The drawing has no crop region".to_owned())?;
        let new = scaled(rect, 1.0 / unit);
        let finite = new.iter().flatten().all(|value| value.is_finite());
        let least = MIN_SIDE - 1e-6;
        if !finite || new[1][0] - new[0][0] < least || new[1][1] - new[0][1] < least {
            return Err(format!(
                "The crop region must be at least {MIN_SIDE:.2} m wide and high"
            ));
        }
        if new == crop.rect {
            return Ok(Task::none());
        }
        let mut changed = definition.clone();
        let section = box_for_crop(definition.oriented(), request.view, crop.rect, new);
        changed.section = crate::saved_drawings::DrawingBox {
            min: section.bounds.min,
            max: section.bounds.max,
            rotation: section.rotation_degrees,
        };
        self.remake_in_place(changed, crop.centre(), "set_sheet_crop", api_job_id)
    }

    /// Figures of the crop region of a drawing set one after the other, as
    /// Properties and `set_sheet_crop` set them; the drawing is made again.
    pub(crate) fn set_crop_figures(
        &mut self,
        guid: &str,
        changes: &[(Field, f64)],
        api_job_id: Option<String>,
    ) -> Result<Task<Message>, String> {
        let definition = self.editable_definition(guid)?;
        let request = definition
            .request()
            .ok_or_else(|| "The settings of the drawing cannot be used".to_owned())?;
        let crop = crop_frame(definition.oriented(), request.view, request.origin)
            .ok_or_else(|| "The drawing has no crop region".to_owned())?;
        let (mut section, mut thickness) = (definition.oriented(), definition.thickness);
        let mut percent = request.sample_percent;
        for (field, value) in changes {
            if *field == Field::Points {
                if let Some(problem) = percent_problem(*value) {
                    return Err(problem);
                }
                percent = *value;
                continue;
            }
            (section, thickness) = with_figure(section, request.view, thickness, *field, *value)?;
        }
        if crop_frame(section, request.view, request.origin).is_none() {
            return Err("The crop region must have a size".into());
        }
        let mut changed = definition.clone();
        changed.section = crate::saved_drawings::DrawingBox {
            min: section.bounds.min,
            max: section.bounds.max,
            rotation: section.rotation_degrees,
        };
        changed.thickness = thickness;
        changed.request.sample_percent = percent;
        if changed == definition {
            return Ok(Task::none());
        }
        let operation = if changes.iter().all(|(field, _)| *field == Field::Rotation) {
            "rotate_crop"
        } else {
            "set_sheet_crop"
        };
        self.remake_in_place(changed, crop.centre(), operation, api_job_id)
    }

    /// Turn the crop region of a plan `degrees` counter-clockwise on the
    /// sheet: its box turns as far about the vertical through the centre of
    /// the region, and the plan is made again upright in it.
    pub(crate) fn turn_plan(
        &mut self,
        guid: &str,
        degrees: f64,
        api_job_id: Option<String>,
    ) -> Result<Task<Message>, String> {
        let definition = self.editable_definition(guid)?;
        let request = definition
            .request()
            .ok_or_else(|| "The settings of the drawing cannot be used".to_owned())?;
        if request.view != DrawingView::Plan {
            return Err("Only the crop region of a plan turns".into());
        }
        let crop = crop_frame(definition.oriented(), request.view, request.origin)
            .ok_or_else(|| "The drawing has no crop region".to_owned())?;
        if degrees == 0.0 {
            return Ok(Task::none());
        }
        let section = turned(definition.oriented(), degrees);
        let mut changed = definition.clone();
        changed.section.rotation = section.rotation_degrees;
        self.remake_in_place(changed, crop.centre(), "rotate_crop", api_job_id)
    }

    /// Make a drawing again from a changed definition, under its name, in
    /// place: when it is the drawing the Drawing view holds, the point
    /// `anchor` of the model stays where it is on the sheet, at the same
    /// zoom, and the layers that were switched off stay off. The window
    /// stays on what it shows when the drawing is made.
    pub(crate) fn remake_in_place(
        &mut self,
        definition: SavedDrawing,
        anchor: [f64; 3],
        operation: &'static str,
        api_job_id: Option<String>,
    ) -> Result<Task<Message>, String> {
        if self
            .drawing_view
            .remake
            .as_ref()
            .is_some_and(|remake| remake.guid == definition.guid)
        {
            return Err(format!(
                "The drawing {} is being made again; wait for it",
                definition.name
            ));
        }
        let kept = self.crop_definition(&definition.guid)?;
        let request = definition
            .request()
            .ok_or_else(|| "The settings of the drawing cannot be used".to_owned())?;
        // The view holds the drawing, shown or behind the 3D scene.
        let shown = self.drawing_view.current_guid() == Some(definition.guid.as_str());
        let camera = shown
            .then(|| {
                let old = crop_frame(kept.oriented(), request.view, request.origin)?;
                let new = crop_frame(definition.oriented(), request.view, request.origin)?;
                let unit = request.units.factor();
                let camera = self.drawing_view.camera();
                let [was, now] = [old, new].map(|crop| crop.frame.to_uv(anchor));
                Some(ViewCamera {
                    center: [
                        camera.center[0] + (now[0] - was[0]) * unit,
                        camera.center[1] + (now[1] - was[1]) * unit,
                    ],
                    scale: camera.scale,
                })
            })
            .flatten();
        let layers = if shown {
            self.drawing_view.layer_switches()
        } else {
            Vec::new()
        };
        let task = self.remake_sheet(definition.clone(), api_job_id)?;
        self.drawing_view.remake = Some(Remake {
            guid: definition.guid.clone(),
            definition,
            camera,
            operation,
            layers,
        });
        Ok(task)
    }

    /// The status line once a drawing was made again in place.
    pub(crate) fn remade_status(
        definition: &SavedDrawing,
        operation: &str,
        reused: bool,
    ) -> String {
        if crate::layouts::made_for_sheet(operation) {
            return format!("Drawing {} made for the sheet", definition.name);
        }
        let Some(request) = definition.request() else {
            return format!("Drawing {} made again", definition.name);
        };
        let shown = figures(definition.oriented(), request.view, request.thickness);
        if operation == "rotate_crop" {
            format!(
                "Crop region of {} turned: the box stands at {}°, the plan is made again upright in it",
                definition.name,
                format_rotation(shown.rotation)
            )
        } else {
            format!(
                "Crop region of {} set to {:.2} × {:.2} m and the drawing made again{}",
                definition.name,
                shown.width,
                shown.height,
                if reused {
                    " from the points it had read"
                } else {
                    ""
                }
            )
        }
    }

    /// RO was typed: start turning what the window shows, or say why
    /// nothing turns.
    pub(crate) fn start_turn(&mut self) -> Task<Message> {
        let sheet = self
            .shown_sheet()
            .and_then(|drawing| Some((drawing.guid.clone(), drawing.request()?.view)));
        let start = turn_start(
            self.drawing_view.shown,
            sheet.as_ref().map(|(guid, view)| (guid.as_str(), *view)),
            self.section_box().is_some(),
        );
        match start {
            TurnStart::Plan(guid) if self.drawing_locked(&guid) => {
                if let Some(definition) = self.shown_sheet() {
                    self.status = crate::locks::locked_status(&definition.name);
                }
            }
            TurnStart::SectionBox if self.locked_scene_view().is_some() => {
                self.status =
                    crate::locks::locked_status(&self.locked_scene_view().unwrap_or_default());
            }
            TurnStart::Plan(guid) => {
                if self.drawing.busy() {
                    self.status =
                        "Wait until the drawing is made before turning its crop region".into();
                    return Task::none();
                }
                let Some(centre) = self.crop_overlay_centre(&guid) else {
                    self.status = "The drawing has no crop region".into();
                    return Task::none();
                };
                let pointer = self.drawing_view.pointer();
                self.turn = Some(Turning::new(TurnTarget::Plan { guid, centre }, pointer));
                self.turn_status();
            }
            TurnStart::NotAPlan => {
                self.status = "RO turns the crop region of a plan; an elevation or a section keeps its direction".into();
            }
            TurnStart::NoCropRegion => {
                self.status = "RO turns the crop region of a plan made with Create 2D plan / elevation / section".into();
            }
            TurnStart::SectionBox => {
                let Some(original) = self.section_box() else {
                    return Task::none();
                };
                let pointer = self
                    .viewport_pointer
                    .get()
                    .map(|[x, y]| [f64::from(x), f64::from(y)]);
                self.turn = Some(Turning::new(TurnTarget::SectionBox(original), pointer));
                self.turn_status();
            }
            TurnStart::NoSectionBox => {
                self.status = "RO turns the section box: switch the section box on first".into();
            }
        }
        Task::none()
    }

    /// The centre of the crop region of a drawing, in its units.
    fn crop_overlay_centre(&self, guid: &str) -> Option<[f64; 2]> {
        let definition = self
            .drawing_view
            .saved
            .iter()
            .find(|drawing| drawing.guid == guid)?;
        let request = definition.request()?;
        let crop = crop_frame(definition.oriented(), request.view, request.origin)?;
        Some(middle(scaled(crop.rect, request.units.factor())))
    }

    /// The status line while turning.
    fn turn_status(&mut self) {
        let Some(turn) = &self.turn else {
            return;
        };
        let angle = turn.label(self.modifiers.shift());
        let subject = match turn.target {
            TurnTarget::Plan { .. } => "Turning the crop region",
            TurnTarget::SectionBox(_) => "Turning the section box",
        };
        // While walking the pointer looks around, so only a typed angle
        // turns the box.
        let how = if self.walk.is_some() && matches!(turn.target, TurnTarget::SectionBox(_)) {
            "Type an angle; while walking the pointer does not turn it"
        } else {
            "Move the pointer or type an angle (Shift: steps of 15°)"
        };
        self.status = format!(
            "{subject}: {angle}. {how}; Enter or a click applies, Escape or a right click cancels"
        );
    }

    /// The pointer moved over the Drawing view while the crop region turns.
    fn turn_pointer(&mut self, at: [f64; 2]) {
        let Some(turn) = &mut self.turn else {
            return;
        };
        let TurnTarget::Plan { centre, .. } = turn.target else {
            return;
        };
        let start = turn.start_at(at);
        if let (Some(from), Some(to)) = (angle_about(centre, start), angle_about(centre, at)) {
            turn.set_pointer(to - from);
        }
        self.turn_status();
    }

    /// The pointer moved over the 3D view while the section box turns: the
    /// box turns with it, as the turning handles turn it. Not while walking:
    /// the angle the pointer gives is of the orbit camera, which is not the
    /// one drawn then.
    fn turn_pointer_3d(&mut self, pixel: [f32; 2], size: Size) -> Task<Message> {
        if self.walk.is_some() {
            return Task::none();
        }
        let Some(turn) = &mut self.turn else {
            return Task::none();
        };
        let TurnTarget::SectionBox(original) = turn.target else {
            return Task::none();
        };
        let Some(scene) = combined_bounds(&self.clouds) else {
            return Task::none();
        };
        let projection = Projection::new(
            scene,
            self.yaw,
            self.pitch,
            self.zoom,
            self.pan,
            size.width,
            size.height,
        );
        let start = turn.start_at([f64::from(pixel[0]), f64::from(pixel[1])]);
        let start = iced::Point::new(start[0] as f32, start[1] as f32);
        if let Some(degrees) = section_turn_delta(
            original,
            projection,
            start,
            iced::Point::new(pixel[0], pixel[1]),
        ) {
            turn.set_pointer(degrees);
        }
        self.turn_changed()
    }

    /// The angle of the turn under way changed: the status line follows, and
    /// so does the section box while it turns.
    pub(crate) fn turn_changed(&mut self) -> Task<Message> {
        self.turn_status();
        let shift = self.modifiers.shift();
        let Some(Turning {
            target: TurnTarget::SectionBox(original),
            ..
        }) = &self.turn
        else {
            return Task::none();
        };
        let original = *original;
        let Some(degrees) = self.turn.as_ref().and_then(|turn| turn.degrees(shift)) else {
            return Task::none();
        };
        let rotation = normalized_degrees(original.rotation_degrees + degrees) + 0.0;
        if self.section_box().map(|section| section.rotation_degrees) == Some(rotation) {
            return Task::none();
        }
        if !self.place_section(OrientedBox::new(original.bounds, rotation)) {
            return Task::none();
        }
        let placed = self.section_shape();
        if let Some(turn) = &mut self.turn {
            turn.placed = placed;
        }
        self.sync_section_coordinate_inputs();
        self.revision += 1;
        self.schedule_detail()
    }

    /// Enter or a left click: apply the turn under way.
    pub(crate) fn apply_turn(&mut self) -> Task<Message> {
        let Some(turn) = self.turn.take() else {
            return Task::none();
        };
        let Some(degrees) = turn.degrees(self.modifiers.shift()) else {
            self.turn = Some(turn);
            self.status = "Type a number of degrees, or press Escape to cancel the turn".into();
            return Task::none();
        };
        match turn.target {
            TurnTarget::Plan { guid, .. } => {
                if degrees == 0.0 {
                    self.status = "Crop region not turned".into();
                    return Task::none();
                }
                match self.turn_plan(&guid, degrees, None) {
                    Ok(task) => task,
                    Err(reason) => {
                        self.status = reason;
                        Task::none()
                    }
                }
            }
            TurnTarget::SectionBox(original) => {
                let rotation = normalized_degrees(original.rotation_degrees + degrees) + 0.0;
                if !self.place_section(OrientedBox::new(original.bounds, rotation)) {
                    return Task::none();
                }
                self.section_enabled = true;
                self.sync_section_coordinate_inputs();
                self.revision += 1;
                self.status = format!(
                    "Section box turned to {}° about its centre",
                    format_rotation(self.section_rotation)
                );
                self.schedule_detail()
            }
        }
    }

    /// Escape or a right click: stop turning, and put the section box back
    /// as it was.
    pub(crate) fn cancel_turn(&mut self) -> Task<Message> {
        let Some(turn) = self.turn.take() else {
            return Task::none();
        };
        self.status = "Turn cancelled".into();
        if let TurnTarget::SectionBox(original) = turn.target {
            if self.section_box() != Some(original) && self.place_section(original) {
                self.sync_section_coordinate_inputs();
                self.revision += 1;
                return self.schedule_detail();
            }
        }
        Task::none()
    }

    /// After every message: a turn of a crop region ends when its drawing is
    /// no longer shown, and a turn of the section box when the 3D view is no
    /// longer shown or the box is off. A section box set from elsewhere
    /// while it turns, by a view restored, Reset box or limits typed, ends
    /// the turn and stays as it was set.
    pub(crate) fn settle_turn(&mut self) {
        let Some(turn) = &self.turn else {
            return;
        };
        if matches!(turn.target, TurnTarget::SectionBox(_)) && self.section_shape() != turn.placed {
            self.turn = None;
            self.status = if self.status.starts_with("Turning the section box") {
                "The turn ended: the section box was set anew".into()
            } else {
                format!("{}; the turn of the section box ended", self.status)
            };
            return;
        }
        let lost = match &turn.target {
            TurnTarget::Plan { guid, .. } => {
                self.drawing_view.shown_guid() != Some(guid.as_str()) || self.model_covered()
            }
            TurnTarget::SectionBox(_) => {
                self.drawing_view.shown || !self.section_enabled || self.model_covered()
            }
        };
        if lost {
            let _ = self.cancel_turn();
        }
    }

    /// The Crop region section of Properties, while the crop region of the
    /// drawing shown is selected: its figures and the points used, each set
    /// with Enter.
    pub(crate) fn crop_properties(&self) -> Option<Element<'_, Message>> {
        let definition = self.shown_sheet()?;
        if self.drawing_view.crop_selected.as_deref() != Some(definition.guid.as_str()) {
            return None;
        }
        let request = definition.request()?;
        let shown = figures(definition.oriented(), request.view, request.thickness);
        let edits = &self.drawing_view.crop_edits;
        let typed = |field: Field| {
            (edits.0.as_deref() == Some(definition.guid.as_str()))
                .then(|| {
                    edits
                        .1
                        .iter()
                        .find(|(known, _)| *known == field)
                        .map(|(_, text)| text.clone())
                })
                .flatten()
        };
        let mut block = column![opencad_properties::section_header("Crop region")]
            .spacing(0)
            .width(Fill);
        for field in Field::of(request.view) {
            let field = *field;
            let value = typed(field).unwrap_or_else(|| field.text(&shown, request.sample_percent));
            let input = text_input("", &value)
                .on_input(move |value| Message::Crop(CropAction::Field(field, value)))
                .on_submit(Message::Crop(CropAction::Apply(field)))
                .size(11)
                .padding([2, 4]);
            block = block.push(opencad_properties::property_control(
                field.label(request.view),
                input.into(),
            ));
        }
        let colors = self.ui_theme.colors();
        block = block.push(
            container(
                text(crate::i18n::tr(
                    "Enter applies a value and makes the drawing again, from the points it read as long as the cut, the view depth and the points used stay. Drag a handle of the crop region on the sheet; RO turns the crop region of a plan; Escape deselects it.",
                ))
                .size(10)
                .color(colors.muted),
            )
            .padding([4, 8]),
        );
        Some(block.into())
    }

    /// The `set_sheet_crop` command of the local API.
    pub(crate) fn api_set_sheet_crop(
        &mut self,
        options: &SheetCropOptions,
    ) -> (Value, Task<Message>) {
        let refuse = |error: String| (json!({"ok": false, "error": error}), Task::none());
        let guid =
            match options.name.as_deref() {
                Some(name) => match self.drawing_named(name) {
                    Some(guid) => guid,
                    None => return refuse(format!("no drawing {name} of an open scan")),
                },
                None => match self.drawing_view.shown_guid() {
                    Some(guid) => guid.to_owned(),
                    None => return refuse(
                        "name a drawing, or show one of the Project Browser in the Drawing view"
                            .into(),
                    ),
                },
            };
        let Ok(definition) = self.crop_definition(&guid) else {
            return refuse("that drawing is no longer kept".into());
        };
        let plan = definition
            .request()
            .is_some_and(|request| request.view == DrawingView::Plan);
        if options.rotation.is_some() && !plan {
            return refuse("only the crop region of a plan turns".into());
        }
        let mut changes = Vec::new();
        let mut push = |field: Field, value: Option<f64>| {
            if let Some(value) = value {
                changes.push((field, value));
            }
        };
        push(Field::Width, options.width);
        push(Field::Height, options.height);
        push(Field::CentreA, options.center.map(|centre| centre[0]));
        push(Field::CentreB, options.center.map(|centre| centre[1]));
        push(Field::Rotation, options.rotation);
        push(Field::Cut, options.cut);
        push(Field::Depth, options.depth);
        push(Field::Points, options.sample_percent);
        if options.rect.is_none() && changes.is_empty() {
            return refuse(
                "give rect, width, height, center, rotation, cut, depth or sample_percent".into(),
            );
        }
        let id = self.record_api_job(json!({"state": "running", "operation": "set_sheet_crop"}));
        let result = match options.rect {
            Some(rect) => {
                // The rectangle first, then the figures on the box it gives.
                self.set_crop_rect(&guid, rect, Some(id.clone()))
                    .and_then(|task| {
                        if changes.is_empty() {
                            return Ok(task);
                        }
                        Err("give rect alone, or the figures without it".to_owned())
                    })
            }
            None => self.set_crop_figures(&guid, &changes, Some(id.clone())),
        };
        self.answer_crop_job(id, &guid, result)
    }

    /// The `drag_crop_handle` command of the local API: a handle of the crop
    /// region of the drawing shown dragged to a point of the drawing, in
    /// its units, as the pointer drags it. Held, the region is drawn as
    /// during a drag, with its size; let go, the drawing is made again.
    pub(crate) fn api_drag_crop_handle(
        &mut self,
        handle: &str,
        to: [f64; 2],
        release: bool,
    ) -> (Value, Task<Message>) {
        let refuse = |error: &str| (json!({"ok": false, "error": error}), Task::none());
        let Some(handle) = Handle::from_key(handle) else {
            return refuse("handle must be left, right, bottom, top, bottom_left, bottom_right, top_left or top_right");
        };
        if !to.iter().all(|value| value.is_finite()) {
            return refuse("to must be two numbers");
        }
        let Some(crop) = self.crop_overlay() else {
            return refuse(
                "the Drawing view shows no crop region: show a drawing of create_drawing with its crop region on",
            );
        };
        if !crop.editable {
            return refuse("the crop region cannot change while a drawing is made or RO turns it");
        }
        // The pointer drags a handle of a selected crop region only.
        self.drawing_view.crop_selected = Some(crop.guid.clone());
        let rect = dragged(crop.rect, handle, to, crop.unit);
        let size = [
            (rect[1][0] - rect[0][0]) / crop.unit,
            (rect[1][1] - rect[0][1]) / crop.unit,
        ];
        if !release {
            self.drawing_view.held = Some(crate::drawing_view::CropDrag {
                guid: crop.guid,
                handle,
                rect,
            });
            let value = json!({"ok": true, "held": true, "rect": rect, "width": size[0], "height": size[1]});
            return (value, Task::none());
        }
        self.drawing_view.held = None;
        let id = self.record_api_job(json!({"state": "running", "operation": "set_sheet_crop"}));
        let result = self.set_crop_rect(&crop.guid, rect, Some(id.clone()));
        let (mut value, task) = self.answer_crop_job(id, &crop.guid, result);
        if value["ok"] == true {
            value["rect"] = json!(rect);
            value["width"] = json!(size[0]);
            value["height"] = json!(size[1]);
        }
        (value, task)
    }

    /// The `select_crop_region` command of the local API: the crop region
    /// of the drawing shown selected or deselected, as a click on its
    /// outline or Escape does.
    pub(crate) fn api_select_crop_region(&mut self, selected: bool) -> Value {
        let Some(definition) = self.shown_sheet() else {
            return json!({"ok": false, "error": "the Drawing view shows no crop region: show a drawing of create_drawing with its crop region on"});
        };
        let crop = crop_value(definition);
        if self.crop_overlay().is_none() {
            return json!({"ok": false, "error": "the Drawing view shows no crop region: show a drawing of create_drawing with its crop region on"});
        }
        let changed = self.select_crop(selected);
        json!({
            "ok": true,
            "selected": selected,
            "changed": changed,
            "crop": crop,
        })
    }

    /// The answer of a command that makes a drawing again: its job, or that
    /// nothing changed, or why it cannot.
    fn answer_crop_job(
        &mut self,
        id: String,
        guid: &str,
        result: Result<Task<Message>, String>,
    ) -> (Value, Task<Message>) {
        match result {
            Ok(task) => {
                let remade = self
                    .drawing_view
                    .remake
                    .as_ref()
                    .filter(|remake| remake.guid == guid);
                match remade {
                    Some(remake) => {
                        let value = json!({
                            "ok": true,
                            "accepted": true,
                            "job_id": id,
                            "name": remake.definition.name,
                            "guid": guid,
                            "crop": crop_value(&remake.definition),
                        });
                        (value, task)
                    }
                    None => {
                        self.forget_api_job(&id);
                        (json!({"ok": true, "changed": false}), task)
                    }
                }
            }
            Err(error) => {
                self.forget_api_job(&id);
                (json!({"ok": false, "error": error}), Task::none())
            }
        }
    }

    /// The `rotate_crop` command of the local API: what RO turns, by a
    /// number of degrees. With `apply` false the turn starts as RO starts
    /// it, at the angle given, and waits for Enter, a click or Escape, or
    /// for this command with `apply` true and no angle.
    pub(crate) fn api_rotate_crop(
        &mut self,
        name: Option<&str>,
        degrees: Option<f64>,
        apply: bool,
    ) -> (Value, Task<Message>) {
        let refuse = |error: &str| (json!({"ok": false, "error": error}), Task::none());
        if degrees.is_some_and(|degrees| !degrees.is_finite() || degrees.abs() > 3_600.0) {
            return refuse("degrees must be a number from -3600 to 3600");
        }
        if apply && degrees.is_none() {
            let Some(turn) = &self.turn else {
                return refuse("give degrees, or start a turn with apply false first");
            };
            let Some(turned) = turn.degrees(self.modifiers.shift()) else {
                return refuse("the angle typed is no number yet");
            };
            // A turn of a plan answers with the job that makes it again.
            if let TurnTarget::Plan { guid, .. } = &turn.target {
                let guid = guid.clone();
                self.turn = None;
                let id =
                    self.record_api_job(json!({"state": "running", "operation": "rotate_crop"}));
                let result = self.turn_plan(&guid, turned, Some(id.clone()));
                return self.answer_crop_job(id, &guid, result);
            }
            let task = self.apply_turn();
            let mut answer = self.turn_answer();
            answer["section"] = self.section_value();
            return (answer, task);
        }
        // A turn under way gives way to the one asked for.
        let mut task = self.cancel_turn();
        let start = match name {
            Some(name) => {
                let Some(guid) = self.drawing_named(name) else {
                    return refuse("no drawing of that name of an open scan");
                };
                let plan = self
                    .crop_definition(&guid)
                    .ok()
                    .and_then(|drawing| drawing.request())
                    .is_some_and(|request| request.view == DrawingView::Plan);
                if plan {
                    TurnStart::Plan(guid)
                } else {
                    TurnStart::NotAPlan
                }
            }
            None => {
                let sheet = self
                    .shown_sheet()
                    .and_then(|drawing| Some((drawing.guid.clone(), drawing.request()?.view)));
                turn_start(
                    self.drawing_view.shown,
                    sheet.as_ref().map(|(guid, view)| (guid.as_str(), *view)),
                    self.section_box().is_some(),
                )
            }
        };
        match start {
            TurnStart::Plan(guid) if apply => {
                let degrees = degrees.unwrap_or_default();
                let id =
                    self.record_api_job(json!({"state": "running", "operation": "rotate_crop"}));
                let result = self.turn_plan(&guid, degrees, Some(id.clone()));
                let (value, more) = self.answer_crop_job(id, &guid, result);
                (value, Task::batch([task, more]))
            }
            TurnStart::Plan(guid) => {
                if self.drawing_view.shown_guid() != Some(guid.as_str()) {
                    return refuse(
                        "show the plan in the Drawing view before turning it with apply false",
                    );
                }
                let Some(centre) = self.crop_overlay_centre(&guid) else {
                    return refuse("the drawing has no crop region");
                };
                let mut turn = Turning::new(TurnTarget::Plan { guid, centre }, None);
                type_degrees(&mut turn, degrees);
                self.turn = Some(turn);
                self.turn_status();
                (self.turn_answer(), task)
            }
            TurnStart::NotAPlan => refuse("only the crop region of a plan turns"),
            TurnStart::NoCropRegion => refuse(
                "the Drawing view shows no drawing of the Project Browser; name one or show a plan",
            ),
            TurnStart::SectionBox => {
                let Some(original) = self.section_box() else {
                    return refuse("the section box is off");
                };
                let mut turn = Turning::new(TurnTarget::SectionBox(original), None);
                type_degrees(&mut turn, degrees);
                self.turn = Some(turn);
                task = Task::batch([task, self.turn_changed()]);
                if apply {
                    task = Task::batch([task, self.apply_turn()]);
                }
                let mut answer = self.turn_answer();
                answer["section"] = self.section_value();
                (answer, task)
            }
            TurnStart::NoSectionBox => refuse("the section box is off"),
        }
    }

    /// The turn under way as the local API reports it.
    pub(crate) fn turn_answer(&self) -> Value {
        json!({"ok": true, "turning": self.turn_value()})
    }

    /// The turn under way for `status`: what turns and by how much, or
    /// `null`.
    pub(crate) fn turn_value(&self) -> Value {
        let Some(turn) = &self.turn else {
            return Value::Null;
        };
        let shift = self.modifiers.shift();
        json!({
            "target": match turn.target {
                TurnTarget::Plan { .. } => "crop_region",
                TurnTarget::SectionBox(_) => "section_box",
            },
            "degrees": turn.degrees(shift),
            "typed": turn.typed(),
        })
    }
}

/// Type a number of degrees into a turn, as the keyboard would.
fn type_degrees(turn: &mut Turning, degrees: Option<f64>) {
    if let Some(degrees) = degrees {
        for character in format_rotation(degrees).chars() {
            turn.type_char(character);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOM: Bounds = Bounds {
        min: [2.0, 1.0, 0.0],
        max: [8.0, 5.0, 3.0],
    };

    fn near(a: [f64; 3], b: [f64; 3]) -> bool {
        (0..3).all(|axis| (a[axis] - b[axis]).abs() < 1e-9)
    }

    fn near2(a: [f64; 2], b: [f64; 2]) -> bool {
        (0..2).all(|axis| (a[axis] - b[axis]).abs() < 1e-9)
    }

    /// The corners of a crop region in the model.
    fn world_corners(crop: &CropFrame) -> [[f64; 3]; 4] {
        let [low, high] = crop.rect;
        [low, [high[0], low[1]], high, [low[0], high[1]]].map(|corner| crop.frame.to_world(corner))
    }

    #[test]
    fn the_crop_region_is_the_face_of_the_box_and_a_moved_side_moves_that_face_alone() {
        for rotation in [0.0, 30.0, -112.5] {
            let section = OrientedBox::new(ROOM, rotation);
            for view in DrawingView::ALL {
                for origin in DrawingOrigin::ALL {
                    let crop = crop_frame(section, view, origin).unwrap();
                    let size = section.size();
                    let layout = Layout::of(view);
                    let [low, high] = crop.rect;
                    assert!((high[0] - low[0] - size[layout.across]).abs() < 1e-9);
                    assert!((high[1] - low[1] - size[layout.up]).abs() < 1e-9);
                    // Every corner of the region is a corner of the box.
                    for corner in world_corners(&crop) {
                        assert!(
                            section.corners().iter().any(|known| near(*known, corner)),
                            "{view:?} {rotation} {corner:?}"
                        );
                    }
                    // Each side, and two at a corner, moves the faces under
                    // it and nothing else: the region of the new box is the
                    // new rectangle, in the model.
                    let moves = [
                        [[0.5, 0.0], [0.0, 0.0]],
                        [[0.0, 0.0], [-0.7, 0.0]],
                        [[0.0, 0.25], [0.0, 0.0]],
                        [[0.0, 0.0], [0.0, -0.4]],
                        [[-1.0, 0.3], [0.0, 0.0]],
                        [[0.0, 0.0], [1.5, 2.0]],
                    ];
                    for by in moves {
                        let new = [
                            [low[0] + by[0][0], low[1] + by[0][1]],
                            [high[0] + by[1][0], high[1] + by[1][1]],
                        ];
                        let changed = box_for_crop(section, view, crop.rect, new);
                        assert_eq!(changed.rotation_degrees, section.rotation_degrees);
                        let again = crop_frame(changed, view, origin).unwrap();
                        let wanted = CropFrame {
                            frame: crop.frame,
                            rect: new,
                        };
                        let got = world_corners(&again);
                        for corner in world_corners(&wanted) {
                            assert!(
                                got.iter().any(|known| near(*known, corner)),
                                "{view:?} {rotation} {by:?}"
                            );
                        }
                        // The depth of the box stays.
                        let depth = layout.look;
                        let (before, after) = (section.size()[depth], changed.size()[depth]);
                        assert!((before - after).abs() < 1e-9);
                        // The cut plane stays where it was.
                        assert!(near(again.frame.right, crop.frame.right));
                        let plane = |crop: &CropFrame| {
                            let normal = cross(crop.frame.right, crop.frame.up);
                            dot(normal, crop.frame.to_world(crop.rect[0]))
                        };
                        assert!((plane(&again) - plane(&crop)).abs() < 1e-9);
                    }
                }
            }
        }
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

    #[test]
    fn dragging_the_right_side_of_a_back_section_moves_the_local_minimum_of_x() {
        for rotation in [0.0, 30.0] {
            let section = OrientedBox::new(ROOM, rotation);
            let crop = crop_frame(section, DrawingView::Back, DrawingOrigin::Model).unwrap();
            let mut new = crop.rect;
            // One metre narrower from the right as seen from behind.
            new[1][0] -= 1.0;
            let changed = box_for_crop(section, DrawingView::Back, crop.rect, new);
            // In the frame of the old box: X min went up by a metre, the rest
            // stayed.
            let corners = changed.corners().map(|corner| section.to_box(corner));
            let low = corners.iter().map(|c| c[0]).fold(f64::INFINITY, f64::min);
            let high = corners
                .iter()
                .map(|c| c[0])
                .fold(f64::NEG_INFINITY, f64::max);
            assert!(
                (low - (ROOM.min[0] + 1.0)).abs() < 1e-9,
                "{rotation}: {low}"
            );
            assert!((high - ROOM.max[0]).abs() < 1e-9);
            let y = corners.iter().map(|c| c[1]).fold(f64::INFINITY, f64::min);
            assert!((y - ROOM.min[1]).abs() < 1e-9);
            // The right side of a front section is X max, of a left section
            // Y min and of a right section Y max.
            for (view, axis, at_max) in [
                (DrawingView::Front, 0, true),
                (DrawingView::Left, 1, false),
                (DrawingView::Right, 1, true),
                (DrawingView::Plan, 0, true),
            ] {
                let crop = crop_frame(section, view, DrawingOrigin::BoxCorner).unwrap();
                let mut new = crop.rect;
                new[1][0] -= 1.0;
                let changed = box_for_crop(section, view, crop.rect, new);
                let corners = changed.corners().map(|corner| section.to_box(corner));
                let low = corners
                    .iter()
                    .map(|c| c[axis])
                    .fold(f64::INFINITY, f64::min);
                let high = corners
                    .iter()
                    .map(|c| c[axis])
                    .fold(f64::NEG_INFINITY, f64::max);
                if at_max {
                    assert!((high - (ROOM.max[axis] - 1.0)).abs() < 1e-9, "{view:?}");
                    assert!((low - ROOM.min[axis]).abs() < 1e-9, "{view:?}");
                } else {
                    assert!((low - (ROOM.min[axis] + 1.0)).abs() < 1e-9, "{view:?}");
                    assert!((high - ROOM.max[axis]).abs() < 1e-9, "{view:?}");
                }
            }
        }
    }

    #[test]
    fn a_handle_moves_its_sides_to_the_pointer_in_whole_centimetres() {
        let rect = [[0.0, 0.0], [4000.0, 3000.0]];
        // Millimetres: a metre is 1000 units.
        let unit = 1000.0;
        let right = dragged(rect, Handle::Right, [5123.4, 900.0], unit);
        assert_eq!(right, [[0.0, 0.0], [5120.0, 3000.0]]);
        let corner = dragged(rect, Handle::BottomLeft, [-15.0, 1004.0], unit);
        assert_eq!(corner, [[-20.0, 1000.0], [4000.0, 3000.0]]);
        // Past the other side the region keeps its smallest size.
        let top = dragged(rect, Handle::Top, [0.0, -500.0], unit);
        assert_eq!(top, [[0.0, 0.0], [4000.0, 100.0]]);
        let left = dragged(rect, Handle::Left, [3990.0, 0.0], unit);
        assert_eq!(left, [[3900.0, 0.0], [4000.0, 3000.0]]);
        // In metres the steps are a hundredth.
        let metres = dragged(
            [[0.5, 0.5], [2.5, 1.5]],
            Handle::TopRight,
            [3.004, 2.996],
            1.0,
        );
        assert!(near2(metres[1], [3.0, 3.0]), "{metres:?}");
        // Handles sit on the sides and corners, and show arrows.
        assert_eq!(Handle::Top.at(rect), [2000.0, 3000.0]);
        assert_eq!(Handle::BottomRight.at(rect), [4000.0, 0.0]);
        assert_eq!(
            Handle::Left.cursor(),
            iced::mouse::Interaction::ResizingHorizontally
        );
        assert_eq!(
            Handle::Bottom.cursor(),
            iced::mouse::Interaction::ResizingVertically
        );
        assert_eq!(
            Handle::TopRight.cursor(),
            iced::mouse::Interaction::ResizingDiagonallyUp
        );
        assert_eq!(
            Handle::TopLeft.cursor(),
            iced::mouse::Interaction::ResizingDiagonallyDown
        );
        for handle in Handle::ALL {
            assert!(!matches!(
                handle.cursor(),
                iced::mouse::Interaction::Grab
                    | iced::mouse::Interaction::Grabbing
                    | iced::mouse::Interaction::Move
            ));
        }
    }

    #[test]
    fn turning_a_plan_keeps_the_centre_and_turns_the_way_the_region_turned() {
        for rotation in [0.0, 17.0, -170.0] {
            let section = OrientedBox::new(ROOM, rotation);
            let crop = crop_frame(section, DrawingView::Plan, DrawingOrigin::Model).unwrap();
            let centre = crop.centre();
            let after = turned(section, 17.0);
            assert!(near(after.center(), section.center()));
            assert_eq!(after.size(), section.size());
            // Counter-clockwise on the sheet is counter-clockwise from above:
            // the X axis of the box turns 17 degrees that way.
            let [before_x, _] = section.axes();
            let [after_x, _] = after.axes();
            let turn = (before_x[0] * after_x[1] - before_x[1] * after_x[0])
                .atan2(before_x[0] * after_x[0] + before_x[1] * after_x[1])
                .to_degrees();
            assert!((turn - 17.0).abs() < 1e-9, "{rotation}: {turn}");
            // The plan made again has the region upright about the same
            // centre: the model is drawn turned the other way.
            let again = crop_frame(after, DrawingView::Plan, DrawingOrigin::Model).unwrap();
            assert!(near(again.centre(), centre));
            let east = again.frame.to_uv([centre[0] + 1.0, centre[1], centre[2]]);
            let middle = again.frame.to_uv(centre);
            let seen = (east[1] - middle[1])
                .atan2(east[0] - middle[0])
                .to_degrees();
            let expected = normalized_degrees(-(rotation + 17.0));
            assert!(
                (normalized_degrees(seen - expected)).abs() < 1e-9,
                "{rotation}: {seen}"
            );
        }
        // Turns stay within a half turn either way.
        assert_eq!(
            turned(OrientedBox::new(ROOM, 170.0), 20.0).rotation_degrees,
            -170.0
        );
    }

    #[test]
    fn figures_set_one_at_a_time_change_only_what_they_name() {
        let section = OrientedBox::new(ROOM, 30.0);
        let plan = figures(section, DrawingView::Plan, Some(0.1));
        assert!((plan.width - 6.0).abs() < 1e-9 && (plan.height - 4.0).abs() < 1e-9);
        assert!(near2(plan.centre, [5.0, 3.0]));
        assert_eq!(plan.rotation, 30.0);
        assert_eq!(plan.cut, 3.0);
        assert_eq!(plan.depth, 0.1);

        let set = |field, value| {
            with_figure(section, DrawingView::Plan, Some(0.1), field, value).unwrap()
        };
        let (wider, slab) = set(Field::Width, 8.0);
        assert_eq!(slab, Some(0.1));
        assert!(near(wider.center(), section.center()));
        assert!((wider.size()[0] - 8.0).abs() < 1e-9);
        let (moved, _) = set(Field::CentreA, 7.5);
        assert!(near(moved.center(), [7.5, 3.0, 1.5]));
        assert_eq!(moved.size(), section.size());
        let (turned_box, _) = set(Field::Rotation, 45.0);
        assert_eq!(turned_box.rotation_degrees, 45.0);
        assert!(near(turned_box.center(), section.center()));
        // A lower cut keeps the floor of the box.
        let (lower, _) = set(Field::Cut, 1.2);
        assert!((lower.bounds.max[2] - 1.2).abs() < 1e-9);
        assert!((lower.bounds.min[2] - 0.0).abs() < 1e-9);
        // A cut under the floor takes the floor along, the slab deep.
        let (under, _) = set(Field::Cut, -0.5);
        assert!((under.bounds.min[2] + 0.6).abs() < 1e-9, "{under:?}");
        // A deeper view depth is the slab; the box grows when it is shallower.
        let (same, slab) = set(Field::Depth, 2.0);
        assert_eq!(slab, Some(2.0));
        assert_eq!(same.bounds, section.bounds);
        let thin = OrientedBox::new(
            Bounds {
                min: [2.0, 1.0, 1.0],
                max: [8.0, 5.0, 1.2],
            },
            0.0,
        );
        let (deeper, slab) =
            with_figure(thin, DrawingView::Plan, Some(0.1), Field::Depth, 0.5).unwrap();
        assert_eq!(slab, Some(0.5));
        assert!((deeper.bounds.min[2] - 0.7).abs() < 1e-9 && deeper.bounds.max[2] == 1.2);
        assert!(with_figure(section, DrawingView::Plan, Some(0.1), Field::Depth, 6.0).is_err());
        assert!(with_figure(section, DrawingView::Plan, Some(0.1), Field::Width, 0.05).is_err());

        // A section of the turned box: along it and up.
        let front = figures(section, DrawingView::Front, Some(0.2));
        assert!((front.width - 6.0).abs() < 1e-9 && (front.height - 3.0).abs() < 1e-9);
        let [x_axis, y_axis] = section.axes();
        let along_x = |point: [f64; 3]| x_axis[0] * point[0] + x_axis[1] * point[1];
        assert!((front.centre[0] - along_x(section.center())).abs() < 1e-9);
        assert_eq!(front.centre[1], 1.5);
        let face = section.to_scene([5.0, 1.0, 1.5]);
        assert!((front.cut - (y_axis[0] * face[0] + y_axis[1] * face[1])).abs() < 1e-9);
        assert!(with_figure(section, DrawingView::Front, Some(0.2), Field::Rotation, 5.0).is_err());
        // The cut moves forward half a metre; the back face stays.
        let (cut, _) = with_figure(
            section,
            DrawingView::Front,
            Some(0.2),
            Field::Cut,
            front.cut + 0.5,
        )
        .unwrap();
        let again = figures(cut, DrawingView::Front, Some(0.2));
        assert!((again.cut - front.cut - 0.5).abs() < 1e-9);
        assert!((cut.size()[1] - 3.5).abs() < 1e-9);
        assert!((again.width - front.width).abs() < 1e-9);
        // The centre moves along the section and up.
        let (slid, _) = with_figure(
            section,
            DrawingView::Front,
            Some(0.2),
            Field::CentreA,
            front.centre[0] + 2.0,
        )
        .unwrap();
        let slid_figures = figures(slid, DrawingView::Front, Some(0.2));
        assert!((slid_figures.centre[0] - front.centre[0] - 2.0).abs() < 1e-9);
        assert!((slid_figures.cut - front.cut).abs() < 1e-9);
        // An elevation sees through the whole box: its depth is the box.
        let back = figures(section, DrawingView::Back, None);
        assert!((back.depth - 4.0).abs() < 1e-9);
        let (shallow, slab) =
            with_figure(section, DrawingView::Back, None, Field::Depth, 1.5).unwrap();
        assert_eq!(slab, None);
        let shallow_figures = figures(shallow, DrawingView::Back, None);
        assert!((shallow_figures.depth - 1.5).abs() < 1e-9);
        assert!((shallow_figures.cut - back.cut).abs() < 1e-9);
    }

    #[test]
    fn r_then_o_starts_a_turn_and_anything_else_starts_over() {
        let start = Instant::now();
        let at = |millis: u64| start + Duration::from_millis(millis);
        let mut sequence = KeySequence::default();
        assert!(!sequence.typed("r", at(0), false));
        assert!(sequence.typed("o", at(400), false));
        // Upper case as well; once used, O alone does nothing.
        assert!(!sequence.typed("R", at(1000), false));
        assert!(sequence.typed("O", at(1200), false));
        assert!(!sequence.typed("o", at(1300), false));
        // Another key in between starts over.
        assert!(!sequence.typed("r", at(2000), false));
        assert!(!sequence.typed("w", at(2100), false));
        assert!(!sequence.typed("o", at(2200), false));
        // R again keeps the sequence open from the second R.
        assert!(!sequence.typed("r", at(3000), false));
        assert!(!sequence.typed("r", at(3500), false));
        assert!(sequence.typed("o", at(4900), false));
        // Too slow.
        assert!(!sequence.typed("r", at(6000), false));
        assert!(!sequence.typed("o", at(7501), false));
        assert!(!sequence.typed("r", at(8000), false));
        assert!(sequence.typed("o", at(9500), false));
        // A key a text field takes starts over, and an R there counts not.
        assert!(!sequence.typed("r", at(10_000), false));
        assert!(!sequence.typed("x", at(10_100), true));
        assert!(!sequence.typed("o", at(10_200), false));
        assert!(!sequence.typed("r", at(11_000), true));
        assert!(!sequence.typed("o", at(11_100), false));
        assert!(!sequence.typed("r", at(12_000), false));
        assert!(!sequence.typed("o", at(12_100), true));
        assert!(!sequence.typed("o", at(12_200), false));
        // A key without a character starts over too.
        assert!(!sequence.typed("r", at(13_000), false));
        sequence.interrupt();
        assert!(!sequence.typed("o", at(13_200), false));
    }

    #[test]
    fn shift_and_the_like_are_no_other_key_between_r_and_o() {
        use iced::keyboard::key::Named;
        for named in [Named::Shift, Named::Control, Named::Alt, Named::CapsLock] {
            assert!(is_modifier_key(named), "{named:?}");
        }
        for named in [
            Named::Space,
            Named::Enter,
            Named::Escape,
            Named::Tab,
            Named::ArrowLeft,
            Named::Backspace,
            Named::F5,
        ] {
            assert!(!is_modifier_key(named), "{named:?}");
        }
    }

    #[test]
    fn ro_turns_a_plan_in_the_drawing_view_and_the_section_box_in_3d() {
        let plan = Some(("plan-guid", DrawingView::Plan));
        assert_eq!(
            turn_start(true, plan, true),
            TurnStart::Plan("plan-guid".into())
        );
        assert_eq!(
            turn_start(true, Some(("cut", DrawingView::Front)), true),
            TurnStart::NotAPlan
        );
        assert_eq!(turn_start(true, None, true), TurnStart::NoCropRegion);
        // A plan that is not shown is not turned; the 3D view turns the box.
        assert_eq!(turn_start(false, None, true), TurnStart::SectionBox);
        assert_eq!(turn_start(false, None, false), TurnStart::NoSectionBox);
    }

    #[test]
    fn a_turn_follows_the_pointer_in_whole_degrees_and_a_typed_angle_wins() {
        let mut turn = Turning::new(
            TurnTarget::Plan {
                guid: "plan".into(),
                centre: [10.0, 10.0],
            },
            None,
        );
        assert_eq!(turn.degrees(false), Some(0.0));
        // The first pointer is where it starts.
        assert_eq!(turn.start_at([20.0, 10.0]), [20.0, 10.0]);
        assert_eq!(turn.start_at([0.0, 0.0]), [20.0, 10.0]);
        let at = [
            10.0 + 10.0 * 17.4f64.to_radians().cos(),
            10.0 + 10.0 * 17.4f64.to_radians().sin(),
        ];
        let degrees = angle_about([10.0, 10.0], at).unwrap()
            - angle_about([10.0, 10.0], [20.0, 10.0]).unwrap();
        turn.set_pointer(degrees);
        assert_eq!(turn.degrees(false), Some(17.0));
        assert_eq!(turn.degrees(true), Some(15.0));
        assert_eq!(turn.label(false), "17°");
        turn.set_pointer(-8.0);
        assert_eq!(turn.degrees(true), Some(-15.0));
        turn.set_pointer(-7.4);
        assert_eq!(turn.degrees(true), Some(0.0));
        assert_eq!(angle_about([1.0, 1.0], [1.0, 1.0]), None);
        // Typed digits, a sign and a decimal comma set the angle exactly.
        for character in "-12,5".chars() {
            assert!(turn.type_char(character));
        }
        assert!(!turn.type_char('x'));
        assert_eq!(turn.degrees(false), Some(-12.5));
        assert_eq!(turn.label(true), "-12,5°");
        assert!(turn.backspace());
        assert_eq!(turn.degrees(false), Some(-12.0));
        for _ in 0..4 {
            assert!(turn.backspace());
        }
        assert!(!turn.backspace());
        assert_eq!(turn.typed(), "");
        assert_eq!(turn.degrees(false), Some(-7.0));
        assert!(turn.type_char('-'));
        assert_eq!(turn.degrees(false), None, "a sign alone is no angle yet");
        // A turned rectangle turns about its middle.
        let corners = turned_corners([[0.0, 0.0], [4.0, 2.0]], 90.0);
        assert!(near2(corners[0], [3.0, -1.0]));
        assert!(near2(corners[2], [1.0, 3.0]));
    }
}
