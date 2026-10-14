use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::thread::sleep;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tracing::warn;

/// Side effects against a bcachefs filesystem.
///
/// Implemented for real by shelling out to the `bcachefs` CLI and mocked in
/// tests, so the planner and orchestration can be exercised without a mount.
pub trait Bcachefs {
    /// Create a read-only snapshot of `source` at `dest`.
    fn snapshot(&self, source: &Path, dest: &Path) -> Result<()>;
    /// Delete the subvolume at `path`.
    fn delete(&self, path: &Path) -> Result<()>;
}

/// The real implementation, invoking `bcachefs`.
#[derive(Debug, Clone)]
pub struct RealBcachefs {
    /// Number of attempts for a snapshot before giving up.
    pub retries: u32,
    /// Delay between snapshot attempts.
    pub retry_delay: Duration,
}

impl Default for RealBcachefs {
    fn default() -> Self {
        // Mirrors the shell script: 6 attempts, 10s apart, to ride out
        // transient failures while bcachefs is still finishing recovery.
        RealBcachefs {
            retries: 6,
            retry_delay: Duration::from_secs(10),
        }
    }
}

impl Bcachefs for RealBcachefs {
    fn snapshot(&self, source: &Path, dest: &Path) -> Result<()> {
        let attempts = self.retries.max(1);
        let mut last_err = None;

        for attempt in 1..=attempts {
            match run_bcachefs(&["subvolume", "snapshot"], source, dest) {
                Ok(()) => return Ok(()),
                Err(err) => {
                    last_err = Some(err);
                    if attempt < attempts {
                        warn!(
                            attempt,
                            attempts,
                            delay_secs = self.retry_delay.as_secs(),
                            dest = %dest.display(),
                            "snapshot failed, retrying"
                        );
                        sleep(self.retry_delay);
                    }
                }
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("snapshot failed")))
    }

    fn delete(&self, path: &Path) -> Result<()> {
        run_bcachefs(&["subvolume", "delete"], path, path)
    }
}

fn run_bcachefs(args: &[&str], source: &Path, dest: &Path) -> Result<()> {
    let output = Command::new("bcachefs")
        .args(args)
        .arg(source)
        .arg(dest)
        .output()
        .with_context(|| format!("running bcachefs {}", args.join(" ")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "bcachefs {} {} {} failed: {}",
            args.join(" "),
            source.display(),
            dest.display(),
            stderr.trim()
        );
    }
    Ok(())
}

/// A recorded call on a [`MockBcachefs`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    Snapshot { source: PathBuf, dest: PathBuf },
    Delete { path: PathBuf },
}

/// In-memory [`Bcachefs`] that records calls instead of touching the disk.
#[derive(Debug, Default)]
pub struct MockBcachefs {
    calls: Mutex<Vec<Call>>,
    fail_snapshots: Mutex<u32>,
}

impl MockBcachefs {
    /// Create an empty mock.
    pub fn new() -> Self {
        Self::default()
    }

    /// Make the next `n` snapshot calls fail, exercising retry paths.
    pub fn fail_next_snapshots(&self, n: u32) {
        *self.fail_snapshots.lock().unwrap() = n;
    }

    /// Snapshot of all recorded calls.
    pub fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
}

impl Bcachefs for MockBcachefs {
    fn snapshot(&self, source: &Path, dest: &Path) -> Result<()> {
        let mut remaining = self.fail_snapshots.lock().unwrap();
        if *remaining > 0 {
            *remaining -= 1;
            bail!("mock snapshot failure");
        }
        drop(remaining);
        self.calls.lock().unwrap().push(Call::Snapshot {
            source: source.to_path_buf(),
            dest: dest.to_path_buf(),
        });
        Ok(())
    }

    fn delete(&self, path: &Path) -> Result<()> {
        self.calls.lock().unwrap().push(Call::Delete {
            path: path.to_path_buf(),
        });
        Ok(())
    }
}
