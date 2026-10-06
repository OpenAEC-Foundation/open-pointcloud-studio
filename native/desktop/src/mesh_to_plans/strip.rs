//! The wizard as a strip above the scene: what Show in model leaves of the
//! card. It lies in the column of the scene beside the progress lines and
//! outside the canvas, so that a screenshot, a view snapshot or a BCF image
//! never shows it, and the model stays in view and can be worked on.

use iced::widget::{button, container, horizontal_space, row, text};
use iced::{Background, Border, Color, Element, Fill};

use super::WizardAction;
use crate::i18n::{key, tr};
use crate::{opencad_ribbon, ui_theme, Message, Studio};

/// The colour of the text of the scene with this much of it.
fn faded(color: Color, alpha: f32) -> Color {
    Color { a: alpha, ..color }
}

/// A button of the strip: the text colour of the scene, light on the dark
/// scene of a dark theme and dark on the white one of the light theme.
fn strip_button_style(theme: &iced::Theme, status: button::Status) -> button::Style {
    let ink = ui_theme::colors(theme).scene_text;
    let (background, text_color) = match status {
        button::Status::Disabled => (None, faded(ink, 0.35)),
        button::Status::Hovered | button::Status::Pressed => {
            (Some(Background::Color(faded(ink, 0.10))), ink)
        }
        button::Status::Active => (None, ink),
    };
    button::Style {
        background,
        text_color,
        border: Border {
            color: faded(ink, 0.25),
            width: 1.0,
            radius: 3.0.into(),
        },
        ..button::Style::default()
    }
}

impl Studio {
    /// The strip, while the wizard is shown as one: its step and where that
    /// stands, with Previous, Next and Back to wizard.
    pub(crate) fn mesh_to_plans_strip(&self) -> Option<Element<'_, Message>> {
        let wizard = &self.mesh_to_plans;
        if !wizard.open || !wizard.minimized {
            return None;
        }
        let send = Message::MeshToPlans;
        let colors = self.ui_theme.colors();
        let (ink, muted) = (colors.scene_text, colors.scene_muted);
        let step = wizard.step;
        let ready = wizard.step_ready();
        let reason = ready
            .as_ref()
            .err()
            .map(|sentence| sentence.translated())
            .unwrap_or_default();
        let plain = |label: &'static str, message: Option<Message>| {
            button(text(tr(label)).size(11))
                .on_press_maybe(message)
                .style(strip_button_style)
                .padding([2, 10])
        };
        let line = row![
            text(tr("Pointcloud to Drawing")).size(12).color(ink),
            text(format!(
                "{}  {}  ·  {}",
                step.number(),
                tr(step.label()),
                wizard.status(step).text()
            ))
            .size(11)
            .color(muted),
            horizontal_space(),
            text(reason).size(11).color(muted),
            plain(
                key("Previous"),
                step.previous().map(|_| send(WizardAction::Back))
            ),
            plain(
                key("Next"),
                ready.is_ok().then_some(send(WizardAction::Next))
            ),
            button(text(tr("Back to wizard")).size(11))
                .on_press(send(WizardAction::Restore))
                .style(|theme, status| opencad_ribbon::file_tab_style(theme, false, status))
                .padding([2, 12]),
            button(text("×").size(13))
                .on_press(send(WizardAction::Close))
                .style(strip_button_style)
                .padding([0, 8]),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center);
        Some(
            container(container(line).height(24).clip(true))
                .padding(iced::Padding {
                    top: 2.0,
                    right: 14.0,
                    bottom: 7.0,
                    left: 14.0,
                })
                .width(Fill)
                .into(),
        )
    }
}
