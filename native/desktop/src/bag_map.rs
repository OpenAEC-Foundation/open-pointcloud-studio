//! Native RD New map for selecting 3DBAG download areas. Raster background
//! tiles come from Kadaster's PDOK BRT-A WMTS (EPSG:28992), so no webview or
//! browser coordinate conversion is involved.

use std::io::Read;
use std::time::Duration;

use std::collections::HashMap;

use ::image::{Rgba, RgbaImage};
use iced::mouse;
use iced::widget::canvas::{self, event, Frame, Geometry};
use iced::widget::image::Handle;
use iced::{Color, Point, Rectangle, Renderer, Size, Theme};
use pointcloud_core::BagBounds;

use crate::Message;

pub const WIDTH: f32 = 416.0;
pub const HEIGHT: f32 = 300.0;
const ORIGIN_X: f64 = -285_401.92;
const ORIGIN_Y: f64 = 903_401.92;
const BASE_RESOLUTION: f64 = 3_440.64;
const TILE_PIXELS: f64 = 256.0;
const MAX_TILE_BYTES: u64 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TileKey {
    pub zoom: u8,
    pub col: u32,
    pub row: u32,
}

pub type TileBatch = Vec<(TileKey, Result<Vec<u8>, String>)>;

impl TileKey {
    pub fn url(self) -> String {
        format!(
            "https://service.pdok.nl/kadaster/brt-achtergrondkaart/wmts/v2_0/grijs/EPSG:28992/{:02}/{}/{}.png",
            self.zoom, self.col, self.row
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MapView {
    pub center: [f64; 2],
    pub zoom: u8,
    pub width: f32,
    pub height: f32,
}

impl MapView {
    pub fn resolution(self) -> f64 {
        BASE_RESOLUTION / f64::from(1u32 << self.zoom)
    }

    pub fn screen_to_rd(self, point: Point) -> [f64; 2] {
        let resolution = self.resolution();
        [
            self.center[0] + f64::from(point.x - self.width * 0.5) * resolution,
            self.center[1] - f64::from(point.y - self.height * 0.5) * resolution,
        ]
    }

    pub fn rd_to_screen(self, rd: [f64; 2]) -> Point {
        let resolution = self.resolution();
        Point::new(
            (self.width as f64 * 0.5 + (rd[0] - self.center[0]) / resolution) as f32,
            (self.height as f64 * 0.5 - (rd[1] - self.center[1]) / resolution) as f32,
        )
    }

    pub fn pan(&mut self, delta: [f32; 2]) {
        let resolution = self.resolution();
        self.center[0] -= f64::from(delta[0]) * resolution;
        self.center[1] += f64::from(delta[1]) * resolution;
    }

    pub fn zoom_at(&mut self, step: i8, point: Point) {
        let rd = self.screen_to_rd(point);
        self.zoom = (i16::from(self.zoom) + i16::from(step)).clamp(8, 14) as u8;
        let resolution = self.resolution();
        self.center = [
            rd[0] - f64::from(point.x - self.width * 0.5) * resolution,
            rd[1] + f64::from(point.y - self.height * 0.5) * resolution,
        ];
    }

    pub fn fit(&mut self, bounds: BagBounds) {
        self.center = [
            (bounds.min_x + bounds.max_x) * 0.5,
            (bounds.min_y + bounds.max_y) * 0.5,
        ];
        let width = (bounds.max_x - bounds.min_x).max(1.0);
        let height = (bounds.max_y - bounds.min_y).max(1.0);
        self.zoom = (8..=14)
            .rev()
            .find(|&zoom| {
                let resolution = BASE_RESOLUTION / f64::from(1u32 << zoom);
                width <= f64::from(self.width) * resolution * 0.76
                    && height <= f64::from(self.height) * resolution * 0.76
            })
            .unwrap_or(8);
    }

    fn tile_position(self, key: TileKey) -> Point {
        let span = TILE_PIXELS * self.resolution();
        self.rd_to_screen([
            ORIGIN_X + f64::from(key.col) * span,
            ORIGIN_Y - f64::from(key.row) * span,
        ])
    }

    pub fn visible_tiles(self) -> Vec<TileKey> {
        let span = TILE_PIXELS * self.resolution();
        let top_left = self.screen_to_rd(Point::ORIGIN);
        let bottom_right = self.screen_to_rd(Point::new(self.width, self.height));
        let col_start = ((top_left[0] - ORIGIN_X) / span).floor() as i64;
        let col_end = ((bottom_right[0] - ORIGIN_X) / span).floor() as i64;
        let row_start = ((ORIGIN_Y - top_left[1]) / span).floor() as i64;
        let row_end = ((ORIGIN_Y - bottom_right[1]) / span).floor() as i64;
        let limit = 1i64 << self.zoom;
        let mut keys = Vec::new();
        for row in row_start.max(0)..=row_end.min(limit - 1) {
            for col in col_start.max(0)..=col_end.min(limit - 1) {
                keys.push(TileKey {
                    zoom: self.zoom,
                    col: col as u32,
                    row: row as u32,
                });
            }
        }
        keys
    }

    fn rectangle(self, a: Point, b: Point) -> BagBounds {
        let a = self.screen_to_rd(a);
        let b = self.screen_to_rd(b);
        BagBounds {
            min_x: a[0].min(b[0]),
            min_y: a[1].min(b[1]),
            max_x: a[0].max(b[0]),
            max_y: a[1].max(b[1]),
        }
    }
}

/// The map service is told the same name and version as the building
/// service, so both see which release asks.
const TILE_USER_AGENT: &str = pointcloud_core::BAG3D_USER_AGENT;

pub fn fetch_tiles(keys: Vec<TileKey>) -> TileBatch {
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(12))
        .user_agent(TILE_USER_AGENT)
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return keys
                .into_iter()
                .map(|key| (key, Err(error.to_string())))
                .collect();
        }
    };
    keys.into_iter()
        .map(|key| {
            let result = (|| -> Result<Vec<u8>, String> {
                let response = client
                    .get(key.url())
                    .send()
                    .and_then(reqwest::blocking::Response::error_for_status)
                    .map_err(|error| error.to_string())?;
                if response
                    .content_length()
                    .is_some_and(|size| size > MAX_TILE_BYTES)
                {
                    return Err("PDOK tile exceeded size limit".into());
                }
                let mut bytes = Vec::new();
                response
                    .take(MAX_TILE_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|error| error.to_string())?;
                if bytes.len() as u64 > MAX_TILE_BYTES
                    || bytes.get(..8) != Some(b"\x89PNG\r\n\x1a\n")
                {
                    return Err("PDOK tile was not a bounded PNG".into());
                }
                Ok(bytes)
            })();
            (key, result)
        })
        .collect()
}

pub fn compose_raster(view: MapView, tiles: &HashMap<TileKey, RgbaImage>) -> Handle {
    let width = view.width as u32;
    let height = view.height as u32;
    let mut output = RgbaImage::from_pixel(width, height, Rgba([218, 218, 213, 255]));
    for key in view.visible_tiles() {
        if let Some(tile) = tiles.get(&key) {
            let position = view.tile_position(key);
            ::image::imageops::overlay(
                &mut output,
                tile,
                position.x.round() as i64,
                position.y.round() as i64,
            );
        }
    }
    Handle::from_rgba(width, height, output.into_raw())
}

#[derive(Debug, Clone, Copy)]
pub struct Drag {
    start: Point,
    current: Point,
    drawing: bool,
}

pub struct BagMap {
    pub center: [f64; 2],
    pub zoom: u8,
    pub drawing: bool,
    pub selected: Option<BagBounds>,
}

impl BagMap {
    fn view(&self, bounds: Rectangle) -> MapView {
        MapView {
            center: self.center,
            zoom: self.zoom,
            width: bounds.width,
            height: bounds.height,
        }
    }
}

impl canvas::Program<Message> for BagMap {
    type State = Option<Drag>;

    fn update(
        &self,
        state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (event::Status, Option<Message>) {
        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                *state = cursor.position_in(bounds).map(|point| Drag {
                    start: point,
                    current: point,
                    drawing: self.drawing,
                });
                (event::Status::Captured, None)
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if let (Some(drag), Some(point)) = (state.as_mut(), cursor.position_in(bounds)) {
                    let delta = [point.x - drag.current.x, point.y - drag.current.y];
                    drag.current = point;
                    let message = (!drag.drawing).then_some(Message::BagMapPan(delta));
                    (event::Status::Captured, message)
                } else {
                    (event::Status::Ignored, None)
                }
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let message = state.take().and_then(|drag| {
                    if !drag.drawing
                        || (drag.start.x - drag.current.x).hypot(drag.start.y - drag.current.y)
                            < 5.0
                    {
                        return None;
                    }
                    Some(Message::BagMapSelected(
                        self.view(bounds).rectangle(drag.start, drag.current),
                    ))
                });
                (event::Status::Captured, message)
            }
            canvas::Event::Mouse(mouse::Event::WheelScrolled { delta })
                if cursor.is_over(bounds) =>
            {
                let amount = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => y,
                    mouse::ScrollDelta::Pixels { y, .. } => y / 40.0,
                };
                let point = cursor
                    .position_in(bounds)
                    .unwrap_or(Point::new(bounds.width * 0.5, bounds.height * 0.5));
                (
                    event::Status::Captured,
                    Some(Message::BagMapZoom(amount, point)),
                )
            }
            _ => (event::Status::Ignored, None),
        }
    }

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let view = self.view(bounds);
        frame.with_clip(Rectangle::new(Point::ORIGIN, bounds.size()), |frame| {
            let selection = state
                .as_ref()
                .filter(|drag| drag.drawing)
                .map(|drag| view.rectangle(drag.start, drag.current))
                .or(self.selected);
            if let Some(selection) = selection {
                let top_left = view.rd_to_screen([selection.min_x, selection.max_y]);
                let bottom_right = view.rd_to_screen([selection.max_x, selection.min_y]);
                let size = Size::new(bottom_right.x - top_left.x, bottom_right.y - top_left.y);
                frame.fill_rectangle(top_left, size, Color::from_rgba(0.85, 0.43, 0.03, 0.18));
                frame.stroke_rectangle(
                    top_left,
                    size,
                    canvas::Stroke::default()
                        .with_color(Color::from_rgb8(217, 119, 6))
                        .with_width(2.0),
                );
            }
        });
        if let Some(point) = cursor.position_in(bounds) {
            let rd = view.screen_to_rd(point);
            frame.fill_text(canvas::Text {
                content: format!("RD {:.0}, {:.0}", rd[0], rd[1]),
                position: Point::new(8.0, bounds.height - 9.0),
                size: iced::Pixels(10.0),
                color: Color::from_rgb8(35, 35, 40),
                ..canvas::Text::default()
            });
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        _state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if cursor.is_over(bounds) {
            if self.drawing {
                mouse::Interaction::Crosshair
            } else {
                mouse::Interaction::Grab
            }
        } else {
            mouse::Interaction::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rd_map_screen_round_trip_and_cursor_zoom() {
        let mut map = MapView {
            center: [121_000.0, 487_000.0],
            zoom: 11,
            width: WIDTH,
            height: HEIGHT,
        };
        let pointer = Point::new(74.0, 95.0);
        let before = map.screen_to_rd(pointer);
        map.zoom_at(1, pointer);
        let after = map.screen_to_rd(pointer);
        assert!((before[0] - after[0]).abs() < 0.0001);
        assert!((before[1] - after[1]).abs() < 0.0001);
        let projected = map.rd_to_screen(before);
        assert!((projected.x - pointer.x).abs() < 0.001);
        assert!((projected.y - pointer.y).abs() < 0.001);
    }

    #[test]
    fn visible_tiles_and_drawn_bounds_are_in_rd() {
        let map = MapView {
            center: [121_000.0, 487_000.0],
            zoom: 11,
            width: WIDTH,
            height: HEIGHT,
        };
        let tiles = map.visible_tiles();
        assert!(tiles.len() >= 2 && tiles.len() <= 9);
        assert!(tiles
            .iter()
            .all(|key| key.zoom == 11 && key.col < 2048 && key.row < 2048));
        assert!(tiles[0].url().contains("/grijs/EPSG:28992/11/"));
        let bounds = map.rectangle(Point::new(80.0, 60.0), Point::new(180.0, 150.0));
        assert!((bounds.max_x - bounds.min_x - 168.0).abs() < 0.001);
        assert!((bounds.max_y - bounds.min_y - 151.2).abs() < 0.001);
    }

    #[test]
    fn map_service_is_told_the_version_of_the_application() {
        // The name `fetch_tiles` gives its client.
        let agent = TILE_USER_AGENT;
        assert!(
            agent.starts_with(concat!(
                "OpenPointcloudStudio/",
                env!("CARGO_PKG_VERSION"),
                " "
            )),
            "{agent}"
        );
    }

    /// Asks the real map service, so it is not part of the normal run. Run it
    /// by hand before a release, together with the check of the building
    /// service: `cargo test --workspace -- --ignored live_services`
    #[test]
    #[ignore = "needs the network and the public PDOK map service"]
    fn live_services_serve_a_map_tile_as_a_png() {
        let map = MapView {
            center: [121_000.0, 487_000.0],
            zoom: 11,
            width: WIDTH,
            height: HEIGHT,
        };
        let key = map.visible_tiles()[0];
        let batch = fetch_tiles(vec![key]);
        assert_eq!(batch.len(), 1);
        let (answered, bytes) = &batch[0];
        assert_eq!(*answered, key);
        let bytes = bytes.as_ref().expect("the tile is served");
        let tile = ::image::load_from_memory_with_format(bytes, ::image::ImageFormat::Png).unwrap();
        assert_eq!((tile.width(), tile.height()), (256, 256));
    }
}
