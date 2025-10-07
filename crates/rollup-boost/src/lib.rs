#![allow(clippy::complexity)]

mod client;
pub use client::{auth::*, proxy_rpc::*, rpc::*};

mod debug_api;
pub use debug_api::*;

#[cfg(all(feature = "server", feature = "metrics-exporter-prometheus"))]
mod metrics;
#[cfg(all(feature = "server", feature = "metrics-exporter-prometheus"))]
pub use metrics::*;

mod flashblocks;
pub use flashblocks::*;

mod tracing;
pub use tracing::*;

mod probe;
pub use probe::*;

mod health;
pub use health::*;

#[cfg(test)]
pub mod tests;

mod payload;
pub use payload::*;

mod selection;
// Re-export BlockSelectionPolicy for library usage
pub use selection::BlockSelectionPolicy;

mod engine_api;
pub use engine_api::*;

mod version;
pub use version::*;

pub mod lib_api;
pub use lib_api::*;

// Re-export key types for library usage
pub use debug_api::ExecutionMode;
pub use probe::Health;

// Server-specific modules (only available with "server" feature)
#[cfg(feature = "server")]
mod cli;
#[cfg(feature = "server")]
pub use cli::*;

#[cfg(feature = "server")]
mod proxy;
#[cfg(feature = "server")]
pub use proxy::*;

#[cfg(feature = "server")]
mod server;
#[cfg(feature = "server")]
pub use server::*;

// Re-export init_metrics for CLI compatibility
#[cfg(all(feature = "server", feature = "metrics-exporter-prometheus"))]
pub use crate::metrics::init_metrics;
