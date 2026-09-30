pub use super::commit::{
    LocalCommitRequest, LocalCommitResult, LocalCommitStatus, LocalStore, StagedArtifact,
};
use crate::errors::{CoreError, CoreResult};
use std::fs::{File, OpenOptions};
#[cfg(unix)]
use std::io;
use std::path::Path;
#[cfg(not(unix))]
use std::path::PathBuf;

pub struct RootLock {
    file: File,
    #[cfg(not(unix))]
    path: PathBuf,
}

impl RootLock {
    pub fn acquire(root: impl AsRef<Path>) -> CoreResult<Self> {
        let root = root.as_ref();
        std::fs::create_dir_all(root).map_err(|e| CoreError::Storage(e.to_string()))?;
        let path = root.join(".radiust.lock");
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;

            let file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&path)
                .map_err(|error| {
                    CoreError::Storage(format!("output root lock unavailable: {error}"))
                })?;
            // Python's local store uses fcntl.flock(LOCK_EX | LOCK_NB) on
            // this exact persistent lock file. Keep the same OS-level
            // protocol so mixed Python/Rust writers serialize correctly.
            let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if result != 0 {
                return Err(CoreError::Storage(format!(
                    "output root locked: {}",
                    io::Error::last_os_error()
                )));
            }
            Ok(Self { file })
        }
        #[cfg(not(unix))]
        {
            let file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
                .map_err(|error| CoreError::Storage(format!("output root locked: {error}")))?;
            Ok(Self { file, path })
        }
    }
}

impl Drop for RootLock {
    fn drop(&mut self) {
        let _ = self.file.sync_all();
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
        }
        #[cfg(not(unix))]
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RootLock;
    use std::process::{Command, Stdio};

    #[test]
    fn child_refuses_contended_lock() {
        let Ok(root) = std::env::var("RADIUST_ROOT_LOCK_CHILD") else {
            return;
        };
        assert!(RootLock::acquire(root).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn persistent_flock_serializes_processes_and_reacquires_after_drop() {
        let root = tempfile::tempdir().unwrap();
        let lock = RootLock::acquire(root.path()).unwrap();
        let lock_path = root.path().join(".radiust.lock");
        assert!(lock_path.exists());

        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "storage::local::tests::child_refuses_contended_lock"])
            .env("RADIUST_ROOT_LOCK_CHILD", root.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(child.success());
        drop(lock);

        // Python keeps the lock inode in place between transactions; a stale
        // lock file must therefore be harmless when nobody holds flock.
        assert!(lock_path.exists());
        let reacquired = RootLock::acquire(root.path()).unwrap();
        drop(reacquired);
    }
}
