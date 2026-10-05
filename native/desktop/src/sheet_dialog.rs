//! The dialog behind "Create 2D plan / elevation / section…" in the Project
//! Browser: choose a plan, an elevation or a section, made from the whole 3D
//! model, the section box or a saved section box. The dialog works out the
//! box the drawing is cut from and hands it to the Section drawing job,
//! which shows the result in the Drawing view and lists it under DRAWINGS.

use std::fmt;

use iced::widget::{
    button, center, column, container, horizontal_space, mouse_area, opaque, pick_list, row, text,
    text_input,
};
use iced::{Border, Color, Element, Task};
use pointcloud_core::{Bounds, DrawingView, OrientedBox};

use crate::i18n::{key, tr};
use crate::{
    combined_bounds, flat_tool_style, opencad_ribbon, themed_pick_list_style, ui_theme, Message,
    Studio,
};

/// The cut of a plan lies this far above the floor of the box it starts
/// from.
const CUT_ABOVE_FLOOR: f64 = 1.20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SheetKind {
    Plan,
    Elevation,
    Section,
}

impl SheetKind {
    const ALL: [Self; 3] = [Self::Plan, Self::Elevation, Self::Section];

    fn label(self) -> &'static str {
        match self {
            Self::Plan => key("Plan"),
            Self::Elevation => key("Elevation"),
            Self::Section => key("Section"),
        }
    }
}

/// What the drawing is made from.
#[derive(Debug, Clone, PartialEq)]
pub enum SheetBasis {
    Model,
    SectionBox,
    /// A section box saved in the Project Browser, by its place in the list.
    Saved(usize, String),
}

impl fmt::Display for SheetBasis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Model => f.write_str(tr("3D model")),
            Self::SectionBox => f.write_str(tr("Section box")),
            Self::Saved(_, name) => f.write_str(name),
        }
    }
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
    /// Where a section made from the 3D model cuts, along the axis it looks.
    position: String,
    thickness: String,
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
            let floor = section
                .map(|section| section.bounds.min[2])
                .or_else(|| model.map(|_| 0.0))
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
                position: format!("{middle:.2}"),
                thickness: "0.10".into(),
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
            SheetAction::Create => {
                let dialog = dialog.clone();
                match self.sheet_job(&dialog) {
                    Ok((section, view, thickness, name)) => {
                        self.sheet_dialog = None;
                        return self.create_sheet(section, view, thickness, name);
                    }
                    Err(reason) => self.status = reason,
                }
            }
        }
        Task::none()
    }

    /// The box, the view, the slab and the name of the drawing the dialog
    /// asks for, or why it cannot be made.
    fn sheet_job(
        &self,
        dialog: &SheetDialog,
    ) -> Result<(OrientedBox, DrawingView, Option<f64>, String), String> {
        let base = match &dialog.basis {
            SheetBasis::Model => combined_bounds(&self.clouds)
                .map(|bounds| OrientedBox::new(bounds, 0.0))
                .ok_or_else(|| "Open a scan to make a drawing of it".to_owned())?,
            SheetBasis::SectionBox => self.section_box().ok_or_else(|| {
                "Switch on the section box, or make the drawing from the 3D model".to_owned()
            })?,
            SheetBasis::Saved(place, _) => {
                let saved = self
                    .sections
                    .list
                    .get(*place)
                    .ok_or_else(|| "That section box is no longer saved".to_owned())?;
                OrientedBox::new(
                    Bounds {
                        min: saved.min,
                        max: saved.max,
                    },
                    saved.rotation,
                )
            }
        };
        let thickness = parse(&dialog.thickness)
            .filter(|value| *value > 0.0)
            .ok_or_else(|| "The slab thickness must be a number above 0".to_owned())?;
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
                format!("{} {} · {basis}", tr("Elevation"), dialog.side),
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
        Ok((
            OrientedBox::new(bounds, base.rotation_degrees),
            view,
            slab,
            name,
        ))
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
        let source = self.active_camera_source();
        for (place, saved) in self.sections.list.iter().enumerate() {
            if Some(&saved.source) == source.as_ref() {
                bases.push(SheetBasis::Saved(place, saved.name.clone()));
            }
        }
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
        let (section, view, slab, name) = studio.sheet_job(&dialog).unwrap();
        assert_eq!(view, DrawingView::Plan);
        assert_eq!(slab, Some(0.10));
        assert!((section.bounds.max[2] - 1.20).abs() < 1e-9);
        assert!(name.contains("+1.20"), "{name}");

        // A section from the model looking at the front cuts at y = 4.
        let mut section_dialog = dialog.clone();
        section_dialog.kind = SheetKind::Section;
        section_dialog.position = "4".into();
        let (section, view, _, _) = studio.sheet_job(&section_dialog).unwrap();
        assert_eq!(view, DrawingView::Front);
        assert!((section.bounds.min[1] - 4.0).abs() < 1e-9);

        // An elevation takes the whole depth.
        let mut elevation = dialog.clone();
        elevation.kind = SheetKind::Elevation;
        elevation.side = Side(DrawingView::Left);
        let (_, view, slab, _) = studio.sheet_job(&elevation).unwrap();
        assert_eq!(view, DrawingView::Left);
        assert_eq!(slab, None);

        // A cut outside the model is refused, and the section box needs to
        // be on.
        let mut outside = dialog.clone();
        outside.height = "20".into();
        assert!(studio.sheet_job(&outside).is_err());
        let mut from_box = dialog;
        from_box.basis = SheetBasis::SectionBox;
        assert!(studio.sheet_job(&from_box).is_err());

        let _ = studio.update_sheet_dialog(SheetAction::Close);
        assert!(studio.sheet_dialog.is_none());
    }
}
