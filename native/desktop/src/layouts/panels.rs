//! What the sheets show in the window besides the paper: SHEETS in the
//! Project Browser with "New sheet…", the rows of VIEWS that can be dragged
//! onto a sheet, and the sheet or its selected viewport in Properties.

use std::sync::Arc;

use iced::widget::{
    button, column, container, pick_list, row, stack, text, text_input, tooltip, Canvas,
};
use iced::{Element, Fill};

use crate::i18n::tr;
use crate::project_browser::{indented, remove_button, row_button, view_row, ViewRow};
use crate::{
    flat_tool_style, icon_svg, opencad_properties, themed_pick_list_style, ui_theme, Message,
    Studio, ToolIcon,
};

use super::model::{
    self, drawing_size, image_size, scale_label, Layout, Paper, PlacedKind, Viewport, SCALES,
};
use super::plot::Content;
use super::{
    canvas, drag, drawing_rect, image_room, rename_input_id, Field, LayoutAction, NewSheet,
    Orientation, Placeable, ScaleChoice, PICTURE_GUESS, SHEETS,
};

impl Studio {
    /// The sheet in the main area, while one is shown.
    pub(crate) fn layout_canvas(&self) -> Option<Element<'_, Message>> {
        let guid = self.drawing_view.shown_layout()?;
        let layout = self.layouts.layout(guid)?;
        let plot = self.shown_plot()?;
        let dropping = self
            .layouts
            .drag_row
            .as_ref()
            .map(|(kind, placed)| match kind {
                PlacedKind::Drawing => self
                    .drawing_view
                    .saved
                    .iter()
                    .find(|drawing| drawing.guid == *placed)
                    .and_then(drawing_rect)
                    .map_or([60.0, 40.0], |rect| {
                        drawing_size(rect, model::DEFAULT_SCALE)
                    }),
                PlacedKind::View => image_size(
                    self.layouts
                        .images
                        .get(placed)
                        .map_or(PICTURE_GUESS, |picture| picture.pixels),
                    image_room(layout),
                ),
            });
        let hint = layout.viewports.is_empty().then(|| {
            tr("Drag a view from VIEWS onto the paper, or use Place view in Properties").to_owned()
        });
        let colors = self.ui_theme.colors();
        Some(
            stack![
                canvas::Paper {
                    tool: &self.layouts,
                    plot: Arc::clone(&plot),
                    desk: canvas::desk(self.ui_theme),
                    paper: iced::Color::WHITE,
                },
                Canvas::new(canvas::Overlay {
                    tool: &self.layouts,
                    plot,
                    selected: self.layouts.selected.clone(),
                    dropping,
                    accent: colors.accent,
                    hint,
                })
                .width(Fill)
                .height(Fill),
            ]
            .width(Fill)
            .height(Fill)
            .into(),
        )
    }

    /// A row of VIEWS that can be dragged onto the sheet shown: a saved view
    /// or a drawing, while a sheet is shown.
    pub(crate) fn draggable_row<'a>(
        &self,
        listed: &ViewRow,
        element: Element<'a, Message>,
    ) -> Element<'a, Message> {
        if self.drawing_view.shown_layout().is_none() {
            return element;
        }
        let (kind, guid) = match listed {
            ViewRow::Saved(guid) => (PlacedKind::View, guid.clone()),
            ViewRow::Drawing(guid) => (PlacedKind::Drawing, guid.clone()),
            _ => return element,
        };
        drag::Draggable::new(element, Message::Layouts(LayoutAction::DragRow(kind, guid))).into()
    }

    /// What can be placed on a sheet: the saved views of the active scan
    /// and the drawings of the open scans.
    fn placeables(&self) -> Vec<Placeable> {
        let views = self.listed_views().into_iter().map(|view| Placeable {
            kind: PlacedKind::View,
            guid: view.guid.clone(),
            name: view.name.clone(),
        });
        let drawings = self.listed_drawings().into_iter().map(|drawing| Placeable {
            kind: PlacedKind::Drawing,
            guid: drawing.guid.clone(),
            name: drawing.name.clone(),
        });
        views.chain(drawings).collect()
    }

    /// SHEETS in the Project Browser: a row per sheet with Rename, Duplicate
    /// and ×, and "New sheet…" with what a new sheet gets.
    pub(crate) fn sheets_group(&self) -> Element<'_, Message> {
        use crate::project_browser::{band, hint, Mark};
        let open = self.browser.is_open(SHEETS);
        let new = tooltip(
            button(icon_svg(ToolIcon::Sheet, 15.0))
                .on_press(Message::Layouts(LayoutAction::NewSheet))
                .style(flat_tool_style)
                .padding(3),
            hint(tr("New sheet…").to_owned()),
            tooltip::Position::Bottom,
        )
        .gap(4);
        let mut group = column![band(
            SHEETS.to_owned(),
            open,
            ToolIcon::Sheet,
            tr("SHEETS").to_owned(),
            self.layouts.list.len().to_string(),
            vec![new.into()],
            Mark::Views,
            false,
        )]
        .spacing(3);
        if !open {
            return group.into();
        }
        let mut list = column![].spacing(2);
        let shown = self.drawing_view.shown_layout();
        for layout in &self.layouts.list {
            list = list.push(self.sheet_row(layout, shown == Some(layout.guid.as_str())));
        }
        match &self.layouts.form {
            Some(form) => list = list.push(self.new_sheet_form(form)),
            None => {
                list = list.push(
                    button(text(tr("New sheet…")).size(11))
                        .on_press(Message::Layouts(LayoutAction::NewSheet))
                        .style(flat_tool_style)
                        .width(Fill),
                );
            }
        }
        group = group.push(indented(list, 4.0));
        group.into()
    }

    fn sheet_row(&self, layout: &Layout, shown: bool) -> Element<'_, Message> {
        let guid = || layout.guid.clone();
        if let Some((renamed, name)) = &self.layouts.renaming {
            if *renamed == layout.guid {
                let small = |label: &'static str, action: LayoutAction| {
                    button(text(tr(label)).size(10))
                        .on_press(Message::Layouts(action))
                        .style(flat_tool_style)
                        .padding([3, 4])
                };
                return row![
                    text_input(tr("Sheet name"), name)
                        .id(rename_input_id())
                        .on_input(|name| Message::Layouts(LayoutAction::RenameText(name)))
                        .on_submit(Message::Layouts(LayoutAction::FinishRename))
                        .size(11)
                        .padding([3, 5])
                        .width(Fill),
                    small("OK", LayoutAction::FinishRename),
                    small("Cancel", LayoutAction::CancelRename),
                ]
                .spacing(2)
                .align_y(iced::Alignment::Center)
                .into();
            }
        }
        view_row(
            ToolIcon::Sheet,
            format!("{} · {}", layout.caption(), layout.paper),
            shown,
            false,
            Message::Layouts(LayoutAction::Show(guid())),
            vec![
                row_button(
                    ToolIcon::Rename,
                    tr("Rename"),
                    Message::Layouts(LayoutAction::StartRename(guid())),
                ),
                row_button(
                    ToolIcon::Duplicate,
                    tr("Duplicate"),
                    Message::Layouts(LayoutAction::Duplicate(guid())),
                ),
                remove_button(Message::Layouts(LayoutAction::Delete(guid()))),
            ],
        )
    }

    fn new_sheet_form(&self, form: &NewSheet) -> Element<'_, Message> {
        let colors = self.ui_theme.colors();
        let named = |caption: &str, field: Element<'static, Message>| {
            row![
                text(caption.to_owned())
                    .size(11)
                    .width(64)
                    .color(colors.muted),
                field
            ]
            .spacing(4)
            .align_y(iced::Alignment::Center)
        };
        container(
            column![
                text(tr("New sheet")).size(11),
                named(
                    tr("Number"),
                    text_input("01", &form.number)
                        .on_input(|value| Message::Layouts(LayoutAction::FormNumber(value)))
                        .size(11)
                        .padding([3, 5])
                        .width(Fill)
                        .into(),
                ),
                named(
                    tr("Name"),
                    text_input(tr("Sheet name"), &form.name)
                        .on_input(|value| Message::Layouts(LayoutAction::FormName(value)))
                        .on_submit(Message::Layouts(LayoutAction::Create))
                        .size(11)
                        .padding([3, 5])
                        .width(Fill)
                        .into(),
                ),
                named(
                    tr("Paper"),
                    pick_list(Paper::ALL, Some(form.paper), |paper| {
                        Message::Layouts(LayoutAction::FormPaper(paper))
                    })
                    .style(themed_pick_list_style)
                    .text_size(11)
                    .width(Fill)
                    .into(),
                ),
                named(
                    tr("Orientation"),
                    pick_list(Orientation::ALL, Some(form.orientation), |orientation| {
                        Message::Layouts(LayoutAction::FormOrientation(orientation))
                    })
                    .style(themed_pick_list_style)
                    .text_size(11)
                    .width(Fill)
                    .into(),
                ),
                row![
                    button(text(tr("Create")).size(11))
                        .on_press(Message::Layouts(LayoutAction::Create))
                        .style(flat_tool_style),
                    button(text(tr("Cancel")).size(11))
                        .on_press(Message::Layouts(LayoutAction::NewSheet))
                        .style(flat_tool_style),
                ]
                .spacing(4),
            ]
            .spacing(4),
        )
        .padding(6)
        .style(|theme| {
            let colors = ui_theme::colors(theme);
            container::Style::default()
                .background(colors.panel_alt)
                .border(iced::Border {
                    color: colors.border,
                    width: 1.0,
                    radius: 3.0.into(),
                })
        })
        .into()
    }

    /// Properties while a sheet is shown: the selected viewport, else the
    /// sheet with its title block, Place view and Export PDF.
    pub(crate) fn layout_properties(&self) -> Option<Element<'_, Message>> {
        let guid = self.drawing_view.shown_layout()?;
        let layout = self.layouts.layout(guid)?;
        let colors = self.ui_theme.colors();
        let note =
            |content: String| container(text(content).size(10).color(colors.muted)).padding([4, 8]);
        if let Some(viewport) = self
            .layouts
            .selected
            .as_deref()
            .and_then(|id| layout.viewport(id))
        {
            return Some(self.viewport_properties(viewport).into());
        }
        let on = |action: fn(String) -> LayoutAction| move |value| Message::Layouts(action(value));
        let mut block = column![]
            .spacing(0)
            .width(Fill)
            .push(opencad_properties::section_header("Sheet"))
            .push(opencad_properties::property_input(
                "Number",
                "01",
                &layout.number,
                on(LayoutAction::Number),
            ))
            .push(opencad_properties::property_input(
                "Name",
                "",
                &layout.name,
                on(LayoutAction::Name),
            ))
            .push(opencad_properties::property_control(
                "Paper",
                pick_list(Paper::ALL, Some(layout.paper), |paper| {
                    Message::Layouts(LayoutAction::SetPaper(paper))
                })
                .style(themed_pick_list_style)
                .text_size(11)
                .width(Fill)
                .into(),
            ))
            .push(opencad_properties::property_control(
                "Orientation",
                pick_list(
                    Orientation::ALL,
                    Some(Orientation::of(layout.landscape)),
                    |orientation| Message::Layouts(LayoutAction::SetOrientation(orientation)),
                )
                .style(themed_pick_list_style)
                .text_size(11)
                .width(Fill)
                .into(),
            ))
            .push(opencad_properties::section_header("Title block"))
            .push(opencad_properties::property_input(
                "Project",
                "",
                &layout.project,
                on(LayoutAction::Project),
            ))
            .push(opencad_properties::property_input(
                "Date",
                "2026-01-31",
                &layout.date,
                on(LayoutAction::Date),
            ))
            .push(opencad_properties::property_input(
                "Drawn by",
                "",
                &layout.drawn_by,
                on(LayoutAction::DrawnBy),
            ))
            .push(opencad_properties::property_row(
                "Drawing scale",
                layout.scale_text().unwrap_or_else(|| "—".to_owned()),
            ))
            .push(opencad_properties::section_header("Place view"));
        let choices = self.placeables();
        if choices.is_empty() {
            block = block.push(note(
                tr("Save a 3D view or make a drawing to place it on the sheet.").to_owned(),
            ));
        } else {
            let chosen = self
                .layouts
                .place
                .clone()
                .filter(|chosen| choices.contains(chosen));
            let ready = chosen.is_some();
            block = block.push(
                container(
                    row![
                        pick_list(choices, chosen, |choice| {
                            Message::Layouts(LayoutAction::PlaceChoice(choice))
                        })
                        .placeholder(tr("Choose a view"))
                        .style(themed_pick_list_style)
                        .text_size(11)
                        .width(Fill),
                        button(text(tr("Place")).size(11))
                            .on_press_maybe(ready.then_some(Message::Layouts(LayoutAction::Place)))
                            .style(flat_tool_style),
                    ]
                    .spacing(4)
                    .align_y(iced::Alignment::Center),
                )
                .padding([3, 8]),
            );
            block = block.push(note(
                tr("Or drag a row of VIEWS onto the paper.").to_owned(),
            ));
        }
        block = block
            .push(opencad_properties::section_header("Output"))
            .push(
                container(
                    row![
                        button(text(tr("Export PDF…")).size(11))
                            .on_press_maybe(
                                (!self.layouts.export_pending)
                                    .then_some(Message::Layouts(LayoutAction::ExportPdf))
                            )
                            .style(flat_tool_style),
                        button(text(tr("Fit sheet")).size(11))
                            .on_press(Message::Layouts(LayoutAction::Fit))
                            .style(flat_tool_style),
                    ]
                    .spacing(3),
                )
                .padding([3, 8]),
            );
        Some(block.into())
    }

    fn viewport_properties<'a>(
        &'a self,
        viewport: &'a Viewport,
    ) -> iced::widget::Column<'a, Message> {
        let colors = self.ui_theme.colors();
        let note =
            |content: String| container(text(content).size(10).color(colors.muted)).padding([4, 8]);
        let typed = |field: Field, value: String| -> String {
            let edits = &self.layouts.edits;
            if edits.0.as_deref() == Some(viewport.id.as_str()) {
                if let Some((_, typed)) = edits.1.iter().find(|(known, _)| *known == field) {
                    return typed.clone();
                }
            }
            value
        };
        let field = |field: Field| move |value| Message::Layouts(LayoutAction::Typed(field, value));
        let kind = match viewport.kind {
            PlacedKind::View => tr("3D view"),
            PlacedKind::Drawing => tr("Drawing"),
        };
        let missing = matches!(self.viewport_content(viewport), Content::Missing);
        let mut block = column![
            opencad_properties::section_header("Viewport"),
            opencad_properties::property_row("Placed view", viewport.name.clone()),
            opencad_properties::property_row("Kind", kind.to_owned()),
        ]
        .spacing(0)
        .width(Fill);
        if missing {
            block = block.push(note(
                tr("The view of this viewport was deleted; remove it or place another.").to_owned(),
            ));
        }
        if viewport.kind == PlacedKind::Drawing {
            let chosen = SCALES
                .iter()
                .find(|scale| (*scale - viewport.scale).abs() < 1e-9)
                .map(|scale| ScaleChoice(*scale));
            block = block
                .push(opencad_properties::property_control(
                    "Drawing scale",
                    pick_list(SCALES.map(ScaleChoice), chosen, |choice| {
                        Message::Layouts(LayoutAction::ScaleChosen(choice))
                    })
                    .placeholder(scale_label(viewport.scale))
                    .style(themed_pick_list_style)
                    .text_size(11)
                    .width(Fill)
                    .into(),
                ))
                .push(typed_row(
                    tr("Other scale"),
                    "1:75",
                    &typed(Field::Scale, scale_label(viewport.scale)),
                    field(Field::Scale),
                ));
        }
        let mm = |value: f64| format!("{value:.1}");
        block = block
            .push(typed_row(
                tr("X (mm)"),
                "",
                &typed(Field::X, mm(viewport.centre[0])),
                field(Field::X),
            ))
            .push(typed_row(
                tr("Y (mm)"),
                "",
                &typed(Field::Y, mm(viewport.centre[1])),
                field(Field::Y),
            ));
        match viewport.kind {
            PlacedKind::View => {
                block = block
                    .push(typed_row(
                        tr("Width (mm)"),
                        "",
                        &typed(Field::Width, mm(viewport.size[0])),
                        field(Field::Width),
                    ))
                    .push(typed_row(
                        tr("Height (mm)"),
                        "",
                        &typed(Field::Height, mm(viewport.size[1])),
                        field(Field::Height),
                    ));
                if let Some(picture) = self.layouts.images.get(&viewport.guid) {
                    block = block.push(opencad_properties::property_row(
                        "Resolution",
                        format!(
                            "{:.0} dpi · {} × {} px",
                            model::dots_per_inch(picture.pixels, viewport.size),
                            picture.pixels[0],
                            picture.pixels[1]
                        ),
                    ));
                }
            }
            PlacedKind::Drawing => {
                block = block.push(opencad_properties::property_row(
                    "Size (mm)",
                    format!("{} × {}", mm(viewport.size[0]), mm(viewport.size[1])),
                ));
            }
        }
        block = block
            .push(typed_row(
                tr("Title"),
                &viewport.name,
                viewport.title.as_deref().unwrap_or_default(),
                |value| Message::Layouts(LayoutAction::Title(value)),
            ))
            .push(
                container(
                    row![
                        button(text(tr("Remove from sheet")).size(11))
                            .on_press(Message::Layouts(LayoutAction::Remove(viewport.id.clone())))
                            .style(flat_tool_style),
                        button(text(tr("Deselect")).size(11))
                            .on_press(Message::Layouts(LayoutAction::Select(None)))
                            .style(flat_tool_style),
                    ]
                    .spacing(3),
                )
                .padding([3, 8]),
            )
            .push(note(
                tr("Drag the viewport on the paper to move it; Delete takes it off.").to_owned(),
            ));
        block
    }
}

/// A row of Properties with a field whose text is made here, as
/// `opencad_properties::property_input` lays it out; `label` is translated.
fn typed_row<'a>(
    label: &str,
    placeholder: &str,
    value: &str,
    on_input: impl Fn(String) -> Message + 'a,
) -> Element<'a, Message> {
    let label = container(text(label.to_owned()).size(11).style(|theme| text::Style {
        color: Some(ui_theme::colors(theme).muted),
    }))
    .style(|theme| container::Style::default().background(ui_theme::colors(theme).panel_alt))
    .width(iced::Length::FillPortion(5))
    .height(26)
    .align_y(iced::Alignment::Center)
    .padding([0, 6]);
    let value = container(
        text_input(placeholder, value)
            .on_input(on_input)
            .size(11)
            .padding([2, 4]),
    )
    .style(|theme| container::Style::default().background(ui_theme::colors(theme).panel))
    .width(iced::Length::FillPortion(6))
    .height(26)
    .align_y(iced::Alignment::Center)
    .padding([0, 5]);
    container(row![label, value])
        .height(26)
        .width(Fill)
        .style(|theme| container::Style {
            border: iced::Border {
                color: ui_theme::colors(theme).border,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}
