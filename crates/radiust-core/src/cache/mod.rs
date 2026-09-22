pub mod index;
pub mod lease;

use crate::errors::{CoreError, CoreResult};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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
        std::fs::create_dir_all(&root).map_err(|e| CoreError::Cache(e.to_string()))?;
        let root = root.canonicalize().map_err(|e| CoreError::Cache(e.to_string()))?;
        for name in ["objects", "mosaics", "tmp", "leases"] {
            std::fs::create_dir_all(root.join(name))
                .map_err(|e| CoreError::Cache(e.to_string()))?;
        }
        let index = index::CacheIndex::open(root.join("index.sqlite"))?;
        let cache = Self { root, index };
        cache.repair()?;
        Ok(cache)
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
        let digest = bytes_digest(payload);
        let target = directory.join(format!("{digest}.bin"));
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
        let old_path = self.index.get(key)?.map(|(path, _, _)| path);
        let relative =
            target.strip_prefix(&self.root).map_err(|error| CoreError::Cache(error.to_string()))?;
        self.index.put(
            key,
            &relative.to_string_lossy(),
            payload.len() as u64,
            &digest,
            expires_at,
        )?;
        if let Some(old_path) = old_path {
            let old = self.safe_path(&old_path)?;
            if old != target && !self.is_leased(key) && !self.path_is_referenced(&old)? {
                let _ = fs::remove_file(old);
            }
        }
        Ok((payload.len() as u64, digest, target))
    }

    pub fn get_bytes(&self, key: &str) -> CoreResult<Option<Vec<u8>>> {
        let Some((path, size, expected)) = self.index.get(key)? else {
            return Ok(None);
        };
        let target = self.safe_path(&path)?;
        let bytes = match fs::read(&target) {
            Ok(bytes) if bytes.len() as u64 == size && bytes_digest(&bytes) == expected => bytes,
            _ => {
                let _ = fs::remove_file(&target);
                self.index.remove(key)?;
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
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
        let entries = self.index.entries()?;
        let mut report = GcReport {
            bytes_before: entries.iter().map(|entry| entry.size_bytes).sum(),
            ..GcReport::default()
        };
        let mut candidates: Vec<_> = entries
            .iter()
            .filter(|entry| entry.expires_at.is_some_and(|expiry| expiry <= now))
            .cloned()
            .collect();
        let mut remaining = entries
            .iter()
            .filter(|entry| !entry.expires_at.is_some_and(|expiry| expiry <= now))
            .cloned()
            .collect::<Vec<_>>();
        let mut bytes = remaining.iter().map(|entry| entry.size_bytes).sum::<u64>();
        remaining.sort_by(|left, right| {
            left.last_accessed.cmp(&right.last_accessed).then_with(|| left.key.cmp(&right.key))
        });
        for entry in remaining {
            if bytes <= max_bytes {
                break;
            }
            if self.is_leased(&entry.key) {
                continue;
            }
            bytes = bytes.saturating_sub(entry.size_bytes);
            candidates.push(entry);
        }
        candidates.retain(|entry| !self.is_leased(&entry.key));
        let candidate_paths = candidates.iter().map(|entry| entry.path.clone()).collect::<Vec<_>>();
        for entry in candidates {
            self.index.remove(&entry.key)?;
            report.removed.push(entry.key);
        }
        for path in candidate_paths {
            let resolved = self.safe_path(&path)?;
            if !self.path_is_referenced(&resolved)? {
                let _ = fs::remove_file(resolved);
            }
        }
        for path in
            std::fs::read_dir(self.root.join("tmp")).map_err(|e| CoreError::Cache(e.to_string()))?
        {
            let path = path.map_err(|e| CoreError::Cache(e.to_string()))?.path();
            if path.is_file() {
                let name = path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default()
                    .to_owned();
                std::fs::remove_file(&path).map_err(|e| CoreError::Cache(e.to_string()))?;
                report.stale_tmp.push(name);
            }
        }
        report.bytes_after = report.bytes_before.saturating_sub(
            report
                .removed
                .iter()
                .filter_map(|key| {
                    entries.iter().find(|entry| &entry.key == key).map(|entry| entry.size_bytes)
                })
                .sum(),
        );
        Ok(report)
    }

    fn safe_path(&self, path: &str) -> CoreResult<PathBuf> {
        let candidate = PathBuf::from(path);
        let target = if candidate.is_absolute() { candidate } else { self.root.join(candidate) };
        let root = self.root.canonicalize().map_err(|e| CoreError::Cache(e.to_string()))?;
        let resolved = target.canonicalize().unwrap_or(target);
        if resolved != root && !resolved.starts_with(&root) {
            return Err(CoreError::Cache("cache index path escapes cache root".into()));
        }
        Ok(resolved)
    }

    fn repair(&self) -> CoreResult<()> {
        let mut indexed = std::collections::HashSet::new();
        for entry in self.index.entries()? {
            match self.safe_path(&entry.path) {
                Ok(path) if path.is_file() => {
                    indexed.insert(path);
                }
                _ => self.index.remove(&entry.key)?,
            }
        }
        for directory in [self.root.join("objects"), self.root.join("mosaics")] {
            for item in fs::read_dir(directory).map_err(|e| CoreError::Cache(e.to_string()))? {
                let path = item.map_err(|e| CoreError::Cache(e.to_string()))?.path();
                if path.is_file() && !indexed.contains(&path) {
                    fs::remove_file(path).map_err(|e| CoreError::Cache(e.to_string()))?;
                }
            }
        }
        Ok(())
    }

    fn path_is_referenced(&self, path: &Path) -> CoreResult<bool> {
        for entry in self.index.entries()? {
            if self.safe_path(&entry.path)? == path {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn is_leased(&self, key: &str) -> bool {
        self.root.join("leases").join(format!("{}.lease", key_digest(key))).exists()
    }
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
