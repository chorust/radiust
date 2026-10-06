//! Compatibility re-exports for the historical display module.
//!
//! New callers should use [`crate::gray`]. The old module and result name remain
//! available so existing Rust integrations continue to compile.

pub use crate::gray::GrayPreview as LegacyDisplayPreview;
pub use crate::gray::apply_for_source;
