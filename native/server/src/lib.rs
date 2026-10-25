//! The headless HTTP and MCP service of Open Pointcloud Studio: the routes
//! of the OpenAEC SuperCloud contract over the operations of
//! `pointcloud_core::ops`, the same ones the command line and the C library
//! run. See `docs/cloud-api.md`.

pub mod config;
pub mod error;
pub mod http;
pub mod limits;
pub mod mcp;
pub mod openapi;
pub mod payload;
pub mod policy;
pub mod tools;

#[cfg(test)]
mod tests;

/// The version the service reports.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The name of the service, as MCP `serverInfo` gives it.
pub const NAME: &str = "open-pointcloud-studio-api";
/// The newest MCP protocol version the service speaks, as `/v1/health`
/// reports it.
pub const PROTOCOL: &str = mcp::PROTOCOL_VERSIONS[0];
