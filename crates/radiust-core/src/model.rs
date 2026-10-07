//! Shared, serialization-friendly domain types for the native core.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Query {
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub product: Option<String>,
    #[serde(default)]
    pub stations: Vec<String>,
    #[serde(default)]
    pub selector: TimeSelector,
    #[serde(default)]
    pub base_time: Option<String>,
    #[serde(default)]
    pub max_age_secs: Option<f64>,
}

impl Query {
    pub fn validate(&self, multi_source: bool) -> Result<(), ModelError> {
        if self.source.as_deref() == Some("all")
            && (!self.sources.is_empty() || self.product.is_some() || !self.stations.is_empty())
        {
            return Err(ModelError::InvalidQuery("all cannot be combined with source filters"));
        }
        if !self.sources.is_empty() {
            let mut unique = self.sources.clone();
            unique.sort();
            unique.dedup();
            if unique.len() != self.sources.len() {
                return Err(ModelError::InvalidQuery("source list contains duplicates"));
            }
            if self.sources.iter().any(|source| source == "all") {
                return Err(ModelError::InvalidQuery(
                    "all cannot be combined with explicit sources",
                ));
            }
        }
        let mut unique_stations = self.stations.clone();
        unique_stations.sort();
        unique_stations.dedup();
        if unique_stations.len() != self.stations.len() {
            return Err(ModelError::InvalidQuery("station list contains duplicates"));
        }
        if self.source.is_some() && !self.sources.is_empty() {
            return Err(ModelError::InvalidQuery("source and sources cannot be combined"));
        }
        if multi_source
            && (self.product.is_some() || !self.stations.is_empty() || self.base_time.is_some())
        {
            return Err(ModelError::InvalidQuery(
                "multi-source queries do not accept product, station, or base-time filters",
            ));
        }
        if multi_source && !matches!(self.selector, TimeSelector::Latest) {
            return Err(ModelError::InvalidQuery("multi-source queries accept latest only"));
        }
        if self.max_age_secs.is_some_and(|value| !value.is_finite() || value <= 0.0) {
            return Err(ModelError::InvalidQuery("max-age must be a finite positive number"));
        }
        if self.max_age_secs.is_some() && !matches!(self.selector, TimeSelector::Latest) {
            return Err(ModelError::InvalidQuery("max-age is only valid with latest"));
        }
        let has_rdcap_scope = self.source.as_deref() == Some("rdcap")
            || self.sources.iter().any(|source| source == "rdcap");
        if has_rdcap_scope
            && self
                .stations
                .iter()
                .any(|station| !crate::source::rdcap::is_valid_station_selection(station))
        {
            return Err(ModelError::InvalidQuery("RDCAP station identifier is invalid"));
        }
        if let Some(base_time) = self.base_time.as_deref() {
            parse_utc_time(base_time)?;
        }
        match &self.selector {
            TimeSelector::Latest => {}
            TimeSelector::At { time } => {
                parse_utc_time(time)?;
            }
            TimeSelector::Range { start, end } => {
                if start.trim().is_empty() || end.trim().is_empty() {
                    return Err(ModelError::InvalidQuery("time range requires both start and end"));
                }
                let start_utc = parse_utc_time(start)?;
                let end_utc = parse_utc_time(end)?;
                if start_utc >= end_utc {
                    return Err(ModelError::InvalidQuery("time range start must precede end"));
                }
            }
        }
        Ok(())
    }
}

pub fn parse_utc_time(value: &str) -> Result<DateTime<Utc>, ModelError> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| ModelError::InvalidQuery("time must be ISO-8601 with a timezone"))
}

impl Default for Query {
    fn default() -> Self {
        Self {
            source: None,
            sources: Vec::new(),
            product: None,
            stations: Vec::new(),
            selector: TimeSelector::Latest,
            base_time: None,
            max_age_secs: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TimeSelector {
    Latest,
    At { time: String },
    Range { start: String, end: String },
}

impl Default for TimeSelector {
    fn default() -> Self {
        Self::Latest
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct DiscoveryTarget {
    pub source: String,
    pub product: Option<String>,
    pub station: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FrameRef {
    pub source: String,
    pub product: String,
    pub station: Option<String>,
    pub valid_time: String,
    #[serde(default)]
    pub base_time: Option<String>,
    pub logical_id: String,
    #[serde(default)]
    pub revision: Option<String>,
    #[serde(default = "default_locator_version")]
    pub locator_version: String,
    /// Provider locator is runtime-only and never appears in JSON reports.
    #[serde(default, skip_serializing)]
    pub locator: serde_json::Value,
}

impl FrameRef {
    pub fn validate_identity(&self) -> Result<(), ModelError> {
        if !valid_identifier(&self.source)
            || !valid_identifier(&self.product)
            || self.station.as_deref().is_some_and(|value| {
                if self.source == "rdcap" {
                    !crate::source::rdcap::is_valid_station_id(value)
                } else {
                    !valid_identifier(value)
                }
            })
            || !valid_identifier(&self.locator_version)
        {
            return Err(ModelError::InvalidFrame("frame identifiers are invalid"));
        }
        parse_utc_time(&self.valid_time)
            .map_err(|_| ModelError::InvalidFrame("valid_time must be ISO-8601 with a timezone"))?;
        if let Some(base_time) = self.base_time.as_deref() {
            parse_utc_time(base_time).map_err(|_| {
                ModelError::InvalidFrame("base_time must be ISO-8601 with a timezone")
            })?;
        }
        let expected = crate::identity::logical_id(self)
            .map_err(|_| ModelError::InvalidFrame("frame identity cannot be calculated"))?;
        if self.logical_id != expected {
            return Err(ModelError::InvalidFrame("logical_id does not match frame identity"));
        }
        Ok(())
    }
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && !matches!(value, "." | "..")
        && !value.chars().any(|character| matches!(character, '/' | '\\' | '\0'))
}

fn default_locator_version() -> String {
    "1".to_owned()
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ArtifactReceipt {
    pub name: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub sha256: String,
}

/// Secret or provider-specific locator data is intentionally excluded from
/// serde output. It is held only for the acquisition lifetime.
#[derive(Debug)]
pub struct RawArtifact {
    pub receipt: ArtifactReceipt,
    /// Temporary, automatically removed when the fetched frame is dropped.
    pub path: tempfile::TempPath,
}

#[derive(Debug)]
pub struct RawFrame {
    pub frame: FrameRef,
    pub artifacts: Vec<RawArtifact>,
    pub private_locator: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RawFrameReceipt {
    pub frame: FrameRef,
    pub artifacts: Vec<ArtifactReceipt>,
}

impl RawFrame {
    pub fn public_receipt(&self) -> RawFrameReceipt {
        RawFrameReceipt {
            frame: self.frame.clone(),
            artifacts: self.artifacts.iter().map(|artifact| artifact.receipt.clone()).collect(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Grid {
    pub shape: Vec<usize>,
    pub crs: Option<String>,
    pub x: Vec<f64>,
    pub y: Vec<f64>,
    pub affine: Option<[f64; 6]>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RadarField {
    pub name: String,
    pub values: Vec<f32>,
    pub shape: Vec<usize>,
    pub quality: Vec<u16>,
    pub units: Option<String>,
    pub valid_time: String,
    pub grid: Grid,
    pub provenance: Vec<String>,
}

impl RadarField {
    pub fn validate(&self) -> Result<(), ModelError> {
        if self.name.is_empty() {
            return Err(ModelError::InvalidField("field name must not be empty"));
        }
        if self.shape.is_empty() || self.shape.contains(&0) {
            return Err(ModelError::InvalidField("field dimensions must be positive"));
        }
        parse_utc_time(&self.valid_time)
            .map_err(|_| ModelError::InvalidField("valid_time must be ISO-8601 with a timezone"))?;
        let elements = self
            .shape
            .iter()
            .try_fold(1_usize, |n, dim| n.checked_mul(*dim))
            .ok_or(ModelError::InvalidField("shape overflows address space"))?;
        if elements != self.values.len() || elements != self.quality.len() {
            return Err(ModelError::InvalidField(
                "values and quality must match the declared shape",
            ));
        }
        if self
            .values
            .iter()
            .zip(&self.quality)
            .any(|(value, quality)| value.is_infinite() || (value.is_nan() && *quality == 0))
        {
            return Err(ModelError::InvalidField(
                "non-finite values require a nonzero quality flag; infinity is invalid",
            ));
        }
        if self.grid.shape != self.shape {
            return Err(ModelError::InvalidField("field and grid shapes differ"));
        }
        let x_size = *self.shape.last().expect("non-empty shape checked");
        let y_size = (self.shape.len() >= 2).then(|| self.shape[self.shape.len() - 2]);
        if (!self.grid.x.is_empty() && self.grid.x.len() != x_size)
            || (!self.grid.y.is_empty() && y_size != Some(self.grid.y.len()))
        {
            return Err(ModelError::InvalidField("grid coordinates do not match field shape"));
        }
        if self.grid.x.iter().chain(&self.grid.y).any(|value| !value.is_finite())
            || self.grid.affine.is_some_and(|values| values.iter().any(|value| !value.is_finite()))
        {
            return Err(ModelError::InvalidField("grid coordinates must be finite"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RadarDataset {
    pub fields: Vec<RadarField>,
    pub valid_time: String,
    pub source: String,
}

impl RadarDataset {
    pub fn validate(&self) -> Result<(), ModelError> {
        if self.fields.is_empty() || !valid_identifier(&self.source) {
            return Err(ModelError::InvalidDataset(
                "dataset requires a source and at least one field",
            ));
        }
        let valid_time = parse_utc_time(&self.valid_time).map_err(|_| {
            ModelError::InvalidDataset("valid_time must be ISO-8601 with a timezone")
        })?;
        let mut names = BTreeSet::new();
        let grid = &self.fields[0].grid;
        for field in &self.fields {
            field.validate()?;
            if !names.insert(&field.name) {
                return Err(ModelError::InvalidDataset("field names must be unique"));
            }
            if &field.grid != grid {
                return Err(ModelError::InvalidDataset("dataset fields must share one grid"));
            }
            let field_time = parse_utc_time(&field.valid_time)
                .map_err(|_| ModelError::InvalidDataset("field valid_time is invalid"))?;
            if field_time != valid_time {
                return Err(ModelError::InvalidDataset("dataset fields must share valid_time"));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Preview {
    pub width: u32,
    pub height: u32,
    /// RGBA byte order; length must equal width * height * 4.
    pub rgba: Vec<u8>,
    pub frame: Option<FrameRef>,
    pub mode: PreviewMode,
    pub rule_version: Option<String>,
}

impl Preview {
    pub fn validate(&self) -> Result<(), ModelError> {
        if self.width == 0 || self.height == 0 {
            return Err(ModelError::InvalidPreview("pixel dimensions must be positive"));
        }
        let expected = (self.width as usize)
            .checked_mul(self.height as usize)
            .and_then(|n| n.checked_mul(4))
            .ok_or(ModelError::InvalidPreview("pixel dimensions overflow"))?;
        if self.rgba.len() != expected {
            return Err(ModelError::InvalidPreview("RGBA length does not match dimensions"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewMode {
    Raw,
    Gray,
    LegacyDisplay,
    Decoded,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryStatus {
    Pending,
    Running,
    Success,
    NoData,
    Stale,
    MissingCredentials,
    Retired,
    NetworkRestricted,
    UpstreamFailed,
    Ambiguous,
    Timeout,
    Cancelled,
    NotStarted,
}

impl DiscoveryStatus {
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Pending | Self::Running)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SafeError {
    pub code: String,
    pub message: String,
    pub stage: String,
    pub retryable: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DiscoveryItem {
    pub target: DiscoveryTarget,
    pub status: DiscoveryStatus,
    pub valid_time: Option<String>,
    pub frame: Option<FrameRef>,
    pub error: Option<SafeError>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DiscoveryCounts {
    pub total: usize,
    pub success: usize,
    pub no_data: usize,
    pub stale: usize,
    pub missing_credentials: usize,
    pub retired: usize,
    pub network_restricted: usize,
    pub upstream_failed: usize,
    pub ambiguous: usize,
    pub timeout: usize,
    pub cancelled: usize,
    pub not_started: usize,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DiscoveryReport {
    pub schema_version: u8,
    pub query: Query,
    pub counts: DiscoveryCounts,
    pub items: Vec<DiscoveryItem>,
    pub interrupted: bool,
}

impl DiscoveryReport {
    pub fn from_items(
        query: Query,
        mut items: Vec<DiscoveryItem>,
        interrupted: bool,
    ) -> Result<Self, ModelError> {
        let mut targets = BTreeSet::new();
        let mut range_frames = BTreeSet::new();
        let mut non_frame_targets = BTreeSet::new();
        // A range may contain several distinct frames for one station/product.
        // Other selectors and terminal failures still require one item per target.
        let is_range = matches!(query.selector, TimeSelector::Range { .. });
        for item in &items {
            if !item.status.is_terminal() {
                return Err(ModelError::InvalidReport("every item must have a terminal status"));
            }
            let frame_key = if is_range && item.status == DiscoveryStatus::Success {
                item.frame.as_ref().map(|frame| (frame.logical_id.clone(), frame.revision.clone()))
            } else {
                None
            };
            if frame_key.is_none() {
                non_frame_targets.insert(item.target.clone());
            }
            let first_target = targets.insert(item.target.clone());
            let distinct_frame =
                frame_key.is_some_and(|key| range_frames.insert((item.target.clone(), key)));
            if !first_target && (!distinct_frame || non_frame_targets.contains(&item.target)) {
                return Err(ModelError::InvalidReport("each discovery target must appear once"));
            }
            if item.status == DiscoveryStatus::Success {
                let frame = item
                    .frame
                    .as_ref()
                    .ok_or(ModelError::InvalidReport("successful items require a frame"))?;
                frame.validate_identity().map_err(|_| {
                    ModelError::InvalidReport("successful frame identity is invalid")
                })?;
                if frame.source != item.target.source
                    || item.target.product.as_ref().is_some_and(|value| value != &frame.product)
                    || item
                        .target
                        .station
                        .as_ref()
                        .is_some_and(|value| frame.station.as_ref() != Some(value))
                {
                    return Err(ModelError::InvalidReport(
                        "successful frame does not match its target",
                    ));
                }
                let frame_time = parse_utc_time(&frame.valid_time)
                    .map_err(|_| ModelError::InvalidReport("successful frame time is invalid"))?;
                let report_time = item
                    .valid_time
                    .as_deref()
                    .ok_or(ModelError::InvalidReport("successful items require valid_time"))?;
                if parse_utc_time(report_time)
                    .map_err(|_| ModelError::InvalidReport("successful item time is invalid"))?
                    != frame_time
                {
                    return Err(ModelError::InvalidReport("item and frame times differ"));
                }
            }
        }
        items.sort_by(|a, b| a.target.cmp(&b.target));
        let mut counts = DiscoveryCounts { total: items.len(), ..DiscoveryCounts::default() };
        for item in &items {
            match item.status {
                DiscoveryStatus::Success => counts.success += 1,
                DiscoveryStatus::NoData => counts.no_data += 1,
                DiscoveryStatus::Stale => counts.stale += 1,
                DiscoveryStatus::MissingCredentials => counts.missing_credentials += 1,
                DiscoveryStatus::Retired => counts.retired += 1,
                DiscoveryStatus::NetworkRestricted => counts.network_restricted += 1,
                DiscoveryStatus::UpstreamFailed => counts.upstream_failed += 1,
                DiscoveryStatus::Ambiguous => counts.ambiguous += 1,
                DiscoveryStatus::Timeout => counts.timeout += 1,
                DiscoveryStatus::Cancelled => counts.cancelled += 1,
                DiscoveryStatus::NotStarted => counts.not_started += 1,
                DiscoveryStatus::Pending | DiscoveryStatus::Running => {}
            }
        }
        Ok(Self { schema_version: 1, query, counts, items, interrupted })
    }

    /// Serialize a public report without exposing the transport locator kept
    /// on each selected frame for in-process follow-up operations.
    pub fn safe_document(&self) -> serde_json::Value {
        let items = self
            .items
            .iter()
            .map(|item| {
                let frame = item.frame.as_ref().map(|frame| {
                    serde_json::json!({
                        "source": frame.source,
                        "product": frame.product,
                        "station": frame.station,
                        "valid_time": frame.valid_time,
                        "base_time": frame.base_time,
                    })
                });
                let error = item.error.as_ref().map(|error| {
                    serde_json::json!({
                        "code": error.code,
                        "message": crate::safety::safe_text(&error.message),
                        "stage": error.stage,
                        "retryable": error.retryable,
                    })
                });
                serde_json::json!({
                    "target": item.target,
                    "status": item.status,
                    "valid_time": item.valid_time,
                    "frame": frame,
                    "error": error,
                })
            })
            .collect::<Vec<_>>();
        serde_json::json!({
            "schema_version": self.schema_version,
            "query": self.query,
            "counts": self.counts,
            "items": items,
            "interrupted": self.interrupted,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ModelError {
    #[error("invalid query: {0}")]
    InvalidQuery(&'static str),
    #[error("invalid field: {0}")]
    InvalidField(&'static str),
    #[error("invalid frame: {0}")]
    InvalidFrame(&'static str),
    #[error("invalid dataset: {0}")]
    InvalidDataset(&'static str),
    #[error("invalid preview: {0}")]
    InvalidPreview(&'static str),
    #[error("invalid report: {0}")]
    InvalidReport(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_frame() -> FrameRef {
        let mut frame = FrameRef {
            source: "au".into(),
            product: "composite".into(),
            station: Some("east".into()),
            valid_time: "2026-09-24T00:00:00Z".into(),
            base_time: None,
            logical_id: String::new(),
            revision: Some("fixture-revision".into()),
            locator_version: "fixture-v1".into(),
            locator: serde_json::Value::Null,
        };
        frame.logical_id = crate::identity::logical_id(&frame).unwrap();
        frame
    }

    fn valid_field(name: &str, valid_time: &str) -> RadarField {
        RadarField {
            name: name.into(),
            values: vec![f32::NAN, 2.0, 3.0, 4.0],
            shape: vec![2, 2],
            quality: vec![1, 1, 2, 3],
            units: Some("dBZ".into()),
            valid_time: valid_time.into(),
            grid: Grid {
                shape: vec![2, 2],
                crs: Some("EPSG:4326".into()),
                x: vec![100.0, 101.0],
                y: vec![20.0, 21.0],
                affine: None,
            },
            provenance: vec!["fixture".into()],
        }
    }

    #[test]
    fn query_rejects_duplicate_sources_before_dispatch() {
        let query =
            Query { source: None, sources: vec!["au".into(), "au".into()], ..Query::default() };
        assert!(matches!(query.validate(true), Err(ModelError::InvalidQuery(_))));
    }

    #[test]
    fn query_rejects_duplicate_stations_and_invalid_latest_age_combinations() {
        let duplicate_stations = Query {
            source: Some("au".into()),
            stations: vec!["east".into(), "east".into()],
            ..Query::default()
        };
        assert!(matches!(
            duplicate_stations.validate(false),
            Err(ModelError::InvalidQuery("station list contains duplicates"))
        ));

        let age_on_range = Query {
            source: Some("au".into()),
            selector: TimeSelector::Range {
                start: "2026-01-02T00:00:00Z".into(),
                end: "2026-01-01T00:00:00Z".into(),
            },
            max_age_secs: Some(30.0),
            ..Query::default()
        };
        assert!(matches!(age_on_range.validate(false), Err(ModelError::InvalidQuery(_))));
    }

    #[test]
    fn rdcap_query_accepts_canonical_or_short_station_codes_and_rejects_bad_paths() {
        for station in ["TWRCHL", "RCHL"] {
            let query = Query {
                source: Some("rdcap".into()),
                stations: vec![station.into()],
                ..Query::default()
            };
            assert!(query.validate(false).is_ok(), "{station}");
        }
        let invalid = Query {
            source: Some("rdcap".into()),
            stations: vec!["TWN/../RCHL".into()],
            ..Query::default()
        };
        assert!(matches!(invalid.validate(false), Err(ModelError::InvalidQuery(_))));
    }

    #[test]
    fn field_shape_and_quality_must_match() {
        let field = RadarField {
            name: "reflectivity".into(),
            values: vec![1.0, 2.0],
            shape: vec![1, 2],
            quality: vec![0],
            units: Some("dBZ".into()),
            valid_time: "2026-09-24T00:00:00Z".into(),
            grid: Grid { shape: vec![1, 2], crs: None, x: vec![], y: vec![], affine: None },
            provenance: vec![],
        };
        assert!(matches!(field.validate(), Err(ModelError::InvalidField(_))));
    }

    #[test]
    fn private_locator_is_never_serialized() {
        let frame = FrameRef {
            source: "au".into(),
            product: "composite".into(),
            station: None,
            valid_time: "2026-09-24T00:00:00Z".into(),
            base_time: None,
            logical_id: "frame".into(),
            revision: None,
            locator_version: "1".into(),
            locator: serde_json::Value::String("https://example.invalid/?token=secret".into()),
        };
        let raw = RawFrame {
            frame,
            artifacts: vec![],
            private_locator: Some("https://example.invalid/?token=secret".into()),
        };
        let serialized = serde_json::to_string(&raw.public_receipt()).unwrap();
        assert!(!serialized.contains("secret"));
        assert!(!serialized.contains("private_locator"));
    }

    #[test]
    fn report_counts_every_terminal_item_once() {
        let target = DiscoveryTarget {
            source: "au".into(),
            product: Some("composite".into()),
            station: None,
        };
        let report = DiscoveryReport::from_items(
            Query::default(),
            vec![DiscoveryItem {
                target,
                status: DiscoveryStatus::NetworkRestricted,
                valid_time: None,
                frame: None,
                error: None,
            }],
            false,
        )
        .unwrap();
        assert_eq!(report.counts.total, 1);
        assert_eq!(report.counts.network_restricted, 1);
    }

    #[test]
    fn frame_identity_validation_checks_utc_and_canonical_identity() {
        let frame = valid_frame();
        assert!(frame.validate_identity().is_ok());

        let mut invalid = frame;
        invalid.valid_time = "2026-09-24T00:00:00".into();
        assert!(matches!(invalid.validate_identity(), Err(ModelError::InvalidFrame(_))));
    }

    #[test]
    fn rdcap_frame_station_grammar_is_source_specific() {
        let mut rdcap = valid_frame();
        rdcap.source = "rdcap".into();
        rdcap.product = "reflectivity".into();
        rdcap.station = Some("TWRCHL".into());
        rdcap.logical_id = crate::identity::logical_id(&rdcap).unwrap();
        assert!(rdcap.validate_identity().is_ok());

        let mut invalid_rdcap = rdcap.clone();
        invalid_rdcap.station = Some("TWN/RCH/L".into());
        invalid_rdcap.logical_id = crate::identity::logical_id(&invalid_rdcap).unwrap();
        assert!(matches!(invalid_rdcap.validate_identity(), Err(ModelError::InvalidFrame(_))));

        let mut other_source = valid_frame();
        other_source.station = Some("TWN/RCHL".into());
        other_source.logical_id = crate::identity::logical_id(&other_source).unwrap();
        assert!(matches!(other_source.validate_identity(), Err(ModelError::InvalidFrame(_))));
    }

    #[test]
    fn radar_dataset_requires_unique_fields_with_a_shared_grid_and_time() {
        let valid_time = "2026-09-24T00:00:00Z";
        let dataset = RadarDataset {
            fields: vec![
                valid_field("reflectivity", valid_time),
                valid_field("rain_rate", valid_time),
            ],
            valid_time: valid_time.into(),
            source: "fixture".into(),
        };
        assert!(dataset.validate().is_ok());

        let mut mismatched_time = dataset.clone();
        mismatched_time.fields[1].valid_time = "2026-09-24T00:05:00Z".into();
        assert!(matches!(mismatched_time.validate(), Err(ModelError::InvalidDataset(_))));

        let mut mismatched_grid = dataset.clone();
        mismatched_grid.fields[1].grid.x[0] = 99.0;
        assert!(matches!(mismatched_grid.validate(), Err(ModelError::InvalidDataset(_))));

        let mut duplicate_name = dataset;
        duplicate_name.fields[1].name = "reflectivity".into();
        assert!(matches!(duplicate_name.validate(), Err(ModelError::InvalidDataset(_))));
    }

    #[test]
    fn radar_field_requires_quality_flags_for_missing_values_and_rejects_infinity() {
        let mut unmarked_missing = valid_field("reflectivity", "2026-09-24T00:00:00Z");
        unmarked_missing.values[0] = f32::NAN;
        unmarked_missing.quality[0] = 0;
        assert!(matches!(unmarked_missing.validate(), Err(ModelError::InvalidField(_))));

        let mut marked_missing = valid_field("reflectivity", "2026-09-24T00:00:00Z");
        marked_missing.values[0] = f32::NAN;
        marked_missing.quality[0] = 1;
        assert!(marked_missing.validate().is_ok());

        let mut infinite = valid_field("reflectivity", "2026-09-24T00:00:00Z");
        infinite.values[0] = f32::INFINITY;
        infinite.quality[0] = 1;
        assert!(matches!(infinite.validate(), Err(ModelError::InvalidField(_))));
    }

    #[test]
    fn successful_discovery_report_requires_one_matching_valid_frame_per_target() {
        let frame = valid_frame();
        let target = DiscoveryTarget {
            source: frame.source.clone(),
            product: Some(frame.product.clone()),
            station: frame.station.clone(),
        };
        let item = DiscoveryItem {
            target: target.clone(),
            status: DiscoveryStatus::Success,
            valid_time: Some(frame.valid_time.clone()),
            frame: Some(frame.clone()),
            error: None,
        };
        assert!(DiscoveryReport::from_items(Query::default(), vec![item.clone()], false).is_ok());

        assert!(matches!(
            DiscoveryReport::from_items(Query::default(), vec![item.clone(), item.clone()], false),
            Err(ModelError::InvalidReport("each discovery target must appear once"))
        ));

        let mut invalid = item;
        invalid.frame.as_mut().unwrap().logical_id = "wrong-id".into();
        assert!(matches!(
            DiscoveryReport::from_items(Query::default(), vec![invalid], false),
            Err(ModelError::InvalidReport("successful frame identity is invalid"))
        ));
    }
}
