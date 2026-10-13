//! The only bridge between the UI and the system. Every input is validated here.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use usage_core::{Account, LocalDetection, ProviderKind, Snapshot};
use zeroize::Zeroizing;

use crate::config::{self, Config, Settings};
use crate::{refresh, secrets, tray, widget};

pub struct AppState {
    pub client: usage_core::HttpClient,
    pub config_path: std::path::PathBuf,
    pub config: Mutex<Config>,
    pub snapshots: Mutex<HashMap<String, Snapshot>>,
    pub locks: refresh::AccountLocks,
    pub pace: refresh::Pacing,
}

impl AppState {
    pub fn account(&self, id: &str) -> Result<Account, String> {
        self.config
            .lock()
            .unwrap()
            .accounts
            .iter()
            .find(|a| a.id == id)
            .cloned()
            .ok_or_else(|| "That account no longer exists.".to_string())
    }

    pub fn persist(&self, cfg: &Config) -> Result<(), String> {
        config::save(&self.config_path, cfg)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    #[serde(flatten)]
    account: Account,
    has_secret: bool,
}

fn view(account: Account) -> AccountView {
    let has_secret = secrets::exists(&account.id);
    AccountView { account, has_secret }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppView {
    accounts: Vec<AccountView>,
    settings: Settings,
    snapshots: Vec<Snapshot>,
    detected: LocalDetection,
    version: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountInput {
    kind: ProviderKind,
    label: String,
    #[serde(default)]
    monthly_budget: Option<f64>,
    #[serde(default)]
    data_path: Option<String>,
    /// A new key to store. Empty or missing means "keep the saved key".
    #[serde(default)]
    secret: Option<String>,
}

struct CleanInput {
    kind: ProviderKind,
    label: String,
    monthly_budget: Option<f64>,
    data_path: Option<String>,
    secret: Option<Zeroizing<String>>,
}

fn default_label(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::ClaudePlan => "Claude",
        ProviderKind::CodexPlan => "ChatGPT",
        ProviderKind::AnthropicApi => "Anthropic API",
        ProviderKind::OpenaiApi => "OpenAI API",
        ProviderKind::Openrouter => "OpenRouter",
        ProviderKind::Deepseek => "DeepSeek",
        ProviderKind::Copilot => "GitHub Copilot",
        ProviderKind::Antigravity => "Antigravity",
    }
}

fn clean(input: AccountInput) -> Result<CleanInput, String> {
    let secret = input.secret.map(Zeroizing::new);
    let label: String = input.label.trim().chars().filter(|c| !c.is_control()).take(60).collect();
    let label = if label.is_empty() { default_label(input.kind).to_string() } else { label };

    let monthly_budget = match input.monthly_budget {
        None => None,
        Some(b) if !b.is_finite() || !(0.0..=10_000_000.0).contains(&b) => {
            return Err("Budget must be a positive amount.".into())
        }
        Some(b) if b == 0.0 || !input.kind.supports_budget() => None,
        Some(b) => Some(b),
    };

    let data_path = match input.data_path.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        None => None,
        Some(_) if !input.kind.uses_local_folder() => None,
        Some(p) if p.len() > 1024 || !Path::new(p).is_absolute() => {
            return Err("Folder must be a full path, like C:\\Users\\you\\.claude".into())
        }
        Some(p) if !Path::new(p).is_dir() => return Err(format!("Folder not found: {p}")),
        Some(p) => Some(p.to_string()),
    };

    let secret = match secret.as_deref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        None => None,
        Some(s) => Some(Zeroizing::new(secrets::validate(s)?.to_string())),
    };

    Ok(CleanInput { kind: input.kind, label, monthly_budget, data_path, secret })
}

/// Async so it runs off the main thread: detecting installed tools scans processes.
#[tauri::command]
pub async fn get_state(app: AppHandle) -> AppView {
    let state = app.state::<AppState>();
    let (accounts, settings) = {
        let cfg = state.config.lock().unwrap();
        (cfg.accounts.clone(), cfg.settings.clone())
    };
    let snapshots = state.snapshots.lock().unwrap().values().cloned().collect();
    AppView {
        accounts: accounts.into_iter().map(view).collect(),
        settings,
        snapshots,
        detected: usage_core::detect_local(),
        version: app.package_info().version.to_string(),
    }
}

#[tauri::command]
pub fn add_account(state: State<'_, AppState>, input: AccountInput) -> Result<AccountView, String> {
    let input = clean(input)?;
    if input.kind.requires_secret() && input.secret.is_none() {
        return Err("Paste an API key to add this account.".into());
    }
    let account = Account {
        id: uuid::Uuid::new_v4().to_string(),
        kind: input.kind,
        label: input.label,
        monthly_budget: input.monthly_budget,
        data_path: input.data_path,
    };
    if let Some(s) = &input.secret {
        secrets::set(&account.id, s)?;
    }
    let mut cfg = state.config.lock().unwrap();
    cfg.accounts.push(account.clone());
    if let Err(e) = state.persist(&cfg) {
        cfg.accounts.pop();
        let _ = secrets::delete(&account.id);
        return Err(e);
    }
    drop(cfg);
    Ok(view(account))
}

#[tauri::command]
pub fn update_account(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    input: AccountInput,
) -> Result<AccountView, String> {
    let input = clean(input)?;
    let mut cfg = state.config.lock().unwrap();
    let idx = cfg.accounts.iter().position(|a| a.id == id).ok_or("That account no longer exists.")?;
    if cfg.accounts[idx].kind != input.kind {
        return Err("The provider of an existing account can't be changed.".into());
    }
    let before = cfg.accounts[idx].clone();
    {
        let a = &mut cfg.accounts[idx];
        a.label = input.label;
        a.monthly_budget = input.monthly_budget;
        a.data_path = input.data_path;
    }
    if let Err(e) = state.persist(&cfg) {
        cfg.accounts[idx] = before;
        return Err(e);
    }
    let account = cfg.accounts[idx].clone();
    drop(cfg);
    if let Some(s) = &input.secret {
        secrets::set(&account.id, s)?;
    }
    widget::notify(&app);
    Ok(view(account))
}

#[tauri::command]
pub async fn remove_account(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<(), String> {
    secrets::delete(&id)?;
    let settings = {
        let mut cfg = state.config.lock().unwrap();
        cfg.accounts.retain(|a| a.id != id);
        cfg.settings.widget.slots.retain(|s| s.account_id != id);
        state.persist(&cfg)?;
        cfg.settings.clone()
    };
    state.snapshots.lock().unwrap().remove(&id);
    tray::update_tooltip(&app);
    let _ = app.emit("settings-changed", &settings);
    widget::sync(&app);
    Ok(())
}

#[tauri::command]
pub fn reorder_accounts(state: State<'_, AppState>, ids: Vec<String>) -> Result<(), String> {
    let mut cfg = state.config.lock().unwrap();
    let pos = |id: &str| ids.iter().position(|x| x == id).unwrap_or(usize::MAX);
    cfg.accounts.sort_by_key(|a| pos(&a.id));
    state.persist(&cfg)
}

#[tauri::command]
pub async fn refresh_account(app: AppHandle, id: String) -> Result<Snapshot, String> {
    refresh::refresh(&app, &id, refresh::Trigger::Manual).await
}

/// Checks a form before saving. For an existing account, an empty key field means
/// "use the saved key".
#[tauri::command]
pub async fn test_account(
    state: State<'_, AppState>,
    input: AccountInput,
    existing_id: Option<String>,
) -> Result<Snapshot, String> {
    let input = clean(input)?;
    let secret = match (input.secret, existing_id.as_deref()) {
        (Some(s), _) => Some(s),
        (None, Some(id)) => secrets::get(id)?,
        (None, None) => None,
    };
    if input.kind.requires_secret() && secret.is_none() {
        return Err("Paste an API key first.".into());
    }
    let account = Account {
        id: "test".into(),
        kind: input.kind,
        label: input.label,
        monthly_budget: input.monthly_budget,
        data_path: input.data_path,
    };
    let result = usage_core::fetch(&state.client, &account, secret.as_deref().map(|s| s.as_str())).await;
    Ok(Snapshot::from_result("test", result))
}

#[tauri::command]
pub async fn save_settings(app: AppHandle, state: State<'_, AppState>, settings: Settings) -> Result<Settings, String> {
    let settings = settings.sanitized();
    {
        let mut cfg = state.config.lock().unwrap();
        let known: Vec<String> = cfg.accounts.iter().map(|a| a.id.clone()).collect();
        let mut settings = settings.clone();
        settings.widget.slots.retain(|s| known.contains(&s.account_id));
        let before = std::mem::replace(&mut cfg.settings, settings);
        if let Err(e) = state.persist(&cfg) {
            cfg.settings = before;
            return Err(e);
        }
    }
    let saved = state.config.lock().unwrap().settings.clone();
    let _ = app.emit("settings-changed", &saved);
    widget::sync(&app);
    Ok(saved)
}

/// Re-checks all accounts now (toolbar button, tray menu, Ctrl+R).
#[tauri::command]
pub async fn refresh_all(app: AppHandle) {
    refresh::refresh_all(&app, refresh::Trigger::Manual).await;
}

#[tauri::command]
pub fn hide_window(app: AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
}

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}

/// Opens the official GitHub CLI sign-in (`gh auth login --web`) in its own console
/// window. Used for Copilot in VS Code, whose own sign-in UsageCheck doesn't read.
#[tauri::command]
pub async fn github_sign_in() -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
        let installed = std::process::Command::new("gh")
            .arg("--version")
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !installed {
            return Err(
                "The GitHub CLI isn't installed. Install it from cli.github.com (or run `winget install GitHub.cli`), then try again."
                    .into(),
            );
        }
        std::process::Command::new("gh")
            .args(["auth", "login", "--web", "--hostname", "github.com", "--git-protocol", "https"])
            .creation_flags(CREATE_NEW_CONSOLE)
            .spawn()
            .map_err(|e| format!("Could not start the GitHub sign-in: {e}"))?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Err("Run `gh auth login` in a terminal, then try again.".into())
    }
}
