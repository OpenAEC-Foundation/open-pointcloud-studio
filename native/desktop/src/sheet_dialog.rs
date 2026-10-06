//! The dialog behind "Create 2D plan / elevation / section…" in the Project
//! Browser: choose a plan, an elevation or a section, made from the whole 3D
//! model, the section box or a saved view with a section box. The dialog
//! works out the box the drawing is cut from and hands it to the Section
//! drawing job, which shows the result in the Drawing view and lists it
//! under VIEWS; how it was made is kept, so it can be made again.

use std::fmt;

use iced::widget::{
    button, center, column, container, horizontal_space, mouse_area, opaque, pick_list, row, text,
    text_input,
};
use iced::{Border, Color, Element, Task};
use pointcloud_core::{DrawingView, OrientedBox};
use serde::{Deserialize, Serialize};

use crate::i18n::{key, tr};
use crate::{
    combined_bounds, flat_tool_style, opencad_ribbon, themed_pick_list_style, ui_theme, Message,
    Studio,
};

/// The cut of a plan lies this far above the floor of the box it starts
/// from.
const CUT_ABOVE_FLOOR: f64 = 1.20;

/// The share of the points a drawing is made from unless another is typed,
/// in percent: a tenth makes a plan of a scan of a hundred million points in
/// seconds, and still lays several points in every cell of the filled cut of
/// a dense scan.
pub const DEFAULT_SAMPLE_PERCENT: f64 = 10.0;

/// The height steps in which the floor of the model is looked for, in
/// metres.
const FLOOR_STEP: f64 = 0.10;

/// How many points a height step needs, as a share of the fullest step, to
/// be a floor: a floor or a ceiling holds as many as the fullest step, stray
/// points below the building far fewer.
const FLOOR_SHARE: f64 = 0.3;

/// A floor is a centimetre with at least this many times the points of a
/// typical centimetre around it.
const FLOOR_PEAK: usize = 3;

/// The floor of the model as its points show it. The heights are counted in
/// steps of `FLOOR_STEP`; the lowest step that holds about as many points as
/// a floor is the floor's step, and within it and the step above, the
/// centimetre with clearly the most points is the floor, at the middle of
/// its points; without such a peak the floor is the lowest point there. Stray
/// points below a building, and a model that lies far above or below zero,
/// as a surveyed scan does, both give the floor itself. Nothing for no
/// heights.
fn floor_from_heights(heights: impl IntoIterator<Item = f64>) -> Option<f64> {
    let heights: Vec<f64> = heights.into_iter().filter(|z| z.is_finite()).collect();
    let low = heights.iter().copied().reduce(f64::min)?;
    let high = heights.iter().copied().reduce(f64::max)?;
    let step_of = |z: f64, size: f64, steps: usize| (((z - low) / size) as usize).min(steps - 1);
    let steps = (((high - low) / FLOOR_STEP).floor() as usize + 1).min(100_000);
    let mut counts = vec![0usize; steps];
    for &z in &heights {
        counts[step_of(z, FLOOR_STEP, steps)] += 1;
    }
    let fullest = counts.iter().copied().max()?;
    let floor_step = counts
        .iter()
        .position(|&count| count as f64 >= fullest as f64 * FLOOR_SHARE)?;
    // A floor on the edge between two steps falls into both.
    let near: Vec<f64> = heights
        .iter()
        .copied()
        .filter(|&z| (floor_step..=floor_step + 1).contains(&step_of(z, FLOOR_STEP, steps)))
        .collect();
    let near_low = near.iter().copied().reduce(f64::min)?;
    let centimetre = |z: f64| ((z - near_low) / 0.01) as usize;
    let mut fine = vec![0usize; centimetre(low + FLOOR_STEP * (floor_step + 2) as f64) + 2];
    for &z in &near {
        if let Some(count) = fine.get_mut(centimetre(z)) {
            *count += 1;
        }
    }
    let most = fine.iter().copied().max()?;
    let mut filled: Vec<usize> = fine.iter().copied().filter(|&count| count > 0).collect();
    filled.sort_unstable();
    let typical = filled[filled.len() / 2];
    // Without a clear peak, as along walls without a floor slab, the floor is
    // where the points begin.
    if most < typical * FLOOR_PEAK {
        return Some(near_low);
    }
    let floor_centimetre = fine.iter().position(|&count| count == most)?;
    let mut on_floor: Vec<f64> = near
        .iter()
        .copied()
        .filter(|&z| centimetre(z) == floor_centimetre)
        .collect();
    on_floor.sort_by(f64::total_cmp);
    on_floor.get(on_floor.len() / 2).copied()
}

/// The floor of the visible scans, from the points each keeps in memory.
fn model_floor(clouds: &[crate::CloudEntry]) -> Option<f64> {
    let any_visible = clouds.iter().any(|entry| entry.visible);
    floor_from_heights(
        clouds
            .iter()
            .filter(|entry| entry.visible || !any_visible)
            .flat_map(|entry| {
                let transform = entry.transform;
                entry
                    .cloud
                    .points
                    .iter()
                    .map(move |point| transform.xyz(point.xyz)[2])
            }),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SheetKind {
    Plan,
    Elevation,
    Section,
}

impl SheetKind {
    pub const ALL: [Self; 3] = [Self::Plan, Self::Elevation, Self::Section];

    fn label(self) -> &'static str {
        match self {
            Self::Plan => key("Plan"),
            Self::Elevation => key("Elevation view"),
            Self::Section => key("Section"),
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Elevation => "elevation",
            Self::Section => "section",
        }
    }

    pub fn from_key(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.key() == value)
    }
}

/// What the drawing is made from.
#[derive(Debug, Clone, PartialEq)]
pub enum SheetBasis {
    Model,
    SectionBox,
    /// A saved view of the active scan with a section box, by its
    /// identifier and its name.
    View(String, String),
}

impl fmt::Display for SheetBasis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Model => f.write_str(tr("3D model")),
            Self::SectionBox => f.write_str(tr("Section box")),
            Self::View(_, name) => f.write_str(name),
        }
    }
}

/// The face of the box a side drawing looks at, by its key.
pub fn side_from_key(value: &str) -> Option<DrawingView> {
    DrawingView::from_key(value).filter(|view| *view != DrawingView::Plan)
}

/// What the Create 2D dialog hands to the drawing job: the box it cuts, the
/// face it draws, the slab behind that face and the name of the drawing.
pub struct SheetJob {
    pub kind: SheetKind,
    pub section: OrientedBox,
    pub view: DrawingView,
    pub thickness: Option<f64>,
    /// The points used, in percent.
    pub sample_percent: f64,
    pub name: String,
}

/// The side a vertical drawing looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Side(DrawingView);

impl Side {
    const ALL: [Self; 4] = [
        Self(DrawingView::Front),
        Self(DrawingView::Back),
        Self(DrawingView::Left),
        Self(DrawingView::Right),
    ];
}

impl fmt::Display for Side {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(tr(match self.0 {
            DrawingView::Back => key("Back"),
            DrawingView::Left => key("Left"),
            DrawingView::Right => key("Right"),
            _ => key("Front"),
        }))
    }
}

#[derive(Debug, Clone)]
pub struct SheetDialog {
    kind: SheetKind,
    basis: SheetBasis,
    side: Side,
    /// Height of the cut of a plan made from the 3D model, in scene units.
    height: String,
    /// The floor found in the points of the model, which the cut height
    /// starts from.
    floor: Option<f64>,
    /// Where a section made from the 3D model cuts, along the axis it looks.
    position: String,
    thickness: String,
    /// The points used, in percent.
    points: String,
}

#[derive(Debug, Clone)]
pub enum SheetAction {
    Open,
    Close,
    Kind(SheetKind),
    Basis(SheetBasis),
    Side(Side),
    Height(String),
    Position(String),
    Thickness(String),
    Points(String),
    Create,
}

fn parse(value: &str) -> Option<f64> {
    value
        .trim()
        .replace(',', ".")
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

impl Studio {
    pub(crate) fn update_sheet_dialog(&mut self, action: SheetAction) -> Task<Message> {
        if let SheetAction::Open = action {
            let section = self.section_box();
            let model = combined_bounds(&self.clouds);
            let model_floor = model_floor(&self.clouds);
            let floor = section
                .map(|section| section.bounds.min[2])
                .or(model_floor)
                .or_else(|| model.map(|model| model.min[2]))
                .unwrap_or(0.0);
            let middle = model.map_or(0.0, |model| (model.min[1] + model.max[1]) / 2.0);
            self.sheet_dialog = Some(SheetDialog {
                kind: SheetKind::Plan,
                basis: if section.is_some() {
                    SheetBasis::SectionBox
                } else {
                    SheetBasis::Model
                },
                side: Side(DrawingView::Front),
                height: format!("{:.2}", floor + CUT_ABOVE_FLOOR),
                floor: model_floor,
                position: format!("{middle:.2}"),
                thickness: "0.10".into(),
                points: crate::drawing_crop::percent_text(DEFAULT_SAMPLE_PERCENT),
            });
            return Task::none();
        }
        let Some(dialog) = self.sheet_dialog.as_mut() else {
            return Task::none();
        };
        match action {
            SheetAction::Open => {}
            SheetAction::Close => self.sheet_dialog = None,
            SheetAction::Kind(kind) => dialog.kind = kind,
            SheetAction::Basis(basis) => dialog.basis = basis,
            SheetAction::Side(side) => {
                dialog.side = side;
                // A section from the model cuts through its middle along
                // the axis it looks along, until another place is typed.
                if let Some(model) = combined_bounds(&self.clouds) {
                    let axis =
                        usize::from(matches!(side.0, DrawingView::Front | DrawingView::Back));
                    dialog.position = format!("{:.2}", (model.min[axis] + model.max[axis]) / 2.0);
                }
            }
            SheetAction::Height(value) => dialog.height = value,
            SheetAction::Position(value) => dialog.position = value,
            SheetAction::Thickness(value) => dialog.thickness = value,
            SheetAction::Points(value) => dialog.points = value,
            SheetAction::Create => {
                let dialog = dialog.clone();
                match self
                    .sheet_job(&dialog)
                    .and_then(|job| self.create_sheet(job, None))
                {
                    Ok(task) => {
                        self.sheet_dialog = None;
                        return task;
                    }
                    Err(reason) => self.status = reason,
                }
            }
        }
        Task::none()
    }

    /// The saved views of the active scan that have a section box, as the
    /// dialog offers them to make a drawing from.
    fn views_with_a_box(&self) -> Vec<SheetBasis> {
        let source = self.active_camera_source();
        self.views
            .list
            .iter()
            .filter(|view| Some(&view.source) == source.as_ref() && view.section_box().is_some())
            .map(|view| SheetBasis::View(view.guid.clone(), view.name.clone()))
            .collect()
    }

    /// The box, the view, the slab and the name of the drawing the dialog
    /// asks for, or why it cannot be made.
    fn sheet_job(&self, dialog: &SheetDialog) -> Result<SheetJob, String> {
        let base = match &dialog.basis {
            SheetBasis::Model => combined_bounds(&self.clouds)
                .map(|bounds| OrientedBox::new(bounds, 0.0))
                .ok_or_else(|| "Open a scan to make a drawing of it".to_owned())?,
            SheetBasis::SectionBox => self.section_box().ok_or_else(|| {
                "Switch on the section box, or make the drawing from the 3D model".to_owned()
            })?,
            SheetBasis::View(guid, name) => self
                .views
                .list
                .iter()
                .find(|view| view.guid == *guid)
                .ok_or_else(|| format!("The view {name} is no longer saved"))?
                .section_box()
                .ok_or_else(|| format!("The view {name} has no section box"))?
                .oriented(),
        };
        let thickness = parse(&dialog.thickness)
            .filter(|value| *value > 0.0)
            .ok_or_else(|| "The slab thickness must be a number above 0".to_owned())?;
        let sample_percent = crate::drawing_crop::parse_percent(&dialog.points)
            .ok_or_else(|| "The points used must be a number of percent".to_owned())?;
        if let Some(problem) = crate::drawing_crop::percent_problem(sample_percent) {
            return Err(problem);
        }
        let basis = dialog.basis.to_string();
        let mut bounds = base.bounds;
        let (view, slab, name) = match dialog.kind {
            SheetKind::Plan => {
                let mut name = format!("{} · {basis}", tr("Plan"));
                if dialog.basis == SheetBasis::Model {
                    let height = parse(&dialog.height)
                        .ok_or_else(|| "The cut height must be a number".to_owned())?;
                    if height <= bounds.min[2] || height > bounds.max[2] {
                        return Err(format!(
                            "The cut height must lie between {:.2} and {:.2}",
                            bounds.min[2], bounds.max[2]
                        ));
                    }
                    bounds.max[2] = height;
                    name = format!("{} {height:+.2}", tr("Plan"));
                }
                (DrawingView::Plan, Some(thickness), name)
            }
            SheetKind::Elevation => (
                dialog.side.0,
                None,
                if dialog.basis == SheetBasis::Model {
                    format!("{} {}", tr("Elevation view"), dialog.side)
                } else {
                    format!("{} {} · {basis}", tr("Elevation view"), dialog.side)
                },
            ),
            SheetKind::Section => {
                let mut name = format!("{} {} · {basis}", tr("Section"), dialog.side);
                if dialog.basis == SheetBasis::Model {
                    let at = parse(&dialog.position)
                        .ok_or_else(|| "The place of the cut must be a number".to_owned())?;
                    let (axis, from_min) = match dialog.side.0 {
                        DrawingView::Front => (1, true),
                        DrawingView::Back => (1, false),
                        DrawingView::Left => (0, true),
                        _ => (0, false),
                    };
                    if at <= bounds.min[axis] || at >= bounds.max[axis] {
                        return Err(format!(
                            "The cut must lie between {:.2} and {:.2}",
                            bounds.min[axis], bounds.max[axis]
                        ));
                    }
                    if from_min {
                        bounds.min[axis] = at;
                    } else {
                        bounds.max[axis] = at;
                    }
                    name = format!(
                        "{} {} {}={at:.2}",
                        tr("Section"),
                        dialog.side,
                        ["x", "y"][axis]
                    );
                }
                (dialog.side.0, Some(thickness), name)
            }
        };
        Ok(SheetJob {
            kind: dialog.kind,
            section: OrientedBox::new(bounds, base.rotation_degrees),
            view,
            thickness: slab,
            sample_percent,
            name,
        })
    }

    /// The `create_drawing` command of the local API: the choices of the
    /// dialog, each left out keeping what the dialog starts with. `basis` is
    /// `model`, `section_box` or the name of a saved view of the active scan
    /// with a section box. Answers with the job that makes the drawing.
    pub(crate) fn api_create_drawing(
        &mut self,
        options: &crate::native_api::CreateDrawingOptions,
    ) -> (serde_json::Value, Task<Message>) {
        use serde_json::json;
        let refuse = |error: String| (json!({"ok": false, "error": error}), Task::none());
        let Some(kind) = SheetKind::from_key(&options.kind.to_ascii_lowercase()) else {
            return refuse("kind must be plan, elevation or section".into());
        };
        let basis = match options.basis.as_deref().map(str::trim) {
            None | Some("model") => SheetBasis::Model,
            Some("section_box") => SheetBasis::SectionBox,
            Some(name) => match self.views_with_a_box().into_iter().find(
                |basis| matches!(basis, SheetBasis::View(_, view) if view.eq_ignore_ascii_case(name)),
            ) {
                Some(basis) => basis,
                None => {
                    return refuse(format!(
                        "basis must be model, section_box or the name of a saved view of the active scan with a section box; {name} is none of these"
                    ))
                }
            },
        };
        let side = match options.side.as_deref() {
            None => None,
            Some(side) => match side_from_key(&side.to_ascii_lowercase()) {
                Some(side) => Some(Side(side)),
                None => return refuse("side must be front, back, left or right".into()),
            },
        };
        let was_open = self.sheet_dialog.take();
        let _ = self.update_sheet_dialog(SheetAction::Open);
        let mut actions = vec![SheetAction::Kind(kind), SheetAction::Basis(basis)];
        actions.extend(side.map(SheetAction::Side));
        actions.extend(
            options
                .height
                .map(|value| SheetAction::Height(value.to_string())),
        );
        actions.extend(
            options
                .position
                .map(|value| SheetAction::Position(value.to_string())),
        );
        actions.extend(
            options
                .thickness
                .map(|value| SheetAction::Thickness(value.to_string())),
        );
        actions.extend(
            options
                .sample_percent
                .map(|value| SheetAction::Points(value.to_string())),
        );
        for action in actions {
            let _ = self.update_sheet_dialog(action);
        }
        let dialog = self.sheet_dialog.take();
        self.sheet_dialog = was_open;
        let Some(dialog) = dialog else {
            return refuse("the dialog could not be opened".into());
        };
        let mut job = match self.sheet_job(&dialog) {
            Ok(job) => job,
            Err(reason) => return refuse(reason),
        };
        if let Some(name) = options.name.as_deref().map(str::trim) {
            if name.is_empty() || name.chars().any(char::is_control) {
                return refuse("name must hold a visible character".into());
            }
            job.name = name.to_owned();
        }
        let id = self.record_api_job(json!({"state": "running", "operation": "create_drawing"}));
        match self.create_sheet(job, Some(id.clone())) {
            Ok(task) => (json!({"ok": true, "accepted": true, "job_id": id}), task),
            Err(reason) => {
                self.forget_api_job(&id);
                refuse(reason)
            }
        }
    }

    /// The dialog over the dimmed window, while it is open.
    pub(crate) fn sheet_dialog_view(&self) -> Option<Element<'_, Message>> {
        let dialog = self.sheet_dialog.as_ref()?;
        let colors = self.ui_theme.colors();
        let send = Message::Sheet;
        let label = |name: &'static str| text(tr(name)).size(12).color(colors.muted).width(130);
        let kinds = SheetKind::ALL
            .into_iter()
            .fold(row![].spacing(4), |kinds, kind| {
                let active = dialog.kind == kind;
                kinds.push(
                    button(text(tr(kind.label())).size(12))
                        .on_press(send(SheetAction::Kind(kind)))
                        .padding([5, 14])
                        .style(move |theme, status| {
                            opencad_ribbon::tool_btn_style(theme, active, status)
                        }),
                )
            });
        let mut bases = vec![SheetBasis::Model];
        if self.section_box().is_some() {
            bases.push(SheetBasis::SectionBox);
        }
        bases.extend(self.views_with_a_box());
        let field = |value: &str, on: fn(String) -> SheetAction| {
            text_input("", value)
                .on_input(move |value| Message::Sheet(on(value)))
                .size(12)
                .padding([4, 6])
                .width(110)
        };
        let mut form = column![
            row![label("Drawing"), kinds].align_y(iced::Alignment::Center),
            row![
                label("Based on"),
                pick_list(bases, Some(dialog.basis.clone()), |basis| Message::Sheet(
                    SheetAction::Basis(basis)
                ))
                .style(themed_pick_list_style)
                .text_size(12)
                .width(220),
            ]
            .align_y(iced::Alignment::Center),
        ]
        .spacing(10);
        if dialog.kind != SheetKind::Plan {
            form = form.push(
                row![
                    label("Looking at"),
                    pick_list(Side::ALL, Some(dialog.side), |side| Message::Sheet(
                        SheetAction::Side(side)
                    ))
                    .style(themed_pick_list_style)
                    .text_size(12)
                    .width(140),
                ]
                .align_y(iced::Alignment::Center),
            );
        }
        if dialog.basis == SheetBasis::Model {
            match dialog.kind {
                SheetKind::Plan => {
                    form = form.push(
                        row![
                            label("Cut height (m)"),
                            field(&dialog.height, SheetAction::Height)
                        ]
                        .align_y(iced::Alignment::Center),
                    );
                    if let Some(floor) = dialog.floor {
                        form = form.push(
                            row![
                                iced::widget::Space::with_width(130),
                                text(crate::i18n::tr_args(
                                    "Floor found at {height} m",
                                    &[("height", &format!("{floor:.2}"))],
                                ))
                                .size(11)
                                .color(colors.muted),
                            ]
                            .align_y(iced::Alignment::Center),
                        );
                    }
                }
                SheetKind::Section => {
                    form = form.push(
                        row![
                            label("Cut at (m)"),
                            field(&dialog.position, SheetAction::Position)
                        ]
                        .align_y(iced::Alignment::Center),
                    );
                }
                SheetKind::Elevation => {}
            }
        }
        if dialog.kind != SheetKind::Elevation {
            form = form.push(
                row![
                    label("Slab thickness (m)"),
                    field(&dialog.thickness, SheetAction::Thickness)
                ]
                .align_y(iced::Alignment::Center),
            );
        }
        form = form.push(
            row![
                label("Points used (%)"),
                field(&dialog.points, SheetAction::Points)
            ]
            .align_y(iced::Alignment::Center),
        );
        let card = container(
            column![
                row![
                    text(tr("Create 2D plan / elevation / section")).size(15),
                    horizontal_space(),
                    button(text("×").size(14))
                        .on_press(send(SheetAction::Close))
                        .style(flat_tool_style)
                        .padding([1, 8]),
                ]
                .align_y(iced::Alignment::Center),
                form,
                row![
                    horizontal_space(),
                    button(text(tr("Cancel")).size(12))
                        .on_press(send(SheetAction::Close))
                        .style(flat_tool_style),
                    button(text(tr("Create")).size(12))
                        .on_press_maybe(
                            (!self.clouds.is_empty()).then_some(send(SheetAction::Create))
                        )
                        .padding([5, 16])
                        .style(|theme, status| opencad_ribbon::tool_btn_style(theme, true, status)),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            ]
            .spacing(18),
        )
        .width(480)
        .padding(18)
        .style(|theme| {
            let colors = ui_theme::colors(theme);
            container::Style::default()
                .background(colors.panel)
                .color(colors.text)
                .border(Border {
                    color: colors.border,
                    width: 1.0,
                    radius: 8.0.into(),
                })
        });
        Some(opaque(
            mouse_area(center(opaque(card)).style(|_| {
                container::Style::default().background(Color::from_rgba8(0, 0, 0, 0.45))
            }))
            .on_press(send(SheetAction::Close)),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn office() -> Studio {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("hall.xyz");
        let mut lines = String::new();
        for step in 0..400 {
            let t = f64::from(step) / 400.0;
            lines.push_str(&format!("{} 0 {}\n", t * 10.0, t * 3.0));
            lines.push_str(&format!("{} 8 {}\n", t * 10.0, 3.0 - t * 3.0));
            lines.push_str(&format!("0 {} {}\n", t * 8.0, t * 3.0));
            lines.push_str(&format!("10 {} {}\n", t * 8.0, 3.0 - t * 3.0));
        }
        std::fs::write(&source, lines).unwrap();
        let cloud = pointcloud_core::open(&source, 10_000).unwrap();
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(Arc::new(cloud))));
        studio
    }

    #[test]
    fn the_dialog_works_out_the_box_of_each_kind_of_drawing() {
        let mut studio = office();
        let _ = studio.update_sheet_dialog(SheetAction::Open);
        let dialog = studio.sheet_dialog.clone().unwrap();
        assert_eq!(dialog.basis, SheetBasis::Model);
        assert_eq!(dialog.height, "1.20");
        let _ = studio.view();

        // A plan from the model: the top of the box is the cut.
        let SheetJob {
            section,
            view,
            thickness: slab,
            name,
            ..
        } = studio.sheet_job(&dialog).unwrap();
        assert_eq!(view, DrawingView::Plan);
        assert_eq!(slab, Some(0.10));
        assert!((section.bounds.max[2] - 1.20).abs() < 1e-9);
        assert!(name.contains("+1.20"), "{name}");

        // A section from the model looking at the front cuts at y = 4.
        let mut section_dialog = dialog.clone();
        section_dialog.kind = SheetKind::Section;
        section_dialog.position = "4".into();
        let SheetJob { section, view, .. } = studio.sheet_job(&section_dialog).unwrap();
        assert_eq!(view, DrawingView::Front);
        assert!((section.bounds.min[1] - 4.0).abs() < 1e-9);

        // An elevation takes the whole depth.
        let mut elevation = dialog.clone();
        elevation.kind = SheetKind::Elevation;
        elevation.side = Side(DrawingView::Left);
        let SheetJob {
            view,
            thickness: slab,
            ..
        } = studio.sheet_job(&elevation).unwrap();
        assert_eq!(view, DrawingView::Left);
        assert_eq!(slab, None);

        // A cut outside the model is refused, and the section box needs to
        // be on.
        let mut outside = dialog.clone();
        outside.height = "20".into();
        assert!(studio.sheet_job(&outside).is_err());
        let mut from_box = dialog.clone();
        from_box.basis = SheetBasis::SectionBox;
        assert!(studio.sheet_job(&from_box).is_err());

        // A tenth of the points by default; another share as typed, with a
        // comma or a percent sign, within 0.1 and 100.
        assert_eq!(dialog.points, "10");
        assert_eq!(studio.sheet_job(&dialog).unwrap().sample_percent, 10.0);
        let mut share = dialog;
        for (typed, percent) in [
            ("2,5", Some(2.5)),
            ("100 %", Some(100.0)),
            ("0.05", None),
            ("x", None),
        ] {
            share.points = typed.into();
            assert_eq!(
                studio.sheet_job(&share).ok().map(|job| job.sample_percent),
                percent,
                "{typed}"
            );
        }

        let _ = studio.update_sheet_dialog(SheetAction::Close);
        assert!(studio.sheet_dialog.is_none());
    }

    #[test]
    fn the_floor_is_the_lowest_height_as_full_as_a_floor() {
        // A surveyed building far above zero: a floor at 100, a ceiling at
        // 103, walls in between and a few stray points well below.
        let mut heights = Vec::new();
        for step in 0..2_000 {
            let t = f64::from(step) / 2_000.0;
            heights.push(100.0 + 0.004 * (t - 0.5));
            heights.push(103.0 + 0.004 * (t - 0.5));
            heights.push(100.0 + 3.0 * t);
        }
        heights.extend([80.0, 80.3, 91.7, 95.2]);
        let floor = floor_from_heights(heights).unwrap();
        assert!((floor - 99.998).abs() < 0.01, "{floor}");
        // Nothing to go by.
        assert_eq!(floor_from_heights(Vec::<f64>::new()), None);
        assert_eq!(floor_from_heights([f64::NAN]), None);
        // One height is its own floor.
        assert_eq!(floor_from_heights([16.4]), Some(16.4));
    }

    #[test]
    fn a_plan_of_a_surveyed_model_is_cut_above_its_own_floor() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("street.xyz");
        let mut lines = String::new();
        for step in 0..600 {
            let t = f64::from(step) / 600.0;
            // A street at 16.2 m with a pit down to 14.1 m and a wall.
            lines.push_str(&format!(
                "{} {} 16.2
",
                313790.0 + t * 12.0,
                5426773.0 + t * 12.0
            ));
            lines.push_str(&format!(
                "{} 5426779 {}
",
                313796.0,
                14.1 + t * 2.1
            ));
            lines.push_str(&format!(
                "{} 5426785 {}
",
                313790.0 + t * 12.0,
                16.2 + t * 3.2
            ));
        }
        std::fs::write(&source, lines).unwrap();
        let cloud = pointcloud_core::open(&source, 10_000).unwrap();
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(Arc::new(cloud))));
        let _ = studio.update_sheet_dialog(SheetAction::Open);
        let dialog = studio.sheet_dialog.clone().unwrap();
        assert_eq!(dialog.height, "17.40");
        assert!(studio.sheet_job(&dialog).is_ok());
    }
}
