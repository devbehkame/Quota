//! Cursor plan usage (included usage this billing cycle, plus on-demand spend).
//!
//! Sign-in comes from the Cursor CLI's own login file (`cursor-agent login`, the
//! same kind of tool credential file Claude Code and Codex keep), or from a
//! cursor.com session cookie the user pastes. The Cursor editor's internal
//! storage is not read. The token is only read and only sent to cursor.com.

use std::path::PathBuf;

use chrono::{TimeZone, Utc};
use serde_json::Value;

use crate::util::{jwt_claims, num, send_json, timestamp};
use crate::{Account, Meter, Unit, Usage, UsageError};

const SUMMARY_URL: &str = "https://cursor.com/api/usage-summary";
const COOKIE: &str = "WorkosCursorSessionToken";

/// Where `cursor-agent login` may keep its session on this OS.
fn agent_auth_paths(over: Option<&str>) -> Vec<PathBuf> {
    if let Some(p) = over.map(str::trim).filter(|p| !p.is_empty()) {
        return vec![PathBuf::from(p).join("auth.json")];
    }
    let mut out = Vec::new();
    if let Some(d) = dirs::config_dir() {
        out.push(d.join("cursor").join("auth.json"));
    }
    if let Some(h) = dirs::home_dir() {
        out.push(h.join(".config").join("cursor").join("auth.json"));
        out.push(h.join(".cursor").join("auth.json"));
    }
    out
}

fn agent_token(over: Option<&str>) -> Option<String> {
    agent_auth_paths(over).into_iter().find_map(|p| {
        let v: Value = serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()?;
        v["accessToken"].as_str().map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned)
    })
}

/// Cursor is installed (editor or CLI) on this PC.
pub fn detected() -> bool {
    let editor = dirs::data_local_dir().map(|d| d.join("Programs").join("cursor")).is_some_and(|p| p.is_dir());
    let home = dirs::home_dir().map(|h| h.join(".cursor")).is_some_and(|p| p.is_dir());
    editor || home || agent_token(None).is_some()
}

/// Installed, but there is no CLI login to use yet (the user can paste a cookie instead).
pub fn needs_sign_in() -> bool {
    agent_token(None).is_none()
}

pub async fn fetch(client: &reqwest::Client, account: &Account, pasted: Option<&str>) -> Result<Usage, UsageError> {
    let cookie = match pasted {
        Some(p) => cookie_from_paste(p)?,
        None => {
            let token = agent_token(account.data_path.as_deref()).ok_or_else(|| {
                UsageError::Setup(
                    "No Cursor sign-in Quota can use yet. Run `cursor-agent login` once (Cursor CLI), or edit this account and paste your cursor.com session cookie."
                        .into(),
                )
            })?;
            cookie_from_jwt(&token)?
        }
    };
    let v = send_json(
        client
            .get(SUMMARY_URL)
            .header("cookie", cookie)
            .header("accept", "application/json")
            .header("origin", "https://cursor.com")
            .header("referer", "https://cursor.com/dashboard"),
    )
    .await
    .map_err(|e| match e {
        UsageError::Unauthorized | UsageError::Forbidden(_) => UsageError::Setup(
            "Cursor didn't accept the sign-in (it may have expired). Run `cursor-agent login` again, or paste a fresh cursor.com session cookie."
                .into(),
        ),
        other => other,
    })?;
    parse_summary(&v).ok_or_else(|| UsageError::Parse("no usage in Cursor's summary".into()))
}

/// Accepts the cookie value (`user_…%3A%3Aey…`), `WorkosCursorSessionToken=…`, or a bare access token.
pub(crate) fn cookie_from_paste(p: &str) -> Result<String, UsageError> {
    let p = p.trim();
    let value = p.strip_prefix(&format!("{COOKIE}=")).unwrap_or(p);
    let value = value.split(';').next().unwrap_or_default().trim();
    if value.contains("%3A%3A") || value.contains("::") {
        return Ok(format!("{COOKIE}={}", value.replace("::", "%3A%3A")));
    }
    if value.split('.').count() == 3 {
        return cookie_from_jwt(value);
    }
    Err(UsageError::Setup(
        "That doesn't look like a Cursor session. Paste the value of the WorkosCursorSessionToken cookie from cursor.com."
            .into(),
    ))
}

/// Cursor's web session cookie is `<user id>::<access token>`; the user id is the
/// token's `sub` claim after the last `|`.
pub(crate) fn cookie_from_jwt(token: &str) -> Result<String, UsageError> {
    let claims = jwt_claims(token).ok_or_else(|| UsageError::Setup("The Cursor sign-in token is unreadable.".into()))?;
    if let Some(exp) = num(&claims["exp"]) {
        if Utc.timestamp_opt(exp as i64, 0).single().is_some_and(|t| t < Utc::now()) {
            return Err(UsageError::Setup(
                "The Cursor sign-in has expired. Run `cursor-agent login` again, then refresh.".into(),
            ));
        }
    }
    let sub = claims["sub"].as_str().unwrap_or_default();
    let user = sub.rsplit('|').next().unwrap_or_default();
    if user.is_empty() || !user.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)) {
        return Err(UsageError::Setup("The Cursor sign-in token has no user id.".into()));
    }
    Ok(format!("{COOKIE}={user}%3A%3A{token}"))
}

/// `GET /api/usage-summary`: cents for amounts, percentages already in 0–100.
pub(crate) fn parse_summary(v: &Value) -> Option<Usage> {
    let reset = timestamp(&v["billingCycleEnd"]);
    let iu = &v["individualUsage"];
    let plan = &iu["plan"];
    let mut meters = Vec::new();

    let cents = |x: &Value| num(x).map(|c| c / 100.0);
    let (used, limit) = (cents(&plan["used"]), cents(&plan["limit"]).filter(|l| *l > 0.0));
    let total_pct = num(&plan["totalPercentUsed"]).or_else(|| Some(used? / limit? * 100.0));
    if let Some(pct) = total_pct {
        let mut m = Meter::percent("Included usage", pct, reset);
        if let (Some(u), Some(l)) = (used, limit) {
            m = m.with_detail(format!("${u:.2} of ${l:.2} this billing cycle"));
        }
        meters.push(m);
    }
    for (key, label) in [("autoPercentUsed", "Auto model"), ("apiPercentUsed", "Named models")] {
        if let Some(p) = num(&plan[key]) {
            meters.push(Meter::percent(label, p, reset));
        }
    }
    // Team / enterprise personal cap when there is no plan block.
    if meters.is_empty() {
        let overall = &iu["overall"];
        if let (Some(u), Some(l)) = (cents(&overall["used"]), cents(&overall["limit"]).filter(|l| *l > 0.0)) {
            meters.push(Meter::spend("Usage this cycle", Unit::Usd, u, Some(l)).with_reset(reset));
        }
    }
    let od = &iu["onDemand"];
    if od["enabled"].as_bool() == Some(true) {
        if let Some(u) = cents(&od["used"]) {
            meters.push(Meter::spend("On-demand", Unit::Usd, u, cents(&od["limit"])).with_reset(reset));
        }
    }
    if meters.is_empty() && v["isUnlimited"].as_bool() != Some(true) {
        return None;
    }
    let mut notes = Vec::new();
    if v["isUnlimited"].as_bool() == Some(true) {
        notes.push("Unlimited plan.".into());
    }
    let plan_name = v["membershipType"].as_str().filter(|s| !s.is_empty()).map(|s| {
        let s = s.replace('_', " ");
        let mut c = s.chars();
        c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
    });
    Some(Usage { plan: plan_name, meters, notes, signed_in_as: None, resets: None, tokens: None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // {"sub":"auth0|user_ABC123","exp":4102444800}
    const JWT: &str = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJhdXRoMHx1c2VyX0FCQzEyMyIsImV4cCI6NDEwMjQ0NDgwMH0.sig";

    #[test]
    fn builds_cookie_from_token_and_paste() {
        assert_eq!(cookie_from_jwt(JWT).unwrap(), format!("{COOKIE}=user_ABC123%3A%3A{JWT}"));
        assert_eq!(cookie_from_paste(JWT).unwrap(), format!("{COOKIE}=user_ABC123%3A%3A{JWT}"));
        assert_eq!(cookie_from_paste("user_X%3A%3Aabc").unwrap(), format!("{COOKIE}=user_X%3A%3Aabc"));
        assert_eq!(cookie_from_paste(&format!("{COOKIE}=user_X::abc; other=1")).unwrap(), format!("{COOKIE}=user_X%3A%3Aabc"));
        assert!(cookie_from_paste("hello").is_err());
    }

    #[test]
    fn parses_usage_summary() {
        let v = json!({
            "billingCycleStart": "2026-10-01T00:00:00.000Z",
            "billingCycleEnd": "2026-11-01T00:00:00.000Z",
            "membershipType": "pro",
            "individualUsage": {
                "plan": {"enabled": true, "used": 1200, "limit": 2000, "remaining": 800,
                         "autoPercentUsed": 20.5, "apiPercentUsed": 70, "totalPercentUsed": 60},
                "onDemand": {"enabled": true, "used": 350, "limit": 5000}
            }
        });
        let u = parse_summary(&v).unwrap();
        assert_eq!(u.plan.as_deref(), Some("Pro"));
        let labels: Vec<_> = u.meters.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(labels, ["Included usage", "Auto model", "Named models", "On-demand"]);
        assert_eq!(u.meters[0].remaining, Some(40.0));
        assert_eq!(u.meters[0].detail.as_deref(), Some("$12.00 of $20.00 this billing cycle"));
        assert!(u.meters[0].resets_at.is_some());
        assert_eq!(u.meters[3].remaining, Some(46.5));
    }

    #[test]
    fn parses_team_overall() {
        let v = json!({"individualUsage": {"overall": {"used": 7384, "limit": 10000}}});
        let u = parse_summary(&v).unwrap();
        assert_eq!(u.meters[0].label, "Usage this cycle");
        assert_eq!(u.meters[0].remaining.map(|r| (r * 100.0).round() / 100.0), Some(26.16));
        assert!(parse_summary(&json!({})).is_none());
    }
}
