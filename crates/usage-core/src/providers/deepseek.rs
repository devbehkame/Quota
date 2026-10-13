//! DeepSeek prepaid balance.

use serde_json::Value;

use crate::util::{fmt_money, num, send_json};
use crate::{Meter, Unit, Usage, UsageError};

const BALANCE_URL: &str = "https://api.deepseek.com/user/balance";

pub async fn fetch(client: &reqwest::Client, key: &str) -> Result<Usage, UsageError> {
    let v = send_json(client.get(BALANCE_URL).bearer_auth(key).header("accept", "application/json")).await?;
    parse(&v)
}

pub(crate) fn parse(v: &Value) -> Result<Usage, UsageError> {
    let infos = v["balance_infos"].as_array().ok_or_else(|| UsageError::Parse("missing balance_infos".into()))?;
    let meters: Vec<Meter> = infos
        .iter()
        .filter_map(|b| {
            let currency = b["currency"].as_str().unwrap_or("USD");
            let (unit, sym) = if currency.eq_ignore_ascii_case("CNY") { (Unit::Cny, "¥") } else { (Unit::Usd, "$") };
            let total = num(&b["total_balance"])?;
            let granted = num(&b["granted_balance"]).unwrap_or(0.0);
            let topped = num(&b["topped_up_balance"]).unwrap_or(0.0);
            Some(
                Meter::balance(format!("Balance ({})", currency.to_uppercase()), unit, total)
                    .with_detail(format!("Topped up {} · Granted {}", fmt_money(sym, topped), fmt_money(sym, granted))),
            )
        })
        .collect();
    let mut notes = vec![];
    if v["is_available"].as_bool() == Some(false) {
        notes.push("Balance is too low for API calls. Top up to keep using DeepSeek.".into());
    }
    Ok(Usage { plan: Some("Prepaid".into()), meters, notes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_balances() {
        let u = parse(&json!({
            "is_available": true,
            "balance_infos": [{"currency": "CNY", "total_balance": "110.00", "granted_balance": "10.00", "topped_up_balance": "100.00"}]
        }))
        .unwrap();
        assert_eq!(u.meters[0].unit, Unit::Cny);
        assert_eq!(u.meters[0].remaining, Some(110.0));
        assert!(u.notes.is_empty());
    }
}
