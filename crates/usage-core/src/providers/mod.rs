pub mod anthropic_api;
pub mod antigravity;
pub mod claude_plan;
pub mod codex_plan;
pub mod copilot;
pub mod deepseek;
pub mod openai_api;
pub mod openrouter;

use std::path::PathBuf;

/// Expands an optional user-supplied folder, falling back to `~/<default_dir>`.
pub(crate) fn config_dir(over: Option<&str>, env_var: &str, default_dir: &str) -> Option<PathBuf> {
    if let Some(p) = over.map(str::trim).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    if let Some(p) = std::env::var_os(env_var).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    dirs::home_dir().map(|h| h.join(default_dir))
}
