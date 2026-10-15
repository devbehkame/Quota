//! OpenAI API spend for the current month, from the organization Costs API.

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::util::{fmt_money, month_start, next_month_start, num, send_json, timestamp};
use crate::{Account, Meter, Unit, Usage, UsageError};

const COSTS_URL: &str = "https://api.openai.com/v1/organization/costs";
const MAX_PAGES: usize = 6;

pub async fn fetch(client: &reqwest::Client, account: &Account, key: &str) -> Result<Usage, UsageError> {
    let now = Utc::now();
    let start = month_start(now).timestamp().to_string();
    let mut pages = Vec::new();
    let mut page: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut q = vec![("start_time", start.clone()), ("bucket_width", "1d".into()), ("limit", "31".into())];
        if let Some(p) = &page {
            q.push(("page", p.clone()));
        }
        let body = send_json(client.get(COSTS_URL).query(&q).bearer_auth(key)).await.map_err(|e| match e {
            UsageError::Forbidden(_) | UsageError::Unauthorized => UsageError::Setup(
                "OpenAI refused the key. Spend data needs an Admin key (sk-admin-…) from platform.openai.com → Settings → Admin keys."
                    .into(),
            ),
            other => other,
        })?;
        page = body["next_page"].as_str().map(str::to_owned).filter(|_| body["has_more"].as_bool() == Some(true));
        pages.push(body);
        if page.is_none() {
            break;
        }
    }
    let (month, today) = sum_costs(&pages, now);
    Ok(spend_usage(month, today, account.monthly_budget, now, "Cost data can lag by a few minutes."))
}

/// Returns (month total, today's total) in USD. Amounts are `{value, currency}` in dollars.
pub(crate) fn sum_costs(pages: &[Value], now: DateTime<Utc>) -> (f64, f64) {
    let today = now.date_naive();
    let mut month = 0.0;
    let mut day = 0.0;
    for bucket in pages.iter().filter_map(|p| p["data"].as_array()).flatten() {
        let is_today = timestamp(&bucket["start_time"]).is_some_and(|s| s.date_naive() == today);
        for r in bucket["results"].as_array().into_iter().flatten() {
            let usd = num(&r["amount"]["value"]).unwrap_or(0.0);
            month += usd;
            if is_today {
                day += usd;
            }
        }
    }
    (month, day)
}

/// Shared "spend this month vs. budget" presentation for API providers.
pub(crate) fn spend_usage(month: f64, today: f64, budget: Option<f64>, now: DateTime<Utc>, note: &str) -> Usage {
    let meter = Meter::spend("This month", Unit::Usd, month, budget)
        .with_reset(Some(next_month_start(now)))
        .with_detail(format!("Today {}", fmt_money("$", today)));
    let mut notes = vec![note.to_owned()];
    if budget.is_none() {
        notes.push("Set a monthly budget on this account to see how much is left.".into());
    }
    Usage { plan: Some("API".into()), meters: vec![meter], notes, signed_in_as: None, resets: None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    #[test]
    fn sums_dollar_amounts() {
        let now = Utc.with_ymd_and_hms(2026, 10, 2, 12, 0, 0).unwrap();
        let d1 = Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap().timestamp();
        let d2 = Utc.with_ymd_and_hms(2026, 10, 2, 0, 0, 0).unwrap().timestamp();
        let pages = [json!({
            "object": "page",
            "data": [
                {"object": "bucket", "start_time": d1, "end_time": d2,
                 "results": [{"object": "organization.costs.result", "amount": {"value": 1.25, "currency": "usd"}}]},
                {"object": "bucket", "start_time": d2, "end_time": d2 + 86400,
                 "results": [{"amount": {"value": "0.75", "currency": "usd"}}]}
            ],
            "has_more": false
        })];
        assert_eq!(sum_costs(&pages, now), (2.0, 0.75));
    }

    #[test]
    fn budget_gives_remaining() {
        let now = Utc.with_ymd_and_hms(2026, 10, 2, 12, 0, 0).unwrap();
        let u = spend_usage(30.0, 1.0, Some(120.0), now, "n");
        assert_eq!(u.meters[0].remaining, Some(90.0));
        assert_eq!(u.meters[0].percent_used, Some(25.0));
        assert_eq!(u.notes.len(), 1);
        let u = spend_usage(30.0, 1.0, None, now, "n");
        assert_eq!(u.meters[0].remaining, None);
        assert_eq!(u.notes.len(), 2);
    }
}
