use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::NaiveDate;
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
    let now = chrono::Utc::now().date_naive();
    snapshot_with(&bcachefs, config, now, dry_run, only)
}

/// Snapshot cycle with an injected filesystem backend and clock (for tests).
pub fn snapshot_with<B: Bcachefs>(
    bcachefs: &B,
    config: &Config,
    now: NaiveDate,
    dry_run: bool,
    only: &[PathBuf],
) -> Result<()> {
    let mut delete_errors: Vec<anyhow::Error> = Vec::new();

    for target in config.selected(only) {
        if !target.path.is_dir() {
            warn!(path = %target.path.display(), "target does not exist, skipping");
            continue;
        }

        let dir = Snapshot::dir_for(&target.path, &config.snapshot_subdir);
        let existing = read_snapshots(&dir)?;
        let plan = policy::plan(now, &dir, &existing, target.retention);

        info!(
            path = %target.path.display(),
            create = plan.create.len(),
            prune = plan.prune.len(),
            "planned"
        );

        apply_target(bcachefs, target, &plan, dry_run, &mut delete_errors)?;
    }

    if !delete_errors.is_empty() {
        let n = delete_errors.len();
        let messages: Vec<String> = delete_errors.iter().map(|e| format!("{e:#}")).collect();
        return Err(anyhow::anyhow!(
            "{n} delete(s) failed:\n{}",
            messages.join("\n")
        ));
    }

    Ok(())
}

fn apply_target<B: Bcachefs>(
    bcachefs: &B,
    target: &Target,
    plan: &policy::Plan,
    dry_run: bool,
    delete_errors: &mut Vec<anyhow::Error>,
) -> Result<()> {
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
            Err(error) => {
                warn!(path = %snap.path.display(), error = %error, "prune failed, continuing");
                delete_errors.push(error.context(snap.path.display().to_string()));
            }
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
        if let Some(snapshot) = Snapshot::parse_entry(dir, name) {
            snapshots.push(snapshot);
        }
    }
    Ok(snapshots)
}
