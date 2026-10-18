pub mod anthropic_api;
pub mod antigravity;
pub mod claude_plan;
pub mod codex_plan;
pub mod copilot;
pub mod cursor;
pub mod deepseek;
pub mod openai_api;
pub mod openrouter;
pub mod tokens;

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

/// Editors that keep extensions under `~/<dir>/extensions` (VS Code and its forks).
const EDITOR_DIRS: &[&str] = &[".vscode", ".vscode-insiders", ".antigravity", ".cursor", ".windsurf", ".vscode-oss"];

/// Whether an editor extension whose folder name contains `needle` is installed.
/// Only folder names are looked at; nothing inside the editors is read.
pub(crate) fn editor_extension_installed(needle: &str) -> bool {
    let Some(home) = dirs::home_dir() else { return false };
    let needle = needle.to_ascii_lowercase();
    EDITOR_DIRS.iter().any(|d| {
        std::fs::read_dir(home.join(d).join("extensions"))
            .map(|entries| {
                entries.flatten().any(|e| e.file_name().to_string_lossy().to_ascii_lowercase().contains(&needle))
            })
            .unwrap_or(false)
    })
}
