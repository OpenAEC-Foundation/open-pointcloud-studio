//! The wizard as a strip above the scene: what Show in model leaves of the
//! card. It lies in the column of the scene beside the progress lines and
//! outside the canvas, so that a screenshot, a view snapshot or a BCF image
//! never shows it, and the model stays in view and can be worked on.

use iced::widget::{button, container, horizontal_space, row, text};
use iced::{Background, Border, Color, Element, Fill};

use super::WizardAction;
use crate::i18n::{key, tr};
use crate::{opencad_ribbon, Message, Studio};

/// Light text on the dark background of the scene, whatever the theme.
const INK: Color = Color::from_rgb(0.98, 0.98, 0.976);
const MUTED: Color = Color::from_rgb(0.745, 0.745, 0.776);

/// A button of the strip: light text on the scene, a light hover.
fn strip_button_style(_: &iced::Theme, status: button::Status) -> button::Style {
    let (background, text_color) = match status {
        button::Status::Disabled => (None, Color::from_rgba(0.98, 0.98, 0.976, 0.35)),
        button::Status::Hovered | button::Status::Pressed => (
            Some(Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.12))),
            INK,
        ),
        button::Status::Active => (None, INK),
    };
    button::Style {
        background,
        text_color,
        border: Border {
            color: Color::from_rgba(1.0, 1.0, 1.0, 0.25),
            width: 1.0,
            radius: 3.0.into(),
        },
        ..button::Style::default()
    }
}

impl Studio {
    /// The strip, while the wizard is shown as one: its step and where that
    /// stands, with Back, Next and Back to wizard.
    pub(crate) fn mesh_to_plans_strip(&self) -> Option<Element<'_, Message>> {
        let wizard = &self.mesh_to_plans;
        if !wizard.open || !wizard.minimized {
            return None;
        }
        let send = Message::MeshToPlans;
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
            text(tr("Mesh to Plans")).size(12).color(INK),
            text(format!(
                "{}  {}  ·  {}",
                step.number(),
                tr(step.label()),
                wizard.status(step).text()
            ))
            .size(11)
            .color(MUTED),
            horizontal_space(),
            text(reason).size(11).color(MUTED),
            plain(
                key("Back"),
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
