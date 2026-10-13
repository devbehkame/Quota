//! ChatGPT Plus / Pro limits as seen by the Codex CLI.
//!
//! OpenAI has no public API for ChatGPT plan limits. The Codex CLI records the
//! rate-limit windows it receives in its local session logs
//! (`~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`), so we read the most recent
//! entry from disk. Nothing is sent over the network and no tokens are touched.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Duration, Utc};
use serde_json::Value;

use crate::util::{num, timestamp};
use crate::{Account, Meter, Usage, UsageError};

/// Only the end of each log is scanned; rate-limit events are written every turn.
const TAIL_BYTES: u64 = 512 * 1024;
const MAX_FILES: usize = 25;

pub fn sessions_dir(over: Option<&str>) -> Option<PathBuf> {
    super::config_dir(over, "CODEX_HOME", ".codex").map(|d| d.join("sessions"))
}

pub async fn fetch(account: &Account) -> Result<Usage, UsageError> {
    let dir = sessions_dir(account.data_path.as_deref())
        .ok_or_else(|| UsageError::Setup("Could not locate your home folder.".into()))?;
    if !dir.is_dir() {
        return Err(UsageError::Setup(format!(
            "No Codex sessions found in {}. Install the Codex CLI, sign in with ChatGPT, and send one message.",
            dir.display()
        )));
    }
    let files = recent_logs(&dir);
    for (modified, path) in &files {
        if let Some(found) = last_rate_limits(path, *modified) {
            return Ok(to_usage(found, Utc::now()));
        }
    }
    Err(UsageError::Setup(
        "Codex is installed but no plan limits were recorded yet. Send a message in Codex (signed in with ChatGPT), then refresh."
            .into(),
    ))
}

/// Newest `.jsonl` files under `sessions/` (which is laid out as YYYY/MM/DD).
fn recent_logs(dir: &Path) -> Vec<(DateTime<Utc>, PathBuf)> {
    let mut out = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), 0u8)];
    while let Some((d, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            let p = e.path();
            if ft.is_dir() && depth < 4 {
                stack.push((p, depth + 1));
            } else if ft.is_file() && p.extension().is_some_and(|x| x == "jsonl") {
                let m = e.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
                out.push((DateTime::<Utc>::from(m), p));
            }
        }
    }
    out.sort_by_key(|e| std::cmp::Reverse(e.0));
    out.truncate(MAX_FILES);
    out
}

pub(crate) struct Found {
    windows: Vec<Window>,
    plan: Option<String>,
    observed_at: DateTime<Utc>,
}

struct Window {
    used_percent: f64,
    window_minutes: Option<f64>,
    resets_at: Option<DateTime<Utc>>,
}

fn last_rate_limits(path: &Path, modified: DateTime<Utc>) -> Option<Found> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::with_capacity((len - start) as usize);
    f.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    // When starting mid-file the first line is partial; JSON parsing just skips it.
    text.lines().rev().filter(|l| l.contains("rate_limits")).find_map(|l| parse_line(l, modified))
}

pub(crate) fn parse_line(line: &str, fallback_time: DateTime<Utc>) -> Option<Found> {
    let v: Value = serde_json::from_str(line).ok()?;
    let rl = find_key(&v, "rate_limits", 0)?;
    if !rl.is_object() {
        return None;
    }
    let observed_at = timestamp(&v["timestamp"]).unwrap_or(fallback_time);
    let windows: Vec<Window> = ["primary", "secondary"]
        .iter()
        .filter_map(|k| {
            let w = &rl[*k];
            let used_percent = num(&w["used_percent"])?;
            let resets_at = timestamp(&w["resets_at"]).or_else(|| {
                let secs = num(&w["resets_in_seconds"]).or_else(|| num(&w["reset_after_seconds"]))?;
                Some(observed_at + Duration::seconds(secs as i64))
            });
            Some(Window { used_percent, window_minutes: num(&w["window_minutes"]), resets_at })
        })
        .collect();
    if windows.is_empty() {
        return None;
    }
    let plan = rl["plan_type"].as_str().map(|p| {
        let mut c = p.chars();
        c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
    });
    Some(Found { windows, plan, observed_at })
}

fn find_key<'a>(v: &'a Value, key: &str, depth: u8) -> Option<&'a Value> {
    if depth > 6 {
        return None;
    }
    match v {
        Value::Object(map) => map
            .get(key)
            .filter(|x| !x.is_null())
            .or_else(|| map.values().find_map(|c| find_key(c, key, depth + 1))),
        _ => None,
    }
}

fn window_label(minutes: Option<f64>, index: usize) -> String {
    match minutes.map(|m| m.round() as i64) {
        Some(300) => "Session (5-hour)".into(),
        Some(10080) => "Weekly".into(),
        Some(m) if m > 0 && m % 1440 == 0 => format!("{}-day window", m / 1440),
        Some(m) if m > 0 && m % 60 == 0 => format!("{}-hour window", m / 60),
        Some(m) if m > 0 => format!("{m}-minute window"),
        _ if index == 0 => "Short window".into(),
        _ => "Long window".into(),
    }
}

pub(crate) fn to_usage(found: Found, now: DateTime<Utc>) -> Usage {
    let meters = found
        .windows
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let label = window_label(w.window_minutes, i);
            if w.resets_at.is_some_and(|r| r <= now) {
                // The window rolled over since Codex last reported; it starts fresh.
                Meter::percent(label, 0.0, None).with_detail("Window has reset since your last Codex message")
            } else {
                Meter::percent(label, w.used_percent, w.resets_at)
            }
        })
        .collect();
    Usage {
        plan: found.plan,
        meters,
        notes: vec![format!(
            "As of your last Codex message ({}). Updates each time you use Codex.",
            found.observed_at.format("%Y-%m-%d %H:%M UTC")
        )],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 13, h, 0, 0).unwrap()
    }

    #[test]
    fn parses_resets_in_seconds() {
        let line = r#"{"timestamp":"2026-10-13T10:00:00Z","type":"event_msg","payload":{"type":"token_count","info":null,"rate_limits":{"primary":{"used_percent":42.0,"window_minutes":300,"resets_in_seconds":3600},"secondary":{"used_percent":7.5,"window_minutes":10080,"resets_in_seconds":86400}}}}"#;
        let f = parse_line(line, t(0)).unwrap();
        let u = to_usage(f, t(10));
        assert_eq!(u.meters[0].label, "Session (5-hour)");
        assert_eq!(u.meters[0].remaining, Some(58.0));
        assert_eq!(u.meters[0].resets_at, Some(t(11)));
        assert_eq!(u.meters[1].label, "Weekly");
    }

    #[test]
    fn parses_resets_at_and_plan() {
        let line = r#"{"timestamp":"2026-10-13T10:00:00Z","payload":{"rate_limits":{"plan_type":"plus","primary":{"used_percent":90,"window_minutes":300,"resets_at":1792000000},"secondary":null}}}"#;
        let f = parse_line(line, t(0)).unwrap();
        assert_eq!(f.plan.as_deref(), Some("Plus"));
        assert_eq!(f.windows.len(), 1);
    }

    #[test]
    fn expired_window_shows_fresh() {
        let line = r#"{"timestamp":"2026-10-13T01:00:00Z","payload":{"rate_limits":{"primary":{"used_percent":95,"window_minutes":300,"resets_in_seconds":60}}}}"#;
        let u = to_usage(parse_line(line, t(0)).unwrap(), t(9));
        assert_eq!(u.meters[0].percent_used, Some(0.0));
        assert!(u.meters[0].detail.is_some());
    }

    #[test]
    fn ignores_lines_without_limits() {
        assert!(parse_line(r#"{"payload":{"rate_limits":null}}"#, t(0)).is_none());
        assert!(parse_line("not json", t(0)).is_none());
    }

    #[test]
    fn reads_newest_log_from_disk() {
        let root = std::env::temp_dir().join(format!("usagecheck-codex-{}", std::process::id()));
        let day = root.join("sessions/2026/10/13");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(
            day.join("rollout-a.jsonl"),
            concat!(
                r#"{"payload":{"type":"user_message"}}"#, "\n",
                r#"{"timestamp":"2099-01-01T00:00:00Z","payload":{"rate_limits":{"primary":{"used_percent":20,"window_minutes":300,"resets_in_seconds":100}}}}"#, "\n",
                r#"{"payload":{"type":"agent_message"}}"#, "\n",
            ),
        )
        .unwrap();
        let files = recent_logs(&root.join("sessions"));
        assert_eq!(files.len(), 1);
        let f = last_rate_limits(&files[0].1, files[0].0).unwrap();
        assert_eq!(f.windows[0].used_percent, 20.0);
        std::fs::remove_dir_all(root).ok();
    }
}
