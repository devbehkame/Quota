//! Fetches "how much is left" for AI accounts.
//!
//! All network access and local file reads happen here, in Rust; the UI only
//! ever receives the resulting [`Snapshot`]s and never sees credentials.

mod error;
mod model;
pub mod providers;
mod util;

use std::time::Duration;

pub use error::UsageError;
pub use reqwest::Client as HttpClient;
pub use model::{Account, Meter, ProviderKind, Snapshot, Unit, Usage};

/// Builds the shared HTTPS client. TLS only, short timeouts, no redirects to other hosts.
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .https_only(true)
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("Quota/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("TLS backend available")
}

/// Fetches current usage for one account.
pub async fn fetch(
    client: &reqwest::Client,
    account: &Account,
    secret: Option<&str>,
) -> Result<Usage, UsageError> {
    let secret = secret.map(str::trim).filter(|s| !s.is_empty());
    let need = || {
        secret.ok_or_else(|| UsageError::Setup("No API key is saved for this account. Edit it to add one.".into()))
    };
    match account.kind {
        ProviderKind::ClaudePlan => providers::claude_plan::fetch(client, account, secret).await,
        ProviderKind::CodexPlan => providers::codex_plan::fetch(client, account).await,
        ProviderKind::AnthropicApi => providers::anthropic_api::fetch(client, account, need()?).await,
        ProviderKind::OpenaiApi => providers::openai_api::fetch(client, account, need()?).await,
        ProviderKind::Openrouter => providers::openrouter::fetch(client, need()?).await,
        ProviderKind::Deepseek => providers::deepseek::fetch(client, need()?).await,
        ProviderKind::Copilot => providers::copilot::fetch(client, account, secret).await,
        ProviderKind::Antigravity => providers::antigravity::fetch().await,
        ProviderKind::Cursor => providers::cursor::fetch(client, account, secret).await,
    }
}

/// Reports which local sign-ins exist so the UI can offer one-click setup.
#[derive(serde::Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LocalDetection {
    pub claude: bool,
    pub codex: bool,
    pub copilot: bool,
    /// Copilot is installed but still needs a one-time GitHub sign-in for Quota.
    pub copilot_needs_sign_in: bool,
    pub antigravity: bool,
    pub cursor: bool,
    /// Cursor is installed but has no CLI login (a pasted cookie works too).
    pub cursor_needs_sign_in: bool,
}

pub fn detect_local() -> LocalDetection {
    LocalDetection {
        claude: providers::claude_plan::credentials_path(None).is_some_and(|p| p.is_file()),
        codex: providers::codex_plan::auth_path(None).is_some_and(|p| p.is_file())
            || providers::codex_plan::sessions_dir(None).is_some_and(|p| p.is_dir()),
        copilot: providers::copilot::detected(),
        copilot_needs_sign_in: providers::copilot::needs_sign_in(),
        antigravity: providers::antigravity::detected(),
        cursor: providers::cursor::detected(),
        cursor_needs_sign_in: providers::cursor::needs_sign_in(),
    }
}
