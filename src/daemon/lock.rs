//! Single-instance lock
//!
//! The daemon holds an exclusive `flock` on a lock file next to the `IPC` socket for
//! its whole lifetime. The kernel releases the lock when the process exits, however it
//! exits, so a crashed daemon never blocks the next start and a reused PID cannot be
//! mistaken for a running daemon. The file itself is never deleted (deleting a lock
//! file another process may be about to lock reintroduces the race).
//!
//! The file lives where the socket lives (`$XDG_RUNTIME_DIR`, else the verified
//! private `/tmp/pwsw-<uid>/` directory), is opened with `O_NOFOLLOW` and must be a
//! regular file owned by the current user.

use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};

use color_eyre::eyre::{self, Context, Result};
use rustix::fs::{Mode, OFlags};

use crate::ipc;

/// Lock file name, next to the socket
const LOCK_FILE_NAME: &str = "pwsw.lock";

/// An acquired single-instance lock; released when dropped (or the process exits)
#[derive(Debug)]
pub(crate) struct InstanceLock {
    _file: File,
}

/// The lock file path (`$XDG_RUNTIME_DIR/pwsw.lock` or `/tmp/pwsw-<uid>/pwsw.lock`)
///
/// # Errors
/// Returns an error if the private fallback directory is unsafe or cannot be created.
pub(crate) fn lock_path() -> Result<PathBuf> {
    ipc::runtime_file_path(LOCK_FILE_NAME)
}

impl InstanceLock {
    /// Try to take the lock without waiting
    ///
    /// Returns `Ok(None)` if another process holds it.
    ///
    /// # Errors
    /// Returns an error if the lock path is a symlink, cannot be opened or locked, or
    /// is not a regular file owned by the current user.
    pub(crate) fn try_acquire(path: &Path) -> Result<Option<Self>> {
        use std::os::unix::fs::MetadataExt;

        let fd = rustix::fs::open(
            path,
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .with_context(|| {
            format!(
                "Failed to open lock file {} (symlinks are refused)",
                path.display()
            )
        })?;
        let file = File::from(fd);

        let meta = file
            .metadata()
            .with_context(|| format!("Failed to stat lock file: {}", path.display()))?;
        if !meta.file_type().is_file() || meta.uid() != rustix::process::getuid().as_raw() {
            eyre::bail!(
                "Refusing to use lock file {}: not a regular file owned by the current user",
                path.display()
            );
        }

        match file.try_lock() {
            Ok(()) => Ok(Some(Self { _file: file })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(e)) => {
                Err(e).with_context(|| format!("Failed to lock {}", path.display()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_second_instance_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(LOCK_FILE_NAME);

        let first = InstanceLock::try_acquire(&path).expect("lock io");
        assert!(first.is_some());
        // flock locks belong to the open file description, so a second open in the
        // same process conflicts just like another process would
        let second = InstanceLock::try_acquire(&path).expect("lock io");
        assert!(second.is_none(), "second instance must be refused");

        drop(first);
        assert!(
            InstanceLock::try_acquire(&path).expect("lock io").is_some(),
            "lock is free again once the holder is gone"
        );
    }

    #[test]
    fn test_stale_lock_file_does_not_block() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(LOCK_FILE_NAME);
        // A leftover file from a crashed daemon (contents irrelevant, nobody holds it)
        std::fs::write(&path, b"12345\n").expect("write");
        assert!(InstanceLock::try_acquire(&path).expect("lock io").is_some());
    }

    #[test]
    fn test_symlinked_lock_path_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("someone-elses-file");
        std::fs::write(&target, b"").expect("write");
        let path = dir.path().join(LOCK_FILE_NAME);
        std::os::unix::fs::symlink(&target, &path).expect("symlink");
        assert!(InstanceLock::try_acquire(&path).is_err());

        // A dangling symlink must not create its target either
        let dangling = dir.path().join("dangling.lock");
        let missing = dir.path().join("missing");
        std::os::unix::fs::symlink(&missing, &dangling).expect("symlink");
        assert!(InstanceLock::try_acquire(&dangling).is_err());
        assert!(!missing.exists());
    }

    #[test]
    fn test_non_regular_lock_path_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(InstanceLock::try_acquire(dir.path()).is_err());
    }
}
