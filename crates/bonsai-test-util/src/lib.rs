//! Test doubles for the [`bonsai`] side-effect traits.
//!
//! Split out of the main crate so that `bonsai` can be published to
//! crates.io: a package cannot dev-depend on itself, and path-only
//! dev-dependencies are fine as long as they point elsewhere.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Result, bail};

use bonsai::bcachefs::{Bcachefs, SubvolumeEntry};
use bonsai::restore::Restore;

/// A recorded call on a [`MockBcachefs`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    Snapshot { source: PathBuf, dest: PathBuf },
    Delete { path: PathBuf },
    ListSnapshots { target: PathBuf },
}

/// In-memory [`Bcachefs`] that records calls instead of touching the disk.
#[derive(Debug, Default)]
pub struct MockBcachefs {
    calls: Mutex<Vec<Call>>,
    fail_snapshots: Mutex<u32>,
    fail_deletes: Mutex<u32>,
    list_entries: Mutex<Vec<SubvolumeEntry>>,
    fail_list: Mutex<bool>,
}

impl MockBcachefs {
    /// Create an empty mock.
    pub fn new() -> Self {
        Self::default()
    }

    /// Make the next `n` snapshot calls fail. The attempts are still recorded
    /// in [`Self::calls`] so tests can verify what was attempted.
    pub fn fail_next_snapshots(&self, n: u32) {
        *self.fail_snapshots.lock().unwrap() = n;
    }

    /// Make the next `n` delete calls fail. Deletions are still recorded in
    /// [`Self::calls`] so tests can verify the run continued past the failure.
    pub fn fail_next_deletes(&self, n: u32) {
        *self.fail_deletes.lock().unwrap() = n;
    }

    /// Set the entries returned by [`Bcachefs::list_snapshots`].
    pub fn set_list_entries(&self, entries: Vec<SubvolumeEntry>) {
        *self.list_entries.lock().unwrap() = entries;
    }

    /// Make every [`Bcachefs::list_snapshots`] call fail (e.g. to simulate a
    /// missing binary or an unmounted filesystem).
    pub fn fail_list_snapshots(&self) {
        *self.fail_list.lock().unwrap() = true;
    }

    /// Snapshot of all recorded calls.
    pub fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
}

impl Bcachefs for MockBcachefs {
    fn snapshot(&self, source: &Path, dest: &Path) -> Result<()> {
        self.calls.lock().unwrap().push(Call::Snapshot {
            source: source.to_path_buf(),
            dest: dest.to_path_buf(),
        });
        let mut remaining = self.fail_snapshots.lock().unwrap();
        if *remaining > 0 {
            *remaining -= 1;
            bail!("mock snapshot failure");
        }
        Ok(())
    }

    fn delete(&self, path: &Path) -> Result<()> {
        self.calls.lock().unwrap().push(Call::Delete {
            path: path.to_path_buf(),
        });
        let mut remaining = self.fail_deletes.lock().unwrap();
        if *remaining > 0 {
            *remaining -= 1;
            bail!("mock delete failure");
        }
        Ok(())
    }

    fn list_snapshots(&self, target: &Path) -> Result<Vec<SubvolumeEntry>> {
        self.calls.lock().unwrap().push(Call::ListSnapshots {
            target: target.to_path_buf(),
        });
        if *self.fail_list.lock().unwrap() {
            bail!("mock list failure");
        }
        Ok(self.list_entries.lock().unwrap().clone())
    }
}

/// In-memory [`Restore`] that records calls instead of copying anything.
#[derive(Debug, Default)]
pub struct MockRestore {
    calls: Mutex<Vec<(PathBuf, PathBuf)>>,
}

impl MockRestore {
    /// Create an empty mock.
    pub fn new() -> Self {
        Self::default()
    }

    /// Recorded `(source, dest)` pairs.
    pub fn calls(&self) -> Vec<(PathBuf, PathBuf)> {
        self.calls.lock().unwrap().clone()
    }
}

impl Restore for MockRestore {
    fn restore(&self, source: &Path, dest: &Path) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push((source.to_path_buf(), dest.to_path_buf()));
        Ok(())
    }
}
