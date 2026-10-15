//! Integration tests for the `app` orchestration layer.

use std::fs;
use std::path::Path;

use bonsai::app;
use bonsai::bcachefs::{Call, MockBcachefs};
use bonsai::config::Config;
use bonsai::restore::MockRestore;
use chrono::NaiveDateTime;

fn dt(year: i32, month: u32, day: u32) -> NaiveDateTime {
    NaiveDateTime::new(
        chrono::NaiveDate::from_ymd_opt(year, month, day).unwrap(),
        chrono::NaiveTime::MIN,
    )
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
    app::snapshot_with(&mock, &config, dt(2026, 9, 10), false, &[]).unwrap();

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
    app::snapshot_with(&mock, &config, dt(2026, 9, 11), false, &[]).unwrap();

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
    app::snapshot_with(&mock, &config, dt(2026, 9, 11), true, &[]).unwrap();
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
    app::snapshot_with(&mock, &config, dt(2026, 9, 10), false, &[]).unwrap();
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
    let result = app::snapshot_with(&mock, &config, dt(2026, 9, 10), false, &[]);
    assert!(result.is_err(), "partial delete failure must surface");

    assert_eq!(deleted_snapshots(&mock.calls()).len(), 3);
    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains("2 operation(s) failed"),
        "unexpected error: {message}"
    );
}

#[test]
fn snapshot_named_regular_files_are_ignored() {
    // A regular file with a snapshot-like name must be reported and ignored,
    // not planned for deletion (the old shell script checked `[[ -d ]]`).
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("photos");
    let snap_dir = target.join(".snapshots");
    fs::create_dir_all(&snap_dir).unwrap();
    for day in 1..=10 {
        fs::create_dir_all(snap_dir.join(format!("daily-202609{day:02}"))).unwrap();
    }
    fs::write(snap_dir.join("daily-20260920"), b"not a subvolume").unwrap();
    let config_path = dir.path().join("bonsai.toml");
    fs::write(
        &config_path,
        format!(
            r#"
            [[target]]
            path = "{}"
            retention = {{ daily = 7 }}
            "#,
            target.display()
        ),
    )
    .unwrap();
    let config = Config::load(&config_path).unwrap();
    let mock = MockBcachefs::new();
    app::snapshot_with(&mock, &config, dt(2026, 9, 10), false, &[]).unwrap();

    assert_eq!(
        deleted_snapshots(&mock.calls()),
        vec!["daily-20260901", "daily-20260902", "daily-20260903"],
        "only the three oldest directories should be pruned, never the file"
    );
}

#[test]
fn create_failure_does_not_abort_remaining_targets() {
    // The create for target `a` fails; target `b` must still be processed,
    // and the run must surface a single aggregated error.
    let dir = tempfile::tempdir().unwrap();
    let target_a = dir.path().join("a");
    let target_b = dir.path().join("b");
    for target in [&target_a, &target_b] {
        fs::create_dir_all(target.join(".snapshots")).unwrap();
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
    mock.fail_next_snapshots(1);
    let result = app::snapshot_with(&mock, &config, dt(2026, 9, 10), false, &[]);
    assert!(result.is_err(), "partial create failure must surface");

    let processed: std::collections::HashSet<_> = mock
        .calls()
        .iter()
        .filter(|c| matches!(c, Call::Snapshot { .. }))
        .filter_map(target_of)
        .collect();
    assert!(
        processed.contains(&target_b),
        "target b must still be processed, got {processed:?}"
    );

    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains("1 operation(s) failed"),
        "unexpected error: {message}"
    );
    assert!(
        message.contains(target_a.file_name().unwrap().to_string_lossy().as_ref()),
        "error should name the failed target: {message}"
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
        dt(2026, 9, 10),
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

#[test]
fn plan_report_reflects_prune_plan() {
    let (_dir, config) = setup_with_snapshots(".snapshots", 7);
    let report = app::plan_report(&config, &[], dt(2026, 9, 10)).unwrap();
    assert_eq!(report.targets.len(), 1);
    let target = &report.targets[0];
    assert!(target.create.is_empty());
    assert_eq!(
        target
            .prune
            .iter()
            .map(|s| s.name.clone())
            .collect::<Vec<_>>(),
        vec!["daily-20260901", "daily-20260902", "daily-20260903"]
    );

    let value = serde_json::to_value(&report).unwrap();
    assert!(value["targets"][0]["prune"][0]["name"].is_string());
    assert!(value["targets"][0]["prune"][0]["tier"].is_string());
    assert!(value["targets"][0]["prune"][0]["time"].is_string());
}

#[test]
fn list_report_is_newest_first() {
    let (_dir, config) = setup_with_snapshots(".snapshots", 7);
    let report = app::list_report(&config, &[]).unwrap();
    let target = &report.targets[0];
    assert_eq!(target.snapshots.len(), 10);
    assert_eq!(target.snapshots[0].name, "daily-20260910");

    let value = serde_json::to_value(&report).unwrap();
    assert!(value["targets"][0]["snapshots"].is_array());
}

#[test]
fn resolve_delete_paths_accepts_snapshot_names() {
    let (dir, config) = setup_with_snapshots(".snapshots", 7);
    let target = dir.path().join("photos");
    let paths =
        app::resolve_delete_paths(&config, &target, &["daily-20260905".to_string()]).unwrap();
    assert_eq!(
        paths,
        vec![target.join(".snapshots").join("daily-20260905")]
    );
}

#[test]
fn resolve_delete_paths_rejects_unknown_target_and_bad_names() {
    let (dir, config) = setup_with_snapshots(".snapshots", 7);
    let target = dir.path().join("photos");

    let err = app::resolve_delete_paths(
        &config,
        &dir.path().join("other"),
        &["daily-20260905".to_string()],
    )
    .unwrap_err();
    assert!(err.to_string().contains("not in config"), "{err}");

    let err = app::resolve_delete_paths(&config, &target, &["nonsense".to_string()]).unwrap_err();
    assert!(err.to_string().contains("not a snapshot name"), "{err}");
}

#[test]
fn delete_paths_records_and_aggregates_failures() {
    let (dir, _config) = setup_with_snapshots(".snapshots", 7);
    let snap_dir = dir.path().join("photos").join(".snapshots");
    let paths = vec![
        snap_dir.join("daily-20260901"),
        snap_dir.join("daily-20260902"),
    ];

    let mock = MockBcachefs::new();
    app::delete_paths(&mock, &paths).unwrap();
    assert_eq!(
        deleted_snapshots(&mock.calls()),
        vec!["daily-20260901", "daily-20260902"]
    );

    let mock = MockBcachefs::new();
    mock.fail_next_deletes(1);
    let result = app::delete_paths(&mock, &paths);
    assert!(result.is_err());
    assert!(format!("{:#}", result.unwrap_err()).contains("1 deletion(s) failed"));
}

#[test]
fn resolve_restore_source_defaults_to_snapshot_root() {
    let (dir, config) = setup_with_snapshots(".snapshots", 7);
    let target = dir.path().join("photos");
    let source = app::resolve_restore_source(&config, &target, "daily-20260905", None).unwrap();
    assert_eq!(source, target.join(".snapshots").join("daily-20260905"));
}

#[test]
fn resolve_restore_source_with_subpath() {
    let (dir, config) = setup_with_snapshots(".snapshots", 7);
    let target = dir.path().join("photos");
    let snap = target.join(".snapshots").join("daily-20260905");
    fs::write(snap.join("photo.jpg"), b"x").unwrap();

    let source = app::resolve_restore_source(
        &config,
        &target,
        "daily-20260905",
        Some(Path::new("photo.jpg")),
    )
    .unwrap();
    assert_eq!(source, snap.join("photo.jpg"));
}

#[test]
fn resolve_restore_source_rejects_bad_input() {
    let (dir, config) = setup_with_snapshots(".snapshots", 7);
    let target = dir.path().join("photos");

    assert!(
        app::resolve_restore_source(&config, &target, "daily-20260905", Some(Path::new("../x")))
            .is_err()
    );
    assert!(
        app::resolve_restore_source(&config, &target, "daily-20260905", Some(Path::new("/abs")))
            .is_err()
    );
    assert!(
        app::resolve_restore_source(
            &config,
            &target,
            "daily-20260905",
            Some(Path::new("missing"))
        )
        .is_err()
    );
    assert!(
        app::resolve_restore_source(&config, &dir.path().join("other"), "daily-20260905", None)
            .is_err()
    );
}

#[test]
fn restore_into_records_call() {
    let (dir, config) = setup_with_snapshots(".snapshots", 7);
    let target = dir.path().join("photos");
    let source = app::resolve_restore_source(&config, &target, "daily-20260905", None).unwrap();
    let dest = dir.path().join("restored");

    let mock = MockRestore::new();
    app::restore_into(&mock, &source, &dest).unwrap();
    assert_eq!(mock.calls(), vec![(source, dest)]);
}
