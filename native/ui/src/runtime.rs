use crate::*;
use iced::{Element, Task, Subscription};
use std::path::PathBuf;

/// Implement only domain behavior; the shared runtime owns all window chrome.
pub trait Application: 'static {
    type Command: Clone + std::fmt::Debug + Send + 'static;
    fn title(&self) -> String;
    fn ribbon(&self, language: Language) -> Vec<RibbonTab<Self::Command>>;
    fn chrome_mode(&self) -> ChromeMode { ChromeMode::Ribbon }
    fn tab_event(&mut self, _id: &str) -> Task<Self::Command> { Task::none() }
    fn workspace(&self, theme: Theme, language: Language) -> Element<'_, Self::Command>;
    /// Optional application dialog above the complete window shell.
    fn overlay(&self, _theme: Theme, _language: Language) -> Option<Element<'_, Self::Command>> { None }
    /// Domain Escape command when no shared settings dialog is open.
    fn escape(&self) -> Option<Self::Command> { None }
    fn update(&mut self, command: Self::Command) -> Task<Self::Command>;
    fn explorer(&self, theme: Theme, language: Language) -> Element<'_, Self::Command> {
        widgets::empty_state(language.text("Verkenner","Explorer"), "", theme)
    }
    fn properties(&self, theme: Theme, language: Language) -> Element<'_, Self::Command> {
        widgets::empty_state(language.text("Eigenschappen","Properties"), "", theme)
    }
    fn file_commands(&self, _language: Language) -> Vec<Command<Self::Command>> { Vec::new() }
    fn file_active_label(&self, _language: Language) -> Option<String> { None }
    fn quick_commands(&self, _language: Language) -> Vec<Command<Self::Command>> { Vec::new() }
    fn documents(&self) -> Vec<Document> { Vec::new() }
    fn active_document(&self) -> Option<String> { None }
    fn document_event(&mut self, _event: DocumentEvent) -> Task<Self::Command> { Task::none() }
    fn status(&self, language: Language) -> String { language.text("Gereed","Ready").into() }
    fn subscription(&self) -> Subscription<Self::Command> { Subscription::none() }
    /// Return false to keep the window open while the application asks to save.
    fn close_requested(&mut self) -> bool { true }
}
struct Runtime<A:Application> { app:A, shell:ShellState, preferences_path:Option<PathBuf>, error:Option<String> }
impl<A:Application> Runtime<A> {
    fn update(&mut self, message: Message<A::Command>) -> Task<Message<A::Command>> {
        match message {
            Message::Command(command) => {
                self.shell.backstage = false;
                self.app.update(command).map(Message::Command)
            }
            Message::Document(event) => self.app.document_event(event).map(Message::Command),
            Message::Ui(event) => {
                if matches!(event, UiEvent::SettingsEscape) && !self.shell.settings {
                    if let Some(command) = self.app.escape() {
                        return self.app.update(command).map(Message::Command);
                    }
                }
                let window_task:Option<Task<Message<A::Command>>> = match &event {
                    UiEvent::DragWindow => Some(iced::window::get_latest().and_then(iced::window::drag)),
                    UiEvent::Minimize => Some(iced::window::get_latest().and_then(|id|iced::window::minimize(id,true))),
                    UiEvent::Maximize => Some(iced::window::get_latest().and_then(iced::window::toggle_maximize)),
                    UiEvent::CloseWindow if self.app.close_requested() => Some(iced::window::get_latest().and_then(iced::window::close)),
                    _ => None,
                };
                let previewing_settings = self.shell.settings;
                self.shell.update(&event);
                if let UiEvent::Tab(id) = &event {
                    return self.app.tab_event(id).map(Message::Command);
                }
                if matches!(&event, UiEvent::SettingsSave | UiEvent::ToggleLeft | UiEvent::ToggleRight | UiEvent::Resize(_))
                    || (!previewing_settings && matches!(&event, UiEvent::Theme(_) | UiEvent::Language(_) | UiEvent::Scale(_))) {
                    if let Some(path)=&self.preferences_path {
                        self.error=self.shell.preferences.save(path).err().map(|e|format!("Settings: {e}"));
                    }
                }
                window_task.unwrap_or_else(Task::none)
            }
        }
    }
    fn view(&self) -> Element<'_,Message<A::Command>> {
        let theme=self.shell.preferences.theme; let language=self.shell.preferences.language;
        let mode=self.app.chrome_mode();
        shell(&self.shell, Slots {
            title:self.app.title(), mode, tabs:self.app.ribbon(language), documents:self.app.documents(),
            active_document:self.app.active_document(), file_commands:self.app.file_commands(language), file_active_label:self.app.file_active_label(language), quick_commands:self.app.quick_commands(language),
            status:self.error.clone().unwrap_or_else(||self.app.status(language)),
            overlay:self.app.overlay(theme,language).map(|element|element.map(Message::Command)),
            content:Box::new(move |region| match region {
                Region::Workspace => self.app.workspace(theme,language).map(Message::Command),
                Region::Explorer if mode == ChromeMode::SidebarCompact => self.app.explorer(theme,language).map(Message::Command),
                Region::Explorer if matches!(mode, ChromeMode::ThreePaneRibbon { .. } | ChromeMode::ThreePaneFixed { .. }) => self.app.explorer(theme,language).map(Message::Command),
                Region::Explorer => widgets::panel(if mode == ChromeMode::Workflow { language.text("Werkstroom","Workflow") } else if mode == ChromeMode::Sidebar { language.text("Navigatie","Navigation") } else if mode == ChromeMode::CanvasRibbon { language.text("Eigenschappen","Properties") } else { language.text("Verkenner","Explorer") },self.app.explorer(theme,language).map(Message::Command),theme),
                Region::Properties if matches!(mode, ChromeMode::ThreePaneRibbon { .. } | ChromeMode::ThreePaneFixed { .. }) => self.app.properties(theme,language).map(Message::Command),
                Region::Properties => widgets::panel(language.text("Eigenschappen","Properties"),self.app.properties(theme,language).map(Message::Command),theme),
            }),
        })
    }
    fn subscription(&self)->Subscription<Message<A::Command>> {
        Subscription::batch([
            self.app.subscription().map(Message::Command),
            iced::window::close_requests().map(|_|Message::Ui(UiEvent::CloseWindow)),
            iced::window::resize_events().map(|(_, size)|Message::Ui(UiEvent::WindowWidth(size.width))),
            iced::keyboard::on_key_press(|key,mods| {
                use iced::keyboard::{Key,key::Named};
                match key {
                    Key::Named(Named::Escape) => Some(Message::Ui(UiEvent::SettingsEscape)),
                    Key::Character(c) if mods.command() && c.as_str()=="," => Some(Message::Ui(UiEvent::Settings(true))),
                    _ => None,
                }
            }),
        ])
    }
}
/// Start the same shell in every tool. Pass a per-application preferences path,
/// or `None` for an ephemeral showcase. All fonts are embedded in this crate.
pub fn run<A:Application>(app:A, preferences_path:Option<PathBuf>) -> iced::Result {
    let (preferences,error)= match preferences_path.as_deref().filter(|p|p.exists()) {
        Some(path)=>match Preferences::load(path) { Ok(p)=>(p,None),Err(e)=>(Preferences::default(),Some(format!("Settings: {e}"))) },
        None=>(Preferences::default(),None),
    };
    iced::application(|state:&Runtime<A>|state.app.title(),Runtime::update,Runtime::view)
        .subscription(Runtime::subscription)
        .theme(|state:&Runtime<A>|state.shell.preferences.theme.iced())
        .scale_factor(|state:&Runtime<A>|state.shell.preferences.scale)
        .default_font(fonts::BODY).font(fonts::FILES[0]).font(fonts::FILES[1]).font(fonts::FILES[2])
        .font(fonts::FILES[3]).font(fonts::FILES[4]).font(fonts::FILES[5])
        .window(iced::window::Settings { size:iced::Size::new(model::metrics::WINDOW_DEFAULT_WIDTH as f32,800.0), min_size:Some(iced::Size::new(model::metrics::WINDOW_MIN_WIDTH as f32,480.0)), decorations:false, exit_on_close_request:false,..Default::default() })
        .run_with(||(Runtime{app,shell:ShellState::new(preferences),preferences_path,error},Task::none()))
}

#[cfg(test)]
mod settings_tests {
    use super::*;

    struct TestApplication;
    impl Application for TestApplication {
        type Command = ();
        fn title(&self) -> String { "Test".into() }
        fn ribbon(&self, _: Language) -> Vec<RibbonTab<Self::Command>> { Vec::new() }
        fn workspace(&self, _: Theme, _: Language) -> Element<'_, Self::Command> {
            iced::widget::text("test").into()
        }
        fn update(&mut self, _: Self::Command) -> Task<Self::Command> { Task::none() }
    }

    #[test]
    fn choosing_a_file_page_returns_to_the_workspace() {
        let mut runtime = Runtime {
            app: TestApplication,
            shell: ShellState::default(),
            preferences_path: None,
            error: None,
        };
        runtime.shell.backstage = true;
        let _ = runtime.update(Message::Command(()));
        assert!(!runtime.shell.backstage);
    }

    #[test]
    fn preview_does_not_write_preferences_until_save() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("openaec-ui-settings-{}-{unique}.json", std::process::id()));
        let mut runtime = Runtime {
            app: TestApplication,
            shell: ShellState::default(),
            preferences_path: Some(path.clone()),
            error: None,
        };
        let _ = runtime.update(Message::Ui(UiEvent::Settings(true)));
        let _ = runtime.update(Message::Ui(UiEvent::Theme(Theme::Forge)));
        assert!(!path.exists());
        let _ = runtime.update(Message::Ui(UiEvent::Settings(false)));
        assert_eq!(runtime.shell.preferences.theme, Theme::Light);
        let _ = runtime.update(Message::Ui(UiEvent::Settings(true)));
        let _ = runtime.update(Message::Ui(UiEvent::Theme(Theme::Forge)));
        let _ = runtime.update(Message::Ui(UiEvent::SettingsSave));
        assert_eq!(Preferences::load(&path).unwrap().theme, Theme::Forge);
        std::fs::remove_file(path).unwrap();
    }

    struct OverlayApplication { open: bool }
    impl Application for OverlayApplication {
        type Command = ();
        fn title(&self) -> String { "Overlay test".into() }
        fn ribbon(&self, _: Language) -> Vec<RibbonTab<Self::Command>> { Vec::new() }
        fn workspace(&self, _: Theme, _: Language) -> Element<'_, Self::Command> {
            iced::widget::text("workspace").into()
        }
        fn overlay(&self, _: Theme, _: Language) -> Option<Element<'_, Self::Command>> {
            self.open.then(|| iced::widget::text("dialog").into())
        }
        fn escape(&self) -> Option<Self::Command> { self.open.then_some(()) }
        fn update(&mut self, _: Self::Command) -> Task<Self::Command> {
            self.open = false;
            Task::none()
        }
    }

    #[test]
    fn escape_closes_domain_overlay_after_shared_settings() {
        let mut runtime = Runtime {
            app: OverlayApplication { open: true },
            shell: ShellState::default(),
            preferences_path: None,
            error: None,
        };
        let _ = runtime.update(Message::Ui(UiEvent::Settings(true)));
        let _ = runtime.update(Message::Ui(UiEvent::SettingsEscape));
        assert!(!runtime.shell.settings);
        assert!(runtime.app.open);
        let _ = runtime.update(Message::Ui(UiEvent::SettingsEscape));
        assert!(!runtime.app.open);
    }
}
