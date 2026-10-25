use std::path::Path;
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
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
    /// List subvolumes and snapshots under `target`, with sizes where
    /// reported. Paths are relative to `target` and use `/` separators.
    fn list_snapshots(&self, target: &Path) -> Result<Vec<SubvolumeEntry>>;
}

/// A subvolume or snapshot reported by `bcachefs subvolume list --json`.
///
/// Only the fields bonsai uses are kept; the raw JSON carries more (IDs,
/// timestamps, parents) and unknown fields are ignored, so output from newer
/// bcachefs versions still parses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubvolumeEntry {
    /// Path relative to the listed target, e.g. `.snapshots/daily-20261019`.
    pub path: String,
    /// Size in bytes (`sectors * 512`), when reported.
    pub size_bytes: Option<u64>,
    /// Human-readable size as printed by bcachefs (e.g. `1.56T`), when reported.
    pub size: Option<String>,
}

/// Wire format of one entry in `bcachefs subvolume list --json` output.
#[derive(Debug, Deserialize)]
struct RawSubvolume {
    #[serde(default)]
    path: String,
    #[serde(default)]
    sectors: Option<u64>,
    #[serde(default)]
    size: Option<String>,
}

/// Parse the JSON array printed by `bcachefs subvolume list --json`.
///
/// Entries without a path (e.g. a filesystem root placeholder) are dropped.
pub fn parse_subvolume_list(json: &[u8]) -> Result<Vec<SubvolumeEntry>> {
    let raw: Vec<RawSubvolume> =
        serde_json::from_slice(json).context("parsing `bcachefs subvolume list --json` output")?;
    Ok(raw
        .into_iter()
        .filter(|entry| !entry.path.is_empty())
        .map(|entry| SubvolumeEntry {
            path: entry.path,
            size_bytes: entry.sectors.map(|sectors| sectors * 512),
            size: entry.size.filter(|size| !size.is_empty()),
        })
        .collect())
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

    /// `bcachefs subvolume list --json -s <target>`.
    fn list_command(target: &Path) -> Command {
        let mut command = Command::new("bcachefs");
        command
            .args(["subvolume", "list", "--json", "-s"])
            .arg(target);
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

    fn list_snapshots(&self, target: &Path) -> Result<Vec<SubvolumeEntry>> {
        let output = run_output(Self::list_command(target))?;
        parse_subvolume_list(&output.stdout)
    }
}

fn run(command: Command) -> Result<()> {
    run_output(command).map(|_| ())
}

fn run_output(mut command: Command) -> Result<std::process::Output> {
    let described = describe(&command);
    let output = command
        .output()
        .with_context(|| format!("running {described}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{described} failed: {}", stderr.trim());
    }
    Ok(output)
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

    #[test]
    fn list_passes_json_and_snapshots_flags() {
        let command = RealBcachefs::list_command(Path::new("/spool/garage"));
        assert_eq!(
            args(&command),
            ["subvolume", "list", "--json", "-s", "/spool/garage"]
        );
    }

    #[test]
    fn parses_subvolume_list_json() {
        // Trimmed real-world output from bcachefs 1.39.7; unknown fields
        // (otime, subvolid, ...) must be ignored, sectors convert to bytes.
        let json = br#"[
            {
                "otime": "2026-10-11 20:52",
                "otime_unix": 1791744743,
                "path": ".snapshots/weekly-20261011",
                "sectors": 3348588604,
                "size": "1.56T",
                "snapshot_parent": "/garage",
                "subvolid": 332
            },
            {
                "path": ".snapshots/daily-20261019",
                "sectors": 1024,
                "size": "512K",
                "subvolid": 361
            }
        ]"#;
        let entries = parse_subvolume_list(json).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0],
            SubvolumeEntry {
                path: ".snapshots/weekly-20261011".to_owned(),
                size_bytes: Some(3348588604 * 512),
                size: Some("1.56T".to_owned()),
            }
        );
        assert_eq!(entries[1].size_bytes, Some(1024 * 512));
    }

    #[test]
    fn subvolume_list_parsing_is_tolerant() {
        // Missing optional fields and an empty path (root placeholder).
        let entries = parse_subvolume_list(br#"[{"path": ""}, {"path": "a/b"}]"#).unwrap();
        assert_eq!(
            entries,
            vec![SubvolumeEntry {
                path: "a/b".to_owned(),
                size_bytes: None,
                size: None,
            }]
        );

        assert!(parse_subvolume_list(b"not json").is_err());
    }
}
