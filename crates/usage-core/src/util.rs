use chrono::{DateTime, Datelike, NaiveDate, TimeZone, Utc};
use reqwest::{RequestBuilder, StatusCode};
use serde_json::Value;

use crate::UsageError;

/// Sends a request and returns the JSON body, mapping HTTP failures to friendly errors.
pub async fn send_json(req: RequestBuilder) -> Result<Value, UsageError> {
    let resp = req.send().await?;
    let status = resp.status();
    if status.is_success() {
        return resp.json::<Value>().await.map_err(UsageError::from);
    }
    let body = resp.text().await.unwrap_or_default();
    let message = error_message(&body);
    Err(match status {
        StatusCode::UNAUTHORIZED => UsageError::Unauthorized,
        StatusCode::FORBIDDEN => UsageError::Forbidden(message),
        StatusCode::TOO_MANY_REQUESTS => UsageError::RateLimited,
        s => UsageError::Http { status: s.as_u16(), message },
    })
}

/// Pulls a human-readable message out of a provider error body.
pub fn error_message(body: &str) -> String {
    let from_json = serde_json::from_str::<Value>(body).ok().and_then(|v| {
        let candidates = [&v["error"]["message"], &v["error"], &v["message"], &v["detail"]];
        candidates.into_iter().find_map(|c| c.as_str().map(str::to_owned))
    });
    let msg = from_json.unwrap_or_else(|| body.to_owned());
    let msg: String = msg.chars().filter(|c| !c.is_control()).take(200).collect();
    if msg.trim().is_empty() {
        "no details given".into()
    } else {
        msg.trim().to_owned()
    }
}

/// Reads a number that providers sometimes encode as a string.
pub fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
    .filter(|f: &f64| f.is_finite())
}

/// Parses an RFC 3339 string or a unix timestamp (seconds or milliseconds).
pub fn timestamp(v: &Value) -> Option<DateTime<Utc>> {
    match v {
        Value::String(s) => DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc)),
        Value::Number(_) => {
            let n = num(v)?;
            let secs = if n > 1e12 { n / 1000.0 } else { n };
            Utc.timestamp_opt(secs as i64, 0).single()
        }
        _ => None,
    }
}

pub fn month_start(now: DateTime<Utc>) -> DateTime<Utc> {
    let d = NaiveDate::from_ymd_opt(now.year(), now.month(), 1).expect("valid date");
    Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).expect("valid time"))
}

pub fn next_month_start(now: DateTime<Utc>) -> DateTime<Utc> {
    let (y, m) = if now.month() == 12 { (now.year() + 1, 1) } else { (now.year(), now.month() + 1) };
    let d = NaiveDate::from_ymd_opt(y, m, 1).expect("valid date");
    Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).expect("valid time"))
}

pub fn fmt_money(symbol: &str, v: f64) -> String {
    format!("{symbol}{v:.2}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn month_bounds() {
        let now = Utc.with_ymd_and_hms(2026, 12, 17, 13, 5, 0).unwrap();
        assert_eq!(month_start(now).to_rfc3339(), "2026-12-01T00:00:00+00:00");
        assert_eq!(next_month_start(now).to_rfc3339(), "2027-01-01T00:00:00+00:00");
    }

    #[test]
    fn parses_numbers_and_times() {
        assert_eq!(num(&Value::from("12.5")), Some(12.5));
        assert_eq!(num(&Value::from(3)), Some(3.0));
        assert_eq!(num(&Value::Null), None);
        let a = timestamp(&Value::from(1_770_412_938_485_i64)).unwrap();
        let b = timestamp(&Value::from(1_770_412_938_i64)).unwrap();
        assert_eq!(a, b);
        assert!(timestamp(&Value::from("2026-02-06T22:00:00+00:00")).is_some());
    }

    #[test]
    fn extracts_error_messages() {
        assert_eq!(error_message(r#"{"error":{"message":"bad key"}}"#), "bad key");
        assert_eq!(error_message(""), "no details given");
        assert_eq!(error_message("plain\ntext"), "plaintext");
    }
}
