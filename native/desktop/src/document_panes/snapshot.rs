//! A document's camera/render state. Selection, sources, measures and section
//! geometry remain shared through Studio; a pane only owns how it looks.
#[cfg(test)]
use super::store;
use crate::drawing_view::DrawingViewTool;
use crate::file_photos::PhotoTool;
use crate::station_photos::{PhotoSet, WalkView};
use crate::view_tabs::TabId;
use crate::{ColorMode, PointViewport, Studio};
use iced::{Rectangle, Size};
use std::cell::Cell;
use std::path::PathBuf;
use std::sync::Arc;

pub(crate) struct Snapshot {
    pub tab: TabId,
    pub load_error: Option<String>,
    yaw: f32,
    pitch: f32,
    zoom: f32,
    pan: [f32; 2],
    orbit_point: Option<[f64; 3]>,
    walk: Option<WalkView>,
    active_station: Option<(PathBuf, usize)>,
    last_station: Option<usize>,
    station_tab: Option<PathBuf>,
    panorama: Option<Arc<PhotoSet>>,
    label: &'static str,
    colour: ColorMode,
    reference_size: Size,
    pub bounds: Cell<Option<Rectangle>>,
    pub pointer: Cell<Option<[f32; 2]>>,
    pub drawing: Option<DrawingViewTool>,
    /// None while this document's PhotoTool is owned by the active Studio.
    pub photos: Option<PhotoTool>,
}
impl Snapshot {
    pub fn capture(tab: TabId, studio: &Studio) -> Self {
        let drawing = matches!(tab, TabId::Drawing(_) | TabId::File(_) | TabId::Layout(_))
            .then(|| studio.drawing_view.render_copy());
        let mut snapshot = Self {
            tab,
            load_error: None,
            yaw: 0.,
            pitch: 0.,
            zoom: 1.,
            pan: [0.; 2],
            orbit_point: None,
            walk: None,
            active_station: None,
            last_station: None,
            station_tab: None,
            panorama: None,
            label: studio.view_label,
            colour: studio.color_mode,
            reference_size: studio.drawn_viewport(),
            bounds: Cell::new(None),
            pointer: Cell::new(None),
            drawing,
            photos: None,
        };
        snapshot.capture_camera(studio);
        snapshot
    }
    #[allow(clippy::too_many_arguments)]
    pub fn set_model_camera(
        &mut self,
        yaw: f32,
        pitch: f32,
        zoom: f32,
        pan: [f32; 2],
        orbit: Option<[f64; 3]>,
        walk: Option<WalkView>,
        label: &'static str,
        colour: ColorMode,
    ) {
        self.yaw = yaw;
        self.pitch = pitch;
        self.zoom = zoom;
        self.pan = pan;
        self.orbit_point = orbit;
        self.walk = walk;
        self.label = label;
        self.colour = colour;
        self.active_station = None;
        self.station_tab = None;
        self.panorama = None;
    }
    pub fn panorama_ready(&self) -> bool {
        self.panorama.is_some()
    }
    pub fn accept_panorama(
        &mut self,
        source: &std::path::Path,
        station: usize,
        result: &Result<Arc<PhotoSet>, String>,
    ) -> bool {
        if self
            .active_station
            .as_ref()
            .is_none_or(|(path, current)| path != source || *current != station)
        {
            return false;
        }
        match result {
            Ok(set) if set.source == source && set.station == station => {
                self.panorama = Some(set.clone());
                self.load_error = None;
            }
            Ok(_) => {}
            Err(error) => self.load_error = Some(format!("Station photo failed: {error}")),
        }
        true
    }
    pub fn capture_camera(&mut self, studio: &Studio) {
        self.yaw = studio.yaw;
        self.pitch = studio.pitch;
        self.zoom = studio.zoom;
        self.pan = studio.pan;
        self.orbit_point = studio.orbit_point;
        self.walk = studio.walk;
        self.active_station = studio.walk_station.and_then(|(cloud, station)| {
            studio
                .clouds
                .get(cloud)
                .map(|entry| (entry.cloud.path.clone(), station))
        });
        self.last_station = self
            .active_station
            .as_ref()
            .map(|(_, station)| *station)
            .or_else(|| match &self.tab {
                TabId::StationPhoto(source) => studio
                    .tabs
                    .station_looks
                    .get(source)
                    .map(|(station, _)| *station),
                _ => None,
            });
        self.station_tab = studio.tabs.active_station_photo.clone();
        self.panorama = studio.panorama_photos.clone();
        self.label = studio.view_label;
        self.colour = studio.color_mode;
        self.reference_size = studio.viewport_size;
        if let Some(drawing) = &mut self.drawing {
            drawing.copy_render_from(&studio.drawing_view);
        }
    }
    fn station(&self, studio: &Studio) -> Option<(usize, usize)> {
        let (source, station) = self.active_station.as_ref()?;
        studio
            .clouds
            .iter()
            .position(|entry| {
                entry.cloud.path == *source && *station < entry.cloud.scan_poses.len()
            })
            .map(|cloud| (cloud, *station))
    }
    fn size(&self) -> Size {
        self.bounds
            .get()
            .map(|bounds| bounds.size())
            .filter(|size| size.width > 0. && size.height > 0.)
            .unwrap_or(self.reference_size)
    }
    fn pan_at_size(&self, size: Size) -> [f32; 2] {
        let reference = self
            .reference_size
            .width
            .min(self.reference_size.height)
            .max(1.);
        let ratio = size.width.min(size.height).max(1.) / reference;
        self.pan.map(|value| value * ratio)
    }
    pub fn restore_camera(&self, studio: &mut Studio) {
        let size = self.size();
        studio.yaw = self.yaw;
        studio.pitch = self.pitch;
        studio.zoom = self.zoom;
        studio.pan = self.pan_at_size(size);
        studio.orbit_point = self.orbit_point;
        studio.walk = self.walk;
        studio.walk_station = self.station(studio);
        studio.panorama_photos = self.panorama.clone();
        studio.tabs.active_station_photo = self.station_tab.clone();
        studio.view_label = self.label;
        studio.color_mode = self.colour;
        studio.viewport_size = size;
        studio.viewport_pointer.set(self.pointer.get());
        studio.walk_keys = [false; 6];
        studio.walk_tick = None;
        studio.drawing_view.shown = matches!(
            self.tab,
            TabId::Drawing(_) | TabId::File(_) | TabId::Layout(_)
        );
        studio.file_open = false;
        if let Some(drawing) = &self.drawing {
            studio.drawing_view.copy_render_from(drawing);
        }
    }
    pub fn viewport<'a>(&'a self, studio: &'a Studio, slot: usize, live: u8) -> PointViewport<'a> {
        let mut view = studio.point_viewport();
        let size = self.size();
        view.yaw = self.yaw;
        view.pitch = self.pitch;
        view.zoom = self.zoom;
        view.pan = self.pan_at_size(size);
        view.orbit_point = self.orbit_point;
        view.walk = self.walk;
        view.walk_station = self.station(studio);
        view.panorama_photos = self.panorama.as_ref();
        view.color_mode = self.colour;
        view.viewport_size = size;
        view.pointer = &self.pointer;
        view.annotate = studio.document_views_overlay(
            &self.tab,
            &self.bounds,
            studio.shown_tab().as_ref() == Some(&self.tab),
        );
        view.photo_visible = matches!(self.tab, TabId::Photo(_));
        if let Some(photos) = &self.photos {
            view.photos = photos;
        } else if studio.shown_tab().as_ref() != Some(&self.tab) {
            view.photo_visible = false;
        }
        view.budget = (view.budget / live.count_ones().max(1) as usize).max(1);
        view.view_slot = slot;
        view.live_views = live;
        view.resident_points = studio.shown_tab().as_ref() != Some(&self.tab);
        view
    }
    #[cfg(test)]
    pub fn stored_look(&self, active_photos: &PhotoTool) -> Option<store::Look> {
        let walking = |walk: WalkView| store::Walking {
            eye: walk.eye,
            yaw: walk.yaw,
            pitch: walk.pitch,
            field_of_view: walk.field_of_view,
        };
        Some(match &self.tab {
            TabId::Model | TabId::View(_) => store::Look::Scene {
                camera: store::Camera {
                    yaw: self.yaw,
                    pitch: self.pitch,
                    zoom: self.zoom,
                    pan: self.pan,
                    orbit_point: self.orbit_point,
                    walk: self.walk.map(walking),
                },
            },
            TabId::Photo(source) => {
                let photos = self.photos.as_ref().unwrap_or(active_photos);
                let view = photos.view.as_ref().filter(|view| view.source == *source)?;
                store::Look::Photo {
                    index: view.index,
                    camera: walking(self.walk?),
                    pinned: view.pinned,
                    zoom: view.zoom,
                    blend: photos.blend,
                }
            }
            TabId::StationPhoto(_) => store::Look::Station {
                station: self.last_station?,
                camera: walking(self.walk?),
            },
            TabId::Drawing(_) | TabId::File(_) | TabId::Layout(_) => {
                let camera = self.drawing.as_ref()?.camera();
                store::Look::Drawing {
                    center: camera.center,
                    scale: camera.scale,
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn document_cameras_are_independent_and_leave_shared_section_untouched() {
        let mut studio = Studio {
            yaw: 0.2,
            pitch: 0.3,
            pan: [12., -8.],
            ..Studio::default()
        };
        let first = Snapshot::capture(TabId::Model, &studio);
        studio.yaw = 1.2;
        studio.pitch = -0.4;
        let second = Snapshot::capture(TabId::View("other".into()), &studio);
        studio.section_enabled = true;
        studio.section_min_percent = [10., 20., 30.];
        let a = first.viewport(&studio, 0, 3);
        let b = second.viewport(&studio, 1, 3);
        assert_ne!(a.yaw, b.yaw);
        assert_eq!((a.view_slot, b.view_slot), (0, 1));
        assert_eq!(a.section, b.section);
        first.restore_camera(&mut studio);
        assert_eq!(studio.yaw, 0.2);
        assert!(studio.section_enabled);
        assert_eq!(studio.section_min_percent, [10., 20., 30.]);
        assert!(matches!(
            first.stored_look(&studio.photos),
            Some(store::Look::Scene { .. })
        ));
    }
    #[test]
    fn walking_out_keeps_the_selected_station_without_rendering_its_panorama() {
        let mut studio = Studio::default();
        let source = PathBuf::from("/source.e57");
        let walk = WalkView::new([100., 0., 1.6], 0.8);
        studio.walk = Some(walk);
        studio.tabs.station_looks.insert(source.clone(), (7, walk));
        let snapshot = Snapshot::capture(TabId::StationPhoto(source), &studio);
        assert!(snapshot.viewport(&studio, 2, 4).walk_station.is_none());
        assert!(matches!(
            snapshot.stored_look(&studio.photos),
            Some(store::Look::Station { station: 7, .. })
        ));
    }
}
