use std::path::Path;

use chrono::{Datelike, NaiveDate, Weekday};

use crate::config::Retention;
use crate::snapshot::{Snapshot, Tier};

/// The work computed for a single target: snapshots to create and to prune.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// Snapshots to create, sorted by file name (ascending) for determinism.
    pub create: Vec<Snapshot>,
    /// Existing snapshots that fall outside the retention window.
    pub prune: Vec<Snapshot>,
}

/// Tiers that should be created for a run on `now` (UTC).
///
/// Mirrors the original shell logic:
/// * daily — always,
/// * weekly — on Sundays (ISO weekday 7),
/// * monthly — on the 1st,
/// * yearly — on January 1st.
pub fn tiers_for(now: NaiveDate) -> Vec<Tier> {
    let mut tiers = vec![Tier::Daily];
    if now.weekday() == Weekday::Sun {
        tiers.push(Tier::Weekly);
    }
    if now.day() == 1 {
        tiers.push(Tier::Monthly);
    }
    if now.month() == 1 && now.day() == 1 {
        tiers.push(Tier::Yearly);
    }
    tiers
}

/// Compute the full plan for a target.
///
/// `existing` are the snapshots currently present in `snapshot_dir`; the plan
/// is pure and performs no I/O. New snapshots are included when deciding what
/// to prune, matching the shell script's create-then-prune ordering.
///
/// A tier with `retention.get(tier) == 0` is treated as disabled: no new
/// snapshots are created for it, but any existing snapshots of that tier are
/// still pruned to zero.
pub fn plan(
    now: NaiveDate,
    snapshot_dir: &Path,
    existing: &[Snapshot],
    retention: Retention,
) -> Plan {
    let mut create = Vec::new();
    let mut combined = existing.to_vec();

    for tier in tiers_for(now) {
        if retention.get(tier) == 0 {
            continue;
        }
        let already_present = existing.iter().any(|s| s.tier == tier && s.date == now);
        if !already_present {
            let snapshot = Snapshot::new(tier, now, snapshot_dir);
            combined.push(snapshot.clone());
            create.push(snapshot);
        }
    }

    let mut prune = Vec::new();
    for tier in Tier::ALL {
        let keep = retention.get(tier) as usize;
        // Names are `<tier>-YYYYMMDD`, so reverse lexicographic order equals
        // newest-first within a tier — the same ordering `sort -r` produced.
        let mut of_tier: Vec<&Snapshot> = combined.iter().filter(|s| s.tier == tier).collect();
        of_tier.sort_by_key(|s| std::cmp::Reverse(s.file_name()));
        prune.extend(of_tier.into_iter().skip(keep).cloned());
    }

    prune.sort_by_key(Snapshot::file_name);
    create.sort_by_key(Snapshot::file_name);
    Plan { create, prune }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn names(snaps: &[Snapshot]) -> Vec<String> {
        snaps.iter().map(Snapshot::file_name).collect()
    }

    #[test]
    fn daily_always() {
        // 2026-10-14 is a Wednesday.
        assert_eq!(tiers_for(date(2026, 10, 14)), vec![Tier::Daily]);
    }

    #[test]
    fn weekly_on_sunday() {
        // 2026-10-11 is a Sunday.
        assert_eq!(
            tiers_for(date(2026, 10, 11)),
            vec![Tier::Daily, Tier::Weekly]
        );
    }

    #[test]
    fn monthly_on_first_and_yearly_on_jan_first() {
        // 2026-01-01 is a Thursday.
        assert_eq!(
            tiers_for(date(2026, 1, 1)),
            vec![Tier::Daily, Tier::Monthly, Tier::Yearly]
        );
    }

    #[test]
    fn weekly_plus_monthly_on_sunday_first() {
        // 2026-02-01 is a Sunday.
        assert_eq!(
            tiers_for(date(2026, 2, 1)),
            vec![Tier::Daily, Tier::Weekly, Tier::Monthly]
        );
    }

    #[test]
    fn creates_daily_when_missing() {
        let dir = Path::new("/spool/x/.snapshots");
        let plan = plan(date(2026, 10, 14), dir, &[], Retention::default());
        assert_eq!(names(&plan.create), vec!["daily-20261014"]);
        assert!(plan.prune.is_empty());
    }

    #[test]
    fn skips_existing_same_day_snapshot() {
        let dir = Path::new("/spool/x/.snapshots");
        let existing = vec![Snapshot::new(Tier::Daily, date(2026, 10, 14), dir)];
        let plan = plan(date(2026, 10, 14), dir, &existing, Retention::default());
        assert!(plan.create.is_empty());
    }

    #[test]
    fn prunes_oldest_beyond_keep() {
        let dir = Path::new("/spool/x/.snapshots");
        // Ten daily snapshots, keep 7 -> three oldest pruned.
        let existing: Vec<Snapshot> = (1..=10)
            .map(|d| Snapshot::new(Tier::Daily, date(2026, 9, d), dir))
            .collect();
        let plan = plan(date(2026, 9, 10), dir, &existing, Retention::default());

        assert_eq!(
            names(&plan.prune),
            vec!["daily-20260901", "daily-20260902", "daily-20260903",]
        );
    }

    #[test]
    fn keep_zero_disables_creation_with_no_existing() {
        let dir = Path::new("/spool/x/.snapshots");
        let retention = Retention {
            daily: 0,
            ..Retention::default()
        };
        let plan = plan(date(2026, 10, 14), dir, &[], retention);
        assert!(plan.create.is_empty(), "keep=0 must not create");
        assert!(plan.prune.is_empty());
    }

    #[test]
    fn keep_zero_prunes_existing_snapshots() {
        let dir = Path::new("/spool/x/.snapshots");
        let existing: Vec<Snapshot> = (1..=4)
            .map(|d| Snapshot::new(Tier::Daily, date(2026, 9, d), dir))
            .collect();
        let retention = Retention {
            daily: 0,
            ..Retention::default()
        };
        let plan = plan(date(2026, 9, 10), dir, &existing, retention);
        assert!(plan.create.is_empty());
        assert_eq!(
            names(&plan.prune),
            vec![
                "daily-20260901",
                "daily-20260902",
                "daily-20260903",
                "daily-20260904",
            ],
            "keep=0 must prune every existing snapshot of that tier"
        );
    }
}
