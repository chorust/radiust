//! Reusable radar data core for the `radiust` CLI, Python bindings, and future
//! desktop applications. Validated scientific decoding belongs in Rust;
//! Python is an optional adapter for xarray/Zarr interoperability.

pub mod cache;
pub mod config;
pub mod digest;
pub mod discovery;
pub mod download;
pub mod engine;
pub mod error_contract;
pub mod errors;
pub mod grid;
pub mod identity;
pub mod legacy_display;
pub mod limits;
pub mod model;
pub mod output;
pub mod preview;
pub mod raw_manifest;
pub mod runtime;
pub mod safety;
pub mod science;
pub mod source;
pub mod storage;
pub mod temp;
pub mod tiles;
pub mod transport;

pub use runtime::{
    OperationContext, OperationEvent, OperationId, OperationKind, OperationProgress,
    OperationStage, RuntimeEventReceiver, RuntimeEvents,
};

#[cfg(feature = "extension-module")]
pub mod python;
#[cfg(feature = "extension-module")]
pub mod python_types;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
