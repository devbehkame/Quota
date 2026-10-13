//! Anthropic API spend for the current month, from the Admin API cost report.

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::util::{month_start, num, send_json, timestamp};
use crate::{Account, Usage, UsageError};

const COST_URL: &str = "https://api.anthropic.com/v1/organizations/cost_report";
const MAX_PAGES: usize = 6;

pub async fn fetch(client: &reqwest::Client, account: &Account, key: &str) -> Result<Usage, UsageError> {
    if !key.starts_with("sk-ant-admin") {
        return Err(UsageError::Setup(
            "Spend reports need an Admin API key (starts with sk-ant-admin). Create one in the Claude Console under Settings → Admin keys."
                .into(),
        ));
    }
    let now = Utc::now();
    let start = month_start(now).format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let mut pages = Vec::new();
    let mut page: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut q = vec![("starting_at", start.clone()), ("bucket_width", "1d".into()), ("limit", "31".into())];
        if let Some(p) = &page {
            q.push(("page", p.clone()));
        }
        let body = send_json(
            client
                .get(COST_URL)
                .query(&q)
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01"),
        )
        .await?;
        page = body["next_page"].as_str().map(str::to_owned).filter(|_| body["has_more"].as_bool() == Some(true));
        pages.push(body);
        if page.is_none() {
            break;
        }
    }
    let (month, today) = sum_costs(&pages, now);
    Ok(super::openai_api::spend_usage(month, today, account.monthly_budget, now, "Cost reports can lag by a few minutes."))
}

/// Returns (month total, today's total) in USD. Amounts are decimal strings in cents.
pub(crate) fn sum_costs(pages: &[Value], now: DateTime<Utc>) -> (f64, f64) {
    let today = now.date_naive();
    let mut month = 0.0;
    let mut day = 0.0;
    for bucket in pages.iter().filter_map(|p| p["data"].as_array()).flatten() {
        let is_today = timestamp(&bucket["starting_at"]).is_some_and(|s| s.date_naive() == today);
        for r in bucket["results"].as_array().into_iter().flatten() {
            let usd = num(&r["amount"]).unwrap_or(0.0) / 100.0;
            month += usd;
            if is_today {
                day += usd;
            }
        }
    }
    (month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    #[test]
    fn sums_cents_into_dollars() {
        let now = Utc.with_ymd_and_hms(2026, 10, 2, 12, 0, 0).unwrap();
        let pages = [json!({
            "data": [
                {"starting_at": "2026-10-01T00:00:00Z", "ending_at": "2026-10-02T00:00:00Z",
                 "results": [{"amount": "1234.5", "currency": "USD"}, {"amount": "100", "currency": "USD"}]},
                {"starting_at": "2026-10-02T00:00:00Z", "ending_at": "2026-10-03T00:00:00Z",
                 "results": [{"amount": "250", "currency": "USD"}]}
            ],
            "has_more": false, "next_page": null
        })];
        let (m, d) = sum_costs(&pages, now);
        assert!((m - 15.845).abs() < 1e-9);
        assert!((d - 2.5).abs() < 1e-9);
    }
}
