//! Where "this week" starts, and how much of the weekly limit this PC used.
//!
//! The week starts 7 days before the weekly limit resets, unless the weekly
//! usage drops while the reset time stays put: then a banked limit reset was
//! used, and the week restarts at that moment.
//!
//! The services only report usage for the whole account, so this PC's share is
//! estimated between refreshes: when the weekly percentage rises while this PC's
//! logs gained tokens, the rise counts as this PC; when it rises while they
//! didn't, it was another device. A rise Quota can't place (from before it was
//! watching, or across a long gap such as the PC sleeping) is kept apart.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::WeeklyShare;

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
    /// This PC's tokens for the week at the last refresh; `None` until the first
    /// count of a week Quota didn't see begin (earlier tokens aren't attributed).
    pub last_tokens: Option<u64>,
    /// Percentage points of the limit attributed to this PC.
    pub this_pc: f64,
    /// Percentage points that couldn't be placed.
    pub unsplit: f64,
    pub tracked_since: DateTime<Utc>,
}

/// Two reset times this close are the same window (the reported time can wobble).
const SAME_WINDOW: i64 = 60 * 60;
/// A drop this large (percentage points) means the limit was reset early.
const DROP: f64 = 1.0;
/// Refreshes further apart than this can't tell whose usage it was.
const MAX_GAP: i64 = 60 * 60;

/// Decides the week for this refresh. Call before counting tokens (from
/// `window_start`), then record the count with [`Track::observe`].
pub fn begin(prev: Option<&Track>, pct: f64, resets_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Track {
    let resets_at = resets_at.filter(|r| *r > now);
    let natural = resets_at.map(|r| r - Duration::days(7));
    // A new week: if Quota was looking right before it began, it's split from
    // the start; otherwise what's already used stays unsplit.
    let new_week = |start: DateTime<Utc>, last_seen: Option<DateTime<Utc>>| match last_seen {
        Some(seen) => Track {
            window_start: start,
            resets_at,
            last_seen: seen,
            last_pct: 0.0,
            last_tokens: Some(0),
            this_pc: 0.0,
            unsplit: 0.0,
            tracked_since: start,
        },
        None => Track {
            window_start: start,
            resets_at,
            last_seen: now,
            last_pct: pct,
            last_tokens: None,
            this_pc: 0.0,
            unsplit: pct,
            tracked_since: now,
        },
    };

    let Some(prev) = prev else {
        return new_week(natural.unwrap_or(now - Duration::days(7)), None);
    };
    let moved = match (prev.resets_at, resets_at) {
        (Some(a), Some(b)) => (a - b).num_seconds().abs() > SAME_WINDOW,
        (None, Some(_)) => true,
        // The old window ran out and no new one has started yet.
        (Some(a), None) => a <= now,
        (None, None) => false,
    };
    if moved {
        let start = natural.or(prev.resets_at.filter(|r| *r <= now)).unwrap_or(now - Duration::days(7));
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
    /// `tokens` this PC's tokens since `window_start`.
    pub fn observe(&mut self, pct: f64, tokens: u64, now: DateTime<Utc>) {
        if let Some(last) = self.last_tokens {
            let rose = pct - self.last_pct;
            if rose > 0.0 {
                if (now - self.last_seen).num_seconds() > MAX_GAP {
                    self.unsplit += rose;
                } else if tokens > last {
                    self.this_pc += rose;
                }
            }
        }
        self.last_pct = pct;
        self.last_tokens = Some(tokens);
        self.last_seen = now;
    }

    pub fn share(&self) -> WeeklyShare {
        let used = self.last_pct.max(0.0);
        let this_pc = self.this_pc.min(used);
        WeeklyShare {
            used,
            this_pc,
            unsplit: self.unsplit.min(used - this_pc),
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

    fn step(prev: Option<&Track>, pct: f64, reset: i64, tokens: u64, now: i64) -> Track {
        let mut t = begin(prev, pct, Some(at(reset)), at(now));
        t.observe(pct, tokens, at(now));
        t
    }

    fn parts(t: &Track) -> (f64, f64, f64) {
        let s = t.share();
        (s.used, s.this_pc, s.unsplit)
    }

    #[test]
    fn first_look_mid_week_leaves_earlier_usage_unsplit() {
        let t = step(None, 30.0, 4 * DAY, 5_000, 0);
        assert_eq!(t.window_start, at(4 * DAY) - Duration::days(7));
        assert_eq!(parts(&t), (30.0, 0.0, 30.0));
    }

    #[test]
    fn rises_count_for_this_pc_only_while_it_was_busy() {
        let t = step(None, 30.0, 4 * DAY, 5_000, 0);
        let t = step(Some(&t), 34.0, 4 * DAY, 9_000, 5); // this PC worked
        let t = step(Some(&t), 40.0, 4 * DAY, 9_000, 10); // another device
        let t = step(Some(&t), 41.0, 4 * DAY, 9_500, 15); // this PC again
        assert_eq!(parts(&t), (41.0, 5.0, 30.0));
    }

    #[test]
    fn a_long_gap_is_not_guessed() {
        let t = step(None, 30.0, 4 * DAY, 5_000, 0);
        let t = step(Some(&t), 50.0, 4 * DAY, 9_000, 300); // PC was asleep for 5 hours
        assert_eq!(parts(&t), (50.0, 0.0, 50.0));
    }

    #[test]
    fn an_early_reset_restarts_the_week() {
        let t = step(None, 30.0, 4 * DAY, 5_000, 0);
        let t = step(Some(&t), 80.0, 4 * DAY, 9_000, 5);
        // A banked reset: usage drops, the reset time doesn't move.
        let t = step(Some(&t), 2.0, 4 * DAY, 600, 10);
        assert_eq!(t.window_start, at(5));
        assert_eq!(parts(&t), (2.0, 2.0, 0.0));
    }

    #[test]
    fn a_new_week_seen_from_its_start_is_fully_split() {
        let t = step(None, 90.0, 100, 5_000, 95);
        // The weekly reset passed between refreshes.
        let t = step(Some(&t), 1.0, 100 + 7 * DAY, 300, 102);
        assert_eq!(t.window_start, at(100));
        assert_eq!(parts(&t), (1.0, 1.0, 0.0));
    }

    #[test]
    fn a_new_week_after_the_app_was_off_is_not_split() {
        let t = step(None, 90.0, 100, 5_000, 95);
        // Quota was closed for two days; the new week began meanwhile.
        let t = step(Some(&t), 12.0, 100 + 7 * DAY, 3_000, 2 * DAY);
        assert_eq!(t.window_start, at(100));
        assert_eq!(parts(&t), (12.0, 0.0, 12.0));
    }

    #[test]
    fn survives_a_save_and_load() {
        let t = step(None, 30.0, 4 * DAY, 5_000, 0);
        let back: Track = serde_json::from_str(&serde_json::to_string(&t).unwrap()).unwrap();
        assert_eq!(back, t);
    }
}
