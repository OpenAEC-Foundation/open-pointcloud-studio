//! Shared point cloud kernel for Open Pointcloud Studio.
//!
//! The same crate is linked natively by the Tauri backend and compiled to
//! WebAssembly for the browser, so desktop and web run one implementation
//! instead of two that drift apart. Feature `parallel` turns on rayon and is
//! default for native builds; feature `wasm` adds the JS bindings.

pub mod frustum;
pub mod octree;
pub mod types;

pub use frustum::{select_visible, Camera, Frustum, SelectStats, VisibleNode};
pub use octree::{Node, Octree};
pub use types::{Bounds, PointCloud};

#[cfg(feature = "wasm")]
mod wasm_api;
