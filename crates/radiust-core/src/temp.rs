use crate::digest::{Receipt, digest_reader};
use crate::errors::{CoreError, CoreResult};
use sha2::Digest;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

pub struct TempOwner {
    file: Option<NamedTempFile>,
    path: PathBuf,
}

impl TempOwner {
    pub fn new(dir: &Path) -> CoreResult<Self> {
        fs::create_dir_all(dir).map_err(|e| CoreError::Temporary(e.to_string()))?;
        let file = NamedTempFile::new_in(dir).map_err(|e| CoreError::Temporary(e.to_string()))?;
        let path = file.path().to_path_buf();
        Ok(Self { file: Some(file), path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write_all(&mut self, bytes: &[u8]) -> CoreResult<Receipt> {
        let file =
            self.file.as_mut().ok_or_else(|| CoreError::Temporary("owner is closed".into()))?;
        file.write_all(bytes).map_err(|e| CoreError::Temporary(e.to_string()))?;
        file.as_file().sync_all().map_err(|e| CoreError::Temporary(e.to_string()))?;
        Ok(Receipt {
            size_bytes: bytes.len() as u64,
            sha256: hex::encode(sha2::Sha256::digest(bytes)),
        })
    }

    pub fn read_receipt(&self, limit: u64) -> CoreResult<Receipt> {
        let file = File::open(&self.path).map_err(|e| CoreError::Temporary(e.to_string()))?;
        Ok(digest_reader(file, limit)?.1)
    }

    pub fn close(&mut self) -> CoreResult<()> {
        if let Some(file) = self.file.take() {
            file.close().map_err(|e| CoreError::Temporary(e.to_string()))?;
        }
        Ok(())
    }
}

impl Drop for TempOwner {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
