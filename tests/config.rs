//! Tests for shipped configuration examples.

use std::path::Path;

use bonsai::config::Config;

#[test]
fn example_config_parses() {
    // Guards the example in the repository root against drifting from the
    // schema (unknown fields, wrong types, invalid paths).
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("config.example.toml");
    let config = Config::load(&path).unwrap();
    assert_eq!(config.snapshot_subdir, ".snapshots");
    assert!(!config.targets.is_empty());
    for target in &config.targets {
        assert!(
            target.path.is_absolute(),
            "example target paths should be absolute: {}",
            target.path.display()
        );
    }
}
