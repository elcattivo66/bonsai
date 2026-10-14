use std::path::{Path, PathBuf};

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
        for target in &self.targets {
            if target.path.as_os_str().is_empty() {
                anyhow::bail!("target path must not be empty");
            }
        }
        Ok(())
    }

    /// Targets selected by an optional path filter. An empty filter selects all.
    pub fn selected(&self, only: &[PathBuf]) -> Vec<&Target> {
        if only.is_empty() {
            self.targets.iter().collect()
        } else {
            self.targets
                .iter()
                .filter(|t| only.iter().any(|p| p == &t.path))
                .collect()
        }
    }
}

fn default_snapshot_subdir() -> String {
    ".snapshots".to_owned()
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
}
