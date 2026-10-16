use crate::{fonts, icons::Icon, model::*, widgets, Language, Theme};
use iced::{widget::{button, column, container, mouse_area, pane_grid, pick_list, row, scrollable, slider, stack, text, tooltip, Row, Space}, Element, Fill, Padding};

/// Application-owned contents. Shell layout and styling stay inside the crate.
pub struct Slots<'a, M> {
    pub title: String,
    pub mode: ChromeMode,
    pub tabs: Vec<RibbonTab<M>>,
    pub documents: Vec<Document>,
    pub active_document: Option<String>,
    pub file_commands: Vec<Command<M>>,
    pub file_active_label: Option<String>,
    pub quick_commands: Vec<Command<M>>,
    pub status: String,
    pub content: Box<dyn Fn(Region) -> Element<'a, Message<M>> + 'a>,
    pub overlay: Option<Element<'a, Message<M>>>,
}
fn horizontal<'a,M:'a>(content: impl Into<Element<'a,M>>) -> Element<'a,M> {
    scrollable(content).direction(scrollable::Direction::Horizontal(scrollable::Scrollbar::new())).into()
}
pub fn shell<'a, M: Clone + 'a>(state: &'a ShellState, slots: Slots<'a,M>) -> Element<'a,Message<M>> {
    let theme = state.preferences.theme;
    let t = theme.tokens(); let l = state.preferences.language;
    let title = mouse_area(container(text(slots.title).font(fonts::MEDIUM).size(12).color(t.accent)).width(Fill).height(metrics::TITLE).center(Fill))
        .on_press(Message::Ui(UiEvent::DragWindow));
    let mut quick = Row::new().spacing(1);
    for command in &slots.quick_commands {
        let hint=format!("{}{}",command.label,command.shortcut.as_ref().map(|s|format!(" ({s})")).unwrap_or_default());
        quick=quick.push(widgets::title_action(
            command.icon.view(16,if command.enabled {t.accent} else {t.text_faint}).into(),
            hint,
            command.enabled.then(||Message::Command(command.message.clone())),
            theme,
        ));
    }
    let titlebar = container(row![
        container(Icon::Cube.view(18,t.accent)).padding([0,8]), quick, title,
        widgets::title_action(Icon::Settings.view(16,t.text).into(),l.text("Instellingen","Settings"),Some(Message::Ui(UiEvent::Settings(true))),theme),
        widgets::title_action(Icon::Minimize.view(16,t.text).into(),l.text("Minimaliseren","Minimize"),Some(Message::Ui(UiEvent::Minimize)),theme),
        widgets::title_action(Icon::Maximize.view(16,t.text).into(),l.text("Maximaliseren","Maximize"),Some(Message::Ui(UiEvent::Maximize)),theme),
        widgets::title_action(Icon::Close.view(16,t.text).into(),l.text("Sluiten","Close"),Some(Message::Ui(UiEvent::CloseWindow)),theme),
    ].align_y(iced::Alignment::Center)).height(metrics::TITLE).style(move |_| container::Style::default().background(t.bg_lighter).color(t.text));
    let mut tabs = Row::new().spacing(if slots.mode == ChromeMode::CanvasRibbon { 6 } else { 2 });
    if !matches!(slots.mode, ChromeMode::TopNav | ChromeMode::CanvasRibbon | ChromeMode::Plain) {
        let label = l.text("Bestand", "File");
        tabs = tabs.push(widgets::ribbon_tab(
            text(label).font(fonts::STRONG).size(12).into(),
            Message::Ui(UiEvent::Backstage(true)),
            label.chars().count() as f32 * 7.0 + 32.0,
            widgets::RibbonTabKind::File,
            theme,
        ));
    }
    let active = slots.tabs.iter().find(|tab| tab.id == state.active_tab).or(slots.tabs.first());
    for tab in &slots.tabs {
        let selected = !state.backstage && active.is_some_and(|a| a.id == tab.id);
        tabs = tabs.push(widgets::ribbon_tab(
            text(tab.label.clone()).font(fonts::MEDIUM).size(12).into(),
            Message::Ui(UiEvent::Tab(tab.id.clone())),
            tab.label.chars().count() as f32 * 7.0 + 32.0,
            widgets::RibbonTabKind::Regular { active: selected },
            theme,
        ));
    }
    let tabs = widgets::ribbon_tab_strip(horizontal(tabs), theme);
    let mut groups = Row::new().spacing(0);
    if let Some(active) = active {
        for group in &active.groups {
            let mut commands = Row::new().spacing(1);
            let mut tools_width = 0.0_f32;
            for command in &group.commands {
                if let Some(control) = &command.control {
                    tools_width += control.width();
                    commands = commands.push(widgets::ribbon_control(theme, command.label.clone(), control.clone()).map(Message::Command));
                    continue;
                }
                let selected = command.selected;
                let color = if !command.enabled {t.text_faint} else if selected {t.accent} else {t.text};
                let label = command.label.clone();
                let control_width = (label.chars().count() as f32 * 7.0 + 20.0).clamp(66.0, 160.0);
                tools_width += control_width;
                let hint = format!("{}{}", label, command.shortcut.as_ref().map(|s|format!(" ({s})")).unwrap_or_default());
                let control = button(column![command.icon.view(24,color),text(label).font(fonts::BODY).size(12)].spacing(6).align_x(iced::Alignment::Center))
                    .padding([3,10]).width(control_width).height(52).on_press_maybe(command.enabled.then(|| Message::Command(command.message.clone())))
                    .style(move |_,s| widgets::ribbon_button_style(theme,selected,s));
                commands=commands.push(widgets::themed_tooltip(control,hint,tooltip::Position::Bottom,theme));
            }
            tools_width += group.commands.len().saturating_sub(1) as f32;
            let caption_width = (group.label.chars().count() as f32 * 6.0 + 8.0).max(50.0);
            let group_width = tools_width.max(caption_width) + 17.0;
            groups=groups.push(widgets::ribbon_group(theme, group.label.clone(), commands.into(), group_width));
        }
    }
    let ribbon = container(
        scrollable(groups)
            .direction(scrollable::Direction::Horizontal(
                scrollable::Scrollbar::new().width(3).scroller_width(3).margin(0),
            ))
            .height(Fill),
    ).height(metrics::RIBBON).padding(Padding { top: 0.0, right: 8.0, bottom: 0.0, left: 2.0 });
    let mut documents = Row::new().spacing(1).align_y(iced::Alignment::End);
    for doc in slots.documents {
        let selected=slots.active_document.as_ref() == Some(&doc.id);
        let label=format!("{}{}",doc.title,if doc.dirty {" •"} else {""});
        let close = button(text("×").size(12))
            .on_press(Message::Document(DocumentEvent::CloseRequested(doc.id.clone())))
            .style(move |_, status| widgets::document_close_style(theme, selected, status))
            .padding([0, 4]);
        let content = row![text(label).size(11), close]
            .spacing(6)
            .align_y(iced::Alignment::Center);
        let tab = button(content)
            .on_press(Message::Document(DocumentEvent::Activate(doc.id)))
            .height(24)
            .padding(Padding { top: 0.0, bottom: 0.0, left: 10.0, right: 4.0 })
            .style(move |_, status| widgets::document_tab_style(theme, selected, status));
        documents = documents.push(widgets::document_tab_bottom_gap(theme, tab.into()));
    }
    let document_bar=crate::frame::document_bar(theme, horizontal(documents));
    let compact_sidebar = (slots.mode == ChromeMode::SidebarCompact)
        .then(|| (slots.content)(Region::Explorer));
    let body: Element<'a,Message<M>> = if state.backstage {
        let pages = slots.file_commands.into_iter().map(|command| {
            let active = slots.file_active_label.as_ref() == Some(&command.label);
            widgets::BackstagePage {
                label: command.label,
                message: Message::Command(command.message),
                active,
                enabled: command.enabled,
            }
        }).collect();
        let footer = widgets::BackstageFooter {
            settings: (l.text("Instellingen…", "Settings…").into(), Message::Ui(UiEvent::Settings(true))),
            back: (l.text("←  Terug naar werkgebied", "←  Return to workspace").into(), Message::Ui(UiEvent::Backstage(false))),
            exit: (l.text("Afsluiten", "Exit").into(), Message::Ui(UiEvent::CloseWindow)),
        };
        row![
            widgets::backstage_nav_with(pages, Vec::new(), footer, move |_| theme),
            (slots.content)(Region::Workspace),
        ].height(Fill).width(Fill).into()
    } else if matches!(slots.mode, ChromeMode::TopNav | ChromeMode::Plain) {
        (slots.content)(Region::Workspace)
    } else if slots.mode == ChromeMode::RibbonWorkspace {
        (slots.content)(Region::Workspace)
    } else if slots.mode == ChromeMode::RibbonSidebar {
        row![
            container((slots.content)(Region::Explorer)).width(288).height(Fill),
            container((slots.content)(Region::Workspace)).width(Fill).height(Fill),
        ].height(Fill).width(Fill).into()
    } else if let ChromeMode::ThreePaneRibbon { left, right } = slots.mode {
        let mut panes = Row::new().spacing(2);
        if left {
            panes = panes.push(container((slots.content)(Region::Explorer)).width(iced::FillPortion(1)).height(Fill));
        }
        panes = panes.push(container((slots.content)(Region::Workspace))
            .width(iced::FillPortion(if left && right { 2 } else if left || right { 3 } else { 4 })).height(Fill));
        if right {
            panes = panes.push(container((slots.content)(Region::Properties)).width(iced::FillPortion(1)).height(Fill));
        }
        panes.height(Fill).width(Fill).into()
    } else if let ChromeMode::ThreePaneFixed { left_width, right_width } = slots.mode {
        row![
            container((slots.content)(Region::Explorer)).width(left_width).height(Fill),
            container((slots.content)(Region::Workspace)).width(Fill).height(Fill),
            container((slots.content)(Region::Properties)).width(right_width).height(Fill),
        ].height(Fill).width(Fill).into()
    } else if slots.mode == ChromeMode::CanvasRibbon {
        row![
            container((slots.content)(Region::Explorer)).width(255).height(Fill),
            container((slots.content)(Region::Workspace)).width(Fill).height(Fill),
        ].height(Fill).width(Fill).into()
    } else if matches!(slots.mode, ChromeMode::Workflow | ChromeMode::Sidebar) {
        row![
            container((slots.content)(Region::Explorer)).width(260).height(Fill),
            container((slots.content)(Region::Workspace)).width(Fill).height(Fill),
        ].height(Fill).width(Fill).into()
    } else if slots.mode == ChromeMode::SidebarCompact {
        (slots.content)(Region::Workspace)
    } else {
        pane_grid::PaneGrid::new(&state.panes, move |_, region, _| pane_grid::Content::new((slots.content)(*region)))
            .spacing(2).on_resize(6,|e|Message::Ui(UiEvent::Resize(e))).height(Fill).into()
    };
    let mut status_row = Row::new().push(text(slots.status).size(11).width(Fill));
    if slots.mode == ChromeMode::Ribbon {
        status_row = status_row
            .push(widgets::status_icon_button(l.text("Verkenner","Explorer"),Icon::PanelLeft,Message::Ui(UiEvent::ToggleLeft),theme))
            .push(widgets::status_icon_button(l.text("Eigenschappen","Properties"),Icon::PanelRight,Message::Ui(UiEvent::ToggleRight),theme));
    }
    let status = crate::frame::status_bar(
        theme,
        status_row.align_y(iced::Alignment::Center).height(Fill).into(),
        8,
    );
    let bands = if state.backstage {
        crate::frame::frame(crate::frame::FrameSlots {
            titlebar: titlebar.into(), tabs: tabs.into(), ribbon: None,
            documents: None, content: body, status: status.into(),
        })
    } else if slots.mode == ChromeMode::SidebarCompact {
        crate::frame::sidebar_frame(
            crate::frame::SidebarFrameProfile::WORKSPACE_COMPOSER,
            titlebar.into(),
            compact_sidebar.expect("compact sidebar"),
            body,
            status.into(),
        )
    } else if slots.mode == ChromeMode::Plain {
        crate::frame::workflow_frame(titlebar.into(), None, body, status.into())
    } else if slots.mode == ChromeMode::TopNav {
        crate::frame::frame(crate::frame::FrameSlots {
            titlebar: titlebar.into(), tabs: tabs.into(), ribbon: None,
            documents: None, content: body, status: status.into(),
        })
    } else if matches!(slots.mode, ChromeMode::Workflow | ChromeMode::Sidebar) {
        crate::frame::workflow_frame(titlebar.into(), (slots.mode == ChromeMode::Workflow).then_some(document_bar.into()), body, status.into())
    } else {
        crate::frame::frame(crate::frame::FrameSlots {
            titlebar: titlebar.into(),
            tabs: tabs.into(),
            ribbon: Some(ribbon.into()),
            documents: Some(document_bar.into()),
            content: body,
            status: status.into(),
        })
    };
    let base: Element<'a,Message<M>>=container(bands).height(Fill).width(Fill).style(move |_| widgets::surface(theme)).into();
    let mut layers = vec![base];
    if let Some(overlay) = slots.overlay { layers.push(overlay); }
    if state.settings {
        layers.push(settings(state));
        if state.settings_reset_confirm { layers.push(reset_confirmation(state)); }
    }
    if layers.len() == 1 { layers.pop().unwrap() }
    else { stack(layers).width(Fill).height(Fill).into() }
}
fn settings<'a,M:Clone+'a>(state:&'a ShellState) -> Element<'a,Message<M>> {
    let p=&state.preferences; let l=p.language;
    let heading = |label: &'static str| widgets::settings_section_heading(label,p.theme);
    let form: Element<'a, Message<M>> = match state.settings_page {
        SettingsPage::General => column![
            heading(l.text("Algemeen","General")),
            text(l.text("Taal","Language")).size(11),
            pick_list(Language::ALL,Some(p.language),|language|Message::Ui(UiEvent::Language(language)))
                .style(move |_,status|widgets::pick_list_style(p.theme,status))
                .menu_style(move |_|widgets::pick_list_menu_style(p.theme)).width(Fill),
            text(format!("{}: {:.0}%",l.text("Schaal","Scale"),p.scale*100.0)).size(11),
            slider(0.75..=2.0,p.scale,|scale|Message::Ui(UiEvent::Scale(scale)))
                .step(0.05).style(move |_,status|widgets::slider_style(p.theme,status)),
        ].spacing(10).into(),
        SettingsPage::Appearance => column![
            heading(l.text("Thema","Theme")),
            row![
                Space::with_width(Fill),
                widgets::settings_theme_picker(
                    Theme::ALL.into_iter().map(|theme| (
                        theme, theme.to_string(), Message::Ui(UiEvent::Theme(theme)),
                    )).collect(),
                    p.theme, p.theme, state.settings_theme_open,
                    Message::Ui(UiEvent::SettingsThemeMenu(!state.settings_theme_open)),
                ),
            ],
        ].spacing(10).into(),
        SettingsPage::About => column![
            heading(l.text("Over","About")),
            text("OpenAEC UI").font(fonts::STRONG).size(12),
            text(l.text("Gedeelde native Rust-interface voor OpenAEC-tools.","Shared native Rust interface for OpenAEC tools.")).size(11),
        ].spacing(10).into(),
    };
    let sidebar=column![
        widgets::settings_tab(l.text("Algemeen","General"),state.settings_page==SettingsPage::General,Message::Ui(UiEvent::SettingsPage(SettingsPage::General)),p.theme),
        widgets::settings_tab(l.text("Uiterlijk","Appearance"),state.settings_page==SettingsPage::Appearance,Message::Ui(UiEvent::SettingsPage(SettingsPage::Appearance)),p.theme),
        widgets::settings_tab(l.text("Over","About"),state.settings_page==SettingsPage::About,Message::Ui(UiEvent::SettingsPage(SettingsPage::About)),p.theme),
    ].spacing(2);
    let footer=row![
        widgets::dialog_button(l.text("Standaardwaarden herstellen","Reset to Defaults"),move |_,status|widgets::secondary_button_style(p.theme,status))
            .on_press(Message::Ui(UiEvent::SettingsResetPrompt(true))),
        Space::with_width(Fill),
        widgets::dialog_button(l.text("Annuleren","Cancel"),move |_,status|widgets::secondary_button_style(p.theme,status))
            .on_press(Message::Ui(UiEvent::Settings(false))),
        widgets::dialog_button(l.text("Opslaan","Save"),move |_,status|widgets::primary_button_style(p.theme,status))
            .on_press(Message::Ui(UiEvent::SettingsSave)),
    ].spacing(6).align_y(iced::Alignment::Center);
    widgets::settings_modal(
        l.text("Instellingen","Settings"), sidebar.into(), form, footer.into(),
        Message::Ui(UiEvent::Settings(false)), p.theme,
    )
}
fn reset_confirmation<'a,M:Clone+'a>(state:&'a ShellState) -> Element<'a,Message<M>> {
    let l=state.preferences.language;
    widgets::settings_reset_confirmation(
        l.text("Standaardwaarden herstellen","Reset to Defaults"),
        l.text("Weet je zeker dat je alle instellingen naar hun standaardwaarden wilt herstellen?",
            "Are you sure you want to reset all settings to their default values?"),
        l.text("Annuleren","Cancel"),
        l.text("Standaardwaarden herstellen","Reset to Defaults"),
        Message::Ui(UiEvent::SettingsResetPrompt(false)),
        Message::Ui(UiEvent::SettingsReset),
        state.preferences.theme,
    )
}
