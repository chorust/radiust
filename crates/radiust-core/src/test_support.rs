//! Fixture adapters for the opt-in cross-entry local-view integration test.

use crate::errors::{CoreError, CoreResult};
use crate::identity::logical_id;
use crate::model::{
    ArtifactReceipt, DiscoveryTarget, FrameRef, LocalViewData, LocalViewResult, RawArtifact,
    RawFrame,
};
use crate::source::{SourceAdapter, SourceContext, SourceRegistry};
use futures_util::future::BoxFuture;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

pub fn local_view_registry() -> SourceRegistry {
    let mut registry = SourceRegistry::default();
    for source in ["pt", "rainviewer", "sg"] {
        registry
            .register(Arc::new(LocalViewFixtureAdapter { source }))
            .expect("fixture registry source IDs are unique");
    }
    registry
}

pub fn local_view_data_fingerprints(result: &LocalViewResult) -> Option<serde_json::Value> {
    match result.data.as_ref()? {
        LocalViewData::RawImage { width, height, rgba } => Some(serde_json::json!({
            "data_kind": "raw_image",
            "shape": [height, width],
            "rgba_sha256": hex::encode(Sha256::digest(rgba)),
        })),
        LocalViewData::Numeric(dataset) => Some(serde_json::json!({
            "data_kind": "numeric",
            "fields": dataset.fields.iter().map(|field| {
                let mut values = Sha256::new();
                for value in &field.values {
                    values.update(value.to_le_bytes());
                }
                let mut quality = Sha256::new();
                for value in &field.quality {
                    quality.update(value.to_le_bytes());
                }
                serde_json::json!({
                    "name": field.name,
                    "units": field.units,
                    "shape": field.shape,
                    "values_sha256": hex::encode(values.finalize()),
                    "quality_sha256": hex::encode(quality.finalize()),
                })
            }).collect::<Vec<_>>(),
        })),
    }
}

struct LocalViewFixtureAdapter {
    source: &'static str,
}

impl SourceAdapter for LocalViewFixtureAdapter {
    fn source_id(&self) -> &'static str {
        self.source
    }

    fn discover(
        self: Arc<Self>,
        _target: DiscoveryTarget,
        context: SourceContext,
    ) -> BoxFuture<'static, CoreResult<Vec<FrameRef>>> {
        // Replay bytes stay fixed, while the synthetic frame uses one captured
        // time shared by CLI and SDK calls so production freshness still runs.
        let valid_time = context
            .source_options
            .get("fixture_valid_time")
            .and_then(serde_yaml_ng::Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
        let frames = match self.source {
            "pt" => Some(pt_frame(valid_time)),
            "rainviewer" => Some(rainviewer_frame(valid_time)),
            // The test must prove SG is excluded before any provider discovery.
            "sg" => None,
            _ => unreachable!("only registered fixture sources are supported"),
        };
        Box::pin(async move {
            record_event(&context, "fixture_discovery_log", b"discover\n")?;
            Ok(frames.into_iter().collect())
        })
    }

    fn fetch_raw(
        self: Arc<Self>,
        frame: FrameRef,
        context: SourceContext,
        temp_root: PathBuf,
    ) -> Option<BoxFuture<'static, CoreResult<RawFrame>>> {
        let source = self.source;
        Some(Box::pin(async move {
            record_fetch(&context)?;
            if fixture_option(&context, "fixture_fetch_failure") {
                return Err(CoreError::Transport("controlled fixture fetch failure".into()));
            }
            if source == "sg" {
                return Err(CoreError::Transport("categorical fixture is not fetchable".into()));
            }
            let artifacts: Vec<(String, &str, Vec<u8>)> = match source {
                "pt" => {
                    vec![("PTST2.png".into(), "image/png", fixture_bytes(&context, "PTST2.png")?)]
                }
                "rainviewer" if fixture_option(&context, "fixture_native_failure") => {
                    vec![(
                        "tile-z1-x0-y0.png".into(),
                        "image/png",
                        b"invalid fixture image".to_vec(),
                    )]
                }
                "rainviewer" => vec![
                    (
                        "tile-z1-x0-y0.png".into(),
                        "image/png",
                        fixture_bytes(&context, "tile-z1-x0-y0.png")?,
                    ),
                    (
                        "tile-z1-x1-y0.png".into(),
                        "image/png",
                        fixture_bytes(&context, "tile-z1-x1-y0.png")?,
                    ),
                    (
                        "tile-z1-x0-y1.png".into(),
                        "image/png",
                        fixture_bytes(&context, "tile-z1-x0-y1.png")?,
                    ),
                    (
                        "tile-z1-x1-y1.png".into(),
                        "image/png",
                        fixture_bytes(&context, "tile-z1-x1-y1.png")?,
                    ),
                ],
                _ => unreachable!("only registered fixture sources are supported"),
            };
            let mut raw_artifacts = Vec::with_capacity(artifacts.len());
            for (name, media_type, bytes) in artifacts {
                std::fs::create_dir_all(&temp_root)
                    .map_err(|_| CoreError::Temporary("fixture temp directory failed".into()))?;
                let mut file = tempfile::Builder::new()
                    .prefix("local-view-fixture-")
                    .tempfile_in(&temp_root)
                    .map_err(|_| CoreError::Temporary("fixture staging failed".into()))?;
                file.write_all(&bytes)
                    .and_then(|()| file.as_file().sync_all())
                    .map_err(|_| CoreError::Temporary("fixture staging failed".into()))?;
                raw_artifacts.push(RawArtifact {
                    receipt: ArtifactReceipt {
                        name,
                        media_type: media_type.to_owned(),
                        size_bytes: bytes.len() as u64,
                        sha256: hex::encode(Sha256::digest(&bytes)),
                    },
                    path: file.into_temp_path(),
                });
            }
            Ok(RawFrame { frame, artifacts: raw_artifacts, private_locator: None })
        }))
    }
}

fn fixture_option(context: &SourceContext, key: &str) -> bool {
    context.source_options.get(key).and_then(serde_yaml_ng::Value::as_bool).unwrap_or(false)
}

fn fixture_bytes(context: &SourceContext, name: &str) -> CoreResult<Vec<u8>> {
    let root = context
        .source_options
        .get("fixture_root")
        .and_then(serde_yaml_ng::Value::as_str)
        .ok_or_else(|| CoreError::Temporary("fixture root is not configured".into()))?;
    std::fs::read(PathBuf::from(root).join(name))
        .map_err(|_| CoreError::Temporary("fixture image could not be read".into()))
}

fn record_fetch(context: &SourceContext) -> CoreResult<()> {
    record_event(context, "fixture_fetch_log", b"fetch\n")
}

fn record_event(context: &SourceContext, option: &str, event: &[u8]) -> CoreResult<()> {
    let Some(path) = context.source_options.get(option).and_then(serde_yaml_ng::Value::as_str)
    else {
        return Ok(());
    };
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|_| CoreError::Temporary("fixture event log could not be opened".into()))?;
    file.write_all(event)
        .map_err(|_| CoreError::Temporary("fixture event log could not be written".into()))
}

fn pt_frame(valid_time: String) -> FrameRef {
    let mut frame = FrameRef {
        source: "pt".into(),
        product: "composite".into(),
        station: Some("PTST2".into()),
        valid_time,
        base_time: None,
        logical_id: String::new(),
        revision: Some("pcr_pst-2026-09-18T0250".into()),
        locator_version: "pt-legacy-v1".into(),
        locator: serde_json::json!({
            "url": "https://www.ipma.pt/resources.www/transf/radar/mad/pcr_pst-2026-09-18T0250.png",
            "artifacts": [],
            "station": "PTST2",
            "revision": "pcr_pst-2026-09-18T0250",
            "headers": {"Referer": "https://www.ipma.pt/pt/otempo/obs.remote/index-md.jsp"}
        }),
    };
    frame.logical_id = logical_id(&frame).expect("fixture frame identity is valid");
    frame
}

fn rainviewer_frame(valid_time: String) -> FrameRef {
    let mut frame = FrameRef {
        source: "rainviewer".into(),
        product: "composite".into(),
        station: None,
        valid_time,
        base_time: None,
        logical_id: String::new(),
        revision: Some("7cc4a10f8d53".into()),
        locator_version: "rainviewer-v2".into(),
        locator: serde_json::json!({
            "api_url": "https://api.rainviewer.com/public/weather-maps.json",
            "host": "https://tilecache.rainviewer.com",
            "path": "/v2/radar/7cc4a10f8d53",
            "tile_size": 512,
            "zoom": 1,
            "color": 2,
            "options": "0_0"
        }),
    };
    frame.logical_id = logical_id(&frame).expect("fixture frame identity is valid");
    frame
}
