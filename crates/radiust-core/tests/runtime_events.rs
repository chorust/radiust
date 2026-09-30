use radiust_core::config::CoreConfig;
use radiust_core::download::FetchErrorPolicy;
use radiust_core::engine::{Engine, EngineError};
use radiust_core::model::{FrameRef, Query};
use radiust_core::source::SourceRegistry;
use radiust_core::{
    OperationContext, OperationEvent, OperationKind, OperationStage, RuntimeEventReceiver,
    RuntimeEvents,
};
use serde_json::{Value, json};
use std::time::Duration;

fn offline_engine() -> Engine {
    let mut config = CoreConfig::default();
    config.runtime.allow_network = false;
    Engine::new(config, SourceRegistry::default()).expect("offline Engine initializes")
}

fn sensitive_frame() -> FrameRef {
    FrameRef {
        source: "tw".into(),
        product: "grid".into(),
        station: Some("private-station".into()),
        valid_time: "2026-09-28T00:00:00Z".into(),
        base_time: None,
        logical_id: "private-logical-id".into(),
        revision: None,
        locator_version: "fixture-v1".into(),
        locator: json!({
            "url": "https://private.example/data?token=private-credential",
            "authorization": "private-header-value",
            "artifact": "private-artifact-name"
        }),
    }
}

async fn collect_operation(receiver: &mut RuntimeEventReceiver) -> Vec<OperationEvent> {
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut events = Vec::new();
        loop {
            let event = receiver.recv().await.expect("event channel remains open");
            let terminal = matches!(
                event.stage,
                OperationStage::Completed | OperationStage::Cancelled | OperationStage::Failed
            );
            events.push(event);
            if terminal {
                return events;
            }
        }
    })
    .await
    .expect("operation publishes a terminal event")
}

fn assert_safe_event_shape(event: &OperationEvent) {
    let value = serde_json::to_value(event).expect("event serializes");
    let fields = value.as_object().expect("event serializes as an object");
    assert!(fields.keys().all(|key| {
        matches!(key.as_str(), "operation_id" | "operation" | "stage" | "progress")
    }));
    let rendered = value.to_string();
    for sensitive in [
        "private.example",
        "private-credential",
        "private-header-value",
        "private-artifact-name",
        "private-logical-id",
        "private-station",
    ] {
        assert!(!rendered.contains(sensitive), "event leaked {sensitive}");
    }
}

#[tokio::test]
async fn discover_events_are_ordered_have_distinct_ids_and_expose_only_safe_fields() {
    let engine = offline_engine();
    let mut receiver = engine.subscribe_events();
    let query = Query { source: Some("tw".into()), ..Query::default() };

    engine.discover_seeded(query.clone(), 1).await.expect("offline discovery returns a report");
    let first = collect_operation(&mut receiver).await;
    engine.discover_seeded(query, 2).await.expect("offline discovery returns a report");
    let second = collect_operation(&mut receiver).await;

    for events in [&first, &second] {
        assert_eq!(events.first().unwrap().stage, OperationStage::Started);
        assert_eq!(events.last().unwrap().stage, OperationStage::Completed);
        assert!(events.iter().all(|event| event.operation == OperationKind::Discover));
        for event in events {
            assert_safe_event_shape(event);
        }
    }
    assert_ne!(first[0].operation_id, second[0].operation_id);
    assert!(first.iter().any(|event| {
        event.stage == OperationStage::Progress
            && event.progress.is_some_and(|progress| progress.total.is_some())
    }));
}

#[tokio::test]
async fn fetch_errors_and_engine_cancellation_publish_safe_terminal_events() {
    let engine = offline_engine();
    let mut receiver = engine.subscribe_events();
    let error = engine.fetch_raw(sensitive_frame()).await.unwrap_err();
    assert!(matches!(error, EngineError::Core(_)));
    let failed = collect_operation(&mut receiver).await;
    assert_eq!(failed[0].operation, OperationKind::FetchRaw);
    assert_eq!(failed[0].stage, OperationStage::Started);
    assert_eq!(failed.last().unwrap().stage, OperationStage::Failed);
    for event in &failed {
        assert_safe_event_shape(event);
    }

    let cancelled_engine = offline_engine();
    let mut cancelled_receiver = cancelled_engine.subscribe_events();
    cancelled_engine.cancel();
    let error = cancelled_engine.fetch_raw(sensitive_frame()).await.unwrap_err();
    assert!(matches!(error, EngineError::Core(radiust_core::errors::CoreError::Cancelled)));
    let cancelled = collect_operation(&mut cancelled_receiver).await;
    assert_eq!(cancelled[0].stage, OperationStage::Started);
    assert_eq!(cancelled.last().unwrap().stage, OperationStage::Cancelled);
    for event in &cancelled {
        assert_safe_event_shape(event);
    }
}

#[tokio::test]
async fn download_entry_publishes_events_and_operations_work_without_subscribers() {
    let engine = offline_engine();
    let report = engine.download_png(Vec::new(), FetchErrorPolicy::Collect, true, false).await;
    assert_eq!(report.planned, 0);

    let engine = offline_engine();
    let mut receiver = engine.subscribe_events();
    engine.download_png(Vec::new(), FetchErrorPolicy::Collect, true, false).await;
    let events = collect_operation(&mut receiver).await;
    assert_eq!(events[0].operation, OperationKind::DownloadPng);
    assert_eq!(events[0].stage, OperationStage::Started);
    assert_eq!(events.last().unwrap().stage, OperationStage::Completed);
}

#[tokio::test]
async fn unfinished_context_is_cancelled_and_slow_subscribers_observe_bounded_lag() {
    fn assert_send<T: Send>() {}
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send::<RuntimeEventReceiver>();
    assert_send::<OperationContext>();
    assert_send_sync::<RuntimeEvents>();
    assert_send_sync::<OperationEvent>();

    let events = RuntimeEvents::new(1);
    let mut receiver = events.subscribe();
    let context = events.begin(OperationKind::Regrid);
    let id = context.id();
    drop(context);
    assert!(matches!(
        receiver.recv().await,
        Err(tokio::sync::broadcast::error::RecvError::Lagged(1))
    ));
    let cancelled = receiver.recv().await.expect("terminal event remains in the bounded channel");
    assert_eq!(cancelled.operation_id, id);
    assert_eq!(cancelled.stage, OperationStage::Cancelled);
    assert_eq!(cancelled.progress, None);
    assert_safe_event_shape(&cancelled);
}

#[test]
fn progress_payload_has_only_completed_and_optional_total_counts() {
    let value: Value =
        serde_json::to_value(radiust_core::OperationProgress { completed: 3, total: Some(8) })
            .unwrap();
    assert_eq!(value, json!({ "completed": 3, "total": 8 }));
}
