//! Claude Pro / Max plan limits.
//!
//! Uses the OAuth sign-in that Claude Code stores in `~/.claude/.credentials.json`
//! and the same usage endpoint that Claude Code's `/usage` screen reads. The token is
//! only read, never refreshed or written back, so Claude Code's sign-in is never disturbed.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

use crate::util::{num, send_json, timestamp};
use crate::{Account, Meter, Usage, UsageError};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

pub fn credentials_path(over: Option<&str>) -> Option<PathBuf> {
    super::config_dir(over, "CLAUDE_CONFIG_DIR", ".claude").map(|d| d.join(".credentials.json"))
}

struct Credentials {
    token: String,
    expires_at: Option<DateTime<Utc>>,
    plan: Option<String>,
}

fn read_credentials(account: &Account) -> Result<Credentials, UsageError> {
    let path = credentials_path(account.data_path.as_deref())
        .ok_or_else(|| UsageError::Setup("Could not locate your home folder.".into()))?;
    let raw = std::fs::read_to_string(&path).map_err(|_| {
        UsageError::Setup(format!(
            "No Claude sign-in found at {}. Install Claude Code and run `claude` once to sign in with your Claude account.",
            path.display()
        ))
    })?;
    parse_credentials(&raw)
}

fn parse_credentials(raw: &str) -> Result<Credentials, UsageError> {
    let v: Value = serde_json::from_str(raw)
        .map_err(|_| UsageError::Setup("The Claude credentials file is not valid JSON.".into()))?;
    let o = &v["claudeAiOauth"];
    let token = o["accessToken"]
        .as_str()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| UsageError::Setup("The Claude credentials file has no sign-in token. Run `claude` and sign in.".into()))?
        .to_owned();
    Ok(Credentials {
        token,
        expires_at: timestamp(&o["expiresAt"]),
        plan: plan_name(o["subscriptionType"].as_str(), o["rateLimitTier"].as_str()),
    })
}

/// Claude Code keeps the signed-in account in `.claude.json`: in the config folder
/// when one is set, otherwise next to it in the home folder.
fn account_email(over: Option<&str>) -> Option<String> {
    let dir = super::config_dir(over, "CLAUDE_CONFIG_DIR", ".claude")?;
    let mut files = vec![dir.join(".claude.json")];
    files.extend(dir.parent().map(|p| p.join(".claude.json")));
    files.iter().find_map(|f| parse_account_email(&std::fs::read_to_string(f).ok()?))
}

/// The file also holds Claude Code's project history, so only `oauthAccount` is decoded.
fn parse_account_email(raw: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Global {
        #[serde(rename = "oauthAccount")]
        oauth_account: Option<OAuthAccount>,
    }
    #[derive(Deserialize)]
    struct OAuthAccount {
        #[serde(rename = "emailAddress")]
        email_address: Option<String>,
    }
    let g: Global = serde_json::from_str(raw).ok()?;
    g.oauth_account?.email_address.map(|e| e.trim().to_owned()).filter(|e| e.contains('@'))
}

fn plan_name(sub: Option<&str>, tier: Option<&str>) -> Option<String> {
    let tier = tier.unwrap_or_default().to_ascii_lowercase();
    for (needle, name) in [("max_20x", "Max 20x"), ("max_5x", "Max 5x")] {
        if tier.contains(needle) {
            return Some(name.into());
        }
    }
    sub.filter(|s| !s.is_empty()).map(|s| {
        let mut c = s.chars();
        c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
    })
}

pub async fn fetch(client: &reqwest::Client, account: &Account, manual_token: Option<&str>) -> Result<Usage, UsageError> {
    let (token, plan, email) = match manual_token {
        Some(t) => (t.to_owned(), None, None),
        None => {
            let c = read_credentials(account)?;
            if c.expires_at.is_some_and(|e| e < Utc::now()) {
                return Err(UsageError::Setup(
                    "Your Claude sign-in token has expired. Open Claude Code (run `claude`) once to renew it, then refresh."
                        .into(),
                ));
            }
            (c.token, c.plan, account_email(account.data_path.as_deref()))
        }
    };
    let body = send_json(
        client
            .get(USAGE_URL)
            .bearer_auth(&token)
            .header("anthropic-beta", "oauth-2025-04-20")
            .header("accept", "application/json"),
    )
    .await
    .map_err(|e| match e {
        UsageError::Unauthorized => UsageError::Setup(
            "Claude rejected the sign-in token. Open Claude Code (run `claude`) to sign in again, then refresh.".into(),
        ),
        other => other,
    })?;

    let meters = parse_usage(&body);
    if meters.is_empty() {
        return Err(UsageError::Parse("no usage windows in the response".into()));
    }
    Ok(Usage { plan, meters, notes: vec![], signed_in_as: email })
}

/// Understands both the newer `limits` array and the older flat `five_hour` / `seven_day` shape.
pub(crate) fn parse_usage(v: &Value) -> Vec<Meter> {
    let mut meters = Vec::new();

    if let Some(limits) = v["limits"].as_array().filter(|l| !l.is_empty()) {
        for l in limits {
            let Some(pct) = num(&l["percent"]).or_else(|| num(&l["utilization"])) else { continue };
            let label = match l["kind"].as_str().unwrap_or_default() {
                "session" => "Session (5-hour)".to_owned(),
                "weekly_all" => "Weekly · all models".to_owned(),
                "weekly_scoped" => {
                    let model = l["scope"]["model"]["display_name"].as_str().unwrap_or("scoped");
                    format!("Weekly · {model}")
                }
                other => humanize(other),
            };
            meters.push(Meter::percent(label, pct, timestamp(&l["resets_at"])));
        }
    } else if let Some(obj) = v.as_object() {
        let mut keys: Vec<&String> = obj.keys().filter(|k| *k == "five_hour" || k.starts_with("seven_day")).collect();
        keys.sort_by_key(|k| (k.as_str() != "five_hour", k.as_str() != "seven_day", k.to_string()));
        for k in keys {
            let w = &obj[k];
            let Some(pct) = num(&w["utilization"]) else { continue };
            let label = match k.as_str() {
                "five_hour" => "Session (5-hour)".to_owned(),
                "seven_day" => "Weekly · all models".to_owned(),
                other => format!("Weekly · {}", humanize(other.trim_start_matches("seven_day_"))),
            };
            meters.push(Meter::percent(label, pct, timestamp(&w["resets_at"])));
        }
    }

    let extra = &v["extra_usage"];
    if extra["is_enabled"].as_bool() == Some(true) {
        let pct = num(&extra["utilization"]).or_else(|| {
            let used = num(&extra["used_credits"])?;
            let limit = num(&extra["monthly_limit"]).filter(|l| *l > 0.0)?;
            Some(used / limit * 100.0)
        });
        if let Some(pct) = pct {
            meters.push(Meter::percent("Extra usage (monthly)", pct, None));
        }
    }
    meters
}

fn humanize(s: &str) -> String {
    let s = s.replace('_', " ");
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_flat_shape() {
        let v = json!({
            "seven_day_sonnet": {"utilization": 39.0, "resets_at": "2026-02-09T14:00:00+00:00"},
            "five_hour": {"utilization": 35.0, "resets_at": "2026-02-06T22:00:00+00:00"},
            "seven_day": {"utilization": 14.0, "resets_at": "2026-02-12T20:00:00+00:00"},
            "seven_day_opus": null,
            "extra_usage": {"is_enabled": true, "monthly_limit": 100000, "used_credits": 25000.0, "utilization": null}
        });
        let m = parse_usage(&v);
        let labels: Vec<_> = m.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(labels, ["Session (5-hour)", "Weekly · all models", "Weekly · Sonnet", "Extra usage (monthly)"]);
        assert_eq!(m[0].remaining, Some(65.0));
        assert!(m[0].resets_at.is_some());
        assert_eq!(m[3].percent_used, Some(25.0));
    }

    #[test]
    fn parses_limits_array() {
        let v = json!({
            "five_hour": null,
            "limits": [
                {"kind": "session", "percent": 12, "resets_at": "2026-10-13T18:00:00Z"},
                {"kind": "weekly_all", "percent": 50.5, "resets_at": "2026-10-17T18:00:00Z"},
                {"kind": "weekly_scoped", "percent": 80, "resets_at": null,
                 "scope": {"model": {"display_name": "Opus"}}}
            ]
        });
        let m = parse_usage(&v);
        assert_eq!(m.len(), 3);
        assert_eq!(m[2].label, "Weekly · Opus");
        assert_eq!(m[1].remaining, Some(49.5));
        assert_eq!(m[2].resets_at, None);
    }

    #[test]
    fn reads_credentials() {
        let c = parse_credentials(
            r#"{"claudeAiOauth":{"accessToken":"tok","expiresAt":1770412938485,"subscriptionType":"max","rateLimitTier":"default_claude_max_5x"}}"#,
        )
        .unwrap();
        assert_eq!(c.token, "tok");
        assert_eq!(c.plan.as_deref(), Some("Max 5x"));
        assert!(c.expires_at.is_some());
        assert_eq!(plan_name(Some("pro"), None).as_deref(), Some("Pro"));
        assert!(parse_credentials("{}").is_err());
    }

    #[test]
    fn reads_account_email() {
        let raw = r#"{"numStartups":3,"projects":{"C:/x":{"history":[]}},"oauthAccount":{"accountUuid":"u","emailAddress":"me@example.com","organizationName":"Org"}}"#;
        assert_eq!(parse_account_email(raw).as_deref(), Some("me@example.com"));
        assert_eq!(parse_account_email(r#"{"projects":{}}"#), None);
    }
}
