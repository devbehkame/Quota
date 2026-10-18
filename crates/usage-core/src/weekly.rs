//! Where a limit window (the week, or the 5-hour session) starts, and how much
//! of that limit this PC used. Written for the week; the session works the same.
//!
//! The week starts 7 days before the weekly limit resets, unless the weekly
//! usage drops while the reset time stays put: then a banked limit reset was
//! used, and the week restarts at that moment.
//!
//! The services only report usage for the whole account, so this PC's share is
//! estimated between refreshes. When the weekly percentage rises and this PC was
//! busy within the last half hour (the services update the percentage a little
//! after the work), the rise counts as this PC; otherwise it was another device.
//! A rise Quota couldn't watch (before it started, or across a long gap such as
//! the PC asleep) is "unsplit". Once Quota has seen how much of the limit this
//! PC's work uses, it estimates this PC's part of the unsplit usage from the
//! tokens logged during it; with no tokens logged then, it was all elsewhere.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::{TokenUsage, WeeklyShare};

/// Remembered per account between refreshes (and app restarts).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    /// Start of the current week (moved forward by an early reset).
    pub window_start: DateTime<Utc>,
    /// The weekly reset time last reported.
    pub resets_at: Option<DateTime<Utc>>,
    pub last_seen: DateTime<Utc>,
    pub last_pct: f64,
    /// This PC's work this week (see [`units`]) at the last refresh; `None`
    /// until the first count of a week Quota didn't see begin.
    pub last_units: Option<f64>,
    /// When this PC's logs last grew.
    pub last_active: Option<DateTime<Utc>>,
    /// Percentage points of the limit that rose while this PC was busy, and the
    /// work this PC logged in the watched stretches (gives the rate).
    pub pc_pct: f64,
    pub watched_units: f64,
    /// Percentage points Quota couldn't watch, and this PC's work logged then.
    pub unsplit_pct: f64,
    pub unsplit_units: f64,
    pub tracked_since: DateTime<Utc>,
}

/// Two reset times this close are the same window (the reported time can wobble).
const SAME_WINDOW: i64 = 60 * 60;
/// A drop this large (percentage points) means the limit was reset early.
const DROP: f64 = 1.0;
/// Refreshes further apart than this can't tell whose usage it was.
const MAX_GAP: i64 = 60 * 60;
/// The percentage can lag the work by this much.
const LAG: i64 = 30 * 60;

/// Rough relative cost of tokens, like the providers' prices: cached input is
/// cheap, output is dear. Only used to compare this PC's work over time.
pub fn units(t: &TokenUsage) -> f64 {
    t.input as f64 + t.cache_write as f64 * 1.25 + t.cache_read as f64 * 0.1 + t.output as f64 * 5.0
}

/// How long each window lasts.
pub const WEEK: Duration = Duration::days(7);
pub const SESSION: Duration = Duration::hours(5);

/// Decides the window for this refresh. Call before counting tokens (from
/// `window_start`), then record the count with [`Track::observe`]. `length` is
/// [`WEEK`] or [`SESSION`].
pub fn begin(
    prev: Option<&Track>,
    pct: f64,
    resets_at: Option<DateTime<Utc>>,
    length: Duration,
    now: DateTime<Utc>,
) -> Track {
    let resets_at = resets_at.filter(|r| *r > now);
    let natural = resets_at.map(|r| r - length);
    // A new week: if Quota was looking right before it began, it's watched from
    // the start; otherwise what's already used is unsplit.
    let new_week = |start: DateTime<Utc>, last_seen: Option<DateTime<Utc>>| Track {
        window_start: start,
        resets_at,
        last_seen: last_seen.unwrap_or(now),
        last_pct: if last_seen.is_some() { 0.0 } else { pct },
        last_units: last_seen.map(|_| 0.0),
        last_active: None,
        pc_pct: 0.0,
        watched_units: 0.0,
        unsplit_pct: if last_seen.is_some() { 0.0 } else { pct },
        unsplit_units: 0.0,
        tracked_since: if last_seen.is_some() { start } else { now },
    };

    let Some(prev) = prev else {
        return new_week(natural.unwrap_or(now - length), None);
    };
    let moved = match (prev.resets_at, resets_at) {
        (Some(a), Some(b)) => (a - b).num_seconds().abs() > SAME_WINDOW,
        (None, Some(_)) => true,
        // The old window ran out and no new one has started yet.
        (Some(a), None) => a <= now,
        (None, None) => false,
    };
    if moved {
        let start = natural.or(prev.resets_at.filter(|r| *r <= now)).unwrap_or(now - length);
        return new_week(start, (start >= prev.last_seen).then_some(prev.last_seen));
    }
    if pct + DROP < prev.last_pct {
        // Reset early (a banked limit reset): the week restarts about now.
        return new_week(prev.last_seen, Some(prev.last_seen));
    }
    Track { resets_at, ..prev.clone() }
}

impl Track {
    /// Records this refresh: `pct` is the weekly limit used (all devices), and
    /// `work` this PC's [`units`] since `window_start`.
    pub fn observe(&mut self, pct: f64, work: f64, now: DateTime<Utc>) {
        match self.last_units {
            // First count of a week already under way: all of it is unsplit.
            None => self.unsplit_units = work,
            Some(last) => {
                let grew = (work - last).max(0.0);
                if grew > 0.0 {
                    self.last_active = Some(now);
                }
                let rose = (pct - self.last_pct).max(0.0);
                if (now - self.last_seen).num_seconds() > MAX_GAP {
                    self.unsplit_pct += rose;
                    self.unsplit_units += grew;
                } else {
                    self.watched_units += grew;
                    if self.last_active.is_some_and(|a| (now - a).num_seconds() <= LAG) {
                        self.pc_pct += rose;
                    }
                }
            }
        }
        self.last_pct = pct;
        self.last_units = Some(work);
        self.last_seen = now;
    }

    pub fn share(&self) -> WeeklyShare {
        let used = self.last_pct.max(0.0);
        let watched_pc = self.pc_pct.min(used);
        let unsplit = self.unsplit_pct.min(used - watched_pc);
        // This PC's part of the unsplit usage, at the rate seen while watching.
        let (from_unsplit, still_unsplit) = if self.unsplit_units <= 0.0 {
            (0.0, 0.0)
        } else if watched_pc > 0.0 && self.watched_units > 0.0 {
            let est = (self.unsplit_units * watched_pc / self.watched_units).min(unsplit);
            (est, 0.0)
        } else {
            (0.0, unsplit)
        };
        let this_pc = watched_pc + from_unsplit;
        WeeklyShare {
            used,
            this_pc,
            other: (used - this_pc - still_unsplit).max(0.0),
            unsplit: still_unsplit,
            estimated: from_unsplit > 0.0,
            tracked_since: self.tracked_since,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(min: i64) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-12T00:00:00Z").unwrap().with_timezone(&Utc) + Duration::minutes(min)
    }

    const DAY: i64 = 24 * 60;

    fn step(prev: Option<&Track>, pct: f64, reset: i64, work: f64, now: i64) -> Track {
        let mut t = begin(prev, pct, Some(at(reset)), WEEK, at(now));
        t.observe(pct, work, at(now));
        t
    }

    fn parts(t: &Track) -> (f64, f64, f64, f64) {
        let s = t.share();
        (s.used, s.this_pc, s.other, s.unsplit)
    }

    #[test]
    fn first_look_mid_week_leaves_earlier_usage_unsplit() {
        let t = step(None, 30.0, 4 * DAY, 5_000.0, 0);
        assert_eq!(t.window_start, at(4 * DAY) - Duration::days(7));
        assert_eq!(parts(&t), (30.0, 0.0, 0.0, 30.0));
    }

    #[test]
    fn usage_before_watching_with_no_local_work_was_elsewhere() {
        let t = step(None, 30.0, 4 * DAY, 0.0, 0);
        assert_eq!(parts(&t), (30.0, 0.0, 30.0, 0.0));
    }

    #[test]
    fn a_rise_that_lags_the_work_still_counts_for_this_pc() {
        let t = step(None, 0.0, 4 * DAY, 0.0, 0);
        let t = step(Some(&t), 0.0, 4 * DAY, 1_000.0, 5); // working; % not updated yet
        let t = step(Some(&t), 4.0, 4 * DAY, 1_000.0, 10); // % catches up
        assert_eq!(parts(&t), (4.0, 4.0, 0.0, 0.0));
        // Long after this PC went quiet, a rise is another device's.
        let t = step(Some(&t), 6.0, 4 * DAY, 1_000.0, 45);
        let t = step(Some(&t), 9.0, 4 * DAY, 1_000.0, 50);
        assert_eq!(parts(&t), (9.0, 4.0, 5.0, 0.0));
    }

    #[test]
    fn earlier_usage_is_split_at_the_rate_seen_since() {
        // 30% used before watching, while this PC logged 3,000 units.
        let t = step(None, 30.0, 4 * DAY, 3_000.0, 0);
        // While watched, 1,000 units of this PC's work moved the limit 5%.
        let t = step(Some(&t), 30.0, 4 * DAY, 3_500.0, 5);
        let t = step(Some(&t), 35.0, 4 * DAY, 4_000.0, 10);
        let s = t.share();
        // So the 3,000 earlier units were about 15% of the 30%.
        assert_eq!((s.used, s.this_pc, s.other, s.unsplit), (35.0, 20.0, 15.0, 0.0));
        assert!(s.estimated);
    }

    #[test]
    fn a_long_gap_is_not_guessed_until_there_is_a_rate() {
        let t = step(None, 0.0, 4 * DAY, 0.0, 0);
        let t = step(Some(&t), 20.0, 4 * DAY, 9_000.0, 300); // PC asleep for 5 hours
        assert_eq!(parts(&t), (20.0, 0.0, 0.0, 20.0));
    }

    #[test]
    fn an_early_reset_restarts_the_week() {
        let t = step(None, 30.0, 4 * DAY, 5_000.0, 0);
        let t = step(Some(&t), 80.0, 4 * DAY, 9_000.0, 5);
        // A banked reset: usage drops, the reset time doesn't move.
        let t = step(Some(&t), 2.0, 4 * DAY, 600.0, 10);
        assert_eq!(t.window_start, at(5));
        assert_eq!(parts(&t), (2.0, 2.0, 0.0, 0.0));
    }

    #[test]
    fn a_new_week_seen_from_its_start_is_fully_split() {
        let t = step(None, 90.0, 100, 5_000.0, 95);
        // The weekly reset passed between refreshes.
        let t = step(Some(&t), 1.0, 100 + 7 * DAY, 300.0, 102);
        assert_eq!(t.window_start, at(100));
        assert_eq!(parts(&t), (1.0, 1.0, 0.0, 0.0));
    }

    #[test]
    fn a_new_week_after_the_app_was_off_is_unsplit() {
        let t = step(None, 90.0, 100, 5_000.0, 95);
        // Quota was closed for two days; the new week began meanwhile.
        let t = step(Some(&t), 12.0, 100 + 7 * DAY, 3_000.0, 2 * DAY);
        assert_eq!(t.window_start, at(100));
        assert_eq!(parts(&t), (12.0, 0.0, 0.0, 12.0));
    }

    #[test]
    fn a_session_window_is_five_hours() {
        let mut t = begin(None, 10.0, Some(at(120)), SESSION, at(115));
        t.observe(10.0, 0.0, at(115));
        assert_eq!(t.window_start, at(120 - 300));
        // The session resets: a new one, watched from its start.
        let mut t = begin(Some(&t), 3.0, Some(at(120 + 300)), SESSION, at(125));
        t.observe(3.0, 200.0, at(125));
        assert_eq!(t.window_start, at(120));
        assert_eq!(parts(&t), (3.0, 3.0, 0.0, 0.0));
    }

    #[test]
    fn survives_a_save_and_load() {
        let t = step(None, 30.0, 4 * DAY, 5_000.0, 0);
        let back: Track = serde_json::from_str(&serde_json::to_string(&t).unwrap()).unwrap();
        assert_eq!(back, t);
    }
}
