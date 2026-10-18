//! Tokens used this week, counted from the logs Claude Code and Codex keep on
//! this PC (the same numbers their own usage reports add up). Only read; nothing
//! is sent anywhere. Only files changed since the window started are opened.

use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Duration, Utc};
use serde_json::Value;

use crate::util::timestamp;
use crate::{Meter, TokenUsage};

/// Start of the current weekly window: a week before the weekly limit resets,
/// or simply the last 7 days when there's no weekly limit to go by.
pub(crate) fn week_start(meters: &[Meter], now: DateTime<Utc>) -> DateTime<Utc> {
    meters
        .iter()
        .filter(|m| m.label.starts_with("Weekly"))
        .filter_map(|m| m.resets_at)
        .filter(|r| *r > now)
        .min()
        .map(|r| r - Duration::days(7))
        .unwrap_or(now - Duration::days(7))
}

/// Runs a log scan off the async threads; the logs can be large.
pub(crate) async fn scan(f: impl FnOnce() -> Option<TokenUsage> + Send + 'static) -> Option<TokenUsage> {
    tokio::task::spawn_blocking(f).await.ok().flatten()
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

fn some_if_used(t: TokenUsage) -> Option<TokenUsage> {
    (t.input + t.output + t.cache_read + t.cache_write > 0).then_some(t)
}

/// Claude Code: `projects/**/<session>.jsonl`, one line per reply with its
/// `message.usage`. A reply can be written more than once (streaming, resumed
/// sessions), so each message/request pair is counted once.
pub fn claude(config_dir: &Path, since: DateTime<Utc>) -> Option<TokenUsage> {
    let mut t = TokenUsage { since, ..Default::default() };
    let mut seen = HashSet::new();
    for path in logs_since(&config_dir.join("projects"), since) {
        for line in lines(&path).filter(|l| l.contains("\"usage\"")) {
            add_claude_line(&line, since, &mut seen, &mut t);
        }
    }
    some_if_used(t)
}

pub(crate) fn add_claude_line(line: &str, since: DateTime<Utc>, seen: &mut HashSet<String>, t: &mut TokenUsage) {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return };
    if v["type"].as_str() != Some("assistant") || timestamp(&v["timestamp"]).is_none_or(|ts| ts < since) {
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
    t.input += n(&u["input_tokens"]);
    t.output += n(&u["output_tokens"]);
    t.cache_write += n(&u["cache_creation_input_tokens"]);
    t.cache_read += n(&u["cache_read_input_tokens"]);
}

/// Codex: `sessions/YYYY/MM/DD/rollout-*.jsonl`, whose `token_count` events
/// carry the session's running total. A session's share of the window is its
/// last total minus the total it had when the window started.
pub fn codex(sessions_dir: &Path, since: DateTime<Utc>) -> Option<TokenUsage> {
    let mut t = TokenUsage { since, ..Default::default() };
    for path in logs_since(sessions_dir, since) {
        add_codex_session(lines(&path).filter(|l| l.contains("token_count")), since, &mut t);
    }
    some_if_used(t)
}

pub(crate) fn add_codex_session(lines: impl Iterator<Item = String>, since: DateTime<Utc>, t: &mut TokenUsage) {
    // (input without cached, cached input, output)
    let mut before = (0, 0, 0);
    let mut last = None;
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
        match timestamp(&v["timestamp"]) {
            Some(ts) if ts < since => before = now,
            _ => last = Some(now),
        }
    }
    if let Some(l) = last {
        t.input += l.0.saturating_sub(before.0);
        t.cache_read += l.1.saturating_sub(before.1);
        t.output += l.2.saturating_sub(before.2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn claude_counts_each_reply_once() {
        let since = at("2026-10-12T00:00:00Z");
        let reply = |id: &str, ts: &str| {
            format!(
                r#"{{"type":"assistant","timestamp":"{ts}","requestId":"r-{id}","message":{{"id":"m-{id}","usage":{{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":100,"cache_read_input_tokens":1000}}}}}}"#
            )
        };
        let mut t = TokenUsage::default();
        let mut seen = HashSet::new();
        for l in [
            reply("a", "2026-10-13T10:00:00Z"),
            reply("a", "2026-10-13T10:00:00Z"), // written twice
            reply("b", "2026-10-14T10:00:00Z"),
            reply("old", "2026-10-01T10:00:00Z"), // before the window
            r#"{"type":"user","timestamp":"2026-10-13T10:00:00Z","message":{"usage":{"input_tokens":99}}}"#.into(),
        ] {
            add_claude_line(&l, since, &mut seen, &mut t);
        }
        assert_eq!((t.input, t.output, t.cache_write, t.cache_read), (20, 10, 200, 2000));
    }

    #[test]
    fn codex_takes_the_growth_within_the_window() {
        let since = at("2026-10-12T00:00:00Z");
        let ev = |ts: &str, input: u64, cached: u64, output: u64| {
            format!(
                r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{input},"cached_input_tokens":{cached},"output_tokens":{output}}}}}}}}}"#
            )
        };
        let mut t = TokenUsage::default();
        let session = vec![
            ev("2026-10-11T23:00:00Z", 1000, 400, 50), // before the window
            ev("2026-10-12T01:00:00Z", 3000, 1400, 150),
            ev("2026-10-12T01:00:05Z", 3000, 1400, 150), // repeated
            ev("2026-10-12T02:00:00Z", 5000, 2400, 250),
        ];
        add_codex_session(session.into_iter(), since, &mut t);
        assert_eq!((t.input, t.cache_read, t.output), (2000, 2000, 200));
    }

    #[test]
    fn week_starts_a_week_before_the_weekly_reset() {
        let now = at("2026-10-16T12:00:00Z");
        let meters = vec![
            Meter::percent("Session (5-hour)", 10.0, Some(at("2026-10-16T14:00:00Z"))),
            Meter::percent("Weekly · all models", 40.0, Some(at("2026-10-19T08:00:00Z"))),
        ];
        assert_eq!(week_start(&meters, now), at("2026-10-12T08:00:00Z"));
        assert_eq!(week_start(&[], now), now - Duration::days(7));
    }
}
