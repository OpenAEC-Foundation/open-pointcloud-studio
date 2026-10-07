//! The icons of the window, as the OpenAEC style book draws them: lines of
//! one colour on a square of 24, two units wide with round ends, which take
//! the colour of the text they stand with. A button that is hovered or on
//! colours its icon with its label; a ribbon button that is on gives its
//! icon the accent of the theme.
//!
//! The files are in `assets/ui-icons`, whose README says which of them come
//! from the style book and under which licence. An icon of an extension is
//! not one of these: it keeps the colours its author drew it in.

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::svg::{self, Renderer as _};
use iced::advanced::widget::{Tree, Widget};
use iced::{mouse, Color, Element, Length, Radians, Rectangle, Renderer, Size, Theme};

use crate::ui_theme::{self, UiColors};
use crate::{CameraPreset, Message};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolIcon {
    Open,
    OpenFolder,
    Export,
    Fit,
    Cloud,
    Select,
    SectionBox,
    Pick,
    MeasureDistance,
    MeasureArea,
    Note,
    Line,
    Clear,
    Undo,
    Redo,
    Camera(CameraPreset),
    Save,
    Move,
    Scale,
    Decimate,
    Mesh,
    Building,
    Shading,
    Drawing,
    ClosedMesh,
    Faces,
    MeshToPlans,
    /// Mesh Pointcloud, and two of its methods.
    MeshPointcloud,
    MeshTerrain,
    MeshSurface,
    /// The kinds of the Project Browser: a scan, the classes, the views
    /// and the 3D model, a saved view, a plan, an elevation, a section, a
    /// drawing file and BCF.
    Scan,
    Classes,
    Views,
    Model,
    SavedView,
    PlanSheet,
    ElevationSheet,
    SectionSheet,
    DrawingFile,
    Bcf,
    /// The chevron of an open and of a collapsed group, which point right
    /// and left as the arrows that scroll the ribbon.
    ChevronOpen,
    ChevronClosed,
    ChevronLeft,
    /// Duplicate a row of VIEWS.
    Duplicate,
    /// Rename a saved view, and overwrite it with what the scene shows.
    Rename,
    Update,
    /// A sheet of SHEETS.
    Sheet,
    /// The padlock of something locked, and of something that is not.
    Locked,
    Unlocked,
    /// A text, a dimension and a leader on a drawing.
    Text,
    Dimension,
    Leader,
    /// The four colourings of the points.
    ColourRgb,
    ColourElevation,
    ColourIntensity,
    ColourClasses,
    /// Settings.
    Settings,
}

impl ToolIcon {
    /// The file of the icon.
    fn bytes(self) -> &'static [u8] {
        match self {
            Self::Open => include_bytes!("../../assets/ui-icons/import.svg"),
            Self::OpenFolder => include_bytes!("../../assets/ui-icons/folder.svg"),
            Self::Export => include_bytes!("../../assets/ui-icons/export.svg"),
            Self::Fit => include_bytes!("../../assets/ui-icons/fit.svg"),
            Self::Cloud => include_bytes!("../../assets/ui-icons/cloud.svg"),
            Self::Select => include_bytes!("../../assets/ui-icons/select.svg"),
            Self::SectionBox => include_bytes!("../../assets/ui-icons/section_box.svg"),
            Self::Pick => include_bytes!("../../assets/ui-icons/pick_point.svg"),
            Self::MeasureDistance => include_bytes!("../../assets/ui-icons/measure_distance.svg"),
            Self::MeasureArea => include_bytes!("../../assets/ui-icons/measure_area.svg"),
            Self::Note => include_bytes!("../../assets/ui-icons/note.svg"),
            Self::Line => include_bytes!("../../assets/ui-icons/line.svg"),
            Self::Clear => include_bytes!("../../assets/ui-icons/clear.svg"),
            Self::Undo => include_bytes!("../../assets/ui-icons/undo.svg"),
            Self::Redo => include_bytes!("../../assets/ui-icons/redo.svg"),
            Self::Camera(CameraPreset::Top | CameraPreset::Bottom) => {
                include_bytes!("../../assets/ui-icons/view_top.svg")
            }
            Self::Camera(CameraPreset::Front | CameraPreset::Back) => {
                include_bytes!("../../assets/ui-icons/view_front.svg")
            }
            Self::Camera(CameraPreset::Right | CameraPreset::Left) => {
                include_bytes!("../../assets/ui-icons/view_right.svg")
            }
            Self::Camera(CameraPreset::Isometric) => {
                include_bytes!("../../assets/ui-icons/view_iso.svg")
            }
            Self::Save => include_bytes!("../../assets/ui-icons/save.svg"),
            Self::Move => include_bytes!("../../assets/ui-icons/move.svg"),
            Self::Scale => include_bytes!("../../assets/ui-icons/scale.svg"),
            Self::Decimate => include_bytes!("../../assets/ui-icons/thin.svg"),
            Self::Mesh => include_bytes!("../../assets/ui-icons/mesh.svg"),
            Self::Building => include_bytes!("../../assets/ui-icons/building.svg"),
            Self::Shading => include_bytes!("../../assets/ui-icons/eye_dome.svg"),
            Self::Drawing => include_bytes!("../../assets/ui-icons/section_drawing.svg"),
            Self::ClosedMesh => include_bytes!("../../assets/ui-icons/closed_mesh.svg"),
            Self::Faces => include_bytes!("../../assets/ui-icons/detect_faces.svg"),
            Self::MeshToPlans => include_bytes!("../../assets/ui-icons/mesh_to_plans.svg"),
            Self::MeshPointcloud => include_bytes!("../../assets/ui-icons/mesh_pointcloud.svg"),
            Self::MeshTerrain => include_bytes!("../../assets/ui-icons/mesh_terrain.svg"),
            Self::MeshSurface => include_bytes!("../../assets/ui-icons/mesh_surface.svg"),
            Self::Scan => include_bytes!("../../assets/ui-icons/scan.svg"),
            Self::Classes => include_bytes!("../../assets/ui-icons/classes.svg"),
            Self::Views => include_bytes!("../../assets/ui-icons/views.svg"),
            Self::Model => include_bytes!("../../assets/ui-icons/model.svg"),
            Self::SavedView => include_bytes!("../../assets/ui-icons/saved_view.svg"),
            Self::PlanSheet => include_bytes!("../../assets/ui-icons/plan.svg"),
            Self::ElevationSheet => include_bytes!("../../assets/ui-icons/elevation.svg"),
            Self::SectionSheet => include_bytes!("../../assets/ui-icons/section.svg"),
            Self::DrawingFile => include_bytes!("../../assets/ui-icons/drawing_file.svg"),
            Self::Bcf => include_bytes!("../../assets/ui-icons/bcf.svg"),
            Self::ChevronOpen => include_bytes!("../../assets/ui-icons/chevron_down.svg"),
            Self::ChevronClosed => include_bytes!("../../assets/ui-icons/chevron_right.svg"),
            Self::ChevronLeft => include_bytes!("../../assets/ui-icons/chevron_left.svg"),
            Self::Duplicate => include_bytes!("../../assets/ui-icons/duplicate.svg"),
            Self::Rename => include_bytes!("../../assets/ui-icons/rename.svg"),
            Self::Update => include_bytes!("../../assets/ui-icons/update.svg"),
            Self::Sheet => include_bytes!("../../assets/ui-icons/sheet.svg"),
            Self::Locked => include_bytes!("../../assets/ui-icons/locked.svg"),
            Self::Unlocked => include_bytes!("../../assets/ui-icons/unlocked.svg"),
            Self::Text => include_bytes!("../../assets/ui-icons/text.svg"),
            Self::Dimension => include_bytes!("../../assets/ui-icons/dimension.svg"),
            Self::Leader => include_bytes!("../../assets/ui-icons/leader.svg"),
            Self::ColourRgb => include_bytes!("../../assets/ui-icons/colour_rgb.svg"),
            Self::ColourElevation => include_bytes!("../../assets/ui-icons/colour_elevation.svg"),
            Self::ColourIntensity => include_bytes!("../../assets/ui-icons/colour_intensity.svg"),
            Self::ColourClasses => include_bytes!("../../assets/ui-icons/colour_classes.svg"),
            Self::Settings => include_bytes!("../../assets/ui-icons/settings.svg"),
        }
    }

    pub fn handle(self) -> svg::Handle {
        svg::Handle::from_memory(self.bytes())
    }
}

/// The glyphs of the buttons of the window and of the × of a tab, drawn on
/// a square of 10 as the template draws them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowGlyph {
    Minimize,
    Maximize,
    Restore,
    Close,
    TabClose,
}

impl WindowGlyph {
    pub fn handle(self) -> svg::Handle {
        svg::Handle::from_memory(match self {
            Self::Minimize => {
                include_bytes!("../../assets/ui-icons/window_minimize.svg").as_slice()
            }
            Self::Maximize => include_bytes!("../../assets/ui-icons/window_maximize.svg"),
            Self::Restore => include_bytes!("../../assets/ui-icons/window_restore.svg"),
            Self::Close => include_bytes!("../../assets/ui-icons/window_close.svg"),
            Self::TabClose => include_bytes!("../../assets/ui-icons/tab_close.svg"),
        })
    }
}

/// The logo of the application, the nine points, which the title bar draws
/// in the accent colour.
pub fn logo() -> svg::Handle {
    svg::Handle::from_memory(include_bytes!("../../assets/icons/logo.svg").as_slice())
}

/// `icon` at `size` pixels, in the colour of the text around it.
pub fn icon_svg(icon: ToolIcon, size: f32) -> Element<'static, Message> {
    glyph(icon.handle(), size).into()
}

/// The icon of a tool at `size` pixels: in the colour of the text of its
/// button, and in the accent of an active ribbon button while it is on.
pub fn tool_glyph(icon: ToolIcon, size: f32, active: bool) -> Element<'static, Message> {
    let drawn = glyph(icon.handle(), size);
    if active {
        drawn.tint(|colors| colors.ribbon_icon_active).into()
    } else {
        drawn.into()
    }
}

/// The drawing of `handle` at `size` pixels, in the colour of the text
/// around it until it is given another.
pub fn glyph(handle: svg::Handle, size: f32) -> Glyph {
    Glyph {
        handle,
        size,
        tint: None,
        opacity: 1.0,
    }
}

/// An icon that takes the colour of the text it stands with, as an icon of
/// the style book takes `currentColor`: in a button that of the button,
/// which changes under the pointer and while it is on.
pub struct Glyph {
    handle: svg::Handle,
    size: f32,
    tint: Option<fn(&UiColors) -> Color>,
    opacity: f32,
}

impl Glyph {
    /// The icon in a colour of the theme instead.
    pub fn tint(mut self, tint: fn(&UiColors) -> Color) -> Self {
        self.tint = Some(tint);
        self
    }

    /// The icon at `opacity`, as the icon of a tab that is not active.
    pub fn opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity;
        self
    }

    /// The colour the icon is drawn in, over text of `text_color`.
    fn color(&self, theme: &Theme, text_color: Color) -> Color {
        self.tint
            .map_or(text_color, |tint| tint(&ui_theme::colors(theme)))
    }
}

impl Widget<Message, Theme, Renderer> for Glyph {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(self.size), Length::Fixed(self.size))
    }

    fn layout(
        &self,
        _tree: &mut Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.resolve(
            Length::Fixed(self.size),
            Length::Fixed(self.size),
            Size::new(self.size, self.size),
        ))
    }

    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let color = self.color(theme, style.text_color);
        // The renderer paints an icon in the red, green and blue of its
        // colour only; the alpha of a quiet colour becomes its opacity.
        renderer.draw_svg(
            svg::Svg {
                handle: self.handle.clone(),
                color: Some(Color { a: 1.0, ..color }),
                rotation: Radians(0.0),
                opacity: self.opacity * color.a,
            },
            layout.bounds(),
        );
    }
}

impl<'a> From<Glyph> for Element<'a, Message> {
    fn from(glyph: Glyph) -> Self {
        Element::new(glyph)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_theme::UiTheme;
    use iced::widget::{button, container};

    fn folder() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../assets/ui-icons")
    }

    /// The icons, by their file name, with what the file holds.
    fn files() -> Vec<(String, String)> {
        let mut files: Vec<(String, String)> = std::fs::read_dir(folder())
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "svg"))
            .map(|path| {
                (
                    path.file_name().unwrap().to_string_lossy().into_owned(),
                    std::fs::read_to_string(&path).unwrap(),
                )
            })
            .collect();
        files.sort();
        files
    }

    /// The values of the attribute `name` in `svg`, and of `name` in its
    /// `style` attributes.
    fn values<'a>(svg: &'a str, name: &str) -> Vec<&'a str> {
        let mut found = Vec::new();
        let pattern = format!(" {name}=\"");
        for (at, _) in svg.match_indices(&pattern) {
            let rest = &svg[at + pattern.len()..];
            found.push(&rest[..rest.find('"').unwrap()]);
        }
        let pattern = format!("{name}:");
        for (at, _) in svg.match_indices(&pattern) {
            let rest = &svg[at + pattern.len()..];
            found.push(rest[..rest.find([';', '"']).unwrap()].trim());
        }
        found
    }

    #[test]
    fn every_icon_has_no_colour_but_that_of_the_text() {
        let files = files();
        assert!(files.len() > 50, "{}", files.len());
        let mut found = Vec::new();
        for (name, svg) in &files {
            for attribute in [
                "fill",
                "stroke",
                "color",
                "stop-color",
                "flood-color",
                "lighting-color",
            ] {
                for value in values(svg, attribute) {
                    if value != "none" && value != "currentColor" {
                        found.push(format!("{name}: {attribute}=\"{value}\""));
                    }
                }
            }
            // No colour written in another way, no picture and no
            // gradient.
            for other in ["#", "rgb(", "hsl(", "<image", "Gradient", "url("] {
                if svg.contains(other) {
                    found.push(format!("{name}: {other}"));
                }
            }
            // Drawn in lines: the root strokes in the colour of the text and
            // fills nothing unless a part says so.
            assert!(
                svg.starts_with("<svg ") && svg.contains(" fill=\"none\" stroke=\"currentColor\""),
                "{name}"
            );
        }
        assert!(found.is_empty(), "{found:#?}");
    }

    #[test]
    fn the_icons_are_drawn_on_the_square_of_the_style_book() {
        for (name, svg) in files() {
            if name.starts_with("window_") || name.starts_with("tab_") {
                // The glyphs of the window buttons and of the × of a tab, at
                // 10 pixels, as the template draws them.
                assert!(svg.contains("viewBox=\"0 0 10 10\""), "{name}");
            } else {
                assert!(svg.contains("viewBox=\"0 0 24 24\""), "{name}");
                assert!(
                    svg.contains(
                        "stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\""
                    ),
                    "{name}"
                );
                // No part of an icon draws a line of another width.
                assert_eq!(svg.matches("stroke-width").count(), 1, "{name}");
            }
        }
    }

    #[test]
    fn every_icon_file_is_used_and_every_one_used_is_there() {
        let source = include_str!("icons.rs");
        for (name, _) in files() {
            assert!(
                source.contains(&format!("ui-icons/{name}\"")),
                "{name} is used by no icon"
            );
        }
        let mut at = 0;
        while let Some(found) = source[at..].find("ui-icons/") {
            let start = at + found + "ui-icons/".len();
            let end = start + source[start..].find('"').unwrap();
            if !source[start..end].contains('{') {
                assert!(
                    folder().join(&source[start..end]).exists(),
                    "{}",
                    &source[start..end]
                );
            }
            at = end;
        }
    }

    /// Whether some pixel of `picture` has the red, green and blue of
    /// `color`, give or take `slack` a channel.
    fn holds(picture: &crate::test_render::Picture, color: Color, slack: u8) -> bool {
        let [r, g, b, _] = color.into_rgba8();
        (0..picture.height).any(|y| {
            (0..picture.width).any(|x| {
                let [red, green, blue] = picture.rgb(x, y);
                red.abs_diff(r) <= slack && green.abs_diff(g) <= slack && blue.abs_diff(b) <= slack
            })
        })
    }

    #[test]
    fn an_icon_takes_the_colour_of_its_button_and_the_accent_when_it_is_on() {
        for theme in [UiTheme::Light, UiTheme::Forge, UiTheme::Blueprint] {
            let colors = theme.colors();
            let picture = |active: bool, cursor: mouse::Cursor| {
                let icon = if active {
                    glyph(ToolIcon::Fit.handle(), 24.0).tint(|colors| colors.ribbon_icon_active)
                } else {
                    glyph(ToolIcon::Fit.handle(), 24.0)
                };
                crate::test_render::render_under(
                    container(button(icon).on_press(Message::ToggleFile).padding(4).style(
                        move |theme, status| crate::ui_style::ribbon_button(theme, active, status),
                    ))
                    .style(|theme| {
                        container::Style::default().background(ui_theme::colors(theme).bg)
                    })
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into(),
                    &theme.iced(),
                    Size::new(32.0, 32.0),
                    cursor,
                )
            };
            let idle = picture(false, mouse::Cursor::Unavailable);
            assert!(holds(&idle, colors.text, 12), "{}", theme.key());
            assert!(!holds(&idle, colors.accent, 12), "{}", theme.key());
            let over = picture(
                false,
                mouse::Cursor::Available(iced::Point::new(16.0, 16.0)),
            );
            assert!(
                holds(&over, colors.ribbon_text_hover, 12),
                "{}",
                theme.key()
            );
            let on = picture(true, mouse::Cursor::Unavailable);
            assert!(holds(&on, colors.ribbon_icon_active, 12), "{}", theme.key());
        }
    }

    #[test]
    fn a_quiet_colour_draws_a_quiet_icon() {
        // The muted text of Light is the text colour at half its strength:
        // its icon is lighter than the text, not as dark.
        let theme = UiTheme::Light;
        let colors = theme.colors();
        let picture = crate::test_render::render(
            container(glyph(ToolIcon::Clear.handle(), 24.0))
                .style(move |_| {
                    container::Style::default()
                        .background(Color::WHITE)
                        .color(colors.text_muted)
                })
                .width(Length::Fill)
                .height(Length::Fill)
                .into(),
            &theme.iced(),
            Size::new(24.0, 24.0),
        );
        let darkest = (0..24)
            .flat_map(|y| (0..24).map(move |x| (x, y)))
            .map(|(x, y)| picture.rgb(x, y)[0])
            .min()
            .unwrap();
        assert!(darkest > 120, "{darkest}");
        assert!(darkest < 200, "{darkest}");
    }
}
