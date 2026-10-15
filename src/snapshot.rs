use std::fmt;
use std::path::{Path, PathBuf};

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
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

/// A single snapshot subvolume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub tier: Tier,
    pub datetime: NaiveDateTime,
    pub path: PathBuf,
}

impl Snapshot {
    /// File name for a snapshot of `tier` taken at `datetime`, e.g. `daily-20260101`.
    pub fn file_name_for(tier: Tier, datetime: NaiveDateTime) -> String {
        let prefix = tier.prefix();
        format!("{prefix}-{}", datetime.format("%Y%m%d"))
    }

    /// Build a snapshot located inside `dir`. The datetime is truncated to the
    /// tier's granularity, so it is the period key, not the exact run time.
    pub fn new(tier: Tier, datetime: NaiveDateTime, dir: &Path) -> Self {
        let datetime = truncate(tier, datetime);
        let path = dir.join(Self::file_name_for(tier, datetime));
        Snapshot {
            tier,
            datetime,
            path,
        }
    }

    /// Parse a snapshot directory entry. Returns `None` for names that do not
    /// look like snapshots (so listing can skip foreign files).
    pub fn parse_entry(dir: &Path, name: &str) -> Option<Self> {
        let (tier_str, stamp) = name.split_once('-')?;
        let tier = match tier_str {
            "daily" => Tier::Daily,
            "weekly" => Tier::Weekly,
            "monthly" => Tier::Monthly,
            "yearly" => Tier::Yearly,
            _ => return None,
        };
        let datetime = parse_stamp(tier, stamp)?;
        Some(Snapshot {
            tier,
            datetime,
            path: dir.join(name),
        })
    }

    /// The snapshot's file name (last path component).
    pub fn file_name(&self) -> String {
        Self::file_name_for(self.tier, self.datetime)
    }

    /// Directory holding snapshots for a target (e.g. `<target>/.snapshots`).
    pub fn dir_for(target: &Path, subdir: &str) -> PathBuf {
        target.join(subdir)
    }
}

/// Truncate a datetime to the granularity a tier is keyed by.
fn truncate(_tier: Tier, datetime: NaiveDateTime) -> NaiveDateTime {
    NaiveDateTime::new(datetime.date(), NaiveTime::MIN)
}

/// Parse the timestamp part of a snapshot name for a given tier.
fn parse_stamp(_tier: Tier, stamp: &str) -> Option<NaiveDateTime> {
    let date = NaiveDate::parse_from_str(stamp, "%Y%m%d").ok()?;
    Some(NaiveDateTime::new(date, NaiveTime::MIN))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(y: i32, m: u32, d: u32) -> NaiveDateTime {
        NaiveDateTime::new(NaiveDate::from_ymd_opt(y, m, d).unwrap(), NaiveTime::MIN)
    }

    #[test]
    fn formats_and_parses_round_trip() {
        let dir = Path::new("/spool/x/.snapshots");
        let snap = Snapshot::new(Tier::Daily, dt(2026, 1, 5), dir);
        assert_eq!(snap.file_name(), "daily-20260105");

        let parsed = Snapshot::parse_entry(dir, "daily-20260105").unwrap();
        assert_eq!(parsed, snap);
    }

    #[test]
    fn new_truncates_to_day() {
        let dir = Path::new("/spool/x/.snapshots");
        let later = dt(2026, 1, 5) + chrono::Duration::hours(13);
        let snap = Snapshot::new(Tier::Daily, later, dir);
        assert_eq!(snap.datetime, dt(2026, 1, 5));
        assert_eq!(snap.file_name(), "daily-20260105");
    }

    #[test]
    fn ignores_foreign_entries() {
        let dir = Path::new("/spool/x/.snapshots");
        assert!(Snapshot::parse_entry(dir, "lost+found").is_none());
        assert!(Snapshot::parse_entry(dir, "daily-notadate").is_none());
        assert!(Snapshot::parse_entry(dir, "weekly").is_none());
    }
}
