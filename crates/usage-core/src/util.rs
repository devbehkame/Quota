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

/// Runs a helper program without flashing a console window, with a short timeout.
/// Returns stdout on success.
pub async fn run_hidden(program: &str, args: &[&str]) -> Option<String> {
    let out = run_capture(program, args, std::time::Duration::from_secs(8)).await?;
    out.success.then_some(out.stdout)
}

pub struct Captured {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Like [`run_hidden`], with a custom timeout and both output streams.
pub async fn run_capture(program: &str, args: &[&str], timeout: std::time::Duration) -> Option<Captured> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = tokio::time::timeout(timeout, cmd.output()).await.ok()?.ok()?;
    Some(Captured {
        success: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

pub fn fmt_money(symbol: &str, v: f64) -> String {
    format!("{symbol}{v:.2}")
}

/// The claims of a JWT, decoded but not verified (only used to read our own sign-ins).
pub fn jwt_claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    serde_json::from_slice(&base64url(payload)?).ok()
}

fn base64url(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0;
    for c in s.bytes().filter(|c| *c != b'=') {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        } as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
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
