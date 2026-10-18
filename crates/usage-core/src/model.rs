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
    /// GitHub Copilot premium requests and chat/completion quotas.
    Copilot,
    /// Google Antigravity per-model quotas, read from the running IDE.
    Antigravity,
    /// Cursor plan usage and on-demand spend.
    Cursor,
}

impl ProviderKind {
    /// Whether an API key must be stored for this provider.
    pub fn requires_secret(self) -> bool {
        !matches!(
            self,
            ProviderKind::ClaudePlan
                | ProviderKind::CodexPlan
                | ProviderKind::Copilot
                | ProviderKind::Antigravity
                | ProviderKind::Cursor
        )
    }

    /// Whether a user-defined monthly budget makes sense for this provider.
    pub fn supports_budget(self) -> bool {
        matches!(self, ProviderKind::AnthropicApi | ProviderKind::OpenaiApi | ProviderKind::Copilot)
    }

    /// Whether the account reads a local app's folder (which can be overridden).
    pub fn uses_local_folder(self) -> bool {
        matches!(self, ProviderKind::ClaudePlan | ProviderKind::CodexPlan | ProviderKind::Copilot | ProviderKind::Cursor)
    }
}

/// A configured account. Contains no secrets; API keys live in the OS credential store.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub kind: ProviderKind,
    pub label: String,
    /// Monthly spend budget in USD (API providers), or the monthly premium-request
    /// allowance for Copilot when only the billing API is available.
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
    /// A plain count, e.g. premium requests.
    Count,
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
    /// The signed-in account (email or username), when the sign-in says.
    pub signed_in_as: Option<String>,
    /// Banked limit resets (Claude and ChatGPT plans), when the service reports them.
    pub resets: Option<LimitResets>,
    /// Tokens used this week and this session, from the local Claude Code /
    /// Codex logs (filled in by the app after each refresh).
    pub tokens: Vec<TokenUsage>,
}

/// Which limit window a [`TokenUsage`] covers.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum Period {
    /// The 5-hour session limit.
    Session,
    #[default]
    Week,
}

/// Tokens counted from local logs since `since` (the start of the limit window).
#[derive(Serialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    pub period: Period,
    pub since: DateTime<Utc>,
    /// Input that wasn't served from the prompt cache.
    pub input: u64,
    pub output: u64,
    /// Input read back from the prompt cache.
    pub cache_read: u64,
    /// Input written to the prompt cache (Claude only).
    pub cache_write: u64,
    /// This PC's share of the window's limit, estimated by the app.
    pub share: Option<WeeklyShare>,
}

/// How much of a limit (weekly or session) this PC used, in percentage points.
#[derive(Serialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct WeeklyShare {
    /// The limit used so far, all devices together.
    pub used: f64,
    /// This PC's part of `used`.
    pub this_pc: f64,
    /// Other devices, the website and the phone apps.
    pub other: f64,
    /// Usage Quota couldn't place yet (from before it was watching, or a long
    /// gap between refreshes) and can't estimate until it has seen this PC work.
    pub unsplit: f64,
    /// Part of `this_pc` is estimated from the tokens logged while unwatched.
    pub estimated: bool,
    pub tracked_since: DateTime<Utc>,
}

/// One-off credits that restore a usage limit early, as Claude and ChatGPT grant them.
#[derive(Serialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct LimitResets {
    /// How many can be used now.
    pub available: u32,
    /// The usable ones, soonest expiry first. Empty when only the count is known.
    pub items: Vec<LimitReset>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LimitReset {
    /// What it restores, e.g. "Full reset (Weekly + 5 hr)", when the service says.
    pub label: Option<String>,
    pub count: u32,
    pub expires_at: Option<DateTime<Utc>>,
}

impl LimitResets {
    pub fn new(mut items: Vec<LimitReset>) -> Self {
        items.retain(|i| i.count > 0);
        items.sort_by_key(|i| (i.expires_at.is_none(), i.expires_at));
        LimitResets { available: items.iter().map(|i| i.count).sum(), items }
    }
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
