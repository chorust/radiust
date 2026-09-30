pub mod index;
pub mod lease;

use crate::errors::{CoreError, CoreResult};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Default)]
pub struct GcReport {
    pub removed: Vec<String>,
    pub stale_tmp: Vec<String>,
    pub bytes_before: u64,
    pub bytes_after: u64,
}

pub struct Cache {
    pub root: PathBuf,
    pub index: index::CacheIndex,
}

impl Cache {
    pub fn open(root: impl AsRef<Path>) -> CoreResult<Self> {
        let root = root.as_ref().to_path_buf();
        let index_path = root.join("index.sqlite");
        let has_legacy_index = index::CacheIndex::has_legacy_entries(&index_path)?;
        let has_rust_index = index::CacheIndex::has_native_objects_table(&index_path)?;
        if has_legacy_index && has_rust_index {
            return Err(CoreError::Cache(
                "cache root contains conflicting Python and Rust indexes".into(),
            ));
        }
        if !has_legacy_index && !has_rust_index && has_unindexed_objects(&root)? {
            return Err(CoreError::Cache(
                "cache root contains unrecognized objects; select an isolated Rust cache directory"
                    .into(),
            ));
        }
        fs::create_dir_all(&root).map_err(|e| CoreError::Cache(e.to_string()))?;
        let root = root.canonicalize().map_err(|e| CoreError::Cache(e.to_string()))?;
        for name in ["objects", "mosaics", "tmp", "leases"] {
            let directory = root.join(name);
            fs::create_dir_all(&directory).map_err(|e| CoreError::Cache(e.to_string()))?;
            let resolved = directory.canonicalize().map_err(|e| CoreError::Cache(e.to_string()))?;
            if resolved != directory {
                return Err(CoreError::Cache(format!(
                    "cache directory {} must not be a symlink",
                    directory.display()
                )));
            }
        }
        let index = index::CacheIndex::open(root.join("index.sqlite"))?;
        let cache = Self { root, index };
        cache.repair()?;
        Ok(cache)
    }

    /// Open an existing cache without creating directories, upgrading its
    /// index, or running repair. A missing or empty cache returns `None`.
    pub fn open_for_preview(root: impl AsRef<Path>) -> CoreResult<Option<Self>> {
        let requested_root = root.as_ref();
        let metadata = match fs::metadata(requested_root) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(CoreError::Cache(error.to_string())),
        };
        if !metadata.is_dir() {
            return Err(CoreError::Cache("cache root is not a directory".into()));
        }
        let root =
            requested_root.canonicalize().map_err(|error| CoreError::Cache(error.to_string()))?;
        for name in ["objects", "mosaics", "tmp", "leases"] {
            let directory = root.join(name);
            match fs::symlink_metadata(&directory) {
                Ok(metadata)
                    if metadata.file_type().is_dir()
                        && directory.canonicalize().ok().as_deref()
                            == Some(directory.as_path()) =>
                {
                    // Existing cache directories are validated without being created.
                }
                Ok(_) => {
                    return Err(CoreError::Cache(format!(
                        "cache directory {} must not be a symlink",
                        directory.display()
                    )));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(CoreError::Cache(error.to_string())),
            }
        }

        let index_path = root.join("index.sqlite");
        if !index_path.exists() {
            if has_unindexed_objects(&root)? {
                return Err(CoreError::Cache(
                    "cache root contains unrecognized objects; select an isolated Rust cache directory"
                        .into(),
                ));
            }
            return Ok(None);
        }
        let has_legacy_index = index::CacheIndex::has_legacy_entries(&index_path)?;
        let has_rust_index = index::CacheIndex::has_native_objects_table(&index_path)?;
        if has_legacy_index && has_rust_index {
            return Err(CoreError::Cache(
                "cache root contains conflicting Python and Rust indexes".into(),
            ));
        }
        if !has_legacy_index && !has_rust_index {
            if has_unindexed_objects(&root)? {
                return Err(CoreError::Cache(
                    "cache root contains unrecognized objects; select an isolated Rust cache directory"
                        .into(),
                ));
            }
            return Ok(None);
        }
        let index = index::CacheIndex::open_readonly(&index_path)?;
        Ok(Some(Self { root, index }))
    }

    pub fn put_bytes(
        &self,
        key: &str,
        payload: &[u8],
        kind: &str,
        expires_at: Option<i64>,
    ) -> CoreResult<(u64, String, PathBuf)> {
        let directory = match kind {
            "object" => self.root.join("objects"),
            "mosaic" => self.root.join("mosaics"),
            _ => return Err(CoreError::Cache("cache kind must be object or mosaic".into())),
        };
        if self.index.is_legacy() && self.is_leased(key) {
            return Err(CoreError::Cache("cannot replace a leased legacy cache entry".into()));
        }
        let digest = bytes_digest(payload);
        let filename_digest = if self.index.is_legacy() { key_digest(key) } else { digest.clone() };
        let target = directory.join(format!("{filename_digest}.bin"));
        let old_entry = self.index.get_entry(key)?;
        if self.index.is_legacy() && target.exists() {
            let indexed_target =
                old_entry.as_ref().and_then(|entry| self.safe_object_path(&entry.path).ok());
            if indexed_target.as_deref() != Some(target.as_path()) {
                return Err(CoreError::Cache(
                    "legacy cache object path exists without a matching index entry".into(),
                ));
            }
        }
        let temporary =
            self.root.join("tmp").join(format!(".{digest}.{}.tmp", uuid::Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|e| CoreError::Cache(e.to_string()))?;
        if let Err(error) = file.write_all(payload).and_then(|_| file.sync_all()) {
            let _ = fs::remove_file(&temporary);
            return Err(CoreError::Cache(error.to_string()));
        }
        drop(file);
        if let Err(error) = fs::rename(&temporary, &target) {
            let _ = fs::remove_file(&temporary);
            return Err(CoreError::Cache(error.to_string()));
        }
        let stored_path = if self.index.is_legacy() {
            target.to_string_lossy().into_owned()
        } else {
            target
                .strip_prefix(&self.root)
                .map_err(|error| CoreError::Cache(error.to_string()))?
                .to_string_lossy()
                .into_owned()
        };
        self.index.put(key, &stored_path, payload.len() as u64, &digest, expires_at)?;
        if let Some(old_entry) = old_entry {
            if let Ok(old_path) = self.safe_object_path(&old_entry.path) {
                if old_path != target && !self.path_is_referenced(&old_path)? {
                    self.remove_valid_object(&old_entry, &old_path);
                }
            }
        }
        Ok((payload.len() as u64, digest, target))
    }

    /// Copy a staged file into the cache while hashing it in bounded chunks.
    /// The cache is an optimization: callers may ignore this method's errors
    /// after a successful acquisition.
    pub fn put_file(
        &self,
        key: &str,
        source: impl AsRef<Path>,
        kind: &str,
        expires_at: Option<i64>,
        max_bytes: u64,
    ) -> CoreResult<(u64, String, PathBuf)> {
        let directory = match kind {
            "object" => self.root.join("objects"),
            "mosaic" => self.root.join("mosaics"),
            _ => return Err(CoreError::Cache("cache kind must be object or mosaic".into())),
        };
        if self.index.is_legacy() && self.is_leased(key) {
            return Err(CoreError::Cache("cannot replace a leased legacy cache entry".into()));
        }

        let old_entry = self.index.get_entry(key)?;
        let source = source.as_ref();
        let nonce = uuid::Uuid::new_v4();
        let temporary = self.root.join("tmp").join(format!(".file-{nonce}.tmp"));
        let result = copy_and_hash(source, &temporary, max_bytes);
        let (size_bytes, digest) = match result {
            Ok(receipt) => receipt,
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                return Err(error);
            }
        };
        let filename_digest = if self.index.is_legacy() { key_digest(key) } else { digest.clone() };
        let target = directory.join(format!("{filename_digest}.bin"));
        if self.index.is_legacy() && target.exists() {
            let indexed_target =
                old_entry.as_ref().and_then(|entry| self.safe_object_path(&entry.path).ok());
            if indexed_target.as_deref() != Some(target.as_path()) {
                let _ = fs::remove_file(&temporary);
                return Err(CoreError::Cache(
                    "legacy cache object path exists without a matching index entry".into(),
                ));
            }
        }
        if let Err(error) = fs::rename(&temporary, &target) {
            let _ = fs::remove_file(&temporary);
            return Err(CoreError::Cache(error.to_string()));
        }
        let stored_path = if self.index.is_legacy() {
            target.to_string_lossy().into_owned()
        } else {
            target
                .strip_prefix(&self.root)
                .map_err(|error| CoreError::Cache(error.to_string()))?
                .to_string_lossy()
                .into_owned()
        };
        self.index.put(key, &stored_path, size_bytes, &digest, expires_at)?;
        if let Some(old_entry) = old_entry {
            if let Ok(old_path) = self.safe_object_path(&old_entry.path) {
                if old_path != target && !self.path_is_referenced(&old_path)? {
                    self.remove_valid_object(&old_entry, &old_path);
                }
            }
        }
        Ok((size_bytes, digest, target))
    }

    /// Copy a verified cached object into an operation-owned destination.
    /// A lease protects the cache file from GC until the copy is complete.
    pub fn get_to_path(
        &self,
        key: &str,
        destination: impl AsRef<Path>,
        max_bytes: u64,
    ) -> CoreResult<Option<(u64, String)>> {
        let Some(entry) = self.index.get_entry(key)? else {
            return Ok(None);
        };
        if entry.size_bytes > max_bytes {
            return Ok(None);
        }
        let target = match self.safe_object_path(&entry.path) {
            Ok(target) => target,
            Err(_) => {
                self.index.remove(&entry.key)?;
                return Ok(None);
            }
        };
        let lease_path = self.root.join("leases").join(format!("{}.lease", key_digest(key)));
        let Some(lease) = lease::Lease::try_acquire(lease_path)? else {
            return Ok(None);
        };
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
        if entry.expires_at.is_some_and(|expiry| expiry <= now) {
            drop(lease);
            self.remove_entry_safely(&entry, true)?;
            return Ok(None);
        }

        let destination = destination.as_ref();
        let result = copy_and_hash(&target, destination, max_bytes);
        drop(lease);
        match result {
            Ok((size_bytes, digest))
                if size_bytes == entry.size_bytes && digest == entry.sha256 =>
            {
                self.index.touch(key)?;
                Ok(Some((size_bytes, digest)))
            }
            Ok(_) => {
                let _ = fs::remove_file(destination);
                self.index.remove(key)?;
                Ok(None)
            }
            Err(error) => {
                let _ = fs::remove_file(destination);
                if matches!(error, CoreError::Cache(_)) {
                    self.index.remove(key)?;
                    Ok(None)
                } else {
                    Err(error)
                }
            }
        }
    }

    pub fn get_bytes(&self, key: &str) -> CoreResult<Option<Vec<u8>>> {
        let Some(entry) = self.index.get_entry(key)? else {
            return Ok(None);
        };
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
        if entry.expires_at.is_some_and(|expiry| expiry <= now) {
            self.remove_entry_safely(&entry, true)?;
            return Ok(None);
        }
        let target = match self.safe_object_path(&entry.path) {
            Ok(target) => target,
            Err(_) => {
                if !self.is_leased(&entry.key) {
                    self.index.remove(&entry.key)?;
                }
                return Ok(None);
            }
        };
        let bytes = match fs::read(&target) {
            Ok(bytes)
                if bytes.len() as u64 == entry.size_bytes
                    && bytes_digest(&bytes) == entry.sha256 =>
            {
                bytes
            }
            _ => {
                if !self.is_leased(&entry.key) {
                    // A bad digest does not establish ownership of the current bytes. Drop only
                    // the stale index row and leave the file for explicit inspection or repair.
                    self.index.remove(&entry.key)?;
                }
                return Ok(None);
            }
        };
        self.index.touch(key)?;
        Ok(Some(bytes))
    }

    pub fn lease(&self, key: &str) -> CoreResult<lease::Lease> {
        let digest = key_digest(key);
        lease::Lease::acquire(self.root.join("leases").join(format!("{digest}.lease")))
    }

    pub fn gc(&self, max_bytes: u64) -> CoreResult<GcReport> {
        self.gc_with_mode(max_bytes, false)
    }

    /// Preview the cache entries and owned temporary files that `gc` would
    /// remove, without changing the index or filesystem.
    pub fn gc_preview(&self, max_bytes: u64) -> CoreResult<GcReport> {
        self.gc_with_mode(max_bytes, true)
    }

    fn gc_with_mode(&self, max_bytes: u64, preview: bool) -> CoreResult<GcReport> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
        let entries = self.index.entries()?;
        let mut report = GcReport {
            bytes_before: entries.iter().map(|entry| entry.size_bytes).sum(),
            ..GcReport::default()
        };
        let entry_sizes = entries
            .iter()
            .map(|entry| (entry.key.clone(), entry.size_bytes))
            .collect::<std::collections::HashMap<_, _>>();
        let mut candidates = Vec::new();
        let mut remaining = Vec::new();
        for entry in entries {
            let expired = entry.expires_at.is_some_and(|expiry| expiry <= now);
            if expired && !self.is_leased(&entry.key) {
                candidates.push(entry);
            } else {
                remaining.push(entry);
            }
        }
        let mut bytes = remaining.iter().map(|entry| entry.size_bytes).sum::<u64>();
        remaining.sort_by(|left, right| {
            left.last_accessed.cmp(&right.last_accessed).then_with(|| left.key.cmp(&right.key))
        });
        for entry in remaining {
            if max_bytes != 0 && bytes <= max_bytes {
                break;
            }
            if self.is_leased(&entry.key) {
                continue;
            }
            bytes = bytes.saturating_sub(entry.size_bytes);
            candidates.push(entry);
        }
        for entry in candidates {
            if preview {
                if !self.is_leased(&entry.key) {
                    report.removed.push(entry.key);
                }
            } else if self.remove_entry_safely(&entry, true)? {
                report.removed.push(entry.key);
            }
        }
        match fs::read_dir(self.root.join("tmp")) {
            Ok(items) => {
                for item in items {
                    let path = item.map_err(|e| CoreError::Cache(e.to_string()))?.path();
                    if path.is_file() && is_old_owned_temp(&path) {
                        let name = path
                            .file_name()
                            .and_then(|value| value.to_str())
                            .unwrap_or_default()
                            .to_owned();
                        if preview {
                            report.stale_tmp.push(name);
                        } else if fs::remove_file(&path).is_ok() {
                            report.stale_tmp.push(name);
                        }
                    }
                }
            }
            Err(error) if preview && error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(CoreError::Cache(error.to_string())),
        }
        let removed_bytes =
            report.removed.iter().filter_map(|key| entry_sizes.get(key)).copied().sum::<u64>();
        report.bytes_after = report.bytes_before.saturating_sub(removed_bytes);
        Ok(report)
    }

    /// Clear every unleased indexed object and stale cache temporary file.
    /// Unindexed files are never considered owned and are left untouched.
    pub fn clear(&self) -> CoreResult<GcReport> {
        self.gc(0)
    }

    fn safe_path(&self, path: &str) -> CoreResult<PathBuf> {
        let candidate = PathBuf::from(path);
        if candidate.components().any(|part| matches!(part, std::path::Component::ParentDir)) {
            return Err(CoreError::Cache("cache index path contains parent traversal".into()));
        }
        let target = if candidate.is_absolute() { candidate } else { self.root.join(candidate) };
        let root = self.root.canonicalize().map_err(|e| CoreError::Cache(e.to_string()))?;
        let resolved = match target.canonicalize() {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => target,
            Err(error) => return Err(CoreError::Cache(error.to_string())),
        };
        if resolved != root && !resolved.starts_with(&root) {
            return Err(CoreError::Cache("cache index path escapes cache root".into()));
        }
        Ok(resolved)
    }

    fn safe_object_path(&self, path: &str) -> CoreResult<PathBuf> {
        let resolved = self.safe_path(path)?;
        let objects = self.root.join("objects");
        let mosaics = self.root.join("mosaics");
        if resolved.parent() != Some(objects.as_path())
            && resolved.parent() != Some(mosaics.as_path())
        {
            return Err(CoreError::Cache(
                "cache index path is outside managed object directories".into(),
            ));
        }
        Ok(resolved)
    }

    fn repair(&self) -> CoreResult<()> {
        for entry in self.index.entries()? {
            if self.is_leased(&entry.key) {
                continue;
            }
            let valid = self
                .safe_object_path(&entry.path)
                .ok()
                .is_some_and(|path| file_matches_entry(&path, &entry));
            if !valid {
                // Repair only the known index row. A file whose ownership or content no
                // longer matches the row is preserved.
                self.index.remove(&entry.key)?;
            }
        }
        Ok(())
    }

    fn path_is_referenced(&self, path: &Path) -> CoreResult<bool> {
        for entry in self.index.entries()? {
            match self.safe_object_path(&entry.path) {
                Ok(indexed) if indexed == path => return Ok(true),
                Err(_) => return Ok(true),
                _ => {}
            }
        }
        Ok(false)
    }

    fn remove_entry_safely(
        &self,
        entry: &index::CacheEntry,
        remove_file: bool,
    ) -> CoreResult<bool> {
        if self.is_leased(&entry.key) {
            return Ok(false);
        }
        self.index.remove(&entry.key)?;
        if !remove_file {
            return Ok(true);
        }
        if let Ok(path) = self.safe_object_path(&entry.path) {
            if !self.path_is_referenced(&path)? {
                self.remove_valid_object(entry, &path);
            }
        }
        Ok(true)
    }

    fn remove_valid_object(&self, entry: &index::CacheEntry, path: &Path) {
        if !self.index.is_legacy() {
            let _ = fs::remove_file(path);
            return;
        }
        if file_matches_entry(path, entry) {
            let _ = fs::remove_file(path);
        }
    }

    fn is_leased(&self, key: &str) -> bool {
        let digest = key_digest(key);
        let directory = self.root.join("leases");
        let exact = directory.join(format!("{digest}.lease"));
        if exact.is_file() {
            return true;
        }
        let Ok(items) = fs::read_dir(directory) else {
            return true;
        };
        items.filter_map(Result::ok).any(|item| {
            let name = item.file_name();
            let name = name.to_string_lossy();
            item.path().is_file()
                && name.starts_with(&format!("{digest}."))
                && name.ends_with(".lease")
        })
    }
}

fn has_unindexed_objects(root: &Path) -> CoreResult<bool> {
    for directory in [root.join("objects"), root.join("mosaics")] {
        match fs::read_dir(directory) {
            Ok(items) => {
                for item in items {
                    if item.map_err(|error| CoreError::Cache(error.to_string()))?.path().is_file() {
                        return Ok(true);
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(CoreError::Cache(error.to_string())),
        }
    }
    Ok(false)
}

fn is_old_owned_temp(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else { return false };
    let parts = name.split('.').collect::<Vec<_>>();
    if parts.len() != 4
        || !parts[0].is_empty()
        || parts[3] != "tmp"
        || parts[1].len() != 64
        || !parts[1].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return false;
    }
    let nonce = parts[2];
    let valid_nonce = (nonce.len() == 32 && nonce.bytes().all(|byte| byte.is_ascii_hexdigit()))
        || (nonce.len() == 36
            && nonce.bytes().enumerate().all(|(index, byte)| {
                if [8, 13, 18, 23].contains(&index) {
                    byte == b'-'
                } else {
                    byte.is_ascii_hexdigit()
                }
            }));
    if !valid_nonce {
        return false;
    }
    let Ok(metadata) = fs::metadata(path) else { return false };
    let Ok(modified) = metadata.modified() else { return false };
    SystemTime::now().duration_since(modified).is_ok_and(|age| age >= Duration::from_secs(60 * 60))
}

fn file_matches_entry(path: &Path, entry: &index::CacheEntry) -> bool {
    let Ok(metadata) = fs::metadata(path) else { return false };
    if metadata.len() != entry.size_bytes {
        return false;
    }
    let Ok(mut file) = fs::File::open(path) else { return false };
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => digest.update(&buffer[..count]),
            Err(_) => return false,
        }
    }
    hex::encode(digest.finalize()) == entry.sha256
}

fn key_digest(key: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(key.as_bytes());
    hex::encode(digest.finalize())
}

fn bytes_digest(payload: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(payload);
    hex::encode(digest.finalize())
}

fn copy_and_hash(source: &Path, destination: &Path, max_bytes: u64) -> CoreResult<(u64, String)> {
    let mut input = fs::File::open(source)
        .map_err(|error| CoreError::Cache(format!("cache source could not be opened: {error}")))?;
    let mut output =
        OpenOptions::new().create_new(true).write(true).open(destination).map_err(|error| {
            CoreError::Cache(format!("cache staging file could not be created: {error}"))
        })?;
    let mut digest = Sha256::new();
    let mut size_bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = input
            .read(&mut buffer)
            .map_err(|error| CoreError::Cache(format!("cache source read failed: {error}")))?;
        if count == 0 {
            break;
        }
        let count = count as u64;
        if count > max_bytes.saturating_sub(size_bytes) {
            return Err(CoreError::ResourceLimit(format!(
                "cache object exceeds configured limit {max_bytes}"
            )));
        }
        output
            .write_all(&buffer[..count as usize])
            .map_err(|error| CoreError::Cache(format!("cache staging write failed: {error}")))?;
        digest.update(&buffer[..count as usize]);
        size_bytes += count;
    }
    output
        .sync_all()
        .map_err(|error| CoreError::Cache(format!("cache staging flush failed: {error}")))?;
    Ok((size_bytes, hex::encode(digest.finalize())))
}

#[cfg(test)]
mod file_cache_tests {
    use super::*;

    #[test]
    fn file_cache_round_trip_streams_and_verifies_the_payload() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("cache");
        let source = directory.path().join("source.bin");
        let payload = vec![0x5a; 256 * 1024];
        fs::write(&source, &payload).unwrap();
        let cache = Cache::open(&root).unwrap();

        let (size, digest, cached_path) =
            cache.put_file("raw-frame", &source, "object", None, payload.len() as u64).unwrap();
        assert_eq!(size, payload.len() as u64);
        assert_eq!(digest, hex::encode(Sha256::digest(&payload)));
        assert_eq!(fs::read(cached_path).unwrap(), payload);

        let destination = directory.path().join("operation-owned.bin");
        let receipt = cache.get_to_path("raw-frame", &destination, 300 * 1024).unwrap();
        assert_eq!(receipt, Some((size, digest)));
        assert_eq!(fs::read(destination).unwrap(), payload);
    }

    #[test]
    fn file_cache_limit_and_corrupt_rows_fail_closed_without_replacing_destinations() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("cache");
        let source = directory.path().join("source.bin");
        fs::write(&source, b"verified payload").unwrap();
        let cache = Cache::open(&root).unwrap();
        assert!(cache.put_file("raw-frame", &source, "object", None, 2).is_err());

        cache.put_file("raw-frame", &source, "object", None, 64).unwrap();
        let destination = directory.path().join("operation-owned.bin");
        fs::write(&destination, b"caller data").unwrap();
        assert!(cache.get_to_path("raw-frame", &destination, 2).unwrap().is_none());
        assert_eq!(fs::read(&destination).unwrap(), b"caller data");

        let entry = cache.index.get_entry("raw-frame").unwrap().unwrap();
        let object = cache.safe_object_path(&entry.path).unwrap();
        fs::write(&object, b"corrupted payload").unwrap();
        fs::remove_file(&destination).unwrap();
        assert!(cache.get_to_path("raw-frame", &destination, 64).unwrap().is_none());
        assert!(!destination.exists());
        assert!(cache.index.get_entry("raw-frame").unwrap().is_none());
        assert!(object.exists(), "an unverified object remains for explicit inspection");
    }
}
