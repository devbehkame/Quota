//! ChatGPT Plus / Pro limits, the same numbers Codex CLI shows in `/status`.
//!
//! OpenAI has no public API for ChatGPT plan limits. Two sources are used:
//!
//! 1. **Live** (preferred): the usage endpoint Codex itself polls every minute,
//!    authenticated with the ChatGPT sign-in Codex keeps in `~/.codex/auth.json`.
//!    The token is only read, never refreshed or rewritten, and is sent only to chatgpt.com.
//! 2. **Fallback**: the rate-limit windows Codex records in its local session logs
//!    (`~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`) after each message. Used when
//!    there is no file-based sign-in (e.g. Codex stores it in the OS keyring) or the
//!    live call fails.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Duration, Utc};
use serde_json::Value;

use crate::util::{jwt_claims, num, send_json, timestamp};
use crate::{Account, LimitReset, LimitResets, Meter, Usage, UsageError};

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
/// Each banked limit reset with its expiry, as Codex's `/usage` lists them.
const RESETS_URL: &str = "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits";

/// Only the end of each log is scanned; rate-limit events are written every turn.
const TAIL_BYTES: u64 = 512 * 1024;
const MAX_FILES: usize = 25;

fn codex_home(over: Option<&str>) -> Option<PathBuf> {
    super::config_dir(over, "CODEX_HOME", ".codex")
}

pub fn sessions_dir(over: Option<&str>) -> Option<PathBuf> {
    codex_home(over).map(|d| d.join("sessions"))
}

pub fn auth_path(over: Option<&str>) -> Option<PathBuf> {
    codex_home(over).map(|d| d.join("auth.json"))
}

pub async fn fetch(client: &reqwest::Client, account: &Account) -> Result<Usage, UsageError> {
    let auth = read_auth(account.data_path.as_deref());
    let email = auth.as_ref().and_then(|a| a.email.clone());
    fetch_usage(client, account, auth).await.map(|mut u| {
        u.signed_in_as = email;
        u
    })
}

async fn fetch_usage(client: &reqwest::Client, account: &Account, auth: Option<Auth>) -> Result<Usage, UsageError> {
    let over = account.data_path.as_deref();
    let live_err = match auth {
        Some(auth) => match fetch_live(client, &auth).await {
            Ok(u) => return Ok(u),
            Err(e) => Some(e),
        },
        None => None,
    };

    match from_logs(over) {
        Ok(mut usage) => {
            let why = match &live_err {
                Some(UsageError::Unauthorized) | Some(UsageError::Forbidden(_)) => {
                    "Live data unavailable: your Codex sign-in has expired. Run `codex` once to renew it."
                        .to_string()
                }
                Some(e) => format!("Live data unavailable ({e})."),
                None => "Live data needs Codex signed in with ChatGPT (`codex login`).".to_string(),
            };
            usage.notes.insert(0, why);
            Ok(usage)
        }
        Err(log_err) => Err(match live_err {
            Some(UsageError::Unauthorized) | Some(UsageError::Forbidden(_)) => UsageError::Setup(
                "Your Codex sign-in has expired. Run `codex` once to renew it, then refresh.".into(),
            ),
            Some(e) => e,
            None => log_err,
        }),
    }
}

struct Auth {
    access_token: String,
    account_id: Option<String>,
    /// From the ChatGPT sign-in's ID token.
    email: Option<String>,
}

fn read_auth(over: Option<&str>) -> Option<Auth> {
    let raw = std::fs::read_to_string(auth_path(over)?).ok()?;
    parse_auth(&raw)
}

fn parse_auth(raw: &str) -> Option<Auth> {
    let v: Value = serde_json::from_str(raw).ok()?;
    let t = &v["tokens"];
    let access_token = t["access_token"].as_str().filter(|s| !s.is_empty())?.to_owned();
    let account_id = t["account_id"].as_str().filter(|s| !s.is_empty()).map(str::to_owned);
    let claims = t["id_token"].as_str().and_then(jwt_claims).unwrap_or_default();
    let email = claims["email"]
        .as_str()
        .or(claims["https://api.openai.com/profile"]["email"].as_str())
        .map(str::trim)
        .filter(|e| e.contains('@'))
        .map(str::to_owned);
    Some(Auth { access_token, account_id, email })
}

fn get(client: &reqwest::Client, url: &str, auth: &Auth) -> reqwest::RequestBuilder {
    let req = client.get(url).bearer_auth(&auth.access_token).header("accept", "application/json");
    match &auth.account_id {
        Some(id) => req.header("ChatGPT-Account-Id", id),
        None => req,
    }
}

async fn fetch_live(client: &reqwest::Client, auth: &Auth) -> Result<Usage, UsageError> {
    let body = send_json(get(client, USAGE_URL, auth)).await?;
    let mut usage =
        parse_live(&body, Utc::now()).ok_or_else(|| UsageError::Parse("no rate-limit windows in the response".into()))?;
    // The usage response only has the count; the expiry dates take one more call,
    // made only when there is something to list. Without them the count still shows.
    if usage.resets.as_ref().is_some_and(|r| r.available > 0) {
        if let Ok(details) = send_json(get(client, RESETS_URL, auth)).await {
            if let Some(r) = parse_reset_details(&details, Utc::now()) {
                usage.resets = Some(r);
            }
        }
    }
    Ok(usage)
}

/// `rate_limit_reset_credits.available_count` from the usage response.
fn reset_count(v: &Value) -> Option<LimitResets> {
    let n = num(&v["rate_limit_reset_credits"]["available_count"])?;
    Some(LimitResets { available: n.max(0.0) as u32, items: vec![] })
}

/// The reset-credits list: one entry per credit, of which only "available" ones count.
pub(crate) fn parse_reset_details(v: &Value, now: DateTime<Utc>) -> Option<LimitResets> {
    let credits = v["credits"].as_array()?;
    let items = credits
        .iter()
        .filter(|c| c["status"].as_str() == Some("available"))
        .filter_map(|c| {
            let expires_at = timestamp(&c["expires_at"]);
            if expires_at.is_some_and(|e| e <= now) {
                return None;
            }
            let label = c["title"].as_str().map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned);
            Some(LimitReset { label, count: 1, expires_at })
        })
        .collect();
    Some(LimitResets::new(items))
}

fn live_windows(details: &Value) -> Vec<Window> {
    ["primary_window", "secondary_window"]
        .iter()
        .filter_map(|k| {
            let w = &details[*k];
            Some(Window {
                used_percent: num(&w["used_percent"])?,
                window_minutes: num(&w["limit_window_seconds"]).map(|s| s / 60.0),
                resets_at: timestamp(&w["reset_at"]).or_else(|| {
                    num(&w["reset_after_seconds"]).map(|s| Utc::now() + Duration::seconds(s as i64))
                }),
            })
        })
        .collect()
}

pub(crate) fn parse_live(v: &Value, now: DateTime<Utc>) -> Option<Usage> {
    let windows = live_windows(&v["rate_limit"]);
    let mut extra = Vec::new();
    for a in v["additional_rate_limits"].as_array().into_iter().flatten() {
        let name = a["limit_name"].as_str().or(a["metered_feature"].as_str()).unwrap_or("Extra");
        for (i, w) in live_windows(&a["rate_limit"]).into_iter().enumerate() {
            extra.push((format!("{name} · {}", window_label(w.window_minutes, i)), w));
        }
    }
    if windows.is_empty() && extra.is_empty() {
        return None;
    }
    let mut usage = to_usage(Found { windows, plan: plan_name(&v["plan_type"]), observed_at: now }, now, None);
    usage.resets = reset_count(v);
    for (label, w) in extra {
        usage.meters.push(window_meter(label, &w, now));
    }
    if v["rate_limit"]["limit_reached"].as_bool() == Some(true) {
        usage.notes.push("You have hit a usage limit. It frees up at the reset time shown.".into());
    }
    let credits = &v["credits"];
    if credits["unlimited"].as_bool() == Some(true) {
        usage.notes.push("Unlimited credits.".into());
    } else if credits["has_credits"].as_bool() == Some(true) {
        let balance = credits["balance"].as_str().map(str::to_owned).or_else(|| num(&credits["balance"]).map(|n| n.to_string()));
        if let Some(b) = balance.filter(|b| !b.is_empty()) {
            usage.notes.push(format!("Credit balance: {b}"));
        }
    }
    Some(usage)
}

fn plan_name(v: &Value) -> Option<String> {
    let p = v.as_str().filter(|p| !p.is_empty() && *p != "unknown")?;
    Some(match p {
        "prolite" => "Pro Lite".into(),
        "promax" => "Pro Max".into(),
        "edu_plus" => "Edu Plus".into(),
        "edu_pro" => "Edu Pro".into(),
        other => {
            let s = other.replace('_', " ");
            let mut c = s.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
        }
    })
}

fn from_logs(over: Option<&str>) -> Result<Usage, UsageError> {
    let dir = sessions_dir(over).ok_or_else(|| UsageError::Setup("Could not locate your home folder.".into()))?;
    if !dir.is_dir() {
        return Err(UsageError::Setup(format!(
            "No Codex sign-in found in {}. Install the Codex CLI and run `codex login` with your ChatGPT account.",
            dir.parent().unwrap_or(&dir).display()
        )));
    }
    for (modified, path) in &recent_logs(&dir) {
        if let Some(found) = last_rate_limits(path, *modified) {
            let note = format!(
                "As of your last Codex message ({}).",
                found.observed_at.format("%Y-%m-%d %H:%M UTC")
            );
            return Ok(to_usage(found, Utc::now(), Some(note)));
        }
    }
    Err(UsageError::Setup(
        "No Codex sign-in or recorded limits found yet. Run `codex login` with your ChatGPT account, then refresh."
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
    Some(Found { windows, plan: plan_name(&rl["plan_type"]), observed_at })
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

fn window_meter(label: String, w: &Window, now: DateTime<Utc>) -> Meter {
    if w.resets_at.is_some_and(|r| r <= now) {
        // The window rolled over since this snapshot was taken; it starts fresh.
        Meter::percent(label, 0.0, None).with_detail("Window has reset since this was recorded")
    } else {
        Meter::percent(label, w.used_percent, w.resets_at)
    }
}

pub(crate) fn to_usage(found: Found, now: DateTime<Utc>, note: Option<String>) -> Usage {
    let meters = found
        .windows
        .iter()
        .enumerate()
        .map(|(i, w)| window_meter(window_label(w.window_minutes, i), w, now))
        .collect();
    Usage { plan: found.plan, meters, notes: note.into_iter().collect(), signed_in_as: None, resets: None }
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
        let u = to_usage(f, t(10), None);
        assert_eq!(u.meters[0].label, "Session (5-hour)");
        assert_eq!(u.meters[0].remaining, Some(58.0));
        assert_eq!(u.meters[0].resets_at, Some(t(11)));
        assert_eq!(u.meters[1].label, "Weekly");
    }

    #[test]
    fn parses_limit_resets() {
        let now = t(0);
        let usage = serde_json::json!({
            "rate_limit": {"primary_window": {"used_percent": 10, "limit_window_seconds": 18000}},
            "rate_limit_reset_credits": {"available_count": 2}
        });
        assert_eq!(parse_live(&usage, now).unwrap().resets.unwrap().available, 2);
        let plain = serde_json::json!({"rate_limit": {"primary_window": {"used_percent": 10}}});
        assert_eq!(parse_live(&plain, now).unwrap().resets, None);

        let details = serde_json::json!({"available_count": 2, "credits": [
            {"id": "b", "reset_type": "full", "status": "available", "granted_at": "2026-06-18T00:00:00Z", "expires_at": null},
            {"id": "a", "reset_type": "full", "status": "available", "granted_at": "2026-06-17T00:00:00Z",
             "expires_at": "2099-07-17T00:00:00Z", "title": "Full reset (Weekly + 5 hr)"},
            {"id": "c", "reset_type": "full", "status": "redeemed", "granted_at": "2026-06-01T00:00:00Z", "expires_at": null}
        ]});
        let r = parse_reset_details(&details, now).unwrap();
        assert_eq!(r.available, 2);
        assert_eq!(r.items[0].label.as_deref(), Some("Full reset (Weekly + 5 hr)"));
        assert_eq!(r.items[1].expires_at, None);
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
        let u = to_usage(parse_line(line, t(0)).unwrap(), t(9), None);
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

    #[test]
    fn parses_live_usage() {
        let now = t(10);
        let reset = (now + Duration::hours(2)).timestamp();
        let v = serde_json::json!({
            "plan_type": "pro",
            "rate_limit": {
                "allowed": true, "limit_reached": false,
                "primary_window": {"used_percent": 30, "limit_window_seconds": 18000, "reset_after_seconds": 7200, "reset_at": reset},
                "secondary_window": {"used_percent": 55, "limit_window_seconds": 604800, "reset_after_seconds": 400000, "reset_at": reset + 400000}
            },
            "credits": {"has_credits": true, "unlimited": false, "balance": "120"},
            "additional_rate_limits": [
                {"limit_name": "GPT-5-Codex-Spark", "metered_feature": "codex_spark",
                 "rate_limit": {"allowed": true, "limit_reached": false,
                   "primary_window": {"used_percent": 10, "limit_window_seconds": 18000, "reset_after_seconds": 100, "reset_at": reset}}}
            ]
        });
        let u = parse_live(&v, now).unwrap();
        assert_eq!(u.plan.as_deref(), Some("Pro"));
        assert_eq!(plan_name(&serde_json::json!("prolite")).as_deref(), Some("Pro Lite"));
        let labels: Vec<_> = u.meters.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(labels, ["Session (5-hour)", "Weekly", "GPT-5-Codex-Spark · Session (5-hour)"]);
        assert_eq!(u.meters[0].remaining, Some(70.0));
        assert_eq!(u.meters[0].resets_at.map(|r| r.timestamp()), Some(reset));
        assert_eq!(u.notes, ["Credit balance: 120"]);
        assert!(parse_live(&serde_json::json!({"plan_type": "plus", "rate_limit": null}), now).is_none());
    }

    #[test]
    fn reads_auth_file() {
        let a = parse_auth(r#"{"OPENAI_API_KEY":null,"tokens":{"id_token":"x.y.z","access_token":"acc","refresh_token":"ref","account_id":"org-1"},"last_refresh":"2026-10-01T00:00:00Z"}"#).unwrap();
        assert_eq!(a.access_token, "acc");
        assert_eq!(a.account_id.as_deref(), Some("org-1"));
        assert_eq!(a.email, None);
        // {"email":"me@example.com"}
        let a = parse_auth(r#"{"tokens":{"id_token":"x.eyJlbWFpbCI6Im1lQGV4YW1wbGUuY29tIn0.z","access_token":"acc"}}"#).unwrap();
        assert_eq!(a.email.as_deref(), Some("me@example.com"));
        assert!(parse_auth(r#"{"OPENAI_API_KEY":"sk-123"}"#).is_none());
    }

}
