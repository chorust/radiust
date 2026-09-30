//! Native adapter for the retired UK Met Office DataPoint source.

use crate::errors::{CoreError, CoreResult};
use crate::model::{DiscoveryTarget, FrameRef};
use crate::source::{SourceAdapter, SourceContext};
use futures_util::future::BoxFuture;
use std::sync::Arc;

const SOURCE: &str = "uk";
const RETIRED_AT: &str = "2025-12-01";

/// The former DataPoint service is retired and has no like-for-like replacement.
pub struct UkSourceAdapter;

impl SourceAdapter for UkSourceAdapter {
    fn source_id(&self) -> &'static str {
        SOURCE
    }

    fn discover(
        self: Arc<Self>,
        _target: DiscoveryTarget,
        _context: SourceContext,
    ) -> BoxFuture<'static, CoreResult<Vec<FrameRef>>> {
        Box::pin(async {
            Err(CoreError::Transport(format!(
                "source uk is retired; DataPoint was decommissioned on {RETIRED_AT}"
            )))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::{Limits, RequestBudget};
    use crate::model::Query;
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

    #[tokio::test]
    async fn retired_source_rejects_before_any_network_access() {
        let error = Arc::new(UkSourceAdapter)
            .discover(
                DiscoveryTarget {
                    source: SOURCE.into(),
                    product: Some("rain".into()),
                    station: None,
                },
                context(true),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(error, CoreError::Transport(message) if message.contains("retired") && message.contains(RETIRED_AT))
        );
        assert!(!UkSourceAdapter.allows_artifact_host("datapoint.metoffice.gov.uk"));
    }
}
