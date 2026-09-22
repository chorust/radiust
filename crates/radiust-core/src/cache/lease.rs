use crate::errors::{CoreError, CoreResult};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

pub struct Lease {
    file: File,
    path: PathBuf,
}

impl Lease {
    pub fn acquire(path: impl AsRef<Path>) -> CoreResult<Self> {
        let path = path.as_ref().to_path_buf();
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(|e| CoreError::Cache(format!("lease {}: {e}", path.display())))?;
        Ok(Self { file, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let _ = self.file.sync_all();
        let _ = std::fs::remove_file(&self.path);
    }
}
