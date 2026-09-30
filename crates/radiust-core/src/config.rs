//! Typed configuration with deterministic defaults, layering, and redaction.

use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_yaml_ng::{Mapping, Value};
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoreConfig {
    #[serde(default)]
    pub runtime: RuntimeConfig,
    #[serde(default)]
    pub cache: CacheConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub output: OutputConfig,
    #[serde(default)]
    pub sources: BTreeMap<String, BTreeMap<String, Value>>,
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            runtime: RuntimeConfig::default(),
            cache: CacheConfig::default(),
            storage: StorageConfig::default(),
            output: OutputConfig::default(),
            sources: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfig {
    pub frame_concurrency: usize,
    pub request_concurrency: usize,
    pub host_concurrency: usize,
    pub decode_workers: usize,
    pub discovery_workers: usize,
    pub request_timeout: f64,
    pub frame_deadline: f64,
    pub discovery_deadline: f64,
    pub max_artifact_bytes: u64,
    pub max_frame_bytes: u64,
    pub max_pixels: u64,
    pub max_temp_bytes: u64,
    pub allow_network: bool,
    pub temp_root: Option<PathBuf>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            frame_concurrency: 2,
            request_concurrency: 16,
            host_concurrency: 4,
            decode_workers: 2,
            discovery_workers: 4,
            request_timeout: 30.0,
            frame_deadline: 300.0,
            discovery_deadline: 300.0,
            max_artifact_bytes: 512 * 1024 * 1024,
            max_frame_bytes: 2 * 1024 * 1024 * 1024,
            max_pixels: 100_000_000,
            max_temp_bytes: 10 * 1024 * 1024 * 1024,
            allow_network: false,
            temp_root: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CacheConfig {
    pub dir: PathBuf,
    pub max_bytes: u64,
    pub max_age_days: u64,
    pub gc_interval_hours: u64,
    pub enabled: bool,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            dir: default_cache_dir(),
            max_bytes: 20_000_000_000,
            max_age_days: 30,
            gc_interval_hours: 24,
            enabled: true,
        }
    }
}

fn default_cache_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        // Keep the native index separate until the Python `entries` layout is
        // explicitly migrated and cross-version repair semantics are proven.
        .join(".cache/radiust-rust")
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StorageConfig {
    pub output: PathBuf,
    pub endpoint: Option<String>,
    pub region: Option<String>,
    pub access_key: Option<String>,
    pub secret_key: Option<String>,
    pub anonymous: bool,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            output: PathBuf::from("./data"),
            endpoint: None,
            region: None,
            access_key: None,
            secret_key: None,
            anonymous: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OutputConfig {
    pub format: String,
    pub grid: String,
    pub resampling: String,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self { format: "netcdf".into(), grid: "native".into(), resampling: "nearest".into() }
    }
}

impl CoreConfig {
    /// Merge defaults < YAML file < environment < explicit YAML/CLI overrides.
    pub fn layered(
        yaml_file: Option<&str>,
        environment: &BTreeMap<String, String>,
        explicit_overrides: Option<&str>,
    ) -> Result<Self, ConfigError> {
        Self::layered_with_origins(yaml_file, environment, explicit_overrides)
            .map(|(config, _)| config)
    }

    /// Resolve a configuration and retain the source of every supplied value.
    pub fn layered_with_origins(
        yaml_file: Option<&str>,
        environment: &BTreeMap<String, String>,
        explicit_overrides: Option<&str>,
    ) -> Result<(Self, BTreeMap<String, String>), ConfigError> {
        let environment =
            environment.iter().map(|(key, value)| (key.clone(), value.clone())).collect::<Vec<_>>();
        Self::layered_with_ordered_environment(yaml_file, &environment, explicit_overrides)
    }

    /// Ordered environment input preserves Python mapping overwrite behavior
    /// when differently cased variable names normalize to the same path.
    pub fn layered_with_ordered_environment(
        yaml_file: Option<&str>,
        environment: &[(String, String)],
        explicit_overrides: Option<&str>,
    ) -> Result<(Self, BTreeMap<String, String>), ConfigError> {
        let files = yaml_file.map(|input| (input, "file")).into_iter().collect::<Vec<_>>();
        Self::layered_files_with_ordered_environment(&files, environment, explicit_overrides)
    }

    fn layered_files_with_ordered_environment(
        yaml_files: &[(&str, &str)],
        environment: &[(String, String)],
        explicit_overrides: Option<&str>,
    ) -> Result<(Self, BTreeMap<String, String>), ConfigError> {
        let mut value =
            serde_yaml_ng::to_value(Self::default()).map_err(|_| ConfigError::InvalidValue)?;
        let mut origins = BTreeMap::from([
            ("runtime".to_owned(), "default".to_owned()),
            ("cache".to_owned(), "default".to_owned()),
            ("storage".to_owned(), "default".to_owned()),
            ("output".to_owned(), "default".to_owned()),
            ("sources".to_owned(), "default".to_owned()),
        ]);
        for &(input, origin) in yaml_files {
            let patch = parse_yaml(input)?;
            validate_config_keys(&patch)?;
            record_origins(&mut origins, &patch, origin, "");
            merge(&mut value, patch);
        }
        let patch = environment_patch(environment)?;
        validate_config_keys(&patch)?;
        record_origins(&mut origins, &patch, "environment", "");
        merge(&mut value, patch);
        if let Some(input) = explicit_overrides {
            let patch = parse_yaml(input)?;
            validate_config_keys(&patch)?;
            record_origins(&mut origins, &patch, "explicit", "");
            merge(&mut value, patch);
        }
        let result: Self =
            serde_yaml_ng::from_value(value).map_err(|_| ConfigError::InvalidValue)?;
        result.validate()?;
        Ok((result, origins))
    }

    /// Load defaults < user YAML < project (or explicit path) YAML < environment
    /// < explicit SDK overrides. Missing automatic files are optional.
    pub fn load(
        path: Option<&Path>,
        environment: &BTreeMap<String, String>,
        explicit_overrides: Option<&str>,
    ) -> Result<Self, ConfigError> {
        let environment =
            environment.iter().map(|(key, value)| (key.clone(), value.clone())).collect::<Vec<_>>();
        Self::load_with_ordered_environment(path, &environment, explicit_overrides)
            .map(|(config, _)| config)
    }

    pub fn load_with_ordered_environment(
        path: Option<&Path>,
        environment: &[(String, String)],
        explicit_overrides: Option<&str>,
    ) -> Result<(Self, BTreeMap<String, String>), ConfigError> {
        let home = environment
            .iter()
            .rev()
            .find(|(key, _)| key == "HOME")
            .map(|(_, value)| PathBuf::from(value))
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .filter(|path| !path.as_os_str().is_empty());
        let project_dir = std::env::current_dir().map_err(|_| ConfigError::InvalidPath)?;
        Self::load_from_locations(
            home.as_deref(),
            &project_dir,
            path,
            environment,
            explicit_overrides,
        )
    }

    fn load_from_locations(
        home: Option<&Path>,
        project_dir: &Path,
        path: Option<&Path>,
        environment: &[(String, String)],
        explicit_overrides: Option<&str>,
    ) -> Result<(Self, BTreeMap<String, String>), ConfigError> {
        let mut files = Vec::new();
        if let Some(home) = home {
            if let Some(input) = read_optional_config(&home.join(".config/radiust/config.yaml"))? {
                files.push((input, "user_file"));
            }
        }
        if let Some(path) = path {
            let input = fs::read_to_string(path).map_err(|_| ConfigError::ConfigFileUnavailable)?;
            files.push((input, "file"));
        } else if let Some(input) = read_optional_config(&project_dir.join("config.yaml"))? {
            files.push((input, "file"));
        }
        let files =
            files.iter().map(|(input, origin)| (input.as_str(), *origin)).collect::<Vec<_>>();
        Self::layered_files_with_ordered_environment(&files, environment, explicit_overrides)
    }

    pub fn from_file(
        path: &Path,
        environment: &BTreeMap<String, String>,
        explicit_overrides: Option<&str>,
    ) -> Result<Self, ConfigError> {
        Self::from_file_with_origins(path, environment, explicit_overrides)
            .map(|(config, _)| config)
    }

    pub fn from_file_with_origins(
        path: &Path,
        environment: &BTreeMap<String, String>,
        explicit_overrides: Option<&str>,
    ) -> Result<(Self, BTreeMap<String, String>), ConfigError> {
        let input = fs::read_to_string(path).map_err(|_| ConfigError::ConfigFileUnavailable)?;
        Self::layered_with_origins(Some(&input), environment, explicit_overrides)
    }

    pub fn from_file_with_ordered_environment(
        path: &Path,
        environment: &[(String, String)],
        explicit_overrides: Option<&str>,
    ) -> Result<(Self, BTreeMap<String, String>), ConfigError> {
        let input = fs::read_to_string(path).map_err(|_| ConfigError::ConfigFileUnavailable)?;
        Self::layered_with_ordered_environment(Some(&input), environment, explicit_overrides)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        let runtime = &self.runtime;
        if [
            runtime.frame_concurrency,
            runtime.request_concurrency,
            runtime.host_concurrency,
            runtime.decode_workers,
            runtime.discovery_workers,
        ]
        .contains(&0)
        {
            return Err(ConfigError::InvalidLimit);
        }
        if [runtime.request_timeout, runtime.frame_deadline, runtime.discovery_deadline].iter().any(
            |value| {
                !value.is_finite()
                    || *value <= 0.0
                    || std::time::Duration::try_from_secs_f64(*value).is_err()
            },
        ) {
            return Err(ConfigError::InvalidDeadline);
        }
        if runtime.max_artifact_bytes == 0
            || runtime.max_frame_bytes < runtime.max_artifact_bytes
            || runtime.max_pixels == 0
            || runtime.max_temp_bytes == 0
        {
            return Err(ConfigError::InvalidLimit);
        }
        if self.cache.max_age_days == 0 || self.cache.gc_interval_hours == 0 {
            return Err(ConfigError::InvalidLimit);
        }
        if self.storage.access_key.as_deref().is_some_and(|value| !value.is_empty())
            != self.storage.secret_key.as_deref().is_some_and(|value| !value.is_empty())
        {
            return Err(ConfigError::IncompleteCredentials);
        }
        for values in self.sources.values() {
            let has_access = values.get("access_key").is_some_and(is_truthy);
            let has_secret = values.get("secret_key").is_some_and(is_truthy);
            if has_access != has_secret {
                return Err(ConfigError::IncompleteSourceCredentials);
            }
        }
        if !matches!(self.output.format.as_str(), "netcdf" | "geotiff" | "png" | "zarr") {
            return Err(ConfigError::InvalidOutput);
        }
        if !matches!(self.output.grid.as_str(), "native" | "geographic") {
            return Err(ConfigError::InvalidOutput);
        }
        if !matches!(self.output.resampling.as_str(), "nearest" | "bilinear") {
            return Err(ConfigError::InvalidOutput);
        }
        Ok(())
    }

    pub fn redacted(&self) -> Self {
        let mut safe = self.clone();
        if safe.storage.access_key.as_deref().is_some_and(|value| !value.is_empty()) {
            safe.storage.access_key = Some("<configured>".into());
        }
        if safe.storage.secret_key.as_deref().is_some_and(|value| !value.is_empty()) {
            safe.storage.secret_key = Some("<configured>".into());
        }
        for values in safe.sources.values_mut() {
            redact_map(values);
        }
        safe
    }

    /// Match the Python SDK's absolute path values and reject overlapping
    /// cache/output roots before the Engine is constructed.
    pub fn normalize_sdk_paths(&mut self) -> Result<(), ConfigError> {
        if !is_remote_output(&self.storage.output) {
            self.storage.output = resolve_user_path(&self.storage.output)?;
        }
        self.cache.dir = resolve_user_path(&self.cache.dir)?;
        if let Some(temp_root) = &self.runtime.temp_root {
            self.runtime.temp_root = Some(resolve_user_path(temp_root)?);
        }
        if !is_remote_output(&self.storage.output)
            && paths_overlap(&self.storage.output, &self.cache.dir)
        {
            return Err(ConfigError::OverlappingPaths);
        }
        Ok(())
    }
}

fn is_remote_output(path: &Path) -> bool {
    path.to_string_lossy().to_ascii_lowercase().starts_with("s3://")
        || path.to_string_lossy().to_ascii_lowercase().starts_with("oss://")
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

fn resolve_user_path(path: &Path) -> Result<PathBuf, ConfigError> {
    let text = path.to_string_lossy();
    let expanded = if text == "~" || text.starts_with("~/") {
        let home = std::env::var_os("HOME").ok_or(ConfigError::InvalidPath)?;
        let mut result = PathBuf::from(home);
        if text.len() > 2 {
            result.push(&text[2..]);
        }
        result
    } else {
        path.to_path_buf()
    };
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir().map_err(|_| ConfigError::InvalidPath)?.join(expanded)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }

    let mut ancestor = normalized.clone();
    let mut missing = Vec::new();
    while !ancestor.exists() {
        let name = ancestor.file_name().ok_or(ConfigError::InvalidPath)?.to_os_string();
        missing.push(name);
        if !ancestor.pop() {
            return Err(ConfigError::InvalidPath);
        }
    }
    let mut resolved = fs::canonicalize(&ancestor).map_err(|_| ConfigError::InvalidPath)?;
    for component in missing.iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

fn read_optional_config(path: &Path) -> Result<Option<String>, ConfigError> {
    match fs::read_to_string(path) {
        Ok(input) => Ok(Some(input)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(ConfigError::ConfigFileUnavailable),
    }
}

fn record_origins(
    origins: &mut BTreeMap<String, String>,
    value: &Value,
    origin: &str,
    prefix: &str,
) {
    let Value::Mapping(mapping) = value else { return };
    for (key, child) in mapping {
        let Some(key) = key.as_str() else { continue };
        let path = if prefix.is_empty() { key.to_owned() } else { format!("{prefix}.{key}") };
        origins.insert(path.clone(), origin.to_owned());
        record_origins(origins, child, origin, &path);
    }
}

fn validate_config_keys(value: &Value) -> Result<(), ConfigError> {
    let Value::Mapping(root) = value else { return Err(ConfigError::InvalidRoot) };
    for (key, section) in root {
        let Some(name) = key.as_str() else { return Err(ConfigError::InvalidValue) };
        match name {
            "runtime" => validate_section_keys(
                section,
                &[
                    "frame_concurrency",
                    "request_concurrency",
                    "host_concurrency",
                    "decode_workers",
                    "discovery_workers",
                    "request_timeout",
                    "frame_deadline",
                    "discovery_deadline",
                    "max_artifact_bytes",
                    "max_frame_bytes",
                    "max_pixels",
                    "max_temp_bytes",
                    "allow_network",
                    "temp_root",
                ],
            )?,
            "cache" => validate_section_keys(
                section,
                &["dir", "max_bytes", "max_age_days", "gc_interval_hours", "enabled"],
            )?,
            "storage" => validate_section_keys(
                section,
                &["output", "endpoint", "region", "access_key", "secret_key", "anonymous"],
            )?,
            "output" => validate_section_keys(section, &["format", "grid", "resampling"])?,
            "sources" => {
                let Value::Mapping(sources) = section else {
                    return Err(ConfigError::InvalidValue);
                };
                if sources.values().any(|source| !source.is_mapping()) {
                    return Err(ConfigError::InvalidValue);
                }
            }
            _ => return Err(ConfigError::UnknownConfigField),
        }
    }
    Ok(())
}

fn validate_section_keys(section: &Value, allowed: &[&str]) -> Result<(), ConfigError> {
    let Value::Mapping(fields) = section else { return Err(ConfigError::InvalidValue) };
    if fields.keys().any(|key| key.as_str().is_none_or(|key| !allowed.contains(&key))) {
        return Err(ConfigError::UnknownConfigField);
    }
    Ok(())
}

fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|number| number != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Sequence(value) => !value.is_empty(),
        Value::Mapping(value) => !value.is_empty(),
        _ => true,
    }
}

fn redact_map(values: &mut BTreeMap<String, Value>) {
    for (key, value) in values {
        if is_secret_key(key) {
            *value = Value::String("<configured>".into());
        } else {
            redact_value(value);
        }
    }
}

fn redact_value(value: &mut Value) {
    match value {
        Value::Mapping(mapping) => {
            for (key, nested_value) in mapping.iter_mut() {
                if key.as_str().is_some_and(is_secret_key) {
                    *nested_value = Value::String("<configured>".into());
                } else {
                    redact_value(nested_value);
                }
            }
        }
        Value::Sequence(values) => {
            for nested_value in values {
                redact_value(nested_value);
            }
        }
        _ => {}
    }
}

fn is_secret_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.contains("key")
        || key.contains("token")
        || key.contains("secret")
        || key.contains("password")
        || key.contains("authorization")
        || key.contains("cookie")
}

/// Redact configuration-shaped JSON values with the same rules used by CoreConfig.
/// This also protects SDK values changed after load.
pub fn redact_json_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(values) => {
            for (key, child) in values {
                if is_secret_key(key) && json_is_truthy(child) {
                    *child = serde_json::Value::String("<configured>".into());
                } else {
                    redact_json_value(child);
                }
            }
        }
        serde_json::Value::Array(values) => {
            for child in values {
                redact_json_value(child);
            }
        }
        _ => {}
    }
}

fn json_is_truthy(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::Number(value) => value.as_f64().is_some_and(|number| number != 0.0),
        serde_json::Value::String(value) => !value.is_empty(),
        serde_json::Value::Array(value) => !value.is_empty(),
        serde_json::Value::Object(value) => !value.is_empty(),
    }
}

fn parse_yaml(input: &str) -> Result<Value, ConfigError> {
    if input.trim().is_empty()
        || input.lines().all(|line| {
            let line = line.trim();
            line.is_empty() || line.starts_with('#')
        })
    {
        return Ok(Value::Mapping(Mapping::new()));
    }
    // Deserialize mappings ourselves because serde_yaml_ng::Value is a map and
    // therefore cannot retain duplicate keys for a later validation pass.
    let strict: StrictValue = serde_yaml_ng::from_str(input).map_err(|error| {
        if error.to_string().contains("duplicate configuration key") {
            ConfigError::DuplicateConfigKey
        } else {
            ConfigError::InvalidYaml
        }
    })?;
    let value = strict.into_yaml()?;
    if value.is_null() {
        return Ok(Value::Mapping(Mapping::new()));
    }
    if !value.is_mapping() {
        return Err(ConfigError::InvalidRoot);
    }
    Ok(value)
}

enum StrictValue {
    Value(Value),
    Sequence(Vec<StrictValue>),
    Mapping(Vec<(String, StrictValue)>),
}

impl StrictValue {
    fn into_yaml(self) -> Result<Value, ConfigError> {
        match self {
            Self::Value(value) => Ok(value),
            Self::Sequence(items) => items
                .into_iter()
                .map(Self::into_yaml)
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Sequence),
            Self::Mapping(entries) => {
                let mut result = Mapping::new();
                for (key, value) in entries {
                    result.insert(Value::String(key), value.into_yaml()?);
                }
                Ok(Value::Mapping(result))
            }
        }
    }
}

impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct StrictVisitor;

        impl<'de> Visitor<'de> for StrictVisitor {
            type Value = StrictValue;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a YAML configuration value with unique string keys")
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(StrictValue::Value(Value::Null))
            }

            fn visit_none<E>(self) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                self.visit_unit()
            }

            fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
                Ok(StrictValue::Value(Value::Bool(value)))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
                Ok(StrictValue::Value(Value::Number(value.into())))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
                Ok(StrictValue::Value(Value::Number(value.into())))
            }

            fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E> {
                Ok(StrictValue::Value(Value::Number(value.into())))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(StrictValue::Value(Value::String(value.to_owned())))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
                Ok(StrictValue::Value(Value::String(value)))
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element()? {
                    values.push(value);
                }
                Ok(StrictValue::Sequence(values))
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut values = Vec::new();
                let mut keys = std::collections::BTreeSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !keys.insert(key.clone()) {
                        return Err(serde::de::Error::custom("duplicate configuration key"));
                    }
                    values.push((key, map.next_value()?));
                }
                Ok(StrictValue::Mapping(values))
            }
        }

        deserializer.deserialize_any(StrictVisitor)
    }
}

fn environment_patch(environment: &[(String, String)]) -> Result<Value, ConfigError> {
    let mut root = Value::Mapping(Mapping::new());
    for (key, raw) in environment {
        let Some(path) = key.strip_prefix("RADIUST_") else { continue };
        if key.starts_with("RADIUST_TEST_") {
            continue;
        }
        let pieces: Vec<String> =
            path.to_ascii_lowercase().split("__").map(str::to_owned).collect();
        if (pieces[0] == "sources" && pieces.len() != 3)
            || (pieces[0] != "sources" && pieces.len() != 2)
        {
            return Err(ConfigError::UnknownEnvironmentKey);
        }
        let parsed = match serde_json::from_str::<serde_json::Value>(raw) {
            Ok(value) => serde_yaml_ng::to_value(value).map_err(|_| ConfigError::InvalidValue)?,
            Err(_) if raw.eq_ignore_ascii_case("true") => Value::Bool(true),
            Err(_) if raw.eq_ignore_ascii_case("false") => Value::Bool(false),
            Err(_) => Value::String(raw.clone()),
        };
        insert_path(&mut root, &pieces, parsed)?;
    }
    Ok(root)
}

fn insert_path(root: &mut Value, path: &[String], value: Value) -> Result<(), ConfigError> {
    let Some((head, tail)) = path.split_first() else {
        return Err(ConfigError::UnknownEnvironmentKey);
    };
    let Value::Mapping(map) = root else { return Err(ConfigError::InvalidValue) };
    let key = Value::String(head.clone());
    if tail.is_empty() {
        map.insert(key, value);
        return Ok(());
    }
    let child = map.entry(key).or_insert_with(|| Value::Mapping(Mapping::new()));
    insert_path(child, tail, value)
}

fn merge(target: &mut Value, patch: Value) {
    match (target, patch) {
        (Value::Mapping(target), Value::Mapping(patch)) => {
            for (key, value) in patch {
                if let Some(existing) = target.get_mut(&key) {
                    merge(existing, value);
                } else {
                    target.insert(key, value);
                }
            }
        }
        (target, patch) => *target = patch,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ConfigError {
    #[error("configuration file could not be read")]
    ConfigFileUnavailable,
    #[error("configuration contains an invalid value")]
    InvalidValue,
    #[error("invalid YAML configuration")]
    InvalidYaml,
    #[error("configuration root must be a mapping")]
    InvalidRoot,
    #[error("duplicate configuration key")]
    DuplicateConfigKey,
    #[error("unknown configuration field")]
    UnknownConfigField,
    #[error("unknown environment configuration key")]
    UnknownEnvironmentKey,
    #[error("configuration paths are invalid")]
    InvalidPath,
    #[error("cache directory and output root cannot overlap")]
    OverlappingPaths,
    #[error("concurrency and resource limits must be positive and internally consistent")]
    InvalidLimit,
    #[error("timeouts must be finite and positive")]
    InvalidDeadline,
    #[error("storage must provide access_key and secret_key together")]
    IncompleteCredentials,
    #[error("source credentials must be configured together")]
    IncompleteSourceCredentials,
    #[error("output format or grid option is unsupported")]
    InvalidOutput,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_files_merge_user_project_environment_and_explicit_overrides() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let project = root.path().join("project");
        fs::create_dir_all(home.join(".config/radiust")).unwrap();
        fs::create_dir_all(&project).unwrap();
        fs::write(
            home.join(".config/radiust/config.yaml"),
            "runtime:\n  frame_concurrency: 3\n  discovery_workers: 7\nsources:\n  id_sidarma:\n    api_key: user-private\n    radar_ids: [ACE]\n",
        )
        .unwrap();
        fs::write(
            project.join("config.yaml"),
            "runtime:\n  frame_concurrency: 4\nsources:\n  id_sidarma:\n    radar_ids: [JAK]\n",
        )
        .unwrap();
        let env = vec![("RADIUST_RUNTIME__FRAME_CONCURRENCY".into(), "5".into())];
        let (config, origins) =
            CoreConfig::load_from_locations(Some(&home), &project, None, &env, None).unwrap();
        assert_eq!(config.runtime.frame_concurrency, 5);
        assert_eq!(config.runtime.discovery_workers, 7);
        assert_eq!(config.sources["id_sidarma"]["api_key"].as_str(), Some("user-private"));
        assert_eq!(
            config.sources["id_sidarma"]["radar_ids"],
            serde_yaml_ng::from_str::<Value>("[JAK]").unwrap()
        );
        assert_eq!(origins["runtime.frame_concurrency"], "environment");
        assert_eq!(origins["runtime.discovery_workers"], "user_file");
        assert_eq!(origins["sources.id_sidarma.radar_ids"], "file");
        assert!(!serde_json::to_string(&config.redacted()).unwrap().contains("user-private"));
        let (config, origins) = CoreConfig::load_from_locations(
            Some(&home),
            &project,
            None,
            &env,
            Some("runtime:\n  frame_concurrency: 6\n"),
        )
        .unwrap();
        assert_eq!(config.runtime.frame_concurrency, 6);
        assert_eq!(origins["runtime.frame_concurrency"], "explicit");
    }

    #[test]
    fn absent_automatic_files_keep_defaults_and_each_file_can_stand_alone() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let project = root.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let (config, _) =
            CoreConfig::load_from_locations(Some(&home), &project, None, &[], None).unwrap();
        assert_eq!(config, CoreConfig::default());
        fs::write(project.join("config.yaml"), "output:\n  format: png\n").unwrap();
        let (config, _) = CoreConfig::load_from_locations(None, &project, None, &[], None).unwrap();
        assert_eq!(config.output.format, "png");
        fs::remove_file(project.join("config.yaml")).unwrap();
        fs::create_dir_all(home.join(".config/radiust")).unwrap();
        fs::write(home.join(".config/radiust/config.yaml"), "output:\n  format: zarr\n").unwrap();
        let (config, _) =
            CoreConfig::load_from_locations(Some(&home), &project, None, &[], None).unwrap();
        assert_eq!(config.output.format, "zarr");
    }

    #[test]
    fn explicit_config_replaces_project_file_and_keeps_user_defaults() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join(".config/radiust")).unwrap();
        fs::write(
            root.path().join(".config/radiust/config.yaml"),
            "runtime:\n  discovery_workers: 7\n",
        )
        .unwrap();
        fs::write(root.path().join("config.yaml"), "invalid project yaml: [").unwrap();
        let explicit = root.path().join("custom.yaml");
        fs::write(&explicit, "runtime:\n  frame_concurrency: 3\n").unwrap();
        let (config, _) = CoreConfig::load_from_locations(
            Some(root.path()),
            root.path(),
            Some(&explicit),
            &[],
            None,
        )
        .unwrap();
        assert_eq!(config.runtime.discovery_workers, 7);
        assert_eq!(config.runtime.frame_concurrency, 3);
        fs::remove_file(&explicit).unwrap();
        assert_eq!(
            CoreConfig::load_from_locations(None, root.path(), Some(&explicit), &[], None),
            Err(ConfigError::ConfigFileUnavailable)
        );
    }

    #[test]
    fn existing_automatic_files_must_be_readable_and_valid() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join(".config/radiust")).unwrap();
        for path in
            [root.path().join(".config/radiust/config.yaml"), root.path().join("config.yaml")]
        {
            for (yaml, error) in [
                ("runtime: [", ConfigError::InvalidYaml),
                (
                    "runtime:\n  frame_concurrency: 3\n  frame_concurrency: 4\n",
                    ConfigError::DuplicateConfigKey,
                ),
                ("unknown_field: true\n", ConfigError::UnknownConfigField),
            ] {
                fs::write(&path, yaml).unwrap();
                assert_eq!(
                    CoreConfig::load_from_locations(
                        Some(root.path()),
                        root.path(),
                        None,
                        &[],
                        None
                    ),
                    Err(error)
                );
            }
            fs::remove_file(&path).unwrap();
            fs::create_dir(&path).unwrap();
            assert_eq!(
                CoreConfig::load_from_locations(Some(root.path()), root.path(), None, &[], None),
                Err(ConfigError::ConfigFileUnavailable)
            );
            fs::remove_dir(&path).unwrap();
        }
    }

    #[test]
    fn precedence_is_default_file_environment_then_explicit() {
        let env = BTreeMap::from([
            ("RADIUST_RUNTIME__FRAME_CONCURRENCY".into(), "3".into()),
            ("RADIUST_RUNTIME__DISCOVERY_WORKERS".into(), "6".into()),
        ]);
        let config = CoreConfig::layered(
            Some("runtime:\n  frame_concurrency: 5\n"),
            &env,
            Some("runtime:\n  frame_concurrency: 7\n"),
        )
        .unwrap();
        assert_eq!(config.runtime.frame_concurrency, 7);
        assert_eq!(config.runtime.discovery_workers, 6);
        assert!(!config.runtime.allow_network);
        assert_eq!(CoreConfig::default().runtime.discovery_workers, 4);
    }

    #[test]
    fn rejects_invalid_limits_and_partial_credentials() {
        let env = BTreeMap::new();
        assert_eq!(
            CoreConfig::layered(Some("runtime:\n  discovery_workers: 0\n"), &env, None),
            Err(ConfigError::InvalidLimit)
        );
        assert_eq!(
            CoreConfig::layered(Some("storage:\n  access_key: secret\n"), &env, None),
            Err(ConfigError::IncompleteCredentials)
        );
    }

    #[test]
    fn rejects_duplicate_yaml_keys_including_nested_config() {
        let env = BTreeMap::new();
        assert_eq!(
            CoreConfig::layered(
                Some("runtime:\n  frame_concurrency: 2\n  frame_concurrency: 9\n"),
                &env,
                None
            ),
            Err(ConfigError::DuplicateConfigKey)
        );
    }

    #[test]
    fn empty_yaml_and_zero_cache_capacity_keep_sdk_compatibility() {
        let env = BTreeMap::new();
        let config = CoreConfig::layered(
            Some("# intentionally empty\n"),
            &env,
            Some("cache:\n  max_bytes: 0\n"),
        )
        .unwrap();
        assert_eq!(config.cache.max_bytes, 0);
    }

    #[test]
    fn source_credentials_are_validated_as_a_pair() {
        let env = BTreeMap::new();
        assert_eq!(
            CoreConfig::layered(Some("sources:\n  provider:\n    access_key: a\n"), &env, None),
            Err(ConfigError::IncompleteSourceCredentials)
        );
    }

    #[test]
    fn config_origin_tracks_each_provided_path_and_precedence() {
        let env = BTreeMap::from([("RADIUST_RUNTIME__FRAME_CONCURRENCY".into(), "4".into())]);
        let (config, origins) = CoreConfig::layered_with_origins(
            Some("runtime:\n  frame_concurrency: 3\n"),
            &env,
            Some("runtime:\n  frame_concurrency: 5\n"),
        )
        .unwrap();
        assert_eq!(config.runtime.frame_concurrency, 5);
        assert_eq!(origins.get("runtime"), Some(&"explicit".to_owned()));
        assert_eq!(origins.get("runtime.frame_concurrency"), Some(&"explicit".to_owned()));
        assert_eq!(origins.get("cache"), Some(&"default".to_owned()));
    }

    #[test]
    fn sdk_paths_are_absolute_and_overlapping_roots_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("data");
        let cache = root.path().join("data/cache");
        let mut config = CoreConfig::default();
        config.storage.output = output;
        config.cache.dir = cache;
        assert_eq!(config.normalize_sdk_paths(), Err(ConfigError::OverlappingPaths));
    }

    #[test]
    fn secret_values_are_redacted() {
        let env = BTreeMap::new();
        let config = CoreConfig::layered(
            Some("storage:\n  access_key: user\n  secret_key: pass\nsources:\n  au:\n    token: private\n"),
            &env,
            None,
        )
        .unwrap();
        let safe = serde_json::to_string(&config.redacted()).unwrap();
        assert!(!safe.contains("user"));
        assert!(!safe.contains("pass"));
        assert!(!safe.contains("private"));
    }

    #[test]
    fn nested_and_sequence_credentials_are_redacted_recursively() {
        let env = BTreeMap::new();
        let config = CoreConfig::layered(
            Some(
                "sources:\n  au:\n    request:\n      auth:\n        bearer_token: deeply-private\n      headers:\n        - authorization: Bearer-header-private\n        - nested:\n            cookie: session-private\n",
            ),
            &env,
            None,
        )
        .unwrap();
        let safe = serde_json::to_string(&config.redacted()).unwrap();
        assert!(!safe.contains("deeply-private"));
        assert!(!safe.contains("Bearer-header-private"));
        assert!(!safe.contains("session-private"));
    }

    #[test]
    fn redacts_mutable_json_config_values_recursively() {
        let mut value = serde_json::json!({
            "provider": {
                "api_key": "private",
                "headers": [{"Authorization": "secret"}, {"cookie": ""}],
                "zero": 0
            }
        });
        redact_json_value(&mut value);
        assert_eq!(
            value,
            serde_json::json!({
                "provider": {
                    "api_key": "<configured>",
                    "headers": [{"Authorization": "<configured>"}, {"cookie": ""}],
                    "zero": 0
                }
            })
        );
    }
}
