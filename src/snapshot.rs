use std::fmt;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// A retention tier in the grandfather-father-son scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

impl Tier {
    /// All tiers, ordered from finest to coarsest.
    pub const ALL: [Tier; 4] = [Tier::Daily, Tier::Weekly, Tier::Monthly, Tier::Yearly];

    /// The filename prefix used for this tier (`daily`, `weekly`, ...).
    pub fn prefix(self) -> &'static str {
        match self {
            Tier::Daily => "daily",
            Tier::Weekly => "weekly",
            Tier::Monthly => "monthly",
            Tier::Yearly => "yearly",
        }
    }
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.prefix())
    }
}

/// A single read-only snapshot subvolume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub tier: Tier,
    pub date: NaiveDate,
    pub path: PathBuf,
}

impl Snapshot {
    /// File name for a snapshot of `tier` taken on `date`, e.g. `daily-20260101`.
    pub fn file_name_for(tier: Tier, date: NaiveDate) -> String {
        let prefix = tier.prefix();
        format!("{prefix}-{}", date.format("%Y%m%d"))
    }

    /// Build a snapshot located inside `dir`.
    pub fn new(tier: Tier, date: NaiveDate, dir: &Path) -> Self {
        let path = dir.join(Self::file_name_for(tier, date));
        Snapshot { tier, date, path }
    }

    /// Parse a snapshot directory entry. Returns `None` for names that do not
    /// look like snapshots (so listing can skip foreign files).
    pub fn parse_entry(dir: &Path, name: &str) -> Option<Self> {
        let (tier_str, date_str) = name.split_once('-')?;
        let tier = match tier_str {
            "daily" => Tier::Daily,
            "weekly" => Tier::Weekly,
            "monthly" => Tier::Monthly,
            "yearly" => Tier::Yearly,
            _ => return None,
        };
        let date = NaiveDate::parse_from_str(date_str, "%Y%m%d").ok()?;
        Some(Snapshot {
            tier,
            date,
            path: dir.join(name),
        })
    }

    /// The snapshot's file name (last path component).
    pub fn file_name(&self) -> String {
        Self::file_name_for(self.tier, self.date)
    }

    /// Directory holding snapshots for a target (e.g. `<target>/.snapshots`).
    pub fn dir_for(target: &Path, subdir: &str) -> PathBuf {
        target.join(subdir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn formats_and_parses_round_trip() {
        let dir = Path::new("/spool/x/.snapshots");
        let snap = Snapshot::new(Tier::Daily, date(2026, 1, 5), dir);
        assert_eq!(snap.file_name(), "daily-20260105");

        let parsed = Snapshot::parse_entry(dir, "daily-20260105").unwrap();
        assert_eq!(parsed, snap);
    }

    #[test]
    fn ignores_foreign_entries() {
        let dir = Path::new("/spool/x/.snapshots");
        assert!(Snapshot::parse_entry(dir, "lost+found").is_none());
        assert!(Snapshot::parse_entry(dir, "daily-notadate").is_none());
        assert!(Snapshot::parse_entry(dir, "weekly").is_none());
    }
}
