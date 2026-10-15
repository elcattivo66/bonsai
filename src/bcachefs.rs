use std::path::Path;
#[cfg(any(test, feature = "test-util"))]
use std::path::PathBuf;
use std::process::Command;
#[cfg(any(test, feature = "test-util"))]
use std::sync::Mutex;
use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tracing::warn;

/// Side effects against a bcachefs filesystem.
///
/// Implemented for real by shelling out to the `bcachefs` CLI and mocked in
/// tests, so the planner and orchestration can be exercised without a mount.
pub trait Bcachefs {
    /// Create a snapshot of `source` at `dest`.
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

impl RealBcachefs {
    /// `bcachefs subvolume snapshot <source> <dest>`.
    fn snapshot_command(source: &Path, dest: &Path) -> Command {
        let mut command = Command::new("bcachefs");
        command
            .args(["subvolume", "snapshot"])
            .arg(source)
            .arg(dest);
        command
    }

    /// `bcachefs subvolume delete <path>`.
    fn delete_command(path: &Path) -> Command {
        let mut command = Command::new("bcachefs");
        command.args(["subvolume", "delete"]).arg(path);
        command
    }
}

impl Bcachefs for RealBcachefs {
    fn snapshot(&self, source: &Path, dest: &Path) -> Result<()> {
        let attempts = self.retries.max(1);
        let mut last_err = None;

        for attempt in 1..=attempts {
            match run(Self::snapshot_command(source, dest)) {
                Ok(()) => return Ok(()),
                Err(err) => {
                    // A missing or unrunnable binary will not fix itself
                    // within the retry window; fail fast instead of sleeping.
                    if is_not_retryable(&err) {
                        return Err(err);
                    }
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
        run(Self::delete_command(path))
    }
}

fn run(mut command: Command) -> Result<()> {
    let described = describe(&command);
    let output = command
        .output()
        .with_context(|| format!("running {described}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{described} failed: {}", stderr.trim());
    }
    Ok(())
}

/// Render a command as a human-readable string for logs and errors.
fn describe(command: &Command) -> String {
    let program = command.get_program().to_string_lossy().into_owned();
    let args = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    if args.is_empty() {
        program
    } else {
        format!("{program} {args}")
    }
}

/// Whether a failure is permanent in the current environment (the `bcachefs`
/// binary is missing or not executable) and retrying cannot help.
fn is_not_retryable(error: &anyhow::Error) -> bool {
    use std::io::ErrorKind;
    error
        .root_cause()
        .downcast_ref::<std::io::Error>()
        .is_some_and(|cause| {
            matches!(
                cause.kind(),
                ErrorKind::NotFound | ErrorKind::PermissionDenied
            )
        })
}

/// A recorded call on a [`MockBcachefs`].
#[cfg(any(test, feature = "test-util"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    Snapshot { source: PathBuf, dest: PathBuf },
    Delete { path: PathBuf },
}

/// In-memory [`Bcachefs`] that records calls instead of touching the disk.
///
/// Only compiled for tests (and behind the `test-util` feature for
/// integration tests), so it never ships in release binaries.
#[cfg(any(test, feature = "test-util"))]
#[derive(Debug, Default)]
pub struct MockBcachefs {
    calls: Mutex<Vec<Call>>,
    fail_snapshots: Mutex<u32>,
    fail_deletes: Mutex<u32>,
}

#[cfg(any(test, feature = "test-util"))]
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

    /// Snapshot of all recorded calls.
    pub fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
}

#[cfg(any(test, feature = "test-util"))]
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(command: &Command) -> Vec<String> {
        command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn snapshot_passes_source_and_dest() {
        let command = RealBcachefs::snapshot_command(Path::new("/a"), Path::new("/a/.snapshots/x"));
        assert_eq!(
            args(&command),
            ["subvolume", "snapshot", "/a", "/a/.snapshots/x"]
        );
    }

    #[test]
    fn delete_passes_a_single_path() {
        let command = RealBcachefs::delete_command(Path::new("/a/.snapshots/daily-20260101"));
        assert_eq!(
            args(&command),
            ["subvolume", "delete", "/a/.snapshots/daily-20260101"]
        );
    }

    #[test]
    fn missing_binary_is_not_retryable() {
        let err = anyhow::Error::from(std::io::Error::from(std::io::ErrorKind::NotFound))
            .context("running bcachefs subvolume snapshot");
        assert!(is_not_retryable(&err));

        let err = anyhow::anyhow!("bcachefs subvolume snapshot failed: busy");
        assert!(!is_not_retryable(&err));
    }
}
