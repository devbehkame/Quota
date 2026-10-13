use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Every account type the app knows how to read usage for.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// Claude Pro / Max subscription limits, read via the Claude Code sign-in.
    ClaudePlan,
    /// ChatGPT Plus / Pro limits, read from local Codex CLI session logs.
    CodexPlan,
    /// Anthropic API spend, via an Admin API key.
    AnthropicApi,
    /// OpenAI API spend, via an Admin API key.
    OpenaiApi,
    /// OpenRouter credit balance.
    Openrouter,
    /// DeepSeek prepaid balance.
    Deepseek,
}

impl ProviderKind {
    /// Whether an API key must be stored for this provider.
    pub fn requires_secret(self) -> bool {
        !matches!(self, ProviderKind::ClaudePlan | ProviderKind::CodexPlan)
    }

    /// Whether a user-defined monthly budget makes sense for this provider.
    pub fn supports_budget(self) -> bool {
        matches!(self, ProviderKind::AnthropicApi | ProviderKind::OpenaiApi)
    }
}

/// A configured account. Contains no secrets; API keys live in the OS credential store.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub kind: ProviderKind,
    pub label: String,
    /// Monthly spend budget in USD (API providers only).
    #[serde(default)]
    pub monthly_budget: Option<f64>,
    /// Optional override for where local data lives (Claude / Codex config folder).
    #[serde(default)]
    pub data_path: Option<String>,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    Percent,
    Usd,
    Cny,
}

/// One gauge on an account card, e.g. "Session (5-hour)" or "This month".
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Meter {
    pub label: String,
    pub unit: Unit,
    pub used: Option<f64>,
    pub limit: Option<f64>,
    pub remaining: Option<f64>,
    /// 0–100, how much of the limit is consumed. `None` when there is no limit.
    pub percent_used: Option<f64>,
    pub resets_at: Option<DateTime<Utc>>,
    pub detail: Option<String>,
}

impl Meter {
    /// A percentage-based window (subscription limits).
    pub fn percent(label: impl Into<String>, used_pct: f64, resets_at: Option<DateTime<Utc>>) -> Self {
        let used = used_pct.clamp(0.0, 100.0);
        Meter {
            label: label.into(),
            unit: Unit::Percent,
            used: Some(used),
            limit: Some(100.0),
            remaining: Some(100.0 - used),
            percent_used: Some(used),
            resets_at,
            detail: None,
        }
    }

    /// Money spent against an optional limit.
    pub fn spend(label: impl Into<String>, unit: Unit, used: f64, limit: Option<f64>) -> Self {
        let limit = limit.filter(|l| l.is_finite() && *l > 0.0);
        Meter {
            label: label.into(),
            unit,
            used: Some(used),
            limit,
            remaining: limit.map(|l| (l - used).max(0.0)),
            percent_used: limit.map(|l| (used / l * 100.0).clamp(0.0, 100.0)),
            resets_at: None,
            detail: None,
        }
    }

    /// A balance with no known total (prepaid wallets).
    pub fn balance(label: impl Into<String>, unit: Unit, remaining: f64) -> Self {
        Meter {
            label: label.into(),
            unit,
            used: None,
            limit: None,
            remaining: Some(remaining),
            percent_used: None,
            resets_at: None,
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn with_reset(mut self, resets_at: Option<DateTime<Utc>>) -> Self {
        self.resets_at = resets_at;
        self
    }
}

#[derive(Serialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub plan: Option<String>,
    pub meters: Vec<Meter>,
    pub notes: Vec<String>,
}

/// The result of one refresh, as sent to the UI.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub account_id: String,
    pub fetched_at: DateTime<Utc>,
    pub usage: Option<Usage>,
    pub error: Option<String>,
}

impl Snapshot {
    pub fn from_result(account_id: &str, result: Result<Usage, crate::UsageError>) -> Self {
        let (usage, error) = match result {
            Ok(u) => (Some(u), None),
            Err(e) => (None, Some(e.to_string())),
        };
        Snapshot { account_id: account_id.to_string(), fetched_at: Utc::now(), usage, error }
    }
}
