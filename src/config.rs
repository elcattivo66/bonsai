use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::snapshot::Tier;

/// Top-level configuration, loaded from a TOML file.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Subdirectory inside each target where snapshots are stored.
    #[serde(default = "default_snapshot_subdir")]
    pub snapshot_subdir: String,

    /// Targets to snapshot.
    #[serde(default, rename = "target")]
    pub targets: Vec<Target>,
}

/// A directory that should be snapshotted, with its own retention policy.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub path: PathBuf,
    #[serde(default)]
    pub retention: Retention,
}

/// Per-tier retention counts.
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Retention {
    #[serde(default = "default_hourly")]
    pub hourly: u32,
    #[serde(default = "default_daily")]
    pub daily: u32,
    #[serde(default = "default_weekly")]
    pub weekly: u32,
    #[serde(default = "default_monthly")]
    pub monthly: u32,
    #[serde(default = "default_yearly")]
    pub yearly: u32,
}

impl Retention {
    /// Keep-count for a given tier.
    pub fn get(&self, tier: Tier) -> u32 {
        match tier {
            Tier::Hourly => self.hourly,
            Tier::Daily => self.daily,
            Tier::Weekly => self.weekly,
            Tier::Monthly => self.monthly,
            Tier::Yearly => self.yearly,
        }
    }
}

impl Default for Retention {
    fn default() -> Self {
        Retention {
            hourly: default_hourly(),
            daily: default_daily(),
            weekly: default_weekly(),
            monthly: default_monthly(),
            yearly: default_yearly(),
        }
    }
}

impl Config {
    /// Load and validate a configuration file.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let config: Config =
            toml::from_str(&text).with_context(|| format!("parsing config {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    /// Parse a configuration from a TOML string.
    pub fn parse(text: &str) -> Result<Self> {
        let config: Config = toml::from_str(text).context("parsing config")?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if self.snapshot_subdir.is_empty() {
            anyhow::bail!("snapshot_subdir must not be empty");
        }
        // The subdir is joined onto each target path; an absolute path would
        // silently discard the target (`Path::join` semantics) and `..` would
        // escape above it.
        let subdir = Path::new(&self.snapshot_subdir);
        if subdir.is_absolute() {
            anyhow::bail!("snapshot_subdir must be relative: {subdir:?}");
        }
        let mut has_component = false;
        for component in subdir.components() {
            match component {
                Component::ParentDir => {
                    anyhow::bail!("snapshot_subdir must not contain '..': {subdir:?}");
                }
                Component::Normal(_) => has_component = true,
                _ => {}
            }
        }
        if !has_component {
            anyhow::bail!("snapshot_subdir must name a subdirectory: {subdir:?}");
        }

        let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        for target in &self.targets {
            if target.path.as_os_str().is_empty() {
                anyhow::bail!("target path must not be empty");
            }
            let normalized = normalize_path(&target.path);
            if !seen.insert(normalized) {
                anyhow::bail!("duplicate target path: {}", target.path.display());
            }
        }
        Ok(())
    }

    /// Targets selected by an optional path filter. An empty filter selects all.
    /// Path comparison normalises trailing slashes and resolves `.` / `..`
    /// components so that `/spool/garage` and `/spool/garage/` match.
    pub fn selected(&self, only: &[PathBuf]) -> Vec<&Target> {
        if only.is_empty() {
            self.targets.iter().collect()
        } else {
            let wanted: Vec<PathBuf> = only.iter().map(|p| normalize_path(p)).collect();
            self.targets
                .iter()
                .filter(|t| wanted.iter().any(|w| w == &normalize_path(&t.path)))
                .collect()
        }
    }
}

/// Normalise a path for comparison: drop trailing slashes, resolve `.` and
/// `..` lexically. Does not follow symlinks (that would require the path to
/// exist on disk).
fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

fn default_snapshot_subdir() -> String {
    ".snapshots".to_owned()
}

const fn default_hourly() -> u32 {
    0
}

const fn default_daily() -> u32 {
    7
}

const fn default_weekly() -> u32 {
    4
}

const fn default_monthly() -> u32 {
    6
}

const fn default_yearly() -> u32 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_config_with_defaults() {
        let cfg = Config::parse(
            r#"
            [[target]]
            path = "/spool/garage"
            "#,
        )
        .unwrap();

        assert_eq!(cfg.snapshot_subdir, ".snapshots");
        assert_eq!(cfg.targets.len(), 1);
        assert_eq!(cfg.targets[0].path, PathBuf::from("/spool/garage"));
        assert_eq!(cfg.targets[0].retention.daily, 7);
        assert_eq!(cfg.targets[0].retention.yearly, 1);
    }

    #[test]
    fn parses_custom_retention() {
        let cfg = Config::parse(
            r#"
            snapshot_subdir = ".snap"
            [[target]]
            path = "/spool/photos"
            retention = { daily = 3, weekly = 2, monthly = 1, yearly = 0 }
            "#,
        )
        .unwrap();

        assert_eq!(cfg.snapshot_subdir, ".snap");
        assert_eq!(cfg.targets[0].retention.daily, 3);
        assert_eq!(cfg.targets[0].retention.monthly, 1);
        assert_eq!(cfg.targets[0].retention.yearly, 0);
    }

    #[test]
    fn rejects_unknown_fields() {
        let err = Config::parse(
            r#"
            [[target]]
            path = "/spool/x"
            bogus = true
            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("parsing config"));
    }

    #[test]
    fn rejects_duplicate_target_paths() {
        let err = Config::parse(
            r#"
            [[target]]
            path = "/spool/x"
            [[target]]
            path = "/spool/x"
            "#,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("duplicate target path"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn rejects_duplicate_target_paths_after_normalisation() {
        // Trailing slash should be treated as the same path.
        let err = Config::parse(
            r#"
            [[target]]
            path = "/spool/x"
            [[target]]
            path = "/spool/x/"
            "#,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("duplicate target path"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn selected_matches_paths_with_trailing_slash() {
        let cfg = Config::parse(
            r#"
            [[target]]
            path = "/spool/garage"
            "#,
        )
        .unwrap();
        let only = vec![PathBuf::from("/spool/garage/")];
        assert_eq!(cfg.selected(&only).len(), 1);

        let only = vec![PathBuf::from("/spool/other/")];
        assert!(cfg.selected(&only).is_empty());
    }

    #[test]
    fn rejects_absolute_snapshot_subdir() {
        // Path::join would discard the target path entirely.
        let err = Config::parse(
            r#"
            snapshot_subdir = "/evil"
            [[target]]
            path = "/spool/x"
            "#,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("relative"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn rejects_parent_components_in_snapshot_subdir() {
        let err = Config::parse(
            r#"
            snapshot_subdir = "../snapshots"
            [[target]]
            path = "/spool/x"
            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("'..'"), "unexpected error: {err}");
    }

    #[test]
    fn rejects_snapshot_subdir_without_components() {
        for subdir in [".", "./"] {
            let err = Config::parse(&format!(
                r#"
                snapshot_subdir = "{subdir}"
                [[target]]
                path = "/spool/x"
                "#
            ))
            .unwrap_err();
            assert!(
                err.to_string().contains("must name a subdirectory"),
                "unexpected error for {subdir:?}: {err}"
            );
        }
    }

    #[test]
    fn accepts_nested_snapshot_subdir() {
        let cfg = Config::parse(
            r#"
            snapshot_subdir = ".snapshots/deep"
            [[target]]
            path = "/spool/x"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.snapshot_subdir, ".snapshots/deep");
    }
}
