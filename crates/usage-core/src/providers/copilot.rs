//! GitHub Copilot quotas: premium requests, plus chat / completions on the Free plan.
//!
//! The rich numbers (allowance, remaining, reset date) come from the quota endpoint
//! the Copilot editor plugins use. The token is taken from, in order: a token the
//! user pasted, the sign-in that Copilot for JetBrains / Visual Studio / Vim keeps in
//! `%LOCALAPPDATA%\github-copilot`, or the GitHub CLI (`gh auth token`). Tokens are
//! only read and only sent to api.github.com.
//!
//! If that endpoint refuses the token (e.g. a fine-grained personal access token),
//! the official billing API is used instead; it reports requests used this month.

use std::path::PathBuf;

use chrono::{DateTime, Datelike, NaiveDate, TimeZone, Utc};
use serde_json::Value;

use crate::util::{next_month_start, num, run_hidden, send_json, timestamp};
use crate::{Account, Meter, Unit, Usage, UsageError};

const QUOTA_URL: &str = "https://api.github.com/copilot_internal/user";
const USER_URL: &str = "https://api.github.com/user";

/// Where the Copilot plugins store their GitHub sign-in.
pub fn config_dir(over: Option<&str>) -> Option<PathBuf> {
    if let Some(p) = over.map(str::trim).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    if cfg!(windows) {
        dirs::data_local_dir().map(|d| d.join("github-copilot"))
    } else {
        dirs::config_dir().map(|d| d.join("github-copilot"))
    }
}

/// Reads the first github.com OAuth token from `apps.json` or `hosts.json`.
fn plugin_token(over: Option<&str>) -> Option<String> {
    let dir = config_dir(over)?;
    ["apps.json", "hosts.json"].iter().find_map(|f| {
        let raw = std::fs::read_to_string(dir.join(f)).ok()?;
        parse_plugin_token(&raw)
    })
}

pub(crate) fn parse_plugin_token(raw: &str) -> Option<String> {
    let v: Value = serde_json::from_str(raw).ok()?;
    v.as_object()?
        .iter()
        .filter(|(k, _)| k.starts_with("github.com"))
        .find_map(|(_, e)| e["oauth_token"].as_str().filter(|t| !t.is_empty()).map(str::to_owned))
}

async fn gh_cli_token() -> Option<String> {
    let out = run_hidden("gh", &["auth", "token", "--hostname", "github.com"]).await?;
    let t = out.trim();
    (!t.is_empty() && t.chars().all(|c| c.is_ascii_graphic())).then(|| t.to_owned())
}

pub fn detected() -> bool {
    plugin_token(None).is_some()
}

pub async fn fetch(client: &reqwest::Client, account: &Account, pasted: Option<&str>) -> Result<Usage, UsageError> {
    let token = match pasted {
        Some(t) => t.to_owned(),
        None => match plugin_token(account.data_path.as_deref()) {
            Some(t) => t,
            None => gh_cli_token().await.ok_or_else(|| {
                UsageError::Setup(
                    "No GitHub sign-in found. Paste a GitHub token, or sign in with the GitHub CLI (`gh auth login`)."
                        .into(),
                )
            })?,
        },
    };

    let quota = send_json(
        client.get(QUOTA_URL).header("authorization", format!("token {token}")).header("accept", "application/json"),
    )
    .await;
    match quota {
        Ok(v) => parse_quota(&v).ok_or_else(|| UsageError::Parse("no Copilot quota in the response".into())),
        // Tokens without access to the quota endpoint can still read the billing report.
        Err(UsageError::Unauthorized | UsageError::Forbidden(_) | UsageError::Http { status: 404, .. }) => {
            fetch_billing(client, &token, account.monthly_budget).await
        }
        Err(e) => Err(e),
    }
}

/// Parses the quota endpoint. Handles both `quota_snapshots` and the older
/// `limited_user_quotas` / `monthly_quotas` shape used by the Free plan.
pub(crate) fn parse_quota(v: &Value) -> Option<Usage> {
    let resets = reset_time(v);
    let mut meters = Vec::new();
    let mut unlimited = Vec::new();

    if let Some(snaps) = v["quota_snapshots"].as_object() {
        let mut keys: Vec<&String> = snaps.keys().collect();
        let rank = |k: &str| match k {
            "premium_interactions" => 0,
            "chat" => 1,
            "completions" => 2,
            _ => 3,
        };
        keys.sort_by_key(|k| (rank(k), k.to_string()));
        for k in keys {
            let s = &snaps[k];
            let label = quota_label(k);
            if s["unlimited"].as_bool() == Some(true) {
                unlimited.push(label);
                continue;
            }
            let Some(limit) = num(&s["entitlement"]).filter(|e| *e > 0.0) else { continue };
            let remaining = num(&s["remaining"])
                .or_else(|| num(&s["quota_remaining"]))
                .or_else(|| num(&s["percent_remaining"]).map(|p| limit * p / 100.0))
                .unwrap_or(limit);
            let mut m = Meter::spend(label, Unit::Count, (limit - remaining).max(0.0), Some(limit)).with_reset(resets);
            m.remaining = Some(remaining.max(0.0));
            if let Some(over) = num(&s["overage_count"]).filter(|o| *o > 0.0) {
                m = m.with_detail(format!("{over:.0} extra requests billed this month"));
            }
            meters.push(m);
        }
    } else if let (Some(left), Some(total)) = (v["limited_user_quotas"].as_object(), v["monthly_quotas"].as_object()) {
        for (k, total) in total {
            let (Some(limit), Some(rem)) = (num(total), left.get(k).and_then(num)) else { continue };
            if limit <= 0.0 {
                continue;
            }
            let mut m = Meter::spend(quota_label(k), Unit::Count, (limit - rem).max(0.0), Some(limit)).with_reset(resets);
            m.remaining = Some(rem.max(0.0));
            meters.push(m);
        }
    }

    if meters.is_empty() && unlimited.is_empty() {
        return None;
    }
    let mut notes = Vec::new();
    if !unlimited.is_empty() {
        notes.push(format!("Unlimited: {}.", unlimited.join(", ").to_lowercase()));
    }
    Some(Usage { plan: plan_name(v), meters, notes })
}

fn quota_label(key: &str) -> String {
    match key {
        "premium_interactions" => "Premium requests".into(),
        "chat" => "Chat messages".into(),
        "completions" => "Code completions".into(),
        other => {
            let s = other.replace('_', " ");
            let mut c = s.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
        }
    }
}

fn plan_name(v: &Value) -> Option<String> {
    let p = v["copilot_plan"].as_str().or(v["access_type_sku"].as_str())?.to_ascii_lowercase();
    Some(match p.as_str() {
        "free" | "free_limited_copilot" => "Free".into(),
        "individual" | "pro" => "Pro".into(),
        "individual_pro" | "pro_plus" | "pro+" => "Pro+".into(),
        "business" => "Business".into(),
        "enterprise" => "Enterprise".into(),
        other => {
            let s = other.replace('_', " ");
            let mut c = s.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
        }
    })
}

fn reset_time(v: &Value) -> Option<DateTime<Utc>> {
    for k in ["quota_reset_date_utc", "quota_reset_date", "limited_user_reset_date"] {
        if let Some(t) = timestamp(&v[k]) {
            return Some(t);
        }
        if let Some(d) = v[k].as_str().and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()) {
            return d.and_hms_opt(0, 0, 0).map(|dt| Utc.from_utc_datetime(&dt));
        }
    }
    None
}

async fn fetch_billing(client: &reqwest::Client, token: &str, allowance: Option<f64>) -> Result<Usage, UsageError> {
    let gh = |url: &str| {
        client
            .get(url)
            .bearer_auth(token)
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28")
    };
    let user = send_json(gh(USER_URL)).await?;
    let login = user["login"]
        .as_str()
        .filter(|l| l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .ok_or_else(|| UsageError::Parse("could not read your GitHub username".into()))?;
    let now = Utc::now();
    let url = format!(
        "https://api.github.com/users/{login}/settings/billing/premium_request/usage?year={}&month={}",
        now.year(),
        now.month()
    );
    let report = send_json(gh(&url)).await.map_err(|e| match e {
        UsageError::Forbidden(_) | UsageError::Unauthorized | UsageError::Http { status: 404, .. } => UsageError::Setup(
            "GitHub refused this token. Use a token from `gh auth token`, or a fine-grained token with the \"Plan\" (read) account permission."
                .into(),
        ),
        other => other,
    })?;
    Ok(billing_usage(&report, allowance, now))
}

pub(crate) fn billing_usage(report: &Value, allowance: Option<f64>, now: DateTime<Utc>) -> Usage {
    let used: f64 = report["usageItems"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|i| {
            let sku = i["sku"].as_str().unwrap_or_default().to_ascii_lowercase();
            let product = i["product"].as_str().unwrap_or_default().to_ascii_lowercase();
            sku.contains("premium") || (sku.is_empty() && product == "copilot")
        })
        .filter_map(|i| num(&i["grossQuantity"]))
        .sum();
    let meter = Meter::spend("Premium requests", Unit::Count, used, allowance).with_reset(Some(next_month_start(now)));
    let mut notes = vec!["From GitHub's billing report, which can lag by a few hours.".to_owned()];
    if allowance.is_none() {
        notes.push("Set your monthly allowance (e.g. 300 for Pro) on this account to see how many are left.".into());
    }
    Usage { plan: None, meters: vec![meter], notes }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_quota_snapshots() {
        let v = json!({
            "copilot_plan": "individual",
            "quota_reset_date": "2026-11-01",
            "quota_snapshots": {
                "chat": {"entitlement": 0, "unlimited": true, "percent_remaining": 100},
                "completions": {"entitlement": 0, "unlimited": true},
                "premium_interactions": {"entitlement": 300, "remaining": 71, "quota_remaining": 71,
                    "percent_remaining": 23.7, "overage_count": 0, "unlimited": false}
            }
        });
        let u = parse_quota(&v).unwrap();
        assert_eq!(u.plan.as_deref(), Some("Pro"));
        assert_eq!(u.meters.len(), 1);
        let m = &u.meters[0];
        assert_eq!((m.used, m.limit, m.remaining), (Some(229.0), Some(300.0), Some(71.0)));
        assert_eq!(m.unit, Unit::Count);
        assert_eq!(m.resets_at.unwrap().to_rfc3339(), "2026-11-01T00:00:00+00:00");
        assert_eq!(u.notes, ["Unlimited: chat messages, code completions."]);
    }

    #[test]
    fn parses_free_plan_shape() {
        let v = json!({
            "access_type_sku": "free_limited_copilot",
            "limited_user_reset_date": "2026-11-05",
            "limited_user_quotas": {"chat": 40, "completions": 1500},
            "monthly_quotas": {"chat": 50, "completions": 2000}
        });
        let u = parse_quota(&v).unwrap();
        assert_eq!(u.plan.as_deref(), Some("Free"));
        let chat = u.meters.iter().find(|m| m.label == "Chat messages").unwrap();
        assert_eq!(chat.remaining, Some(40.0));
        assert_eq!(chat.percent_used, Some(20.0));
    }

    #[test]
    fn sums_billing_report() {
        let now = Utc.with_ymd_and_hms(2026, 10, 13, 0, 0, 0).unwrap();
        let r = json!({"usageItems": [
            {"product": "Copilot", "sku": "Copilot Premium Request", "model": "Claude Sonnet", "grossQuantity": 120},
            {"product": "Copilot", "sku": "Copilot Premium Request", "model": "GPT-5", "grossQuantity": 30.5},
            {"product": "Actions", "sku": "Actions Linux", "grossQuantity": 999}
        ]});
        let u = billing_usage(&r, Some(300.0), now);
        assert_eq!(u.meters[0].used, Some(150.5));
        assert_eq!(u.meters[0].remaining, Some(149.5));
        assert_eq!(billing_usage(&r, None, now).notes.len(), 2);
    }

    #[test]
    fn reads_plugin_token() {
        let raw = r#"{"github.com:Iv1.b507a08c87ecfe98":{"user":"me","oauth_token":"ghu_abc","githubAppId":"Iv1.b507a08c87ecfe98"}}"#;
        assert_eq!(parse_plugin_token(raw).as_deref(), Some("ghu_abc"));
        assert_eq!(parse_plugin_token(r#"{"ghe.example.com":{"oauth_token":"x"}}"#), None);
    }
}
