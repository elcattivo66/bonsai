use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::NaiveDateTime;
use tracing::{info, warn};

use crate::bcachefs::{Bcachefs, RealBcachefs};
use crate::config::{Config, Target};
use crate::lock;
use crate::policy;
use crate::snapshot::Snapshot;

/// Run the snapshot + prune cycle across the configured targets.
///
/// Acquires an exclusive, non-blocking lock so that overlapping runs (e.g. a
/// slow retry loop racing the next timer fire) cannot prune each other's
/// freshly-created snapshots.
pub fn snapshot(config: &Config, dry_run: bool, only: &[PathBuf]) -> Result<()> {
    let _lock = match lock::try_acquire(Path::new(lock::DEFAULT_LOCK_PATH)) {
        Ok(Some(guard)) => Some(guard),
        Ok(None) => {
            info!("another bonsai run is in progress, skipping");
            return Ok(());
        }
        Err(error) => {
            warn!(error = %error, "could not acquire lock, proceeding without it");
            None
        }
    };

    let bcachefs = RealBcachefs::default();
    let now = chrono::Utc::now().naive_utc();
    snapshot_with(&bcachefs, config, now, dry_run, only)
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
    let mut failures: Vec<anyhow::Error> = Vec::new();

    for target in config.selected(only) {
        if !target.path.is_dir() {
            warn!(path = %target.path.display(), "target does not exist, skipping");
            continue;
        }

        let dir = Snapshot::dir_for(&target.path, &config.snapshot_subdir);
        match process_target(bcachefs, target, &dir, now, dry_run, &mut failures) {
            Ok(()) => {}
            Err(error) => failures.push(error.context(target.path.display().to_string())),
        }
    }

    if !failures.is_empty() {
        let n = failures.len();
        let messages: Vec<String> = failures.iter().map(|e| format!("{e:#}")).collect();
        return Err(anyhow::anyhow!(
            "{n} operation(s) failed:\n{}",
            messages.join("\n")
        ));
    }

    Ok(())
}

/// Read, plan and apply the work for a single target.
///
/// Delete failures are collected into `failures` and do not abort the target;
/// snapshot-create failures abort this target (and are reported by the
/// caller) but not the remaining targets.
fn process_target<B: Bcachefs>(
    bcachefs: &B,
    target: &Target,
    dir: &Path,
    now: NaiveDateTime,
    dry_run: bool,
    failures: &mut Vec<anyhow::Error>,
) -> Result<()> {
    let existing = read_snapshots(dir)?;
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
        if let Some(parent) = snap.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        if snap.path.exists() {
            warn!(path = %snap.path.display(), "already exists, skipping");
            continue;
        }
        bcachefs.snapshot(&target.path, &snap.path)?;
        info!(path = %snap.path.display(), "created");
    }

    for snap in &plan.prune {
        if dry_run {
            info!(path = %snap.path.display(), "would prune");
            continue;
        }
        match bcachefs.delete(&snap.path) {
            Ok(()) => info!(path = %snap.path.display(), "pruned"),
            Err(error) => failures.push(error.context(snap.path.display().to_string())),
        }
    }

    Ok(())
}

/// List the snapshots present for each selected target.
pub fn list(config: &Config, only: &[PathBuf]) -> Result<()> {
    for target in config.selected(only) {
        println!("{}:", target.path.display());
        let dir = Snapshot::dir_for(&target.path, &config.snapshot_subdir);
        let mut snapshots = read_snapshots(&dir)?;
        snapshots.sort_by_key(|b| std::cmp::Reverse(b.file_name()));
        for snap in snapshots {
            println!("  {}", snap.file_name());
        }
    }
    Ok(())
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
