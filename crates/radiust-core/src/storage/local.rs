use crate::errors::{CoreError, CoreResult};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

pub struct RootLock {
    file: File,
    path: PathBuf,
}

impl RootLock {
    pub fn acquire(root: impl AsRef<Path>) -> CoreResult<Self> {
        let root = root.as_ref();
        std::fs::create_dir_all(root).map_err(|e| CoreError::Storage(e.to_string()))?;
        let path = root.join(".radiust.lock");
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(|e| CoreError::Storage(format!("output root locked: {e}")))?;
        Ok(Self { file, path })
    }
}

impl Drop for RootLock {
    fn drop(&mut self) {
        let _ = self.file.sync_all();
        let _ = std::fs::remove_file(&self.path);
    }
}
