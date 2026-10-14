//! Integration tests for the `app` orchestration layer.

use std::fs;
use std::path::Path;

use bonsai::app;
use bonsai::bcachefs::{Call, MockBcachefs};
use bonsai::config::Config;
use chrono::NaiveDate;

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

/// Set up a temp target with snapshots for days 1..=10 and a matching TOML
/// config.
fn setup_with_snapshots(snapshot_subdir: &str, daily_keep: u32) -> (tempfile::TempDir, Config) {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("photos");
    let snap_dir = target.join(snapshot_subdir);
    fs::create_dir_all(&snap_dir).unwrap();
    for day in 1..=10 {
        fs::create_dir_all(snap_dir.join(format!("daily-202609{day:02}"))).unwrap();
    }
    let config_path = dir.path().join("bonsai.toml");
    fs::write(
        &config_path,
        format!(
            r#"
            snapshot_subdir = "{snapshot_subdir}"
            [[target]]
            path = "{}"
            retention = {{ daily = {daily_keep} }}
            "#,
            target.display()
        ),
    )
    .unwrap();
    let config = Config::load(&config_path).unwrap();
    (dir, config)
}

fn created_snapshots(calls: &[Call]) -> Vec<String> {
    calls
        .iter()
        .filter_map(|c| match c {
            Call::Snapshot { dest, .. } => {
                dest.file_name().map(|n| n.to_string_lossy().into_owned())
            }
            _ => None,
        })
        .collect()
}

fn deleted_snapshots(calls: &[Call]) -> Vec<String> {
    calls
        .iter()
        .filter_map(|c| match c {
            Call::Delete { path } => path.file_name().map(|n| n.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

fn target_of(call: &Call) -> Option<std::path::PathBuf> {
    let inner = match call {
        Call::Snapshot { dest, .. } => dest,
        Call::Delete { path } => path,
    };
    inner
        .parent()
        .and_then(|p| p.parent())
        .map(Path::to_path_buf)
}

#[test]
fn prunes_excess_when_today_already_exists() {
    // Day 10 is already present, so no create is needed; 10 - keep 7 = 3 prunes.
    let (_dir, config) = setup_with_snapshots(".snapshots", 7);
    let mock = MockBcachefs::new();
    app::snapshot_with(&mock, &config, date(2026, 9, 10), false, &[]).unwrap();

    assert!(created_snapshots(&mock.calls()).is_empty());
    assert_eq!(
        deleted_snapshots(&mock.calls()),
        vec!["daily-20260901", "daily-20260902", "daily-20260903"],
        "oldest three should be pruned"
    );
}

#[test]
fn creates_when_missing_and_prunes_rest() {
    // Day 11 is not pre-created -> create it, and prune the four oldest
    // (keep 7 + new 1 = 11, prune 01..04).
    let (_dir, config) = setup_with_snapshots(".snapshots", 7);
    let mock = MockBcachefs::new();
    app::snapshot_with(&mock, &config, date(2026, 9, 11), false, &[]).unwrap();

    assert_eq!(created_snapshots(&mock.calls()), vec!["daily-20260911"]);
    assert_eq!(
        deleted_snapshots(&mock.calls()),
        vec![
            "daily-20260901",
            "daily-20260902",
            "daily-20260903",
            "daily-20260904",
        ],
        "oldest four should be pruned"
    );
}

#[test]
fn dry_run_makes_no_calls() {
    let (_dir, config) = setup_with_snapshots(".snapshots", 7);
    let mock = MockBcachefs::new();
    app::snapshot_with(&mock, &config, date(2026, 9, 11), true, &[]).unwrap();
    assert!(
        mock.calls().is_empty(),
        "dry-run must not touch the backend"
    );
}

#[test]
fn nonexistent_target_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("does-not-exist");
    let config_path = dir.path().join("bonsai.toml");
    fs::write(
        &config_path,
        format!(
            r#"
            [[target]]
            path = "{}"
            "#,
            target.display()
        ),
    )
    .unwrap();
    let config = Config::load(&config_path).unwrap();
    let mock = MockBcachefs::new();
    app::snapshot_with(&mock, &config, date(2026, 9, 10), false, &[]).unwrap();
    assert!(mock.calls().is_empty());
}

#[test]
fn delete_failures_continue_and_surface_as_error() {
    // 10 pre-existing dailies with keep=7 -> 3 prunes. Fail the first two
    // deletes and assert (a) the third delete was still attempted and
    // (b) the run returns an error summarising the failures.
    let (_dir, config) = setup_with_snapshots(".snapshots", 7);
    let mock = MockBcachefs::new();
    mock.fail_next_deletes(2);
    let result = app::snapshot_with(&mock, &config, date(2026, 9, 10), false, &[]);
    assert!(result.is_err(), "partial delete failure must surface");

    assert_eq!(deleted_snapshots(&mock.calls()).len(), 3);
    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains("2 delete(s) failed"),
        "unexpected error: {message}"
    );
}

#[test]
fn only_filter_restricts_to_named_targets() {
    let dir = tempfile::tempdir().unwrap();
    let target_a = dir.path().join("a");
    let target_b = dir.path().join("b");
    for target in [&target_a, &target_b] {
        let snap_dir = target.join(".snapshots");
        fs::create_dir_all(&snap_dir).unwrap();
        for day in 1..=10 {
            fs::create_dir_all(snap_dir.join(format!("daily-202609{day:02}"))).unwrap();
        }
    }
    let config_path = dir.path().join("bonsai.toml");
    fs::write(
        &config_path,
        format!(
            r#"
            [[target]]
            path = "{}"
            retention = {{ daily = 7 }}
            [[target]]
            path = "{}"
            retention = {{ daily = 7 }}
            "#,
            target_a.display(),
            target_b.display()
        ),
    )
    .unwrap();
    let config = Config::load(&config_path).unwrap();
    let mock = MockBcachefs::new();
    app::snapshot_with(
        &mock,
        &config,
        date(2026, 9, 10),
        false,
        std::slice::from_ref(&target_a),
    )
    .unwrap();

    let processed: std::collections::HashSet<_> =
        mock.calls().iter().filter_map(target_of).collect();
    assert_eq!(
        processed.len(),
        1,
        "only one target should have been processed"
    );
    assert!(
        processed.contains(&target_a),
        "processed target should be `a`, got {processed:?}"
    );
}
