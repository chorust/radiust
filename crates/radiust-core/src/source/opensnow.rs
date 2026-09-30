//! Fail-closed native adapter for OpenSnow.
//!
//! The legacy tile URL has no accepted canonical raw response or verified
//! frame-time contract. Keep the source addressable in the catalog, but do
//! not turn the unverified locator into a discovery or acquisition claim.

use crate::errors::{CoreError, CoreResult};
use crate::model::{DiscoveryTarget, FrameRef, Query};
use crate::source::{SourceAdapter, SourceContext};
use futures_util::future::BoxFuture;
use std::sync::Arc;

const SOURCE: &str = "opensnow";
const PRODUCT: &str = "composite";

/// OpenSnow remains listed, but has no accepted native raw-data contract.
pub struct OpenSnowSourceAdapter;

impl SourceAdapter for OpenSnowSourceAdapter {
    fn source_id(&self) -> &'static str {
        SOURCE
    }

    fn discover(
        self: Arc<Self>,
        target: DiscoveryTarget,
        context: SourceContext,
    ) -> BoxFuture<'static, CoreResult<Vec<FrameRef>>> {
        Box::pin(async move {
            validate_target_and_query(&target, &context.query)?;
            Err(CoreError::Transport(
                "source opensnow is unavailable: canonical raw evidence and frame-time contract are unverified".into(),
            ))
        })
    }
}

fn validate_target_and_query(target: &DiscoveryTarget, query: &Query) -> CoreResult<()> {
    if target.source != SOURCE
        || query.source.as_deref().is_some_and(|source| source != SOURCE && source != "all")
        || (!query.sources.is_empty() && !query.sources.iter().any(|source| source == SOURCE))
    {
        return Err(CoreError::Transport(
            "source opensnow received a mismatched source query".into(),
        ));
    }
    if target.product.as_deref().is_some_and(|product| product != PRODUCT)
        || query.product.as_deref().is_some_and(|product| product != PRODUCT)
    {
        return Err(CoreError::Transport(
            "source opensnow only supports the composite product".into(),
        ));
    }
    if target.station.is_some() || !query.stations.is_empty() {
        return Err(CoreError::Transport(
            "source opensnow does not have verified station metadata".into(),
        ));
    }
    if query.base_time.is_some() {
        return Err(CoreError::Transport("source opensnow does not expose base times".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::{Limits, RequestBudget};
    use crate::model::TimeSelector;
    use crate::transport::ftp::FtpTransport;
    use crate::transport::http::{HttpRequestCoalescer, HttpTransport};

    fn context(allow_network: bool) -> SourceContext {
        let limits = Limits::default();
        let request_budget = Arc::new(RequestBudget::new(&limits));
        let http_transport = Arc::new(
            HttpTransport::with_budget(limits.clone(), false, request_budget.clone()).unwrap(),
        );
        SourceContext {
            query: Query { source: Some(SOURCE.into()), ..Query::default() },
            allow_network,
            discovery_workers: 1,
            source_options: Arc::new(Default::default()),
            request_budget,
            limits: limits.clone(),
            http_transport,
            ftp_transport: Arc::new(FtpTransport::new(limits, false)),
            request_coalescer: Arc::new(HttpRequestCoalescer::default()),
        }
    }

    fn target() -> DiscoveryTarget {
        DiscoveryTarget { source: SOURCE.into(), product: Some(PRODUCT.into()), station: None }
    }

    #[tokio::test]
    async fn discovery_fails_closed_without_using_the_unverified_legacy_locator() {
        let error =
            Arc::new(OpenSnowSourceAdapter).discover(target(), context(true)).await.unwrap_err();
        assert!(
            matches!(error, CoreError::Transport(message) if message.contains("canonical raw evidence"))
        );
        assert!(!OpenSnowSourceAdapter.allows_artifact_host("opensnow.com"));
    }

    #[test]
    fn only_the_catalogued_product_is_accepted_for_validation() {
        let query = Query {
            source: Some(SOURCE.into()),
            selector: TimeSelector::Latest,
            ..Query::default()
        };
        assert!(validate_target_and_query(&target(), &query).is_ok());
        assert!(
            validate_target_and_query(&target(), &Query { product: Some("other".into()), ..query })
                .is_err()
        );
    }
}
