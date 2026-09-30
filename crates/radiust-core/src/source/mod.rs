mod au;
mod bmkg;
pub mod browser;
mod ca;
mod cam;
pub mod catalog;
mod es;
mod fr;
mod id;
mod id_sidarma;
mod kr;
mod legacy;
mod my;
mod nz;
mod opensnow;
mod ph;
mod pt;
pub(crate) mod rainviewer;
mod sg;
mod th;
mod th_royalrain;
pub mod tiles;
pub(crate) mod tw;
mod tw_http;
mod uk;
mod vn;
mod windy;
mod wunderground;

pub use opensnow::OpenSnowSourceAdapter;
pub use uk::UkSourceAdapter;
pub use windy::WindySourceAdapter;
pub use wunderground::WundergroundSourceAdapter;

use crate::errors::CoreResult;
use crate::limits::{Limits, RequestBudget};
use crate::model::{DiscoveryTarget, FrameRef, Query, RawFrame};
use crate::transport::ftp::FtpTransport;
use crate::transport::http::{HttpRequestCoalescer, HttpTransport};
use futures_util::future::BoxFuture;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;

/// Source-specific values are passed only to the selected native adapter.
/// They may contain credentials and must never be included in reports/logs.
pub type SourceOptions = BTreeMap<String, serde_yaml_ng::Value>;

#[derive(Clone)]
pub struct SourceContext {
    pub query: Query,
    pub allow_network: bool,
    pub discovery_workers: usize,
    pub source_options: Arc<SourceOptions>,
    pub request_budget: Arc<RequestBudget>,
    pub limits: Limits,
    pub http_transport: Arc<HttpTransport>,
    pub ftp_transport: Arc<FtpTransport>,
    pub request_coalescer: Arc<HttpRequestCoalescer>,
}

/// Object-safe async interface implemented by each native provider adapter.
/// The adapter receives an owned target/context so workers can be cancelled
/// without retaining borrowed CLI or Python state.
pub trait SourceAdapter: Send + Sync {
    fn source_id(&self) -> &'static str;

    /// Host allow-list for generic raw HTTP acquisition. Providers without a
    /// reviewed acquisition path leave this disabled.
    fn allows_artifact_host(&self, _host: &str) -> bool {
        false
    }

    /// Validate the complete artifact URL against the selected frame. Source
    /// adapters with URL-derived identity should override this so changing a
    /// locator cannot fetch a different frame under the same logical ID.
    fn allows_artifact_url(&self, frame: &FrameRef, url: &url::Url) -> bool {
        let declared_primary = frame
            .locator
            .get("url")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|declared| declared == url.as_str());
        let declared_artifact =
            frame.locator.get("artifacts").and_then(serde_json::Value::as_array).is_some_and(
                |artifacts| {
                    artifacts.iter().any(|artifact| {
                        artifact
                            .get("url")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|declared| declared == url.as_str())
                    })
                },
            );
        url.host_str().is_some_and(|host| self.allows_artifact_host(host))
            && frame.source == self.source_id()
            && (declared_primary || declared_artifact)
    }

    fn discover(
        self: Arc<Self>,
        target: DiscoveryTarget,
        context: SourceContext,
    ) -> BoxFuture<'static, CoreResult<Vec<FrameRef>>>;

    /// Source-specific raw acquisition hook. HTTP sources use Engine's generic
    /// artifact path; providers with a different transport can opt in here.
    fn fetch_raw(
        self: Arc<Self>,
        _frame: FrameRef,
        _context: SourceContext,
        _temp_root: PathBuf,
    ) -> Option<BoxFuture<'static, CoreResult<RawFrame>>> {
        None
    }
}

#[derive(Default, Clone)]
pub struct SourceRegistry {
    adapters: BTreeMap<String, Arc<dyn SourceAdapter>>,
}

impl SourceRegistry {
    pub fn with_builtins() -> Self {
        let mut registry = Self::default();
        // Built-in registrations are static and unique by construction.
        registry.adapters.insert("au".into(), Arc::new(au::AuSourceAdapter));
        registry.adapters.insert("bmkg".into(), Arc::new(bmkg::BmkgSourceAdapter));
        registry.adapters.insert("ca".into(), Arc::new(ca::CaSourceAdapter));
        registry.adapters.insert("cam".into(), Arc::new(cam::CamSourceAdapter));
        registry.adapters.insert("es".into(), Arc::new(es::EsSourceAdapter));
        registry.adapters.insert("fr".into(), Arc::new(fr::FrSourceAdapter));
        registry.adapters.insert("id".into(), Arc::new(id::IdSourceAdapter));
        registry.adapters.insert("id_sidarma".into(), Arc::new(id_sidarma::IdSidarmaSourceAdapter));
        registry.adapters.insert("kr".into(), Arc::new(kr::KrSourceAdapter));
        registry.adapters.insert("my".into(), Arc::new(my::MySourceAdapter));
        registry.adapters.insert("nz".into(), Arc::new(nz::NzSourceAdapter));
        registry.adapters.insert("opensnow".into(), Arc::new(OpenSnowSourceAdapter));
        registry.adapters.insert("ph".into(), Arc::new(ph::PhSourceAdapter));
        registry.adapters.insert("pt".into(), Arc::new(pt::PtSourceAdapter));
        registry
            .adapters
            .insert("rainviewer".into(), Arc::new(rainviewer::RainViewerSourceAdapter));
        registry.adapters.insert("sg".into(), Arc::new(sg::SgSourceAdapter));
        registry.adapters.insert("th".into(), Arc::new(th::ThSourceAdapter));
        registry
            .adapters
            .insert("th_royalrain".into(), Arc::new(th_royalrain::ThRoyalRainSourceAdapter));
        registry.adapters.insert("tw".into(), Arc::new(tw::TwSourceAdapter));
        registry.adapters.insert("tw-http".into(), Arc::new(tw_http::TwHttpSourceAdapter));
        registry.adapters.insert("uk".into(), Arc::new(UkSourceAdapter));
        registry.adapters.insert("vn".into(), Arc::new(vn::VnSourceAdapter));
        registry.adapters.insert("windy".into(), Arc::new(WindySourceAdapter));
        registry.adapters.insert("wunderground".into(), Arc::new(WundergroundSourceAdapter));
        registry
    }

    /// Explicitly injected adapters replace matching built-ins. This supports
    /// offline replay without weakening duplicate checks in `register`.
    pub fn with_overrides(mut self, overrides: Self) -> Self {
        self.adapters.extend(overrides.adapters);
        self
    }

    pub fn register(&mut self, adapter: Arc<dyn SourceAdapter>) -> Result<(), SourceRegistryError> {
        let id = adapter.source_id().to_owned();
        if self.adapters.contains_key(&id) {
            return Err(SourceRegistryError::DuplicateSource(id));
        }
        self.adapters.insert(id, adapter);
        Ok(())
    }

    pub fn get(&self, source_id: &str) -> Option<Arc<dyn SourceAdapter>> {
        self.adapters.get(source_id).cloned()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum SourceRegistryError {
    #[error("duplicate source adapter registration: {0}")]
    DuplicateSource(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_registry_contains_the_migrated_discovery_adapters() {
        let registry = SourceRegistry::with_builtins();
        assert!(registry.get("au").is_some());
        assert!(registry.get("bmkg").is_some());
        assert!(registry.get("my").is_some());
        assert!(registry.get("ca").is_some());
        assert!(registry.get("cam").is_some());
        assert!(registry.get("es").is_some());
        assert!(registry.get("fr").is_some());
        assert!(registry.get("id").is_some());
        assert!(registry.get("id_sidarma").is_some());
        assert!(registry.get("id").is_some());
        assert!(registry.get("kr").is_some());
        assert!(registry.get("nz").is_some());
        assert!(registry.get("opensnow").is_some());
        assert!(registry.get("ph").is_some());
        assert!(registry.get("pt").is_some());
        assert!(registry.get("rainviewer").is_some());
        assert!(registry.get("sg").is_some());
        assert!(registry.get("th").is_some());
        assert!(registry.get("th_royalrain").is_some());
        assert!(registry.get("tw").is_some());
        assert!(registry.get("tw-http").is_some());
        assert!(registry.get("uk").is_some());
        assert!(registry.get("vn").is_some());
        assert!(registry.get("windy").is_some());
        assert!(registry.get("wunderground").is_some());
        assert_eq!(registry.adapters.len(), 24);
    }

    #[test]
    fn default_artifact_url_policy_accepts_declared_urls_and_rejects_same_host_substitution() {
        struct TestAdapter;

        impl SourceAdapter for TestAdapter {
            fn source_id(&self) -> &'static str {
                "test"
            }

            fn allows_artifact_host(&self, host: &str) -> bool {
                host.eq_ignore_ascii_case("radar.example")
            }

            fn discover(
                self: Arc<Self>,
                _target: DiscoveryTarget,
                _context: SourceContext,
            ) -> BoxFuture<'static, CoreResult<Vec<FrameRef>>> {
                Box::pin(async { Ok(Vec::new()) })
            }
        }

        let adapter = TestAdapter;
        let frame = FrameRef {
            source: "test".into(),
            product: "composite".into(),
            station: Some("A".into()),
            valid_time: "2026-09-24T00:00:00.000000Z".into(),
            base_time: None,
            logical_id: "unused".into(),
            revision: None,
            locator_version: "test-v1".into(),
            locator: serde_json::json!({
                "url": "https://radar.example/selected.png",
                "artifacts": [{"url": "https://radar.example/declared.png"}],
            }),
        };

        assert!(adapter.allows_artifact_url(
            &frame,
            &url::Url::parse("https://radar.example/selected.png").unwrap()
        ));
        assert!(adapter.allows_artifact_url(
            &frame,
            &url::Url::parse("https://radar.example/declared.png").unwrap()
        ));
        assert!(!adapter.allows_artifact_url(
            &frame,
            &url::Url::parse("https://radar.example/forged.png").unwrap()
        ));
    }
}
