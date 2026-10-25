use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDateTime, Utc};
use chrono_tz::Tz;
use serde::Serialize;
use tracing::{info, warn};

use crate::bcachefs::{Bcachefs, RealBcachefs};
use crate::config::{Config, Target};
use crate::lock;
use crate::policy;
use crate::restore::{Restore, RsyncRestore};
use crate::snapshot::{Snapshot, Tier};

/// Outcome of trying to take the run lock.
enum Locking {
    Held(lock::LockGuard),
    Busy,
    Unavailable,
}

fn acquire_lock() -> Locking {
    match lock::try_acquire(Path::new(lock::DEFAULT_LOCK_PATH)) {
        Ok(Some(guard)) => Locking::Held(guard),
        Ok(None) => Locking::Busy,
        Err(error) => {
            warn!(error = %error, "could not acquire lock, proceeding without it");
            Locking::Unavailable
        }
    }
}

/// Run the snapshot + prune cycle across the configured targets.
///
/// Acquires an exclusive, non-blocking lock so that overlapping runs (e.g. a
/// slow retry loop racing the next timer fire) cannot prune each other's
/// freshly-created snapshots. A JSON dry run prints the plan; a real JSON run
/// prints the completed operations and per-target errors.
pub fn snapshot(config: &Config, dry_run: bool, only: &[PathBuf], json: bool) -> Result<()> {
    let _lock = match acquire_lock() {
        Locking::Held(guard) => Some(guard),
        Locking::Busy => {
            info!("another bonsai run is in progress, skipping");
            if json {
                println!("{}", skipped_report_json(dry_run)?);
            }
            return Ok(());
        }
        Locking::Unavailable => None,
    };

    let now = local_datetime(config.tz(), Utc::now());

    if json && dry_run {
        let report = plan_report(config, only, now)?;
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    let bcachefs = RealBcachefs::default();
    if json {
        let report = snapshot_report_with(&bcachefs, config, now, false, only);
        println!("{}", serde_json::to_string_pretty(&report)?);
        return report.into_result();
    }
    snapshot_with(&bcachefs, config, now, dry_run, only)
}

fn skipped_report_json(dry_run: bool) -> Result<String> {
    if dry_run {
        Ok(serde_json::to_string_pretty(&PlanReport {
            skipped: true,
            targets: Vec::new(),
        })?)
    } else {
        Ok(serde_json::to_string_pretty(&SnapshotReport {
            skipped: true,
            targets: Vec::new(),
        })?)
    }
}

/// Current wall-clock time in `tz`, used to decide day/hour boundaries and to
/// name snapshots.
fn local_datetime(tz: Tz, utc: DateTime<Utc>) -> NaiveDateTime {
    utc.with_timezone(&tz).naive_local()
}

/// Snapshot cycle with an injected filesystem backend and clock (for tests).
///
/// Failures are collected instead of being fatal: a target whose snapshots
/// cannot be read or created, or whose deletes fail, does not stop the
/// remaining targets. If anything failed, a single aggregated error is
/// returned (and the process exits non-zero).
pub fn snapshot_with<B: Bcachefs>(
    bcachefs: &B,
    config: &Config,
    now: NaiveDateTime,
    dry_run: bool,
    only: &[PathBuf],
) -> Result<()> {
    snapshot_report_with(bcachefs, config, now, dry_run, only).into_result()
}

/// Run the snapshot cycle and return completed operations and per-target errors.
pub fn snapshot_report_with<B: Bcachefs>(
    bcachefs: &B,
    config: &Config,
    now: NaiveDateTime,
    dry_run: bool,
    only: &[PathBuf],
) -> SnapshotReport {
    let mut targets = Vec::new();

    for target in config.selected(only) {
        if !target.path.is_dir() {
            warn!(path = %target.path.display(), "target does not exist, skipping");
            continue;
        }

        let dir = Snapshot::dir_for(&target.path, &config.snapshot_subdir);
        targets.push(process_target(bcachefs, target, &dir, now, dry_run));
    }

    SnapshotReport {
        skipped: false,
        targets,
    }
}

/// Read, plan and apply the work for a single target.
///
/// Delete failures are recorded and do not abort the target; snapshot-create
/// failures abort this target but not the remaining targets.
fn process_target<B: Bcachefs>(
    bcachefs: &B,
    target: &Target,
    dir: &Path,
    now: NaiveDateTime,
    dry_run: bool,
) -> TargetSnapshotResult {
    let mut result = TargetSnapshotResult {
        path: target.path.display().to_string(),
        created: Vec::new(),
        pruned: Vec::new(),
        errors: Vec::new(),
    };
    let existing = match read_snapshots(dir) {
        Ok(existing) => existing,
        Err(error) => {
            result
                .errors
                .push(format!("{}: {error:#}", target.path.display()));
            return result;
        }
    };
    let plan = policy::plan(now, dir, &existing, target.retention);

    info!(
        path = %target.path.display(),
        create = plan.create.len(),
        prune = plan.prune.len(),
        "planned"
    );

    for snap in &plan.create {
        if dry_run {
            info!(path = %snap.path.display(), "would create");
            continue;
        }
        if let Some(parent) = snap.path.parent()
            && let Err(error) = std::fs::create_dir_all(parent)
        {
            result.errors.push(format!(
                "{}: creating {}: {error}",
                target.path.display(),
                parent.display()
            ));
            return result;
        }
        if snap.path.exists() {
            warn!(path = %snap.path.display(), "already exists, skipping");
            continue;
        }
        match bcachefs.snapshot(&target.path, &snap.path) {
            Ok(()) => {
                info!(path = %snap.path.display(), "created");
                result.created.push(SnapshotInfo::from(snap));
            }
            Err(error) => {
                result
                    .errors
                    .push(format!("{}: {error:#}", target.path.display()));
                return result;
            }
        }
    }

    for snap in &plan.prune {
        if dry_run {
            info!(path = %snap.path.display(), "would prune");
            continue;
        }
        match bcachefs.delete(&snap.path) {
            Ok(()) => {
                info!(path = %snap.path.display(), "pruned");
                result.pruned.push(SnapshotInfo::from(snap));
            }
            Err(error) => result
                .errors
                .push(format!("{}: {error:#}", snap.path.display())),
        }
    }

    result
}

/// List the snapshots present for each selected target.
pub fn list(config: &Config, only: &[PathBuf], json: bool, human_readable: bool) -> Result<()> {
    let bcachefs = RealBcachefs::default();
    let report = list_report_with(&bcachefs, config, only)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    for target in &report.targets {
        println!("{}:", target.path);
        for snapshot in &target.snapshots {
            match size_display(
                snapshot.size_bytes,
                snapshot.size.as_deref(),
                human_readable,
            ) {
                Some(size) => println!("  {}  {size}", snapshot.name),
                None => println!("  {}", snapshot.name),
            }
        }
        for unmanaged in &target.unmanaged {
            let size = size_display(
                unmanaged.size_bytes,
                unmanaged.size.as_deref(),
                human_readable,
            );
            match size {
                Some(size) => println!("  {}  {size} (unmanaged)", unmanaged.name),
                None => println!("  {}  (unmanaged)", unmanaged.name),
            }
        }
    }
    Ok(())
}

/// Size column for the text output of `list`: bcachefs' human-readable string
/// with `--human-readable`, raw bytes otherwise; falls back to whichever is
/// available.
fn size_display(bytes: Option<u64>, human: Option<&str>, human_readable: bool) -> Option<String> {
    if human_readable {
        human
            .map(str::to_owned)
            .or_else(|| bytes.map(|b| b.to_string()))
    } else {
        bytes
            .map(|b| b.to_string())
            .or_else(|| human.map(str::to_owned))
    }
}

/// Serializable summary of a snapshot, used for `--json` output.
#[derive(Debug, Serialize)]
pub struct SnapshotInfo {
    pub tier: Tier,
    pub time: NaiveDateTime,
    pub name: String,
    pub path: PathBuf,
    /// Size in bytes as reported by bcachefs; absent when unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// Human-readable size as printed by bcachefs (e.g. `1.56T`); absent when
    /// unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
}

impl From<&Snapshot> for SnapshotInfo {
    fn from(snapshot: &Snapshot) -> Self {
        SnapshotInfo {
            tier: snapshot.tier,
            time: snapshot.datetime,
            name: snapshot.file_name(),
            path: snapshot.path.clone(),
            size_bytes: None,
            size: None,
        }
    }
}

/// A subvolume inside the snapshot directory whose name does not follow the
/// `<tier>-<date>` scheme, i.e. one bonsai does not manage.
#[derive(Debug, Serialize)]
pub struct UnmanagedInfo {
    pub name: String,
    pub path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
}

/// `--json` output of `list`.
#[derive(Debug, Serialize)]
pub struct ListReport {
    pub targets: Vec<TargetSnapshots>,
}

/// Snapshots of a single target (newest first), plus subvolumes in the
/// snapshot directory that bonsai does not manage.
#[derive(Debug, Serialize)]
pub struct TargetSnapshots {
    pub path: String,
    pub snapshots: Vec<SnapshotInfo>,
    pub unmanaged: Vec<UnmanagedInfo>,
}

/// `--json` output of `snapshot --dry-run`.
#[derive(Debug, Serialize)]
pub struct PlanReport {
    pub skipped: bool,
    pub targets: Vec<TargetPlan>,
}

/// `--json` output of a real snapshot run.
#[derive(Debug, Serialize)]
pub struct SnapshotReport {
    pub skipped: bool,
    pub targets: Vec<TargetSnapshotResult>,
}

impl SnapshotReport {
    fn into_result(self) -> Result<()> {
        let errors: Vec<String> = self
            .targets
            .iter()
            .flat_map(|target| target.errors.iter().cloned())
            .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            anyhow::bail!(
                "{} operation(s) failed:\n{}",
                errors.len(),
                errors.join("\n")
            );
        }
    }
}

/// Completed operations and errors for one configured target.
#[derive(Debug, Serialize)]
pub struct TargetSnapshotResult {
    pub path: String,
    pub created: Vec<SnapshotInfo>,
    pub pruned: Vec<SnapshotInfo>,
    pub errors: Vec<String>,
}

/// Planned work for a single target.
#[derive(Debug, Serialize)]
pub struct TargetPlan {
    pub path: String,
    pub create: Vec<SnapshotInfo>,
    pub prune: Vec<SnapshotInfo>,
}

/// Build the `list` report (newest first per target).
///
/// Sizes come from `bcachefs subvolume list --json` where available; if the
/// command fails (not mounted, old bcachefs, ...) a warning is logged and the
/// report simply carries no sizes — the directory scan remains the basis.
/// Subvolumes inside the snapshot directory that do not follow the naming
/// scheme are reported as `unmanaged`, and directory entries missing from the
/// subvolume list are warned about.
pub fn list_report_with<B: Bcachefs>(
    bcachefs: &B,
    config: &Config,
    only: &[PathBuf],
) -> Result<ListReport> {
    let mut targets = Vec::new();
    for target in config.selected(only) {
        let dir = Snapshot::dir_for(&target.path, &config.snapshot_subdir);
        let mut snapshots = read_snapshots(&dir)?;
        snapshots.sort_by_key(|b| std::cmp::Reverse(b.file_name()));

        let entries = if target.path.is_dir() {
            match bcachefs.list_snapshots(&target.path) {
                Ok(entries) => Some(entries),
                Err(error) => {
                    warn!(
                        path = %target.path.display(),
                        error = %error,
                        "could not list bcachefs subvolumes, sizes unavailable"
                    );
                    None
                }
            }
        } else {
            None
        };

        let prefix = format!("{}/", config.snapshot_subdir);
        let mut infos = Vec::with_capacity(snapshots.len());
        for snapshot in &snapshots {
            let mut info = SnapshotInfo::from(snapshot);
            if let Some(entries) = &entries {
                let relative = format!("{prefix}{}", info.name);
                match entries.iter().find(|entry| entry.path == relative) {
                    Some(entry) => {
                        info.size_bytes = entry.size_bytes;
                        info.size = entry.size.clone();
                    }
                    None => warn!(
                        path = %snapshot.path.display(),
                        "snapshot directory is not a bcachefs subvolume"
                    ),
                }
            }
            infos.push(info);
        }

        let mut unmanaged = Vec::new();
        if let Some(entries) = &entries {
            for entry in entries {
                let Some(name) = entry.path.strip_prefix(&prefix) else {
                    continue;
                };
                // Only direct children of the snapshot directory are
                // considered, matching what the directory scan sees.
                if name.is_empty() || name.contains('/') {
                    continue;
                }
                if Snapshot::parse_entry(&dir, name).is_some() {
                    continue;
                }
                unmanaged.push(UnmanagedInfo {
                    name: name.to_owned(),
                    path: dir.join(name),
                    size_bytes: entry.size_bytes,
                    size: entry.size.clone(),
                });
            }
        }

        targets.push(TargetSnapshots {
            path: target.path.display().to_string(),
            snapshots: infos,
            unmanaged,
        });
    }
    Ok(ListReport { targets })
}

/// Build the dry-run plan report without touching the filesystem.
pub fn plan_report(config: &Config, only: &[PathBuf], now: NaiveDateTime) -> Result<PlanReport> {
    let mut targets = Vec::new();
    for target in config.selected(only) {
        if !target.path.is_dir() {
            continue;
        }
        let dir = Snapshot::dir_for(&target.path, &config.snapshot_subdir);
        let existing = read_snapshots(&dir)?;
        let plan = policy::plan(now, &dir, &existing, target.retention);
        targets.push(TargetPlan {
            path: target.path.display().to_string(),
            create: plan.create.iter().map(SnapshotInfo::from).collect(),
            prune: plan.prune.iter().map(SnapshotInfo::from).collect(),
        });
    }
    Ok(PlanReport {
        skipped: false,
        targets,
    })
}

/// Resolve the paths a `delete` invocation targets.
///
/// The target must be configured, and every name must parse as a snapshot that
/// resolves to a direct child of the target's snapshot directory.
pub fn resolve_delete_paths(
    config: &Config,
    target: &Path,
    names: &[String],
) -> Result<Vec<PathBuf>> {
    let target_buf = target.to_path_buf();
    let selected = config.selected(std::slice::from_ref(&target_buf));
    let Some(target) = selected.first() else {
        anyhow::bail!("target not in config: {}", target_buf.display());
    };
    let dir = Snapshot::dir_for(&target.path, &config.snapshot_subdir);

    let mut paths = Vec::new();
    for name in names {
        let snapshot = Snapshot::parse_entry(&dir, name)
            .ok_or_else(|| anyhow::anyhow!("not a snapshot name: {name}"))?;
        if snapshot.path.parent() != Some(dir.as_path()) {
            anyhow::bail!(
                "refusing to delete {}: not inside {}",
                snapshot.path.display(),
                dir.display()
            );
        }
        paths.push(snapshot.path);
    }
    Ok(paths)
}

/// Delete the given snapshot paths, collecting per-path failures.
pub fn delete_paths<B: Bcachefs>(bcachefs: &B, paths: &[PathBuf]) -> Result<()> {
    let mut failures: Vec<anyhow::Error> = Vec::new();
    for path in paths {
        match bcachefs.delete(path) {
            Ok(()) => info!(path = %path.display(), "deleted"),
            Err(error) => failures.push(error.context(path.display().to_string())),
        }
    }
    if !failures.is_empty() {
        let messages: Vec<String> = failures.iter().map(|e| format!("{e:#}")).collect();
        anyhow::bail!(
            "{} deletion(s) failed:\n{}",
            failures.len(),
            messages.join("\n")
        );
    }
    Ok(())
}

/// Delete individual snapshots (`bonsai delete <target> <name>...`).
///
/// Requires interactive confirmation (there is deliberately no `--force`);
/// with `dry_run` it only prints what would be deleted. Takes the run lock so
/// it cannot race a concurrent prune.
pub fn delete(config: &Config, target: &Path, names: &[String], dry_run: bool) -> Result<()> {
    let paths = resolve_delete_paths(config, target, names)?;

    if dry_run {
        for path in &paths {
            println!("would delete {}", path.display());
        }
        return Ok(());
    }

    eprintln!("About to delete:");
    for path in &paths {
        eprintln!("  {}", path.display());
    }
    if !confirm()? {
        eprintln!("aborted");
        return Ok(());
    }

    let _lock = match acquire_lock() {
        Locking::Held(guard) => Some(guard),
        Locking::Busy => anyhow::bail!("another bonsai run is in progress, try again later"),
        Locking::Unavailable => None,
    };

    let bcachefs = RealBcachefs::default();
    delete_paths(&bcachefs, &paths)
}

/// Ask for confirmation on stderr; returns false on EOF (non-interactive).
fn confirm() -> Result<bool> {
    use std::io::Write;
    eprint!("Delete these snapshots? [y/N] ");
    std::io::stderr().flush().context("flushing stderr")?;
    let mut input = String::new();
    let read = std::io::stdin()
        .read_line(&mut input)
        .context("reading confirmation")?;
    if read == 0 {
        return Ok(false);
    }
    let answer = input.trim().to_ascii_lowercase();
    Ok(answer == "y" || answer == "yes")
}

/// Resolve the source path inside a snapshot to restore from.
///
/// The target must be configured and `name` must be a snapshot of it. An
/// optional `subpath` selects a file or subtree inside the snapshot and must be
/// relative and free of `..`.
pub fn resolve_restore_source(
    config: &Config,
    target: &Path,
    name: &str,
    subpath: Option<&Path>,
) -> Result<PathBuf> {
    let target_buf = target.to_path_buf();
    let selected = config.selected(std::slice::from_ref(&target_buf));
    let Some(target) = selected.first() else {
        anyhow::bail!("target not in config: {}", target_buf.display());
    };
    let dir = Snapshot::dir_for(&target.path, &config.snapshot_subdir);
    let snapshot = Snapshot::parse_entry(&dir, name)
        .ok_or_else(|| anyhow::anyhow!("not a snapshot name: {name}"))?;
    if snapshot.path.parent() != Some(dir.as_path()) {
        anyhow::bail!(
            "refusing to restore {}: not inside {}",
            snapshot.path.display(),
            dir.display()
        );
    }

    let mut source = snapshot.path;
    if let Some(subpath) = subpath {
        if subpath.is_absolute() {
            anyhow::bail!("--path must be relative: {}", subpath.display());
        }
        if subpath.components().any(|c| c == Component::ParentDir) {
            anyhow::bail!("--path must not contain '..': {}", subpath.display());
        }
        source = source.join(subpath);
    }
    if !source.exists() {
        anyhow::bail!("no such path in snapshot: {}", source.display());
    }
    Ok(source)
}

/// Copy a resolved source into `to` using the given backend.
pub fn restore_into<R: Restore>(restore: &R, source: &Path, to: &Path) -> Result<()> {
    restore.restore(source, to)?;
    info!(source = %source.display(), to = %to.display(), "restored");
    Ok(())
}

/// Restore a file or subtree from a snapshot (`bonsai restore ...`).
///
/// Non-destructive: it copies into `--to` and never modifies the target in
/// place. Takes the run lock so a concurrent prune cannot remove the snapshot
/// mid-copy.
pub fn restore(
    config: &Config,
    target: &Path,
    name: &str,
    subpath: Option<&Path>,
    to: &Path,
) -> Result<()> {
    let source = resolve_restore_source(config, target, name, subpath)?;

    let _lock = match acquire_lock() {
        Locking::Held(guard) => Some(guard),
        Locking::Busy => anyhow::bail!("another bonsai run is in progress, try again later"),
        Locking::Unavailable => None,
    };

    restore_into(&RsyncRestore, &source, to)
}

/// Read and parse snapshot entries from a directory, skipping foreign files.
pub fn read_snapshots(dir: &Path) -> Result<Vec<Snapshot>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut snapshots = Vec::new();
    let entries = std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            warn!(path = ?entry.path(), "skipping non-UTF-8 directory entry");
            continue;
        };
        let Some(snapshot) = Snapshot::parse_entry(dir, name) else {
            continue;
        };
        // Only directories are ever snapshots (the shell script checked
        // `[[ -d ]]`); a regular file with a snapshot-like name is reported
        // and ignored instead of being planned for deletion.
        let is_dir = std::fs::metadata(entry.path())
            .map(|metadata| metadata.is_dir())
            .unwrap_or(false);
        if !is_dir {
            warn!(path = %entry.path().display(), "snapshot-like entry is not a directory, ignoring");
            continue;
        }
        snapshots.push(snapshot);
    }
    Ok(snapshots)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_datetime_uses_timezone() {
        let tz: Tz = "Europe/Berlin".parse().unwrap();
        let utc = DateTime::parse_from_rfc3339("2026-01-01T00:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        // Berlin is UTC+1 in winter, so 00:30Z is 01:30 local.
        assert_eq!(
            local_datetime(tz, utc),
            NaiveDateTime::new(
                chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
                chrono::NaiveTime::from_hms_opt(1, 30, 0).unwrap(),
            )
        );
    }

    #[test]
    fn local_datetime_in_utc_is_identity() {
        let utc = DateTime::parse_from_rfc3339("2026-06-15T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(local_datetime(Tz::UTC, utc), utc.naive_utc());
    }

    #[test]
    fn size_display_prefers_requested_format_with_fallback() {
        // Default: bytes; human-readable: bcachefs string.
        assert_eq!(
            size_display(Some(2048), Some("1K"), false).as_deref(),
            Some("2048")
        );
        assert_eq!(
            size_display(Some(2048), Some("1K"), true).as_deref(),
            Some("1K")
        );
        // Fallback to whichever is available.
        assert_eq!(size_display(None, Some("1K"), false).as_deref(), Some("1K"));
        assert_eq!(
            size_display(Some(2048), None, true).as_deref(),
            Some("2048")
        );
        assert_eq!(size_display(None, None, true), None);
    }

    #[test]
    fn skipped_json_report_is_explicit_for_both_modes() {
        for dry_run in [false, true] {
            let json = skipped_report_json(dry_run).unwrap();
            let value: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert_eq!(value["skipped"], true);
            assert_eq!(value["targets"], serde_json::json!([]));
        }
    }
}
