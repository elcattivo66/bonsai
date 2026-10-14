use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

use fs2::FileExt;

/// Default path for the bonsai exclusive lock, used to prevent overlapping
/// runs (e.g. a slow snapshot retry loop racing the next timer fire).
pub const DEFAULT_LOCK_PATH: &str = "/var/lock/bonsai.lock";

/// Guard holding the lock file open; dropping it releases the lock.
pub struct LockGuard {
    _file: File,
}

/// Try to acquire an exclusive, non-blocking lock on `path`.
///
/// Returns `Ok(Some(guard))` on success, `Ok(None)` if another process already
/// holds the lock, and `Err` for I/O failures (e.g. the parent directory does
/// not exist or is not writable).
pub fn try_acquire(path: &Path) -> io::Result<Option<LockGuard>> {
    // Append mode (never truncates): a process that fails to acquire the
    // lock still opens the file, and must not clobber it while doing so.
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(LockGuard { _file: file })),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_then_second_acquire_returns_busy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lock");
        let guard = try_acquire(&path)
            .unwrap()
            .expect("first acquire should succeed");
        assert!(try_acquire(&path).unwrap().is_none());
        drop(guard);
        assert!(try_acquire(&path).unwrap().is_some());
    }
}
