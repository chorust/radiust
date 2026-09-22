//! Bounded I/O and resource primitives for the `radiust` Python package.
//!
//! Scientific interpretation deliberately stays in Python. The extension
//! exposes small, serialization-friendly primitives so the Python facade can
//! own the public identity and data model without duplicating them in Rust.

pub mod cache;
pub mod digest;
pub mod errors;
pub mod limits;
pub mod runtime;
pub mod storage;
pub mod temp;
pub mod tiles;
pub mod transport;

#[cfg(feature = "extension-module")]
mod python;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(feature = "extension-module")]
#[pyo3::pymodule]
fn _core(m: &pyo3::Bound<'_, pyo3::types::PyModule>) -> pyo3::PyResult<()> {
    python::register(m)
}
