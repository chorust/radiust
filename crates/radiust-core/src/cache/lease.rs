use crate::errors::{CoreError, CoreResult};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

pub struct Lease {
    file: File,
    path: PathBuf,
}

impl Lease {
    pub fn acquire(path: impl AsRef<Path>) -> CoreResult<Self> {
        Self::try_acquire(path.as_ref())?.ok_or_else(|| {
            CoreError::Cache(format!("lease {} is already held", path.as_ref().display()))
        })
    }

    pub fn try_acquire(path: impl AsRef<Path>) -> CoreResult<Option<Self>> {
        let path = path.as_ref().to_path_buf();
        let file = match OpenOptions::new().create_new(true).write(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(None),
            Err(error) => {
                return Err(CoreError::Cache(format!("lease {}: {error}", path.display())));
            }
        };
        Ok(Some(Self { file, path }))
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
