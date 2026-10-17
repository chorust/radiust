//! Desktop session policy held in-process with the SwiftUI app.

use radiust_core::config::CoreConfig;
use radiust_core::engine::Engine;
use radiust_core::source::SourceRegistry;
use serde::Serialize;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Locked MVP region key used by Desktop defaults.
pub const MVP_REGION: &str = "jp";

/// Catalog source used for Japan MVP when no independent `jp` source ID exists.
pub const JP_DEFAULT_SOURCE_ID: &str = "rdcap";

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("engine configuration error")]
    EngineConfig,
    #[error("internal error: {0}")]
    Internal(String),
}

/// How the user chooses a viewing location.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocationMode {
    /// CoreLocation / system permission path (UI requests authorization).
    System,
    /// Country-level picker (MVP defaults to Japan).
    Country,
    /// Station name / ID picker aligned with catalog station IDs.
    Station,
}

impl LocationMode {
    pub fn parse(raw: &str) -> Result<Self, SessionError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "system" => Ok(Self::System),
            "country" => Ok(Self::Country),
            "station" => Ok(Self::Station),
            other => Err(SessionError::InvalidArgument(format!(
                "unknown location mode: {other}"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Country => "country",
            Self::Station => "station",
        }
    }
}

/// Region → default source / station policy (single-source; no mashup).
#[derive(Clone, Debug, Serialize)]
pub struct RegionPolicy {
    pub region: String,
    pub default_source_id: String,
    pub country_filter: String,
    pub notes: String,
}

impl RegionPolicy {
    pub fn japan_mvp() -> Self {
        Self {
            region: MVP_REGION.to_owned(),
            default_source_id: JP_DEFAULT_SOURCE_ID.to_owned(),
            country_filter: "JPN".to_owned(),
            notes: "Catalog has no standalone jp source ID; Desktop defaults to rdcap Japan stations. UI must show actual source id rdcap.".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct DesktopDefaults {
    pub allow_network: bool,
    pub shared_cache_with_cli: bool,
    pub process_model: &'static str,
    pub daemon: bool,
    pub arch: &'static str,
    pub distribution: &'static str,
}

impl DesktopDefaults {
    pub fn locked() -> Self {
        Self {
            allow_network: true,
            shared_cache_with_cli: true,
            process_model: "in_process",
            daemon: false,
            arch: "apple-silicon",
            distribution: "direct-download-notarized",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SessionSnapshot {
    pub region: String,
    pub default_source_id: String,
    pub country_filter: String,
    pub allow_network: bool,
    pub cache_dir: String,
    pub location_mode: LocationMode,
    pub selected_country: Option<String>,
    pub selected_station: Option<String>,
    pub process_model: &'static str,
    pub daemon: bool,
    pub shell: &'static str,
    pub notes: String,
}

/// In-process desktop session. Dropping this value ends Engine work with the app.
pub struct DesktopSession {
    engine: Engine,
    policy: RegionPolicy,
    location_mode: LocationMode,
    selected_country: Option<String>,
    selected_station: Option<String>,
}

impl DesktopSession {
    /// Open with locked Desktop defaults (network on, shared cache, jp policy).
    pub fn open_desktop_defaults(
        cache_dir_override: Option<String>,
    ) -> Result<Self, SessionError> {
        let mut config = CoreConfig::default();
        // Locked: Desktop installs with outbound allowed; UI discloses + can disable.
        config.runtime.allow_network = true;
        if let Some(path) = cache_dir_override {
            config.cache.dir = PathBuf::from(path);
        }
        // Otherwise keep CoreConfig default: ~/.cache/radiust-rust (CLI shared root).

        let engine = Engine::new(config, SourceRegistry::default())
            .map_err(|_| SessionError::EngineConfig)?;
        Ok(Self {
            engine,
            policy: RegionPolicy::japan_mvp(),
            location_mode: LocationMode::Country,
            selected_country: Some("jp".to_owned()),
            selected_station: None,
        })
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn allow_network(&self) -> bool {
        self.engine.config().runtime.allow_network
    }

    pub fn set_allow_network(&mut self, allow: bool) -> Result<(), SessionError> {
        // Engine stores Arc<CoreConfig>; rebuild with the same cache root.
        let mut config = self.engine.config().clone();
        config.runtime.allow_network = allow;
        self.engine = Engine::new(config, SourceRegistry::default())
            .map_err(|_| SessionError::EngineConfig)?;
        Ok(())
    }

    pub fn cache_dir(&self) -> &Path {
        &self.engine.config().cache.dir
    }

    pub fn default_source_id(&self) -> &str {
        &self.policy.default_source_id
    }

    pub fn set_location_mode(&mut self, mode: LocationMode) {
        self.location_mode = mode;
    }

    pub fn set_manual_selection(&mut self, country: Option<String>, station: Option<String>) {
        if let Some(ref c) = country {
            self.selected_country = Some(normalize_country_key(c));
        } else {
            self.selected_country = None;
        }
        self.selected_station = station.filter(|s| !s.is_empty());
        if self.selected_station.is_some() {
            self.location_mode = LocationMode::Station;
        } else if self.selected_country.is_some() && self.location_mode == LocationMode::System {
            // Keep system mode if user only cleared station.
        } else if self.selected_country.is_some() {
            self.location_mode = LocationMode::Country;
        }
    }

    pub fn snapshot(&self) -> SessionSnapshot {
        let defaults = DesktopDefaults::locked();
        SessionSnapshot {
            region: self.policy.region.clone(),
            default_source_id: self.policy.default_source_id.clone(),
            country_filter: self.policy.country_filter.clone(),
            allow_network: self.allow_network(),
            cache_dir: self.cache_dir().to_string_lossy().into_owned(),
            location_mode: self.location_mode,
            selected_country: self.selected_country.clone(),
            selected_station: self.selected_station.clone(),
            process_model: defaults.process_model,
            daemon: defaults.daemon,
            shell: "swiftui",
            notes: self.policy.notes.clone(),
        }
    }

    pub fn snapshot_json(&self) -> Result<String, SessionError> {
        serde_json::to_string(&self.snapshot())
            .map_err(|err| SessionError::Internal(err.to_string()))
    }
}

fn normalize_country_key(raw: &str) -> String {
    let trimmed = raw.trim();
    match trimmed.to_ascii_uppercase().as_str() {
        "JP" | "JPN" | "JAPAN" => "jp".to_owned(),
        other if other.eq_ignore_ascii_case("jp") => "jp".to_owned(),
        _ => trimmed.to_ascii_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn japan_policy_points_at_rdcap() {
        let policy = RegionPolicy::japan_mvp();
        assert_eq!(policy.region, "jp");
        assert_eq!(policy.default_source_id, "rdcap");
        assert_eq!(policy.country_filter, "JPN");
    }

    #[test]
    fn defaults_forbid_daemon() {
        let defaults = DesktopDefaults::locked();
        assert!(!defaults.daemon);
        assert_eq!(defaults.process_model, "in_process");
        assert!(defaults.allow_network);
        assert!(defaults.shared_cache_with_cli);
    }
}
