//! Environment defaults for the supported native SuperCloud route.
//! These override startup preferences without changing saved user settings.

use std::path::PathBuf;
use std::sync::OnceLock;

use crate::{i18n::Language, ui_theme::UiTheme};

#[derive(Debug, Default)]
struct Config {
    enabled: bool,
    folder: Option<PathBuf>,
    language: Option<Language>,
    theme: Option<UiTheme>,
}

impl Config {
    fn parse(
        enabled: bool,
        folder: Option<PathBuf>,
        language: Option<&str>,
        theme: Option<&str>,
    ) -> Result<Self, String> {
        if !enabled {
            return Ok(Self::default());
        }
        if let Some(path) = &folder {
            if !path.is_absolute() || !path.is_dir() {
                return Err("OPENAEC_MAP must be an existing absolute directory".into());
            }
        }
        let language = language
            .map(|value| {
                Language::from_key(value)
                    .ok_or_else(|| "OPENAEC_TAAL must be auto, en or nl".to_owned())
            })
            .transpose()?;
        let theme = theme
            .map(|value| {
                UiTheme::from_key(if value == "dark" { "openaec" } else { value })
                    .ok_or_else(|| "OPENAEC_THEMA is not a supported theme".to_owned())
            })
            .transpose()?;
        Ok(Self {
            enabled,
            folder,
            language,
            theme,
        })
    }
}

fn config() -> Result<&'static Config, &'static String> {
    static CONFIG: OnceLock<Result<Config, String>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let language = std::env::var("OPENAEC_TAAL").ok();
            let theme = std::env::var("OPENAEC_THEMA").ok();
            Config::parse(
                std::env::var("OPENAEC_BEHEERD").as_deref() == Ok("1"),
                std::env::var_os("OPENAEC_MAP").map(PathBuf::from),
                language.as_deref(),
                theme.as_deref(),
            )
        })
        .as_ref()
}

pub(crate) fn validate() -> Result<(), String> {
    config().map(|_| ()).map_err(Clone::clone)
}

pub(crate) fn language() -> Option<Language> {
    config().ok().and_then(|config| config.language)
}

pub(crate) fn theme() -> Option<UiTheme> {
    config().ok().and_then(|config| config.theme)
}

pub(crate) fn summary() -> serde_json::Value {
    match config() {
        Ok(config) => serde_json::json!({
            "enabled": config.enabled,
            "directory": config.folder,
        }),
        Err(error) => serde_json::json!({"enabled": false, "error": error}),
    }
}

/// An explicit directory chosen later by a workflow still takes precedence.
pub(crate) fn file_dialog() -> rfd::AsyncFileDialog {
    let dialog = rfd::AsyncFileDialog::new();
    match config().ok().and_then(|config| config.folder.as_ref()) {
        Some(folder) => dialog.set_directory(folder),
        None => dialog,
    }
}

pub(crate) fn shutdown() -> iced::Subscription<()> {
    #[cfg(unix)]
    if config().is_ok_and(|config| config.enabled) {
        return iced::Subscription::run(|| {
            iced::stream::channel(1, |mut output| async move {
                use iced::futures::SinkExt;
                match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                    Ok(mut signal) => {
                        signal.recv().await;
                        let _ = output.send(()).await;
                    }
                    Err(error) => eprintln!("Managed shutdown handler unavailable: {error}"),
                }
                std::future::pending::<()>().await;
            })
        });
    }
    iced::Subscription::none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_desktop_ignores_platform_environment() {
        let config = Config::parse(false, Some("missing".into()), Some("bad"), Some("bad"))
            .expect("platform variables do not configure an unmanaged desktop");
        assert!(!config.enabled);
        assert!(config.folder.is_none());
        assert!(config.language.is_none());
        assert!(config.theme.is_none());
    }

    #[test]
    fn managed_defaults_use_validated_folder_language_and_theme() {
        let directory = tempfile::tempdir().unwrap();
        let config = Config::parse(
            true,
            Some(directory.path().to_owned()),
            Some("nl"),
            Some("dark"),
        )
        .unwrap();
        assert_eq!(config.folder.as_deref(), Some(directory.path()));
        assert_eq!(config.language, Language::from_key("nl"));
        assert_eq!(config.theme, Some(UiTheme::OpenAec));
    }

    #[test]
    fn invalid_configuration_names_the_variable() {
        assert!(Config::parse(true, Some("relative".into()), None, None)
            .unwrap_err()
            .contains("OPENAEC_MAP"));
        assert!(Config::parse(true, None, Some("bad"), None)
            .unwrap_err()
            .contains("OPENAEC_TAAL"));
        assert!(Config::parse(true, None, None, Some("bad"))
            .unwrap_err()
            .contains("OPENAEC_THEMA"));
    }
}
