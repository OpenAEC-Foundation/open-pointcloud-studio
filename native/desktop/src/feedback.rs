//! Native client for Open Feedback Studio's explicit private route.
//! Only the fields shown here and the application version leave the app.

use std::time::Duration;

use iced::widget::{column, container, horizontal_space, row, text, text_editor};
use iced::{Element, Task};
use serde_json::{json, Value};

use crate::i18n::{key, tr};
use crate::{ui_style, Message, Studio};

const ENDPOINT: &str = "https://open-feedback-studio.pages.dev/api/private-feedback";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Category {
    Bug,
    Feature,
    #[default]
    General,
}

impl Category {
    const ALL: [Self; 3] = [Self::Bug, Self::Feature, Self::General];
    fn label(self) -> &'static str {
        match self {
            Self::Bug => key("Bug report"),
            Self::Feature => key("Feature request"),
            Self::General => key("General feedback"),
        }
    }
    fn code(self) -> &'static str {
        match self {
            Self::Bug => "bug",
            Self::Feature => "feature",
            Self::General => "general",
        }
    }
}

impl std::fmt::Display for Category {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(tr(self.label()))
    }
}

#[derive(Default)]
pub(crate) struct Feedback {
    pub open: bool,
    title: String,
    description: text_editor::Content,
    email: String,
    category: Category,
    pending: bool,
    sent: bool,
    error: Option<&'static str>,
}

#[derive(Debug, Clone)]
pub(crate) enum Action {
    Open,
    Close,
    Title(String),
    Description(text_editor::Action),
    Email(String),
    Category(Category),
    Send,
    Ready(Result<(), &'static str>),
}

const INVALID: &str = key("Enter a title (1–200 characters), a description (10–4500 characters) and a valid reply email or leave it empty.");
const UNAVAILABLE: &str =
    key("Feedback could not be confirmed. Your text is kept; try again later.");
const DUPLICATE: &str = key("This feedback has already been received. Your text is kept.");
const RATE_LIMIT: &str =
    key("Too many messages were sent recently. Your text is kept; try again later.");

fn length(s: &str) -> usize {
    // Match the JavaScript API's string length, including emoji.
    s.encode_utf16().count()
}

fn payload(
    title: &str,
    description: &str,
    email: &str,
    category: Category,
) -> Result<Value, &'static str> {
    let (title, description, email) = (title.trim(), description.trim(), email.trim());
    let email_ok = email.is_empty()
        || (length(email) <= 254
            && !email.chars().any(char::is_whitespace)
            && email.split_once('@').is_some_and(|(local, domain)| {
                !local.is_empty()
                    && !domain.contains('@')
                    && domain
                        .split_once('.')
                        .is_some_and(|(a, b)| !a.is_empty() && !b.is_empty())
            }));
    if title.is_empty()
        || length(title) > 200
        || !(10..=4500).contains(&length(description))
        || !email_ok
    {
        return Err(INVALID);
    }
    let message = format!("{title}\n\n{description}");
    if length(&message) > 5000 {
        return Err(INVALID);
    }
    let mut result = json!({
        "app": "Open Pointcloud Studio",
        "category": category.code(),
        "message": message,
        "appVersion": env!("CARGO_PKG_VERSION"),
    });
    if !email.is_empty() {
        result["email"] = json!(email);
    }
    Ok(result)
}

fn acknowledgement(status: u16, body: &str) -> Result<(), &'static str> {
    if !(200..300).contains(&status) {
        return Err(match status {
            400 => INVALID,
            409 => DUPLICATE,
            429 => RATE_LIMIT,
            _ => UNAVAILABLE,
        });
    }
    let value: Value = serde_json::from_str(body).map_err(|_| UNAVAILABLE)?;
    if value["success"] != true
        || value["visibility"] != "private"
        || !value["githubIssueUrl"].is_null()
        || !(value["imageKeys"].is_null()
            || value["imageKeys"].as_array().is_some_and(Vec::is_empty))
    {
        return Err(UNAVAILABLE);
    }
    Ok(())
}

fn submit_to(endpoint: &str, body: Value) -> Result<(), &'static str> {
    use std::io::Read;
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("OpenPointcloudStudio/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| UNAVAILABLE)?;
    let response = client
        .post(endpoint)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body.to_string())
        .send()
        .map_err(|_| UNAVAILABLE)?;
    let status = response.status().as_u16();
    let mut bytes = Vec::new();
    response
        .take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| UNAVAILABLE)?;
    if bytes.len() > 64 * 1024 {
        return Err(UNAVAILABLE);
    }
    acknowledgement(status, &String::from_utf8_lossy(&bytes))
}

impl Studio {
    pub(crate) fn feedback_action(&mut self, action: Action) -> Task<Message> {
        let form = &mut self.feedback;
        match action {
            Action::Open => {
                form.open = true;
                self.settings = None;
            }
            Action::Close => form.open = false,
            Action::Ready(result) => {
                if !form.pending {
                    return Task::none();
                }
                form.pending = false;
                match result {
                    Ok(()) => {
                        form.sent = true;
                        form.title.clear();
                        form.email.clear();
                        form.description = text_editor::Content::new();
                        form.error = None;
                    }
                    Err(error) => form.error = Some(error),
                }
            }
            _ if form.pending => {}
            Action::Title(value) => {
                form.title = value;
                form.sent = false;
                form.error = None;
            }
            Action::Email(value) => {
                form.email = value;
                form.sent = false;
                form.error = None;
            }
            Action::Category(value) => {
                form.category = value;
                form.sent = false;
                form.error = None;
            }
            Action::Description(action) => {
                form.description.perform(action);
                form.sent = false;
                form.error = None;
            }
            Action::Send => {
                let body = match payload(
                    &form.title,
                    &form.description.text(),
                    &form.email,
                    form.category,
                ) {
                    Ok(body) => body,
                    Err(error) => {
                        form.error = Some(error);
                        return Task::none();
                    }
                };
                form.pending = true;
                form.sent = false;
                form.error = None;
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || submit_to(ENDPOINT, body))
                            .await
                            .unwrap_or(Err(UNAVAILABLE))
                    },
                    |result| Message::Feedback(Action::Ready(result)),
                );
            }
        }
        Task::none()
    }

    pub(crate) fn feedback_view(&self) -> Option<Element<'_, Message>> {
        let form = &self.feedback;
        if !form.open {
            return None;
        }
        let colors = self.ui_theme.colors();
        let send = |action| Message::Feedback(action);
        let label = |label| text(tr(label)).size(12);
        let mut title = ui_style::text_input("", &form.title).size(12);
        let mut email = ui_style::text_input("", &form.email).size(12);
        let mut description = ui_style::text_editor(&form.description).height(140);
        if !form.pending {
            title = title.on_input(|value| Message::Feedback(Action::Title(value)));
            email = email.on_input(|value| Message::Feedback(Action::Email(value)));
            description =
                description.on_action(|action| Message::Feedback(Action::Description(action)));
        }
        let category: Element<'_, Message> = if form.pending {
            text(form.category.to_string()).size(12).into()
        } else {
            ui_style::pick_list(Category::ALL, Some(form.category), |value| {
                Message::Feedback(Action::Category(value))
            })
            .into()
        };
        let mut body = column![
            text(tr("Send a private message to Open Feedback Studio. Only this form and the app version are sent; scans, paths, screenshots and logs are not attached.")).size(12),
            label("Feedback type"), category,
            label("Title (up to 200 characters)"), title,
            label("Description (10–4500 characters)"), description,
            label("Reply email (optional)"), email,
        ].spacing(8);
        if form.sent {
            body = body.push(
                text(tr("Your feedback was received privately. Thank you!"))
                    .size(12)
                    .color(colors.accent),
            );
        }
        if let Some(error) = form.error {
            body = body.push(text(tr(error)).size(12).color(colors.text));
        }
        let can_send = !form.pending && !form.sent;
        let footer = row![
            horizontal_space(),
            ui_style::secondary_button(tr("Close")).on_press(send(Action::Close)),
            ui_style::primary_button(if form.pending {
                tr("Sending feedback…")
            } else {
                tr("Send feedback")
            })
            .on_press_maybe(can_send.then_some(send(Action::Send))),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);
        Some(openaec_ui::widgets::dialog_modal(
            tr("Feedback"),
            container(
                ui_style::scrollable(body).height(
                    self.window_size
                        .map_or(430.0, |size| (size.height - 140.0).clamp(140.0, 430.0)),
                ),
            )
            .padding(16)
            .into(),
            footer.into(),
            send(Action::Close),
            self.window_size
                .map_or(520.0, |size| (size.width - 24.0).clamp(280.0, 520.0)),
            self.ui_theme.shared(),
        ))
    }
}

#[cfg(test)]
mod tests;
