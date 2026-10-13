//! OpenRouter credits. Uses the account balance when the key is a management key,
//! otherwise falls back to the spend limit of the key itself.

use serde_json::Value;

use crate::util::{fmt_money, num, send_json};
use crate::{Meter, Unit, Usage, UsageError};

const CREDITS_URL: &str = "https://openrouter.ai/api/v1/credits";
const KEY_URL: &str = "https://openrouter.ai/api/v1/key";

pub async fn fetch(client: &reqwest::Client, key: &str) -> Result<Usage, UsageError> {
    match send_json(client.get(CREDITS_URL).bearer_auth(key)).await {
        Ok(v) => parse_credits(&v),
        Err(UsageError::Forbidden(_)) => {
            let v = send_json(client.get(KEY_URL).bearer_auth(key)).await?;
            parse_key(&v)
        }
        Err(e) => Err(e),
    }
}

pub(crate) fn parse_credits(v: &Value) -> Result<Usage, UsageError> {
    let d = &v["data"];
    let total = num(&d["total_credits"]).ok_or_else(|| UsageError::Parse("missing total_credits".into()))?;
    let used = num(&d["total_usage"]).unwrap_or(0.0);
    let meter = Meter::spend("Credits", Unit::Usd, used, Some(total))
        .with_detail(format!("{} purchased in total", fmt_money("$", total)));
    let meter = if total <= 0.0 { Meter::balance("Credits", Unit::Usd, 0.0) } else { meter };
    Ok(Usage { plan: Some("Credits".into()), meters: vec![meter], notes: vec![] })
}

pub(crate) fn parse_key(v: &Value) -> Result<Usage, UsageError> {
    let d = &v["data"];
    let used = num(&d["usage"]).ok_or_else(|| UsageError::Parse("missing key usage".into()))?;
    let limit = num(&d["limit"]);
    let mut meter = Meter::spend("Key spend", Unit::Usd, used, limit);
    if let Some(rem) = num(&d["limit_remaining"]) {
        meter.remaining = Some(rem.max(0.0));
    }
    if limit.is_none() {
        meter = meter.with_detail("No spend limit set on this key");
    }
    let plan = if d["is_free_tier"].as_bool() == Some(true) { "Free tier" } else { "Key" };
    Ok(Usage {
        plan: Some(plan.into()),
        meters: vec![meter],
        notes: vec!["Use a management key to see your whole account balance.".into()],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn credits_balance() {
        let u = parse_credits(&json!({"data": {"total_credits": 100.5, "total_usage": 25.75}})).unwrap();
        assert_eq!(u.meters[0].remaining, Some(74.75));
    }

    #[test]
    fn key_limits() {
        let u = parse_key(&json!({"data": {"usage": 3.0, "limit": 10.0, "limit_remaining": 7.0, "is_free_tier": false}})).unwrap();
        assert_eq!(u.meters[0].remaining, Some(7.0));
        let u = parse_key(&json!({"data": {"usage": 3.0, "limit": null}})).unwrap();
        assert_eq!(u.meters[0].remaining, None);
        assert!(parse_key(&json!({})).is_err());
    }
}
