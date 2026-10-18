//! Tokens used this week, counted from the logs Claude Code and Codex keep on
//! this PC (the same numbers their own usage reports add up). Only read; nothing
//! is sent anywhere. Only files changed since the window started are opened.
//! The app decides where each window (week, 5-hour session) starts, since it
//! notices early resets, and calls [`for_account`] after each refresh.

use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::util::timestamp;
use crate::{Account, Meter, ProviderKind, TokenUsage};

/// The weekly limit that defines "this week": Claude's all-models week, else the
/// first weekly gauge.
pub fn weekly_meter(meters: &[Meter]) -> Option<&Meter> {
    meters
        .iter()
        .find(|m| m.label == "Weekly · all models")
        .or_else(|| meters.iter().find(|m| m.label.starts_with("Weekly")))
}

/// The 5-hour session limit.
pub fn session_meter(meters: &[Meter]) -> Option<&Meter> {
    meters.iter().find(|m| m.label.starts_with("Session"))
}

/// Tokens this account's local tool logged since each of `sinces`, in the same
/// order; `None` when it keeps no logs on this PC. Blocking: the logs can be
/// large, so call it off the async threads.
pub fn for_account(account: &Account, sinces: &[DateTime<Utc>]) -> Option<Vec<TokenUsage>> {
    let over = account.data_path.as_deref();
    match account.kind {
        ProviderKind::ClaudePlan => Some(claude(&super::config_dir(over, "CLAUDE_CONFIG_DIR", ".claude")?, sinces)),
        ProviderKind::CodexPlan => Some(codex(&super::codex_plan::sessions_dir(over)?, sinces)),
        _ => None,
    }
}

fn empty(sinces: &[DateTime<Utc>]) -> Vec<TokenUsage> {
    sinces.iter().map(|&since| TokenUsage { since, ..Default::default() }).collect()
}

fn earliest(sinces: &[DateTime<Utc>]) -> DateTime<Utc> {
    sinces.iter().copied().min().unwrap_or_else(Utc::now)
}

/// `.jsonl` files under `dir` changed at or after `since`.
fn logs_since(dir: &Path, since: DateTime<Utc>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), 0u8)];
    while let Some((d, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            let p = e.path();
            if ft.is_dir() && depth < 5 {
                stack.push((p, depth + 1));
            } else if ft.is_file() && p.extension().is_some_and(|x| x == "jsonl") {
                let m = e.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
                if DateTime::<Utc>::from(m) >= since {
                    out.push(p);
                }
            }
        }
    }
    out
}

fn lines(path: &Path) -> impl Iterator<Item = String> {
    File::open(path).ok().into_iter().flat_map(|f| BufReader::new(f).lines().map_while(Result::ok))
}

fn n(v: &Value) -> u64 {
    v.as_u64().unwrap_or(0)
}

/// Claude Code: `projects/**/<session>.jsonl`, one line per reply with its
/// `message.usage`. A reply can be written more than once (streaming, resumed
/// sessions), so each message/request pair is counted once.
pub fn claude(config_dir: &Path, sinces: &[DateTime<Utc>]) -> Vec<TokenUsage> {
    let mut out = empty(sinces);
    let mut seen = HashSet::new();
    for path in logs_since(&config_dir.join("projects"), earliest(sinces)) {
        for line in lines(&path).filter(|l| l.contains("\"usage\"")) {
            add_claude_line(&line, &mut seen, &mut out);
        }
    }
    out
}

/// Adds one log line to every window it falls in.
pub(crate) fn add_claude_line(line: &str, seen: &mut HashSet<String>, out: &mut [TokenUsage]) {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return };
    let Some(ts) = timestamp(&v["timestamp"]) else { return };
    if v["type"].as_str() != Some("assistant") || out.iter().all(|t| ts < t.since) {
        return;
    }
    let msg = &v["message"];
    let u = &msg["usage"];
    if !u.is_object() {
        return;
    }
    let key = format!("{}:{}", msg["id"].as_str().unwrap_or(""), v["requestId"].as_str().unwrap_or(""));
    if key != ":" && !seen.insert(key) {
        return;
    }
    for t in out.iter_mut().filter(|t| ts >= t.since) {
        t.input += n(&u["input_tokens"]);
        t.output += n(&u["output_tokens"]);
        t.cache_write += n(&u["cache_creation_input_tokens"]);
        t.cache_read += n(&u["cache_read_input_tokens"]);
    }
}

/// Codex: `sessions/YYYY/MM/DD/rollout-*.jsonl`, whose `token_count` events
/// carry the session's running total. A session's share of the window is its
/// last total minus the total it had when the window started.
pub fn codex(sessions_dir: &Path, sinces: &[DateTime<Utc>]) -> Vec<TokenUsage> {
    let mut out = empty(sinces);
    for path in logs_since(sessions_dir, earliest(sinces)) {
        add_codex_session(lines(&path).filter(|l| l.contains("token_count")), &mut out);
    }
    out
}

/// Adds one session's growth within each window.
pub(crate) fn add_codex_session(lines: impl Iterator<Item = String>, out: &mut [TokenUsage]) {
    // Per window: the running total (input without cached, cached input, output)
    // just before it began, and the last one inside it.
    let mut before = vec![(0, 0, 0); out.len()];
    let mut last = vec![None; out.len()];
    for line in lines {
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let p = &v["payload"];
        if p["type"].as_str() != Some("token_count") {
            continue;
        }
        let total = &p["info"]["total_token_usage"];
        if !total.is_object() {
            continue;
        }
        let cached = n(&total["cached_input_tokens"]);
        let now = (n(&total["input_tokens"]).saturating_sub(cached), cached, n(&total["output_tokens"]));
        let ts = timestamp(&v["timestamp"]);
        for (i, t) in out.iter().enumerate() {
            match ts {
                Some(ts) if ts < t.since => before[i] = now,
                _ => last[i] = Some(now),
            }
        }
    }
    for (i, t) in out.iter_mut().enumerate() {
        if let Some(l) = last[i] {
            t.input += l.0.saturating_sub(before[i].0);
            t.cache_read += l.1.saturating_sub(before[i].1);
            t.output += l.2.saturating_sub(before[i].2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn weekly_meter_prefers_all_models() {
        let meters = vec![
            Meter::percent("Session (5-hour)", 10.0, None),
            Meter::percent("Weekly · Opus", 50.0, None),
            Meter::percent("Weekly · all models", 30.0, None),
        ];
        assert_eq!(weekly_meter(&meters).unwrap().label, "Weekly · all models");
        assert_eq!(weekly_meter(&meters[..2]).unwrap().label, "Weekly · Opus");
    }

    #[test]
    fn claude_counts_each_reply_once() {
        let since = at("2026-10-12T00:00:00Z");
        let reply = |id: &str, ts: &str| {
            format!(
                r#"{{"type":"assistant","timestamp":"{ts}","requestId":"r-{id}","message":{{"id":"m-{id}","usage":{{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":100,"cache_read_input_tokens":1000}}}}}}"#
            )
        };
        let mut out = vec![
            TokenUsage { since, ..Default::default() },
            TokenUsage { since: at("2026-10-14T00:00:00Z"), ..Default::default() },
        ];
        let mut seen = HashSet::new();
        for l in [
            reply("a", "2026-10-13T10:00:00Z"),
            reply("a", "2026-10-13T10:00:00Z"), // written twice
            reply("b", "2026-10-14T10:00:00Z"),
            reply("old", "2026-10-01T10:00:00Z"), // before the window
            r#"{"type":"user","timestamp":"2026-10-13T10:00:00Z","message":{"usage":{"input_tokens":99}}}"#.into(),
        ] {
            add_claude_line(&l, &mut seen, &mut out);
        }
        let t = &out[0];
        assert_eq!((t.input, t.output, t.cache_write, t.cache_read), (20, 10, 200, 2000));
        // The later window only has the reply from the 14th.
        let t = &out[1];
        assert_eq!((t.input, t.output, t.cache_write, t.cache_read), (10, 5, 100, 1000));
    }

    #[test]
    fn codex_takes_the_growth_within_the_window() {
        let since = at("2026-10-12T00:00:00Z");
        let ev = |ts: &str, input: u64, cached: u64, output: u64| {
            format!(
                r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{input},"cached_input_tokens":{cached},"output_tokens":{output}}}}}}}}}"#
            )
        };
        let mut out = vec![
            TokenUsage { since, ..Default::default() },
            TokenUsage { since: at("2026-10-12T01:30:00Z"), ..Default::default() },
        ];
        let session = vec![
            ev("2026-10-11T23:00:00Z", 1000, 400, 50), // before the window
            ev("2026-10-12T01:00:00Z", 3000, 1400, 150),
            ev("2026-10-12T01:00:05Z", 3000, 1400, 150), // repeated
            ev("2026-10-12T02:00:00Z", 5000, 2400, 250),
        ];
        add_codex_session(session.into_iter(), &mut out);
        let t = &out[0];
        assert_eq!((t.input, t.cache_read, t.output), (2000, 2000, 200));
        // From 01:30: 1600→2600 input, 1400→2400 cached, 150→250 output.
        let t = &out[1];
        assert_eq!((t.input, t.cache_read, t.output), (1000, 1000, 100));
    }
}
