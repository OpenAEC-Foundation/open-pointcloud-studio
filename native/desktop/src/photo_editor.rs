use crate::{i18n, ui_style, EditBatch, Message, Studio};
use iced::widget::{button, column, container, row, text};
use iced::{Element, Fill, Task};
use pointcloud_core::{photo_edits::PhotoEdits, FilePhoto, PhotoProjection, ScanImageFormat};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Default)]
pub(crate) struct PhotoEditor {
    source: Option<PathBuf>,
    offset: Option<u64>,
    external: Option<PathBuf>,
    pub fields: [String; 8],
    error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_api::{ApiCommand, ApiRequest};
    fn send(studio: &mut Studio, value: serde_json::Value) -> serde_json::Value {
        let (reply, receive) = std::sync::mpsc::channel();
        let command: ApiCommand = serde_json::from_value(value).unwrap();
        let _ = studio.handle_api(ApiRequest { command, reply });
        receive.recv().unwrap()
    }
    #[test]
    fn api_and_gui_photo_edits_persist_export_and_undo() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("room.xyz");
        std::fs::write(&source, "0 0 0\n2 0 0\n0 2 0\n").unwrap();
        let original = std::fs::read(&source).unwrap();
        let panorama = dir.path().join("camera.png");
        image::RgbImage::from_pixel(8, 4, image::Rgb([30, 80, 120]))
            .save(&panorama)
            .unwrap();
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(Arc::new(
            pointcloud_core::open(&source, 100).unwrap(),
        ))));
        let added = send(
            &mut studio,
            serde_json::json!({"command":"add_panorama","path":panorama,"position":[1,1,1.6],"name":"Entry","heading":90}),
        );
        assert_eq!(added["ok"], true, "{added}");
        let photos = pointcloud_core::file_photos(&source).unwrap();
        assert_eq!(photos.photos.len(), 1);
        assert!((photos.photos[0].axes[0][0] - 1.).abs() < 1e-9);
        assert_eq!(std::fs::read(&source).unwrap(), original);
        let _ = studio.update(Message::UndoDelete);
        assert!(pointcloud_core::file_photos(&source)
            .unwrap()
            .photos
            .is_empty());
        let _ = studio.update(Message::RedoDelete);
        assert_eq!(
            pointcloud_core::file_photos(&source).unwrap().photos.len(),
            1
        );
        let changed = send(
            &mut studio,
            serde_json::json!({"command":"edit_photo","index":0,"position":[2,3,4],"name":"Moved","heading":45,"pitch":10}),
        );
        assert_eq!(changed["ok"], true, "{changed}");
        let photo = pointcloud_core::file_photos(&source)
            .unwrap()
            .photos
            .remove(0);
        assert_eq!(photo.position, [2., 3., 4.]);
        assert_eq!(photo.name.as_deref(), Some("Moved"));
        assert_eq!(
            send(
                &mut studio,
                serde_json::json!({"command":"delete_photo","index":0})
            )["ok"],
            true
        );
        assert!(pointcloud_core::file_photos(&source)
            .unwrap()
            .photos
            .is_empty());
        let _ = studio.update(Message::UndoDelete);
        assert_eq!(
            pointcloud_core::file_photos(&source).unwrap().photos.len(),
            1
        );
        let invalid = send(
            &mut studio,
            serde_json::json!({"command":"delete_photo","index":99}),
        );
        assert_eq!(invalid["ok"], false);
        let _ = studio.photo_editor_action(Action::Open);
        let _ = studio.photo_editor_action(Action::Edit(0));
        let picture = crate::test_render::render(
            studio.photo_editor_properties(),
            &studio.ui_theme.iced(),
            iced::Size::new(350., 650.),
        );
        if let Some(directory) = std::env::var_os("OPS_UI_REVIEW_DIR") {
            let path = std::path::Path::new(&directory);
            std::fs::create_dir_all(path).unwrap();
            picture.save_png(&path.join("photo-editor.png"));
        }
        let output = dir.path().join("edited.e57");
        crate::export_edited_where(
            &studio.clouds[0].cloud,
            &output,
            pointcloud_core::ExportFormat::E57,
            crate::CloudTransform::default(),
            None,
            3,
            |_, _| true,
        )
        .unwrap();
        let embedded = pointcloud_core::file_photos(&output).unwrap();
        assert_eq!(embedded.photos[0].name.as_deref(), Some("Moved"));
        assert_eq!(
            pointcloud_core::read_file_photo(&output, &embedded.photos[0]).unwrap(),
            std::fs::read(panorama).unwrap()
        );
    }
}
#[derive(Debug, Clone)]
pub enum Action {
    Open,
    Close,
    Add,
    Chosen(Option<PathBuf>),
    Edit(usize),
    Field(usize, String),
    UsePicked,
    Apply,
    Delete,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Camera {
    pub position: [f64; 3],
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub heading: f64,
    #[serde(default)]
    pub pitch: f64,
    #[serde(default)]
    pub roll: f64,
}

fn axes(heading: f64, pitch: f64, roll: f64) -> [[f64; 3]; 3] {
    let (s, c) = (90. - heading).to_radians().sin_cos();
    let (sp, cp) = pitch.to_radians().sin_cos();
    let (sr, cr) = roll.to_radians().sin_cos();
    [
        [c * cp, s * cp, sp],
        [-s * cr - c * sp * sr, c * cr - s * sp * sr, cp * sr],
        [s * sr - c * sp * cr, -c * sr - s * sp * cr, cp * cr],
    ]
}

impl Studio {
    pub(crate) fn api_photo_edit(
        &mut self,
        command: crate::native_api::ApiCommand,
    ) -> (serde_json::Value, Task<Message>) {
        use crate::native_api::ApiCommand;
        let (layer, index, path, camera, delete) = match command {
            ApiCommand::AddPanorama {
                path,
                layer,
                camera,
            } => (layer, None, Some(path), Some(camera), false),
            ApiCommand::EditPhoto {
                index,
                layer,
                camera,
            } => (layer, Some(index), None, Some(camera), false),
            ApiCommand::DeletePhoto { index, layer } => (layer, Some(index), None, None, true),
            _ => unreachable!(),
        };
        let layer = layer.or(self.active);
        let Some(layer) = layer.filter(|i| *i < self.clouds.len()) else {
            return (
                serde_json::json!({"ok":false,"error":"choose an open scan layer"}),
                Task::none(),
            );
        };
        let active = self.active;
        self.active = Some(layer);
        let _ = self.photo_editor_action(Action::Open);
        self.active = active;
        if let Some(index) = index {
            let _ = self.photo_editor_action(Action::Edit(index));
        }
        if let Some(path) = path {
            let _ = self.photo_editor_action(Action::Chosen(Some(path)));
        }
        if self.photo_editor.error.is_some() {
            return (
                serde_json::json!({"ok":false,"error":self.photo_editor.error}),
                Task::none(),
            );
        }
        if let Some(camera) = camera {
            if let Some(name) = camera.name {
                self.photo_editor.fields[0] = name;
            }
            for (i, value) in camera
                .position
                .into_iter()
                .chain([camera.heading, camera.pitch, camera.roll])
                .enumerate()
            {
                self.photo_editor.fields[i + 1] = value.to_string();
            }
        }
        let task = self.photo_editor_action(if delete {
            Action::Delete
        } else {
            Action::Apply
        });
        match &self.photo_editor.error {
            Some(error) => (serde_json::json!({"ok":false,"error":error}), task),
            None => (
                serde_json::json!({"ok":true,"photos":self.photos_value()}),
                task,
            ),
        }
    }
    pub(crate) fn reload_photo_edits(&mut self, source: &std::path::Path) -> Result<(), String> {
        let photos = pointcloud_core::file_photos(source).map_err(|e| e.to_string())?;
        self.photos
            .files
            .insert(source.to_path_buf(), Arc::new(photos));
        self.photos.invalidate(source);
        self.revision += 1;
        Ok(())
    }
    fn save_photo_edit(
        &mut self,
        source: PathBuf,
        previous: PhotoEdits,
        edited: PhotoEdits,
    ) -> Result<(), String> {
        edited.save(&source).map_err(|e| e.to_string())?;
        self.reload_photo_edits(&source)?;
        self.push_edit(EditBatch {
            members: Vec::new(),
            colours: Vec::new(),
            photos: vec![(source, previous)],
        });
        Ok(())
    }
    pub(crate) fn photo_editor_action(&mut self, action: Action) -> Task<Message> {
        let choose = matches!(&action, Action::Add);
        let result = (|| -> Result<(), String> {
            match action {
                Action::Open => {
                    let entry = self
                        .active
                        .and_then(|i| self.clouds.get(i))
                        .ok_or("Open a scan first")?;
                    self.photo_editor = PhotoEditor {
                        source: Some(entry.cloud.path.clone()),
                        fields: [
                            "Panorama".into(),
                            "0".into(),
                            "0".into(),
                            "0".into(),
                            "0".into(),
                            "0".into(),
                            "0".into(),
                            "1.6".into(),
                        ],
                        ..Default::default()
                    };
                    let center = entry.cloud.bounds.center();
                    for (i, value) in center.iter().enumerate() {
                        self.photo_editor.fields[i + 1] = value.to_string();
                    }
                }
                Action::Close => self.photo_editor = PhotoEditor::default(),
                Action::Field(index, value) => {
                    if let Some(field) = self.photo_editor.fields.get_mut(index) {
                        *field = value;
                    }
                }
                Action::Add => {
                    self.photo_editor.offset = None;
                    self.photo_editor.external = None;
                    return Ok(());
                }
                Action::Chosen(path) => {
                    if let Some(path) = path {
                        let (width, height) =
                            image::image_dimensions(&path).map_err(|e| e.to_string())?;
                        if width < 2
                            || height < 1
                            || (f64::from(width) / f64::from(height) - 2.).abs() > 0.02
                        {
                            return Err(
                                "Choose a full equirectangular panorama with a 2:1 aspect ratio"
                                    .into(),
                            );
                        }
                        self.photo_editor.external =
                            Some(path.canonicalize().map_err(|e| e.to_string())?);
                        self.photo_editor.offset = None;
                    }
                }
                Action::Edit(index) => {
                    let source = self
                        .photo_editor
                        .source
                        .as_ref()
                        .ok_or("Choose a scan first")?;
                    let photo = self
                        .photos
                        .files
                        .get(source)
                        .and_then(|p| p.photos.get(index))
                        .ok_or("Photo is unavailable")?;
                    self.photo_editor.offset = Some(photo.offset);
                    self.photo_editor.external = None;
                    self.photo_editor.fields[0] = photo
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("Photo {}", index + 1));
                    for i in 0..3 {
                        self.photo_editor.fields[i + 1] = photo.position[i].to_string();
                    }
                    self.photo_editor.fields[4] =
                        (90. - photo.axes[0][1].atan2(photo.axes[0][0]).to_degrees()).to_string();
                    self.photo_editor.fields[5] = photo.axes[0][2]
                        .clamp(-1., 1.)
                        .asin()
                        .to_degrees()
                        .to_string();
                    self.photo_editor.fields[6] = photo.axes[1][2]
                        .atan2(photo.axes[2][2])
                        .to_degrees()
                        .to_string();
                }
                Action::UsePicked => {
                    let source = self
                        .photo_editor
                        .source
                        .as_ref()
                        .ok_or("Choose a scan first")?;
                    let entry = self
                        .clouds
                        .iter()
                        .find(|e| e.cloud.path == *source)
                        .ok_or("Scan is closed")?;
                    let selection = entry
                        .selection
                        .as_ref()
                        .filter(|s| s.count == 1)
                        .ok_or("Pick one point in this scan first")?;
                    let point = selection
                        .highlights
                        .first()
                        .ok_or("Picked point is unavailable")?;
                    let mut xyz = if selection.highlights_source {
                        point.xyz
                    } else {
                        entry
                            .transform
                            .source_xyz(point.xyz)
                            .ok_or("Invalid transform")?
                    };
                    let height = self.photo_editor.fields[7]
                        .parse::<f64>()
                        .map_err(|_| "Invalid camera height")?;
                    xyz[2] += height;
                    for (i, value) in xyz.iter().enumerate() {
                        self.photo_editor.fields[i + 1] = value.to_string();
                    }
                }
                Action::Delete => {
                    let source = self
                        .photo_editor
                        .source
                        .clone()
                        .ok_or("Choose a scan first")?;
                    let offset = self.photo_editor.offset.ok_or("Choose a photo first")?;
                    let previous = PhotoEdits::load(&source).map_err(|e| e.to_string())?;
                    let mut edited = previous.clone();
                    edited.changes.insert(offset, None);
                    self.save_photo_edit(source, previous, edited)?;
                    self.photo_editor.offset = None;
                }
                Action::Apply => {
                    if self.photo_editor.fields[0].trim().is_empty()
                        || self.photo_editor.fields[0].chars().count() > 256
                    {
                        return Err("Photo names need 1 to 256 characters".into());
                    }
                    let source = self
                        .photo_editor
                        .source
                        .clone()
                        .ok_or("Choose a scan first")?;
                    let mut values = [0.; 6];
                    for (i, value) in values.iter_mut().enumerate() {
                        *value = self.photo_editor.fields[i + 1]
                            .parse::<f64>()
                            .map_err(|_| "Invalid camera coordinate or angle")?;
                    }
                    if values.iter().any(|v| !v.is_finite()) {
                        return Err("Camera values must be finite".into());
                    }
                    let previous = PhotoEdits::load(&source).map_err(|e| e.to_string())?;
                    let mut edited = previous.clone();
                    let rotation = axes(values[3], values[4], values[5]);
                    if let Some(offset) = self.photo_editor.offset {
                        let mut photo = self
                            .photos
                            .files
                            .get(&source)
                            .and_then(|p| p.photos.iter().find(|p| p.offset == offset))
                            .cloned()
                            .ok_or("Photo is unavailable")?;
                        photo.name = Some(self.photo_editor.fields[0].clone());
                        photo.position = [values[0], values[1], values[2]];
                        photo.axes = rotation;
                        edited.changes.insert(offset, Some(photo));
                    } else {
                        let path = self
                            .photo_editor
                            .external
                            .clone()
                            .ok_or("Choose a panorama image first")?;
                        let (width, height) =
                            image::image_dimensions(&path).map_err(|e| e.to_string())?;
                        let format = match image::ImageFormat::from_path(&path)
                            .map_err(|e| e.to_string())?
                        {
                            image::ImageFormat::Jpeg => ScanImageFormat::Jpeg,
                            image::ImageFormat::Png => ScanImageFormat::Png,
                            _ => return Err("Choose a JPG or PNG panorama".into()),
                        };
                        let id = edited
                            .add(
                                path,
                                FilePhoto {
                                    name: Some(self.photo_editor.fields[0].clone()),
                                    station: None,
                                    position: [values[0], values[1], values[2]],
                                    axes: rotation,
                                    width,
                                    height,
                                    projection: PhotoProjection::Spherical {
                                        pixel_size: [
                                            std::f64::consts::TAU / f64::from(width),
                                            std::f64::consts::PI / f64::from(height),
                                        ],
                                    },
                                    format,
                                    offset: 0,
                                    length: 0,
                                },
                            )
                            .map_err(|error| error.to_string())?;
                        self.photo_editor.offset = Some(id);
                    }
                    self.save_photo_edit(source, previous, edited)?;
                    self.status = "Photo edit saved; Undo restores the previous camera".into();
                }
            }
            Ok(())
        })();
        self.photo_editor.error = result.err();
        if choose {
            return Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .add_filter("360° panorama", &["jpg", "jpeg", "png"])
                        .pick_file()
                        .await
                        .map(|f| f.path().to_path_buf())
                },
                |path| Message::PhotoEdit(Action::Chosen(path)),
            );
        }
        self.schedule_detail()
    }
    pub(crate) fn photo_editor_properties(&self) -> Element<'_, Message> {
        let mut panel = column![crate::opencad_properties::section_header("Photo editing")];
        let control = |label: &'static str, action: Action| {
            button(text(i18n::tr(label)).size(11))
                .on_press(Message::PhotoEdit(action))
                .style(ui_style::tool)
        };
        if let Some(source) = &self.photo_editor.source {
            panel = panel.push(text(crate::display_name(source)).size(11));
            panel = panel.push(
                row![
                    control(i18n::key("Add panorama…"), Action::Add),
                    control(i18n::key("Close"), Action::Close)
                ]
                .spacing(4),
            );
            if let Some(photos) = self.photos.files.get(source) {
                for (index, photo) in photos.photos.iter().enumerate() {
                    panel = panel.push(
                        button(text(crate::file_photos::photo_label(photo, index)).size(11))
                            .on_press(Message::PhotoEdit(Action::Edit(index)))
                            .style(ui_style::tool),
                    );
                }
            }
            for (i, label) in [
                "Name",
                "X",
                "Y",
                "Z",
                i18n::key("Heading (°)"),
                i18n::key("Pitch (°)"),
                i18n::key("Roll (°)"),
                i18n::key("Camera height (m)"),
            ]
            .into_iter()
            .enumerate()
            {
                panel = panel.push(
                    row![
                        text(i18n::tr(label)).size(11).width(100),
                        ui_style::text_input("", &self.photo_editor.fields[i])
                            .on_input(move |v| Message::PhotoEdit(Action::Field(i, v)))
                            .size(11)
                    ]
                    .spacing(4),
                );
            }
            panel = panel.push(control(
                i18n::key("Place above picked point"),
                Action::UsePicked,
            ));
            if let Some(path) = &self.photo_editor.external {
                panel = panel
                    .push(text(path.file_name().unwrap_or_default().to_string_lossy()).size(10));
            }
            panel = panel
                .push(control(i18n::key("Apply photo edit"), Action::Apply).width(Fill))
                .push(control(i18n::key("Delete photo"), Action::Delete).width(Fill));
            if let Some(error) = &self.photo_editor.error {
                panel = panel.push(text(error).size(11));
            }
        } else {
            panel = panel.push(control(i18n::key("Manage photos…"), Action::Open));
        }
        container(panel.spacing(4))
            .padding([4, 8])
            .width(Fill)
            .into()
    }
}
