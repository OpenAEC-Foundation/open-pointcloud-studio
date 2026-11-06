use crate::{i18n, ui_style, Message, Studio};
use iced::widget::{button, column, container, row, text};
use iced::{Element, Fill, Task};
use pointcloud_core::{
    registration::{self, IcpOptions, IcpResult},
    scene_transform::SceneTransform,
};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

#[derive(Default)]
pub(crate) struct RegistrationTool {
    pub open: bool,
    pub fixed: Option<usize>,
    pub moving: Option<usize>,
    pub fixed_points: Vec<[f64; 3]>,
    pub moving_points: Vec<[f64; 3]>,
    pub transform: SceneTransform,
    pub has_alignment: bool,
    pub report: Option<serde_json::Value>,
    pub fields: [String; 4],
    pub busy: bool,
    pub cancel: Arc<AtomicBool>,
    pub error: Option<String>,
    pub api_job: Option<String>,
    source_paths: [Option<PathBuf>; 2],
    snapshots: Option<Vec<(Arc<pointcloud_core::PointCloud>, crate::CloudTransform)>>,
    preview: Option<Arc<tempfile::TempDir>>,
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
    fn point_pair_gui_and_api_share_the_fit_and_matrix_contract() {
        let dir = tempfile::tempdir().unwrap();
        let mut studio = Studio::default();
        for name in ["reference.xyz", "moving.xyz"] {
            let path = dir.path().join(name);
            std::fs::write(&path, "0 0 0\n1 0 0\n0 1 0\n0 0 1\n").unwrap();
            let _ = studio.update(Message::Loaded(Ok(Arc::new(
                pointcloud_core::open(path, 100).unwrap(),
            ))));
        }
        assert_eq!(
            send(
                &mut studio,
                serde_json::json!({"command":"registration_setup","fixed":0,"moving":1})
            )["ok"],
            true
        );
        for point in [[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [0., 0., 1.]] {
            let fixed = [10. - point[1], 20. + point[0], 30. + point[2]];
            assert_eq!(
                send(
                    &mut studio,
                    serde_json::json!({"command":"registration_pair","fixed":fixed,"moving":point})
                )["ok"],
                true
            );
        }
        let fit = send(
            &mut studio,
            serde_json::json!({"command":"registration_fit"}),
        );
        assert_eq!(fit["ok"], true, "{fit}");
        assert!(fit["registration"]["report"]["rms_error"].as_f64().unwrap() < 1e-9);
        let p = studio.registration.transform.xyz([1., 2., 3.]);
        assert!((p[0] - 8.).abs() < 1e-9 && (p[1] - 21.).abs() < 1e-9);
        let picture = crate::test_render::render(
            studio.registration_properties(),
            &studio.ui_theme.iced(),
            iced::Size::new(350., 1000.),
        );
        if let Some(directory) = std::env::var_os("OPS_UI_REVIEW_DIR") {
            let path = std::path::Path::new(&directory);
            std::fs::create_dir_all(path).unwrap();
            picture.save_png(&path.join("registration.png"));
        }
        let saved = dir.path().join("matrix.json");
        let _ = studio.registration_action(Action::MatrixPath(Some(saved.clone())));
        assert!(std::fs::read_to_string(saved)
            .unwrap()
            .contains("transform"));
        let invalid = send(
            &mut studio,
            serde_json::json!({"command":"registration_set_matrix","transform":{"rotation":[[0,0,0],[0,0,0],[0,0,0]],"translation":[0,0,0],"scale":1}}),
        );
        assert_eq!(invalid["ok"], false);
    }

    #[test]
    fn cancellation_keeps_the_original_job_and_running_inputs_cannot_change() {
        let mut studio = Studio::default();
        let id = studio.record_api_job(serde_json::json!({"state":"running"}));
        studio.registration.busy = true;
        studio.registration.api_job = Some(id.clone());
        for command in [
            serde_json::json!({"command":"registration_pair","fixed":[1,2,3],"moving":[4,5,6]}),
            serde_json::json!({"command":"registration_setup","fixed":0,"moving":1}),
            serde_json::json!({"command":"registration_set_matrix","transform":{"rotation":[[1,0,0],[0,1,0],[0,0,1]],"translation":[10,20,30],"scale":1}}),
        ] {
            let result = send(&mut studio, command);
            assert_eq!(result["ok"], false, "{result}");
            assert_eq!(result["error"], "registration is running");
        }
        assert!(studio.registration.fixed_points.is_empty());
        assert_eq!(studio.registration.transform.translation, [0.; 3]);
        let status = send(
            &mut studio,
            serde_json::json!({"command":"registration_matrix"}),
        );
        assert_eq!(status["ok"], true);
        let cancelled = send(
            &mut studio,
            serde_json::json!({"command":"cancel_registration"}),
        );
        assert_eq!(cancelled["ok"], true);
        assert!(cancelled["job_id"].is_null());
        assert_eq!(studio.registration.api_job.as_deref(), Some(id.as_str()));
        assert_eq!(studio.api_jobs.len(), 1);
        assert!(studio.registration.cancel.load(Ordering::Relaxed));
        let _ = studio.registration_action(Action::Refined(Err("Operation cancelled".into())));
        assert_eq!(studio.api_jobs[&id]["state"], "cancelled");
        assert!(!studio.registration.busy);
    }

    #[test]
    fn registration_roles_follow_their_sources_when_layers_close() {
        let dir = tempfile::tempdir().unwrap();
        let mut studio = Studio::default();
        for name in ["other.xyz", "fixed.xyz", "moving.xyz"] {
            let path = dir.path().join(name);
            std::fs::write(&path, "0 0 0\n1 0 0\n0 1 0\n0 0 1\n").unwrap();
            let _ = studio.update(Message::Loaded(Ok(Arc::new(
                pointcloud_core::open(path, 100).unwrap(),
            ))));
        }
        let _ = send(
            &mut studio,
            serde_json::json!({"command":"registration_setup","fixed":1,"moving":2}),
        );
        let _ = studio.remove_clouds(vec![0]);
        assert_eq!(studio.registration.fixed, Some(0));
        assert_eq!(studio.registration.moving, Some(1));
        assert_eq!(studio.registration_value()["moving"], 1);
        let fixed = Arc::clone(&studio.clouds[0].cloud);
        studio.remove_header_layer(&fixed);
        assert_eq!(studio.registration.fixed, None);
        assert_eq!(studio.registration.moving, Some(0));
        assert!(studio.registration_value()["fixed"].is_null());
        let _ = studio.remove_clouds(vec![0]);
        assert_eq!(studio.registration.moving, None);
    }

    #[test]
    fn the_section_clips_moving_samples_after_the_initial_alignment() {
        let dir = tempfile::tempdir().unwrap();
        let mut studio = Studio::default();
        for (name, points) in [
            (
                "fixed.xyz",
                "100 200 300\n101 200 300\n100 201 300\n100 200 301\n102 202 302\n110 210 310\n",
            ),
            (
                "moving.xyz",
                "0 0 0\n1 0 0\n0 1 0\n0 0 1\n2 2 2\n10 10 10\n",
            ),
        ] {
            let path = dir.path().join(name);
            std::fs::write(&path, points).unwrap();
            let _ = studio.update(Message::Loaded(Ok(Arc::new(
                pointcloud_core::open(path, 100).unwrap(),
            ))));
        }
        let _ = send(
            &mut studio,
            serde_json::json!({"command":"registration_setup","fixed":0,"moving":1}),
        );
        let _ = send(
            &mut studio,
            serde_json::json!({"command":"registration_set_matrix","transform":{"rotation":[[1,0,0],[0,1,0],[0,0,1]],"translation":[100,200,300],"scale":1}}),
        );
        assert_eq!(
            send(
                &mut studio,
                serde_json::json!({"command":"set_section","min":[99.9,199.9,299.9],"max":[101.1,201.1,301.1]})
            )["ok"],
            true
        );
        let fixed = studio.registration_samples(0).unwrap();
        let moving = studio.registration_samples(1).unwrap();
        assert_eq!(fixed.len(), 4);
        assert_eq!(
            moving,
            [[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [0., 0., 1.]]
        );
        for (fixed, moving) in fixed.into_iter().zip(moving) {
            assert_eq!(studio.registration.transform.xyz(moving), fixed);
        }
    }
}
#[derive(Debug, Clone)]
pub enum Action {
    Open,
    Close,
    SetFixed,
    SetMoving,
    PickFixed,
    PickMoving,
    ClearPairs,
    Coarse,
    Refine,
    Cancel,
    Field(usize, String),
    Manual,
    Save(bool),
    SavePath(bool, Option<PathBuf>),
    Matrix,
    MatrixPath(Option<PathBuf>),
    Refined(Result<(Arc<IcpResult>, Vec<[f64; 3]>), String>),
    Written(Result<PathBuf, String>),
}

impl Studio {
    pub(crate) fn sync_registration_sources(&mut self) {
        for i in 0..2 {
            if let Some(path) = &self.registration.source_paths[i] {
                let index = self
                    .clouds
                    .iter()
                    .position(|entry| entry.cloud.path == *path);
                if i == 0 {
                    self.registration.fixed = index;
                } else {
                    self.registration.moving = index;
                }
            }
        }
    }

    fn remember_registration_sources(&mut self) {
        self.registration.snapshots = self
            .registration
            .fixed
            .zip(self.registration.moving)
            .and_then(|(fixed, moving)| {
                Some(vec![
                    (
                        Arc::clone(&self.clouds.get(fixed)?.cloud),
                        self.clouds[fixed].transform,
                    ),
                    (
                        Arc::clone(&self.clouds.get(moving)?.cloud),
                        self.clouds[moving].transform,
                    ),
                ])
            });
    }
    fn registration_sources_current(&self) -> bool {
        self.registration
            .snapshots
            .as_ref()
            .is_none_or(|snapshots| {
                snapshots.iter().all(|(cloud, transform)| {
                    cloud.validate_source().is_ok()
                        && self.clouds.iter().any(|entry| {
                            entry.matches_source(cloud) && entry.transform == *transform
                        })
                })
            })
    }
    pub(crate) fn registration_value(&self) -> serde_json::Value {
        serde_json::json!({"fixed":self.registration.fixed,"moving":self.registration.moving,
            "fixed_points":self.registration.fixed_points,"moving_points":self.registration.moving_points,
            "busy":self.registration.busy,"matrix":self.registration.transform.matrix(),
            "transform":self.registration.transform,"has_alignment":self.registration.has_alignment,"report":self.registration.report,"error":self.registration.error})
    }
    pub(crate) fn api_registration(
        &mut self,
        command: crate::native_api::ApiCommand,
    ) -> (serde_json::Value, Task<Message>) {
        use crate::native_api::ApiCommand;
        if self.registration.busy
            && !matches!(
                &command,
                ApiCommand::CancelRegistration | ApiCommand::RegistrationMatrix
            )
        {
            return (
                serde_json::json!({"ok":false,"error":"registration is running"}),
                Task::none(),
            );
        }
        let was_busy = self.registration.busy;
        let action = match command {
            ApiCommand::RegistrationSetup { fixed, moving } => {
                if fixed == moving || fixed >= self.clouds.len() || moving >= self.clouds.len() {
                    return (
                        serde_json::json!({"ok":false,"error":"choose two different open scan layers"}),
                        Task::none(),
                    );
                }
                self.registration.fixed = Some(fixed);
                self.registration.moving = Some(moving);
                self.registration.source_paths = [
                    Some(self.clouds[fixed].cloud.path.clone()),
                    Some(self.clouds[moving].cloud.path.clone()),
                ];
                self.registration.snapshots = None;
                self.registration.fixed_points.clear();
                self.registration.moving_points.clear();
                self.registration.transform = SceneTransform::default();
                self.registration.has_alignment = false;
                self.registration.report = None;
                Action::Open
            }
            ApiCommand::RegistrationPair { fixed, moving } => {
                if fixed.iter().chain(&moving).any(|v| !v.is_finite()) {
                    return (
                        serde_json::json!({"ok":false,"error":"point pairs must be finite"}),
                        Task::none(),
                    );
                }
                self.registration.fixed_points.push(fixed);
                self.registration.moving_points.push(moving);
                self.registration.has_alignment = false;
                self.registration.report = None;
                return (
                    serde_json::json!({"ok":true,"registration":self.registration_value()}),
                    Task::none(),
                );
            }
            ApiCommand::RegistrationFit => Action::Coarse,
            ApiCommand::RegistrationSetMatrix { transform } => {
                match transform.validate() {
                    Ok(transform) => {
                        self.registration.transform = transform;
                        self.registration.has_alignment = true;
                        self.registration.report = None;
                        self.remember_registration_sources();
                    }
                    Err(error) => {
                        return (
                            serde_json::json!({"ok":false,"error":error.to_string()}),
                            Task::none(),
                        )
                    }
                }
                return (
                    serde_json::json!({"ok":true,"registration":self.registration_value()}),
                    Task::none(),
                );
            }
            ApiCommand::RegistrationRefine => Action::Refine,
            ApiCommand::RegistrationExport {
                path,
                merge,
                keep_stations_photos,
            } => {
                self.merge_keep_photos = keep_stations_photos.unwrap_or(true);
                if !path.is_absolute()
                    || !path
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("e57"))
                {
                    return (
                        serde_json::json!({"ok":false,"error":"registration output needs an absolute .e57 path"}),
                        Task::none(),
                    );
                }
                Action::SavePath(merge, Some(path))
            }
            ApiCommand::RegistrationMatrix => {
                return (
                    serde_json::json!({"ok":true,"registration":self.registration_value()}),
                    Task::none(),
                )
            }
            ApiCommand::CancelRegistration => Action::Cancel,
            _ => unreachable!(),
        };
        if self.registration.busy && !matches!(action, Action::Cancel) {
            return (
                serde_json::json!({"ok":false,"error":"registration is running"}),
                Task::none(),
            );
        }
        self.registration.error = None;
        let task = self.registration_action(action);
        if let Some(error) = &self.registration.error {
            return (serde_json::json!({"ok":false,"error":error}), task);
        }
        let job = if self.registration.busy && !was_busy {
            let id = self
                .record_api_job(serde_json::json!({"state":"running","operation":"registration"}));
            self.registration.api_job = Some(id.clone());
            Some(id)
        } else {
            None
        };
        (
            serde_json::json!({"ok":true,"job_id":job,"registration":self.registration_value()}),
            task,
        )
    }
    fn end_registration_job(&mut self, result: Result<serde_json::Value, String>) {
        if let Some(id) = self.registration.api_job.take() {
            if let Some(job) = self.api_jobs.get_mut(&id) {
                *job = match result {
                    Ok(result) => serde_json::json!({"state":"complete","result":result}),
                    Err(error) if error == "Operation cancelled" => {
                        serde_json::json!({"state":"cancelled"})
                    }
                    Err(error) => serde_json::json!({"state":"failed","error":error}),
                };
            }
        }
    }
    fn registration_samples(&self, index: usize) -> Result<Vec<[f64; 3]>, String> {
        let entry = self
            .clouds
            .get(index)
            .ok_or("Registration layer is closed")?;
        if entry.cloud.provisional {
            return Err("Wait until the registration scan finishes loading".into());
        }
        entry.cloud.validate_source().map_err(|e| e.to_string())?;
        let section = self.section_box();
        let stride = entry.cloud.points.len().div_ceil(50_000).max(1);
        Ok(entry
            .cloud
            .points
            .iter()
            .zip(&entry.cloud.point_ordinals)
            .step_by(stride)
            .filter(|(_, ordinal)| {
                entry
                    .deleted
                    .as_ref()
                    .is_none_or(|mask| !mask.contains(**ordinal))
            })
            .map(|(p, _)| entry.transform.xyz(p.xyz))
            .filter(|p| {
                let aligned = if self.registration.moving == Some(index) {
                    self.registration.transform.xyz(*p)
                } else {
                    *p
                };
                section.is_none_or(|b| b.contains(aligned))
            })
            .collect())
    }
    pub(crate) fn registration_action(&mut self, action: Action) -> Task<Message> {
        if self.registration.busy
            && !matches!(
                &action,
                Action::Cancel | Action::Close | Action::Refined(_) | Action::Written(_)
            )
        {
            self.status = "Registration is running; cancel it before changing the inputs".into();
            return Task::none();
        }
        self.sync_registration_sources();
        match action {
            Action::Open => {
                self.registration.open = true;
                self.registration.fields = ["0".into(), "0".into(), "0".into(), "0".into()];
            }
            Action::Close => self.registration.open = false,
            Action::SetFixed => {
                self.registration.fixed = self.active;
                self.registration.source_paths[0] = self
                    .active
                    .and_then(|i| self.clouds.get(i))
                    .map(|e| e.cloud.path.clone());
                self.registration.snapshots = None;
                self.registration.has_alignment = false;
                self.registration.transform = SceneTransform::default();
                self.registration.report = None;
                self.registration.fixed_points.clear();
                self.registration.moving_points.clear();
            }
            Action::SetMoving => {
                self.registration.moving = self.active;
                self.registration.source_paths[1] = self
                    .active
                    .and_then(|i| self.clouds.get(i))
                    .map(|e| e.cloud.path.clone());
                self.registration.snapshots = None;
                self.registration.has_alignment = false;
                self.registration.transform = SceneTransform::default();
                self.registration.report = None;
                self.registration.fixed_points.clear();
                self.registration.moving_points.clear();
            }
            Action::Cancel => self.registration.cancel.store(true, Ordering::Relaxed),
            Action::Field(i, v) => {
                if let Some(field) = self.registration.fields.get_mut(i) {
                    *field = v;
                }
            }
            Action::ClearPairs => {
                self.registration.fixed_points.clear();
                self.registration.moving_points.clear();
                self.registration.report = None;
                self.registration.has_alignment = false;
                self.registration.transform = SceneTransform::default();
            }
            Action::PickFixed | Action::PickMoving => {
                let fixed = matches!(action, Action::PickFixed);
                let index = if fixed {
                    self.registration.fixed
                } else {
                    self.registration.moving
                };
                let result = index.and_then(|i| self.clouds.get(i)).and_then(|entry| {
                    let selection = entry.selection.as_ref().filter(|s| s.count == 1)?;
                    let p = selection.highlights.first()?.xyz;
                    Some(if selection.highlights_source {
                        entry.transform.xyz(p)
                    } else {
                        p
                    })
                });
                if let Some(point) = result {
                    self.registration.has_alignment = false;
                    self.registration.report = None;
                    if fixed {
                        self.registration.fixed_points.push(point);
                    } else {
                        self.registration.moving_points.push(point);
                    }
                } else {
                    self.registration.error =
                        Some("Pick one point in the selected registration scan first".into());
                }
            }
            Action::Coarse => {
                self.remember_registration_sources();
                if self.registration.fixed_points.len() != self.registration.moving_points.len() {
                    self.registration.error =
                        Some("Choose the same number of reference and moving points".into());
                    return Task::none();
                }
                let pairs: Vec<_> = self
                    .registration
                    .moving_points
                    .iter()
                    .copied()
                    .zip(self.registration.fixed_points.iter().copied())
                    .collect();
                match registration::register_correspondences(&pairs) {
                    Ok(result) => {
                        self.registration.has_alignment = true;
                        self.registration.transform = SceneTransform {
                            rotation: result.rotation,
                            translation: result.translation,
                            scale: 1.,
                        };
                        self.registration.report = serde_json::to_value(result).ok();
                        self.registration.error = None;
                    }
                    Err(error) => self.registration.error = Some(error.to_string()),
                }
            }
            Action::Manual => {
                self.remember_registration_sources();
                let values: Result<Vec<f64>, _> = self
                    .registration
                    .fields
                    .iter()
                    .map(|v| v.parse::<f64>())
                    .collect();
                match values {
                    Ok(v) if v.iter().all(|v| v.is_finite()) => {
                        self.registration.has_alignment = true;
                        let (s, c) = v[3].to_radians().sin_cos();
                        self.registration.transform = SceneTransform {
                            rotation: [[c, -s, 0.], [s, c, 0.], [0., 0., 1.]],
                            translation: [v[0], v[1], v[2]],
                            scale: 1.,
                        };
                        self.registration.report = Some(
                            serde_json::json!({"matrix":self.registration.transform.matrix()}),
                        );
                    }
                    _ => {
                        self.registration.error =
                            Some("Enter finite translation and rotation values".into())
                    }
                }
            }
            Action::Refine => {
                if self.registration.busy {
                    return Task::none();
                }
                let result = (|| {
                    let fixed = self.registration.fixed.ok_or("Choose a reference scan")?;
                    let moving = self.registration.moving.ok_or("Choose a moving scan")?;
                    if fixed == moving {
                        return Err("Choose two different scans".into());
                    }
                    Ok((
                        self.registration_samples(fixed)?,
                        self.registration_samples(moving)?,
                    ))
                })();
                let (fixed, moving) = match result {
                    Ok(pair) => pair,
                    Err(error) => {
                        self.registration.error = Some(error);
                        return Task::none();
                    }
                };
                self.remember_registration_sources();
                self.registration.busy = true;
                self.registration.error = None;
                self.registration.cancel = Arc::new(AtomicBool::new(false));
                let cancel = Arc::clone(&self.registration.cancel);
                let initial = self.registration.transform;
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            let result = registration::refine_point_to_plane(
                                &fixed,
                                &moving,
                                initial,
                                IcpOptions::default(),
                                &mut |_, _| {
                                    if cancel.load(Ordering::Relaxed) {
                                        Err(pointcloud_core::LoadError::Cancelled)
                                    } else {
                                        Ok(())
                                    }
                                },
                            )
                            .map_err(|e| e.to_string())?;
                            Ok((Arc::new(result), moving))
                        })
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|r| r)
                    },
                    |r| Message::Registration(Action::Refined(r)),
                );
            }
            Action::Refined(result) => {
                self.registration.busy = false;
                if !self.registration_sources_current() {
                    let error="Registration result discarded because an input scan or its transform changed".to_string();
                    self.end_registration_job(Err(error.clone()));
                    self.registration.error = Some(error);
                    return Task::none();
                }
                self.end_registration_job(result.as_ref().map(|(r,_)|serde_json::json!({"transform":r.transform,"rms":r.rms,"p95":r.p95,"matched":r.matched})).map_err(Clone::clone));
                match result {
                    Err(error) => self.registration.error = Some(error),
                    Ok((report, moving)) => {
                        self.registration.has_alignment = true;
                        self.registration.transform = report.transform;
                        self.registration.report =
                            serde_json::to_value(report.as_ref()).ok().map(|mut value| {
                                value.as_object_mut().unwrap().remove("deviations");
                                value
                            });
                        self.status = format!(
                            "Registration RMS {:.2} mm · 95% {:.2} mm · {} matches",
                            report.rms * 1000.,
                            report.p95 * 1000.,
                            report.matched
                        );
                        let preview = (|| -> Result<_, String> {
                            use std::io::Write;
                            let directory =
                                Arc::new(tempfile::tempdir().map_err(|e| e.to_string())?);
                            let path = directory.path().join("registration-deviation.ply");
                            let mut file = std::io::BufWriter::new(
                                std::fs::File::create(&path).map_err(|e| e.to_string())?,
                            );
                            writeln!(file,"ply\nformat ascii 1.0\nelement vertex {}\nproperty double x\nproperty double y\nproperty double z\nproperty uchar red\nproperty uchar green\nproperty uchar blue\nend_header",moving.len()).map_err(|e|e.to_string())?;
                            for (point, deviation) in moving.iter().zip(&report.deviations) {
                                let p = report.transform.xyz(*point);
                                let amount = deviation
                                    .map(|v| (v.abs() / report.p95.max(0.001)).clamp(0., 1.));
                                let rgb = amount
                                    .map(|v| [(255. * v) as u8, (255. * (1. - v)) as u8, 0])
                                    .unwrap_or([128; 3]);
                                writeln!(
                                    file,
                                    "{} {} {} {} {} {}",
                                    p[0], p[1], p[2], rgb[0], rgb[1], rgb[2]
                                )
                                .map_err(|e| e.to_string())?;
                            }
                            file.flush().map_err(|e| e.to_string())?;
                            Ok((directory, path))
                        })();
                        if let Ok((directory, path)) = preview {
                            if let Some(previous) = &self.registration.preview {
                                let root = previous.path();
                                self.clouds.retain(|e| !e.cloud.path.starts_with(root));
                            }
                            self.registration.preview = Some(directory);
                            if let Some(entry) = self.clouds.iter_mut().find(|entry| {
                                Some(&entry.cloud.path)
                                    == self.registration.source_paths[1].as_ref()
                            }) {
                                entry.visible = false;
                            }
                            self.color_mode = crate::ColorMode::Rgb;
                            return self.open_paths(vec![path]);
                        }
                    }
                }
            }
            Action::Save(merge) => {
                return Task::perform(
                    async move {
                        crate::managed::file_dialog()
                            .add_filter("E57 scene", &["e57"])
                            .set_file_name(if merge {
                                "registered-scene.e57"
                            } else {
                                "registered-scan.e57"
                            })
                            .save_file()
                            .await
                            .map(|f| f.path().to_path_buf())
                    },
                    move |path| Message::Registration(Action::SavePath(merge, path)),
                )
            }
            Action::SavePath(merge, Some(path)) => {
                if !self.registration.has_alignment {
                    self.registration.error = Some(
                        "Fit point pairs, refine the overlap or apply a matrix before exporting"
                            .into(),
                    );
                    return Task::none();
                }
                if !self.registration_sources_current() {
                    self.registration.error =
                        Some("A registration input changed; calculate the alignment again".into());
                    return Task::none();
                }
                let sources = (|| -> Result<Vec<_>, String> {
                    let moving = self.registration.moving.ok_or("Choose a moving scan")?;
                    let fixed = self.registration.fixed.ok_or("Choose a reference scan")?;
                    if moving == fixed {
                        return Err("Choose two different scans".into());
                    }
                    let ids = if merge {
                        vec![fixed, moving]
                    } else {
                        vec![moving]
                    };
                    ids.into_iter()
                        .map(|index| {
                            let entry = self
                                .clouds
                                .get(index)
                                .ok_or("Registration layer is closed")?;
                            let scale = entry.transform.scale[0];
                            if scale <= 0. || !entry.transform.scale.iter().all(|v| *v == scale) {
                                return Err(
                                    "Registration export needs a positive uniform source scale"
                                        .into(),
                                );
                            }
                            let source = SceneTransform {
                                scale,
                                translation: entry.transform.offset,
                                ..Default::default()
                            };
                            let transform = if index == moving {
                                source.then(self.registration.transform)
                            } else {
                                source
                            };
                            Ok((Arc::clone(&entry.cloud), entry.deleted.clone(), transform))
                        })
                        .collect()
                })();
                let sources = match sources {
                    Ok(s) => s,
                    Err(e) => {
                        self.registration.error = Some(e);
                        return Task::none();
                    }
                };
                self.registration.busy = true;
                self.registration.cancel = Arc::new(AtomicBool::new(false));
                let cancel = Arc::clone(&self.registration.cancel);
                let keep_photos = self.merge_keep_photos;
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            let refs: Vec<_> = sources
                                .iter()
                                .map(|(cloud, _, transform)| {
                                    pointcloud_core::scene_export::SceneSource {
                                        cloud,
                                        transform: *transform,
                                    }
                                })
                                .collect();
                            pointcloud_core::scene_export::write_e57_scene(
                                &refs,
                                &path,
                                keep_photos,
                                &mut |source, ordinal, _| {
                                    sources[source]
                                        .1
                                        .as_ref()
                                        .is_none_or(|m| !m.contains(ordinal))
                                },
                                &mut |_, _| {
                                    if cancel.load(Ordering::Relaxed) {
                                        Err(pointcloud_core::LoadError::Cancelled)
                                    } else {
                                        Ok(())
                                    }
                                },
                            )
                            .map_err(|e| e.to_string())?;
                            let matrix = path.with_extension("registration.json");
                            std::fs::write(
                                matrix,
                                serde_json::to_vec_pretty(&sources.last().unwrap().2)
                                    .map_err(|e| e.to_string())?,
                            )
                            .map_err(|e| e.to_string())?;
                            Ok(path)
                        })
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|r| r)
                    },
                    |r| Message::Registration(Action::Written(r)),
                );
            }
            Action::Written(result) => {
                self.registration.busy = false;
                self.end_registration_job(
                    result
                        .as_ref()
                        .map(|p| serde_json::json!({"path":p}))
                        .map_err(Clone::clone),
                );
                match result {
                    Ok(path) => return self.open_paths(vec![path]),
                    Err(e) => self.registration.error = Some(e),
                }
            }
            Action::Matrix => {
                return Task::perform(
                    async {
                        crate::managed::file_dialog()
                            .add_filter("Registration matrix", &["json"])
                            .set_file_name("registration.json")
                            .save_file()
                            .await
                            .map(|f| f.path().to_path_buf())
                    },
                    |p| Message::Registration(Action::MatrixPath(p)),
                )
            }
            Action::MatrixPath(Some(path)) => {
                let value = serde_json::json!({"transform":self.registration.transform,"matrix":self.registration.transform.matrix(),"report":self.registration.report});
                if let Err(e) = std::fs::write(path, serde_json::to_vec_pretty(&value).unwrap()) {
                    self.registration.error = Some(e.to_string());
                }
            }
            Action::SavePath(_, None) | Action::MatrixPath(None) => {}
        }
        self.schedule_detail()
    }
    pub(crate) fn registration_properties(&self) -> Element<'_, Message> {
        let control = |label: &'static str, action| {
            button(text(i18n::tr(label)).size(11))
                .on_press(Message::Registration(action))
                .style(ui_style::tool)
        };
        let mut panel = column![crate::opencad_properties::section_header(
            "Scan registration"
        )];
        if !self.registration.open {
            return container(panel.push(control(i18n::key("Align scans…"), Action::Open)))
                .padding([4, 8])
                .into();
        }
        let name = |index: Option<usize>| {
            index
                .and_then(|i| self.clouds.get(i))
                .map(|e| crate::display_name(&e.cloud.path))
                .unwrap_or("—")
        };
        panel = panel
            .push(row![
                control(i18n::key("Use active as reference"), Action::SetFixed),
                control(i18n::key("Close"), Action::Close)
            ])
            .push(text(name(self.registration.fixed)).size(11))
            .push(control(
                i18n::key("Use active as moving"),
                Action::SetMoving,
            ))
            .push(text(name(self.registration.moving)).size(11))
            .push(control(i18n::key("Add picked reference"), Action::PickFixed).width(Fill))
            .push(control(i18n::key("Add picked moving"), Action::PickMoving).width(Fill));
        let count = self
            .registration
            .fixed_points
            .len()
            .max(self.registration.moving_points.len());
        for i in 0..count {
            panel = panel.push(
                text(format!(
                    "{}  {} / {}",
                    i + 1,
                    if self.registration.fixed_points.get(i).is_some() {
                        "✓"
                    } else {
                        "—"
                    },
                    if self.registration.moving_points.get(i).is_some() {
                        "✓"
                    } else {
                        "—"
                    }
                ))
                .size(10),
            );
        }
        panel = panel
            .push(control(i18n::key("Fit point pairs"), Action::Coarse).width(Fill))
            .push(control(i18n::key("Clear pairs"), Action::ClearPairs).width(Fill));
        for (i, label) in [
            i18n::key("Move X"),
            i18n::key("Move Y"),
            i18n::key("Move Z"),
            i18n::key("Turn Z (°)"),
        ]
        .into_iter()
        .enumerate()
        {
            panel = panel.push(
                row![
                    text(i18n::tr(label)).size(11).width(90),
                    ui_style::text_input("0", &self.registration.fields[i])
                        .size(11)
                        .on_input(move |v| Message::Registration(Action::Field(i, v)))
                ]
                .spacing(3),
            );
        }
        panel = panel.push(control(i18n::key("Apply manual alignment"), Action::Manual));
        if self.registration.busy {
            panel = panel.push(control(i18n::key("Cancel registration"), Action::Cancel));
        } else {
            panel = panel.push(control(i18n::key("Refine overlap (ICP)"), Action::Refine));
        }
        if let Some(report) = &self.registration.report {
            let rms = report
                .get("rms")
                .or_else(|| report.get("rms_error"))
                .and_then(|v| v.as_f64());
            let p95 = report
                .get("p95")
                .or_else(|| report.get("p95_error"))
                .and_then(|v| v.as_f64());
            if let Some(rms) = rms {
                panel = panel.push(crate::opencad_properties::property_row(
                    "RMS (mm)",
                    format!("{:.3}", rms * 1000.),
                ));
            }
            if let Some(p95) = p95 {
                panel = panel.push(crate::opencad_properties::property_row(
                    "95% (mm)",
                    format!("{:.3}", p95 * 1000.),
                ));
            }
            if let Some(residuals) = report["residuals"].as_array() {
                for (i, value) in residuals.iter().take(32).enumerate() {
                    if let Some(value) = value.as_f64() {
                        panel = panel
                            .push(text(format!("{}: {:.3} mm", i + 1, value * 1000.)).size(10));
                    }
                }
            }
        }
        for row in self.registration.transform.matrix() {
            panel = panel.push(
                text(format!(
                    "{:.5} {:.5} {:.5} {:.5}",
                    row[0], row[1], row[2], row[3]
                ))
                .size(10),
            );
        }
        panel = panel.push(
            ui_style::checkbox(
                i18n::tr("Keep stations and photos in E57"),
                self.merge_keep_photos,
            )
            .on_toggle(Message::MergeKeepPhotos)
            .size(13)
            .text_size(11),
        );
        panel = panel
            .push(control(
                i18n::key("Save registration matrix…"),
                Action::Matrix,
            ))
            .push(control(
                i18n::key("Export registered scan…"),
                Action::Save(false),
            ))
            .push(control(
                i18n::key("Export registered scene…"),
                Action::Save(true),
            ));
        if let Some(error) = &self.registration.error {
            panel = panel.push(text(error).size(11));
        }
        container(panel.spacing(4))
            .padding([4, 8])
            .width(Fill)
            .into()
    }
}
