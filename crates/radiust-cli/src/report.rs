use radiust_core::download::{DownloadBatchReport, DownloadStatus};
use radiust_core::model::{DiscoveryReport, DiscoveryStatus, Query, TimeSelector};
use serde_json::{Value, json};

pub fn discovery_envelope(report: &DiscoveryReport) -> Value {
    let mut items = Vec::with_capacity(report.items.len());
    for item in &report.items {
        let status = serde_json::to_value(item.status).unwrap_or(Value::Null);
        let frame = item.frame.as_ref().map(|frame| {
            json!({
                "source": frame.source,
                "product": frame.product,
                "station": frame.station,
                "valid_time": frame.valid_time,
                "base_time": frame.base_time,
            })
        });
        items.push(json!({
            "source": item.target.source,
            "product": item.target.product,
            "station": item.target.station,
            "status": status,
            "valid_time": item.valid_time,
            "frame": frame,
            "source_urls": item.frame.as_ref().map(crate::public_source_urls).unwrap_or_default(),
            "capabilities": null,
            "error": item.error,
        }));
    }
    json!({
        "schema_version": 1,
        "command": "discover",
        "run_id": null,
        "query": query_json(&report.query),
        "counts": report.counts,
        "items": items,
        "error": null,
        "interrupted": report.interrupted,
    })
}

pub fn query_json(query: &Query) -> Value {
    if query.source.as_deref() == Some("all") {
        return json!({
            "source": "all",
            "latest": matches!(query.selector, TimeSelector::Latest),
            "max_age": query.max_age_secs,
        });
    }
    if !query.sources.is_empty() {
        return json!({
            "sources": query.sources,
            "latest": matches!(query.selector, TimeSelector::Latest),
            "max_age": query.max_age_secs,
        });
    }
    match &query.selector {
        TimeSelector::Latest => json!({
            "source": query.source,
            "product": query.product,
            "stations": query.stations,
            "latest": true,
            "max_age": query.max_age_secs,
            "base_time": query.base_time,
        }),
        TimeSelector::At { time } => json!({
            "source": query.source,
            "product": query.product,
            "stations": query.stations,
            "at": time,
            "base_time": query.base_time,
        }),
        TimeSelector::Range { start, end } => json!({
            "source": query.source,
            "product": query.product,
            "stations": query.stations,
            "start": start,
            "end": end,
            "base_time": query.base_time,
        }),
    }
}

pub fn envelope(command: &str, items: Vec<Value>, result: Option<Value>) -> Value {
    json!({
        "schema_version": 1,
        "command": command,
        "run_id": null,
        "query": null,
        "counts": {"items": items.len()},
        "items": items,
        "result": result,
        "error": null,
        "interrupted": false,
    })
}

pub fn error_envelope(code: &str, message: &str, stage: &str) -> Value {
    json!({
        "schema_version": 1,
        "command": null,
        "run_id": null,
        "query": null,
        "counts": {},
        "items": [],
        "error": {"code": code, "message": message, "stage": stage, "retryable": false},
        "interrupted": false,
    })
}

/// Human reports are a presentation of the stable machine envelope.
pub fn render_human(value: &Value, command_hint: Option<&str>, verbose: bool) -> String {
    crate::human::render(value, command_hint, verbose)
}

pub(crate) fn safe_label(value: &str) -> String {
    value.chars().map(|character| if character.is_control() { ' ' } else { character }).collect()
}

pub(crate) fn display_value(value: &Value) -> String {
    match value {
        Value::String(text) => safe_label(text),
        Value::Array(items) => items.iter().map(display_value).collect::<Vec<_>>().join(", "),
        Value::Object(object) => object
            .iter()
            .map(|(key, value)| format!("{}: {}", safe_label(key), display_value(value)))
            .collect::<Vec<_>>()
            .join("; "),
        _ => value.to_string(),
    }
}

/// Project discovery results into the Python `DownloadReport` dry-run shape.
/// This operation plans frames only; it never acquires or commits artifacts.
pub fn download_dry_run(report: &DiscoveryReport) -> (Value, u8) {
    if report.interrupted {
        return (error_envelope("cancelled", "operation cancelled", "download"), 130);
    }
    if !report.items.is_empty()
        && report.items.iter().all(|item| item.status == DiscoveryStatus::NetworkRestricted)
    {
        return (error_envelope("error", "Unexpected operation failure", "validate"), 2);
    }

    let mut counts = json!({
        "written": 0,
        "skipped": 0,
        "failed": 0,
        "cancelled": 0,
        "not_started": 0,
        "planned": 0,
    });
    let mut items = Vec::with_capacity(report.items.len());
    let mut failure_codes = Vec::new();
    for item in &report.items {
        let planned = item.status == DiscoveryStatus::Success;
        let status = if planned { "planned" } else { "failed" };
        let error = item.error.as_ref().map(|error| {
            json!({
                "code": error.code,
                "message": error.message,
                "stage": error.stage,
                "retryable": error.retryable,
            })
        });
        if planned {
            counts["planned"] = json!(counts["planned"].as_u64().unwrap_or_default() + 1);
        } else {
            counts["failed"] = json!(counts["failed"].as_u64().unwrap_or_default() + 1);
            if let Some(error) = item.error.as_ref() {
                failure_codes.push(error.code.as_str());
            }
        }
        let frame = item.frame.as_ref();
        items.push(json!({
            "source": item.target.source,
            "product": item.target.product,
            "station": item.target.station,
            "valid_time": frame.map(|frame| frame.valid_time.as_str()),
            "logical_id": frame.map(|frame| frame.logical_id.as_str()),
            "status": status,
            "output_uri": null,
            "error": error,
        }));
    }

    let report_json = json!({
        "schema_version": 1,
        "command": "download",
        "run_id": "dry-run",
        "query": query_json(&report.query),
        "counts": counts,
        "items": items,
        "error": null,
        "interrupted": false,
    });
    let failed_count =
        report.items.iter().filter(|item| item.status != DiscoveryStatus::Success).count();
    let exit_code = if report.items.is_empty() || failed_count == 0 {
        0
    } else if !failure_codes.is_empty()
        && failure_codes.iter().all(|code| matches!(*code, "no_data" | "stale" | "stale_frame"))
    {
        3
    } else {
        5
    };
    (report_json, exit_code)
}

/// Combine discovery failures and raw-only commit outcomes into one ordered
/// download report matching the Python `DownloadReport` status vocabulary.
pub fn download_execution(
    discovery: &DiscoveryReport,
    downloaded: &DownloadBatchReport,
) -> (Value, u8) {
    if !discovery.items.is_empty()
        && discovery.items.iter().all(|item| item.status == DiscoveryStatus::NetworkRestricted)
    {
        return (error_envelope("error", "Unexpected operation failure", "validate"), 2);
    }
    let mut counts = json!({
        "written": 0,
        "skipped": 0,
        "failed": 0,
        "cancelled": 0,
        "not_started": 0,
        "planned": 0,
    });
    let mut items = Vec::with_capacity(discovery.items.len());
    let mut download_index = 0;
    let mut failed_codes = Vec::new();
    for item in &discovery.items {
        if item.status == DiscoveryStatus::Success {
            let Some(frame) = item.frame.as_ref() else {
                counts["failed"] = json!(counts["failed"].as_u64().unwrap_or_default() + 1);
                failed_codes.push("integrity".to_owned());
                items.push(json!({
                    "source": item.target.source,
                    "product": item.target.product,
                    "station": item.target.station,
                    "valid_time": item.valid_time,
                    "logical_id": null,
                    "status": "failed",
                    "output_uri": null,
                    "error": {"code": "integrity", "message": "discovery result omitted its frame", "stage": "download", "retryable": false},
                }));
                continue;
            };
            let result = downloaded.items.get(download_index);
            download_index += 1;
            let Some(result) = result else {
                counts["failed"] = json!(counts["failed"].as_u64().unwrap_or_default() + 1);
                failed_codes.push("internal".to_owned());
                items.push(json!({
                    "source": frame.source,
                    "product": frame.product,
                    "station": frame.station,
                    "valid_time": frame.valid_time,
                    "logical_id": frame.logical_id,
                    "status": "failed",
                    "output_uri": null,
                    "error": {"code": "internal", "message": "download result is missing", "stage": "download", "retryable": false},
                }));
                continue;
            };
            let status = match result.status {
                DownloadStatus::Planned => "planned",
                DownloadStatus::Written => "written",
                DownloadStatus::Skipped => "skipped",
                DownloadStatus::Failed => "failed",
                DownloadStatus::Cancelled => "cancelled",
                DownloadStatus::NotStarted => "not_started",
            };
            counts[status] = json!(counts[status].as_u64().unwrap_or_default() + 1);
            if result.status == DownloadStatus::Failed {
                failed_codes.push(
                    result
                        .error
                        .as_ref()
                        .and_then(|error| serde_json::to_value(error.code).ok())
                        .and_then(|value| value.as_str().map(str::to_owned))
                        .unwrap_or_default(),
                );
            }
            let error = result.error.as_ref().and_then(|error| serde_json::to_value(error).ok());
            items.push(json!({
                "source": frame.source,
                "product": frame.product,
                "station": frame.station,
                "valid_time": frame.valid_time,
                "logical_id": frame.logical_id,
                "status": status,
                "output_uri": result.output_uri,
                "error": error,
            }));
        } else {
            let status = if item.status == DiscoveryStatus::Cancelled {
                "cancelled"
            } else if item.status == DiscoveryStatus::NotStarted {
                "not_started"
            } else {
                "failed"
            };
            counts[status] = json!(counts[status].as_u64().unwrap_or_default() + 1);
            if status == "failed" {
                failed_codes
                    .push(item.error.as_ref().map(|error| error.code.clone()).unwrap_or_default());
            }
            let error = item.error.as_ref().map(|error| {
                json!({
                    "code": error.code,
                    "message": error.message,
                    "stage": error.stage,
                    "retryable": error.retryable,
                })
            });
            items.push(json!({
                "source": item.target.source,
                "product": item.target.product,
                "station": item.target.station,
                "valid_time": item.valid_time,
                "logical_id": null,
                "status": status,
                "output_uri": null,
                "error": error,
            }));
        }
    }
    let interrupted = discovery.interrupted || downloaded.interrupted;
    let failed = counts["failed"].as_u64().unwrap_or_default();
    let succeeded = counts["written"].as_u64().unwrap_or_default()
        + counts["skipped"].as_u64().unwrap_or_default();
    let exit_code = if interrupted {
        130
    } else if failed > 0 && succeeded > 0 {
        4
    } else if failed > 0
        && failed_codes
            .iter()
            .all(|code| matches!(code.as_str(), "no_data" | "stale_frame" | "stale"))
    {
        3
    } else if failed > 0 {
        5
    } else if items.is_empty() {
        3
    } else {
        0
    };
    let run_id = format!(
        "native-download-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let report = json!({
        "schema_version": 1,
        "command": "download",
        "run_id": run_id,
        "query": query_json(&discovery.query),
        "counts": counts,
        "items": items,
        "error": null,
        "interrupted": interrupted,
    });
    (report, exit_code)
}

pub fn download_interrupted(query: &Query) -> Value {
    let mut payload = envelope("download", Vec::new(), None);
    payload["query"] = query_json(query);
    payload["error"] = json!({
        "code": "cancelled",
        "message": "operation cancelled",
        "stage": "download",
        "retryable": false,
    });
    payload["interrupted"] = json!(true);
    payload
}

#[cfg(test)]
mod tests {
    use super::*;
    use radiust_core::download::DownloadItem;
    use radiust_core::error_contract::{ErrorCode, ErrorReport, ErrorStage};
    use radiust_core::identity::logical_id;
    use radiust_core::model::{DiscoveryItem, DiscoveryTarget, FrameRef, SafeError};

    #[test]
    fn dry_run_keeps_planned_frames_and_no_data_errors_in_one_report() {
        let target = DiscoveryTarget {
            source: "fr".into(),
            product: Some("composite".into()),
            station: Some("FRCOMP".into()),
        };
        let mut frame = FrameRef {
            source: "fr".into(),
            product: "composite".into(),
            station: Some("FRCOMP".into()),
            valid_time: "2026-09-24T00:00:00.000000Z".into(),
            base_time: None,
            logical_id: String::new(),
            revision: Some("frame-revision".into()),
            locator_version: "fr-wms-v1".into(),
            locator: serde_json::Value::Null,
        };
        frame.logical_id = logical_id(&frame).unwrap();
        let items = vec![
            DiscoveryItem {
                target,
                status: DiscoveryStatus::Success,
                valid_time: Some(frame.valid_time.clone()),
                frame: Some(frame.clone()),
                error: None,
            },
            DiscoveryItem {
                target: DiscoveryTarget {
                    source: "fr".into(),
                    product: Some("composite".into()),
                    station: Some("FRCOMP2".into()),
                },
                status: DiscoveryStatus::NoData,
                valid_time: None,
                frame: None,
                error: Some(SafeError {
                    code: "no_data".into(),
                    message: "no matching frame is available".into(),
                    stage: "discover".into(),
                    retryable: false,
                }),
            },
        ];
        let report = DiscoveryReport::from_items(
            Query { source: Some("fr".into()), ..Query::default() },
            items,
            false,
        )
        .unwrap();

        let (payload, exit_code) = download_dry_run(&report);

        assert_eq!(exit_code, 3);
        assert_eq!(payload["counts"]["planned"], 1);
        assert_eq!(payload["counts"]["failed"], 1);
        assert_eq!(payload["items"][0]["status"], "planned");
        assert_eq!(payload["items"][0]["logical_id"], frame.logical_id);
        assert_eq!(payload["items"][1]["status"], "failed");
        assert_eq!(payload["items"][1]["error"]["code"], "no_data");
    }

    #[test]
    fn raw_execution_maps_partial_and_total_failures_to_legacy_exit_codes() {
        let mut frame = FrameRef {
            source: "fr".into(),
            product: "composite".into(),
            station: Some("FRCOMP".into()),
            valid_time: "2026-09-24T00:00:00.000000Z".into(),
            base_time: None,
            logical_id: String::new(),
            revision: None,
            locator_version: "fr-wms-v1".into(),
            locator: serde_json::Value::Null,
        };
        frame.logical_id = logical_id(&frame).unwrap();
        let discovery = DiscoveryReport::from_items(
            Query { source: Some("fr".into()), ..Query::default() },
            vec![
                DiscoveryItem {
                    target: DiscoveryTarget {
                        source: "fr".into(),
                        product: Some("composite".into()),
                        station: Some("FRCOMP".into()),
                    },
                    status: DiscoveryStatus::Success,
                    valid_time: Some(frame.valid_time.clone()),
                    frame: Some(frame.clone()),
                    error: None,
                },
                DiscoveryItem {
                    target: DiscoveryTarget {
                        source: "fr".into(),
                        product: Some("composite".into()),
                        station: Some("FRCOMP2".into()),
                    },
                    status: DiscoveryStatus::NoData,
                    valid_time: None,
                    frame: None,
                    error: Some(SafeError {
                        code: "no_data".into(),
                        message: "no matching frame is available".into(),
                        stage: "discover".into(),
                        retryable: false,
                    }),
                },
            ],
            false,
        )
        .unwrap();
        let downloaded = DownloadBatchReport {
            items: vec![DownloadItem {
                input_index: 0,
                frame,
                status: DownloadStatus::Written,
                output_uri: Some("frames/example/raw-manifest.json".into()),
                error: None,
            }],
            written: 1,
            ..DownloadBatchReport::default()
        };

        let (payload, exit_code) = download_execution(&discovery, &downloaded);

        assert_eq!(exit_code, 4);
        assert_eq!(payload["counts"]["written"], 1);
        assert_eq!(payload["counts"]["failed"], 1);
        assert_eq!(payload["items"][0]["status"], "written");
        assert_eq!(payload["items"][1]["status"], "failed");
        assert_eq!(payload["items"][1]["error"]["code"], "no_data");

        let failed_download = DownloadBatchReport {
            items: vec![DownloadItem {
                input_index: 0,
                frame: discovery.items[0].frame.as_ref().unwrap().clone(),
                status: DownloadStatus::Failed,
                output_uri: None,
                error: Some(ErrorReport {
                    code: ErrorCode::Transport,
                    message: "transport request failed".into(),
                    stage: ErrorStage::Acquire,
                    retryable: true,
                }),
            }],
            failed: 1,
            ..DownloadBatchReport::default()
        };
        let (failed_payload, failed_exit_code) = download_execution(&discovery, &failed_download);

        assert_eq!(failed_exit_code, 5);
        assert_eq!(failed_payload["counts"]["failed"], 2);
        assert_eq!(failed_payload["items"][0]["error"]["code"], "transport");
        assert_eq!(failed_payload["items"][1]["error"]["code"], "no_data");
    }

    #[test]
    fn interrupted_download_keeps_ordered_partial_results_and_exit_130() {
        let make_frame = |hour: u8, station: &str| {
            let mut frame = FrameRef {
                source: "fr".into(),
                product: "composite".into(),
                station: Some(station.into()),
                valid_time: format!("2026-09-24T{hour:02}:00:00.000000Z"),
                base_time: None,
                logical_id: String::new(),
                revision: None,
                locator_version: "fr-wms-v1".into(),
                locator: serde_json::Value::Null,
            };
            frame.logical_id = logical_id(&frame).unwrap();
            frame
        };
        let written_frame = make_frame(0, "FRCOMP");
        let cancelled_frame = make_frame(1, "FRCOMP2");
        let target = |station: &str| DiscoveryTarget {
            source: "fr".into(),
            product: Some("composite".into()),
            station: Some(station.into()),
        };
        let discovery = DiscoveryReport::from_items(
            Query { source: Some("fr".into()), ..Query::default() },
            vec![
                DiscoveryItem {
                    target: target("FRCOMP"),
                    status: DiscoveryStatus::Success,
                    valid_time: Some(written_frame.valid_time.clone()),
                    frame: Some(written_frame.clone()),
                    error: None,
                },
                DiscoveryItem {
                    target: target("FRCOMP2"),
                    status: DiscoveryStatus::Success,
                    valid_time: Some(cancelled_frame.valid_time.clone()),
                    frame: Some(cancelled_frame.clone()),
                    error: None,
                },
            ],
            false,
        )
        .unwrap();
        let downloaded = DownloadBatchReport {
            items: vec![
                DownloadItem {
                    input_index: 0,
                    frame: written_frame,
                    status: DownloadStatus::Written,
                    output_uri: Some("frames/first/raw-manifest.json".into()),
                    error: None,
                },
                DownloadItem {
                    input_index: 1,
                    frame: cancelled_frame,
                    status: DownloadStatus::Cancelled,
                    output_uri: None,
                    error: None,
                },
            ],
            interrupted: true,
            written: 1,
            cancelled: 1,
            ..DownloadBatchReport::default()
        };

        let (payload, exit_code) = download_execution(&discovery, &downloaded);

        assert_eq!(exit_code, 130);
        assert_eq!(payload["interrupted"], true);
        assert_eq!(payload["counts"]["written"], 1);
        assert_eq!(payload["counts"]["cancelled"], 1);
        assert_eq!(payload["items"][0]["status"], "written");
        assert_eq!(payload["items"][1]["status"], "cancelled");
    }

    #[test]
    fn human_rendering_omits_absent_fields_sanitizes_controls_and_explains_empty_discovery() {
        let report = json!({
            "schema_version": 1,
            "command": "download",
            "counts": {"written": 1},
            "items": [{
                "source": "TH\u{1b}[2J",
                "status": "written",
                "error": null,
                "output_uri": null,
            }],
            "error": null,
            "interrupted": false,
        });
        let human = render_human(&report, None, false);
        assert!(human.contains("DOWNLOAD"));
        assert!(human.contains("TH [2J"));
        assert!(human.contains("written"));
        assert!(!human.contains('\u{1b}'));
        assert!(!human.contains("error:"));
        assert!(!human.contains("output_uri:"));

        let empty = json!({
            "schema_version": 1,
            "command": "discover",
            "counts": {"total": 0},
            "items": [],
        });
        assert!(render_human(&empty, None, false).contains("No data found"));
    }
}
