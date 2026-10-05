//! Named section boxes of a scan, listed under Sections in the Project
//! Browser: Save keeps the section box as it is, a click on a name puts it
//! back, × deletes it. They are kept with the saved views.

use std::path::PathBuf;
use std::sync::mpsc;

use iced::widget::{button, column, row, text};
use iced::{Element, Fill, Task};
use serde::{Deserialize, Serialize};

use crate::{camera_views, i18n, native_api, opencad_ribbon, Message, Studio};

/// The most section boxes a scan keeps.
const MAX_SECTIONS: usize = 32;
/// The longest name of a section box, in characters.
const MAX_NAME_CHARS: usize = 64;

/// A section box kept under a name: its limits before the turn and the
/// turn about the vertical through its centre, in degrees.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedSection {
    /// The scan the box belongs to, as the saved views name it.
    pub source: PathBuf,
    pub name: String,
    pub min: [f64; 3],
    pub max: [f64; 3],
    #[serde(default)]
    pub rotation: f64,
}

#[derive(Debug, Default)]
pub struct SectionsTool {
    pub list: Vec<SavedSection>,
}

impl SectionsTool {
    pub fn load() -> Self {
        Self {
            list: camera_views::load_sections(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum SectionAction {
    /// Keep the section box as it is under the name typed in the field the
    /// views share.
    Save,
    /// Put the section box at this place in the list back.
    Apply(usize),
    Delete(usize),
}

impl Studio {
    pub(crate) fn update_sections(&mut self, action: SectionAction) -> Task<Message> {
        match action {
            SectionAction::Save => {
                let (Some(source), Some(section)) =
                    (self.active_camera_source(), self.section_box())
                else {
                    self.status = "Switch on the section box to save it".into();
                    return Task::none();
                };
                let of_scan = |saved: &&SavedSection| saved.source == source;
                let typed: String = self
                    .views
                    .name
                    .trim()
                    .chars()
                    .take(MAX_NAME_CHARS)
                    .collect();
                let name = if typed.is_empty() {
                    let count = self.sections.list.iter().filter(of_scan).count();
                    format!("Section {}", count + 1)
                } else {
                    typed
                };
                self.sections
                    .list
                    .retain(|saved| !(saved.source == source && saved.name == name));
                if self.sections.list.iter().filter(of_scan).count() >= MAX_SECTIONS {
                    self.status = format!("A scan keeps at most {MAX_SECTIONS} section boxes");
                    return Task::none();
                }
                self.sections.list.push(SavedSection {
                    source,
                    name: name.clone(),
                    min: section.bounds.min,
                    max: section.bounds.max,
                    rotation: section.rotation_degrees,
                });
                self.views.name.clear();
                self.save_sections();
                self.status = format!("Section box saved as {name}");
            }
            SectionAction::Apply(place) => {
                let Some(saved) = self.sections.list.get(place).cloned() else {
                    return Task::none();
                };
                // The same checks and the same placing as the command API.
                let (reply, answer) = mpsc::channel();
                let task = self.handle_api(native_api::ApiRequest {
                    command: native_api::ApiCommand::SetSection {
                        min: saved.min,
                        max: saved.max,
                        rotation: Some(saved.rotation),
                    },
                    reply,
                });
                let placed = answer
                    .try_recv()
                    .ok()
                    .is_some_and(|answer| answer["ok"] == true);
                self.status = if placed {
                    format!("Section box {} put back", saved.name)
                } else {
                    format!("Section box {} lies outside the open scans", saved.name)
                };
                return task;
            }
            SectionAction::Delete(place) => {
                if place < self.sections.list.len() {
                    let removed = self.sections.list.remove(place);
                    self.save_sections();
                    self.status = format!("Section box {} deleted", removed.name);
                }
            }
        }
        Task::none()
    }

    fn save_sections(&mut self) {
        if let Err(error) = camera_views::save_sections(&self.sections.list) {
            self.status = format!("Section boxes could not be saved: {error}");
        }
    }

    /// The section boxes of the active scan, as rows of the views and
    /// sections part of the Project Browser.
    pub(crate) fn section_rows(&self) -> Element<'_, Message> {
        let muted = self.ui_theme.colors().muted;
        let source = self.active_camera_source();
        let mut list = column![].spacing(2);
        for (place, saved) in self.sections.list.iter().enumerate() {
            if Some(&saved.source) != source.as_ref() {
                continue;
            }
            let current = self.section_box().is_some_and(|section| {
                section.bounds.min == saved.min
                    && section.bounds.max == saved.max
                    && section.rotation_degrees == saved.rotation
            });
            list = list.push(
                row![
                    button(text(saved.name.as_str()).size(11))
                        .on_press(Message::Sections(SectionAction::Apply(place)))
                        .style(move |theme, status| {
                            opencad_ribbon::tool_btn_style(theme, current, status)
                        })
                        .padding([3, 5])
                        .width(Fill),
                    text(i18n::tr("section")).size(10).color(muted),
                    button(text("×").size(11))
                        .on_press(Message::Sections(SectionAction::Delete(place)))
                        .style(crate::flat_tool_style)
                        .padding([3, 6]),
                ]
                .spacing(2)
                .align_y(iced::Alignment::Center),
            );
        }
        list.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn a_section_box_is_saved_put_back_and_deleted() {
        let directory = tempfile::tempdir().unwrap();
        camera_views::use_test_directory(directory.path());
        let source = directory.path().join("room.xyz");
        std::fs::write(&source, "0 0 0\n10 0 0\n10 8 0\n0 8 3\n5 4 1.5\n").unwrap();
        let cloud = pointcloud_core::open(&source, 10).unwrap();
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(Arc::new(cloud))));

        // Without the section box there is nothing to save.
        let _ = studio.update_sections(SectionAction::Save);
        assert!(studio.sections.list.is_empty());

        let (reply, _answer) = mpsc::channel();
        let _ = studio.handle_api(native_api::ApiRequest {
            command: native_api::ApiCommand::SetSection {
                min: [1.0, 1.0, 0.0],
                max: [6.0, 5.0, 2.0],
                rotation: Some(20.0),
            },
            reply,
        });
        studio.views.name = "Hall".into();
        let _ = studio.update_sections(SectionAction::Save);
        assert_eq!(studio.sections.list.len(), 1);
        assert_eq!(studio.sections.list[0].name, "Hall");
        assert_eq!(studio.sections.list[0].rotation, 20.0);
        let kept = camera_views::load_sections();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].name, "Hall");
        assert!(
            (0..3).all(|axis| (kept[0].max[axis] - studio.sections.list[0].max[axis]).abs() < 1e-9)
        );

        // Another box, then the saved one comes back.
        let (reply, _answer) = mpsc::channel();
        let _ = studio.handle_api(native_api::ApiRequest {
            command: native_api::ApiCommand::SetSection {
                min: [0.0, 0.0, 0.0],
                max: [10.0, 8.0, 3.0],
                rotation: None,
            },
            reply,
        });
        let _ = studio.update_sections(SectionAction::Apply(0));
        let section = studio.section_box().unwrap();
        assert!((section.rotation_degrees - 20.0).abs() < 1e-9);
        assert!((section.bounds.min[0] - 1.0).abs() < 1e-6);
        assert!((section.bounds.max[1] - 5.0).abs() < 1e-6);
        let _ = studio.view();

        let _ = studio.update_sections(SectionAction::Delete(0));
        assert!(studio.sections.list.is_empty());
        assert!(camera_views::load_sections().is_empty());
    }
}
