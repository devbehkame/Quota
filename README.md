# UsageCheck

A small, fast Windows app that shows how much usage is left on your AI accounts, all in one window and in the system tray.

![UsageCheck](docs/screenshot.png)

| Account | What you see | How it connects |
| --- | --- | --- |
| **Claude plan** (Pro / Max) | 5-hour session and weekly limits, with reset times | Reads the sign-in from [Claude Code](https://claude.com/product/claude-code) on your PC. Nothing to paste. |
| **ChatGPT plan** (Plus / Pro) | Live 5-hour and weekly Codex limits, with reset times | Reuses the ChatGPT sign-in from the [Codex CLI](https://developers.openai.com/codex/cli) on your PC. Nothing to paste. |
| **Anthropic API** | Spend this month, and what's left of your budget | Admin API key (`sk-ant-admin…`) |
| **OpenAI API** | Spend this month, and what's left of your budget | Admin API key (`sk-admin-…`) |
| **OpenRouter** | Credits left | API key (a management key shows the whole account) |
| **DeepSeek** | Prepaid balance | API key |

> **Why the ChatGPT and Claude plans work this way:** neither OpenAI nor Anthropic offers a public API for consumer plan limits. UsageCheck calls the same usage endpoints their own command-line tools use (Claude Code's `/usage`, Codex's `/status`). These endpoints are undocumented, so either company could change them at any time.
>
> For ChatGPT, if the live call isn't possible (for example Codex keeps its sign-in in the OS keyring instead of `~/.codex/auth.json`, or the sign-in has expired), UsageCheck falls back to the limits Codex saved in its session logs after your last message, and says so on the card.

## Install

1. Open the repository's **Actions** tab, pick the latest green **Build** run, and download the `UsageCheck-windows` artifact. Tagged releases (`v*`) also attach the installers to a GitHub Release.
2. Run `UsageCheck_x.y.z_x64-setup.exe`. It installs per-user, so no admin rights are needed. Use the `.msi` instead for managed deployment.
3. Click **Add account**. If Claude Code or Codex is already on your PC, UsageCheck offers to add them in one click.

To see your plan limits:
- **Claude:** install Claude Code and run `claude` once to sign in. If the token expires, opening Claude Code renews it.
- **ChatGPT:** install Codex CLI and run `codex login` with your ChatGPT account. The card then updates live on every refresh, with no messages needed. If the sign-in expires, running `codex` once renews it.

## Security

- **Keys live in Windows Credential Manager**, which encrypts them with DPAPI under your Windows account. They are never written to the app's config file.
- **The interface never sees your keys.** All network calls happen in the Rust backend. The web UI only gets the usage numbers back, and it renders them as plain text (no `innerHTML`), so a provider response can't inject markup.
- **Your Claude and Codex sign-ins are only read.** UsageCheck never refreshes or rewrites them, so Claude Code and Codex keep working normally. Each is sent only to its own service (api.anthropic.com, chatgpt.com).
- **Network access is locked down:** HTTPS only, no redirects followed, strict timeouts, and each key goes only to its own provider's host.
- **Least-privilege Tauri setup:** a strict Content-Security-Policy, a frozen JS prototype, and only the permissions the UI needs. Links open only for an allow-listed set of provider sites.
- **No telemetry, no analytics, no accounts.**

## Features

- Every account on one dashboard, coloured green, amber or red by how much is left.
- Reset countdowns ("Resets in 2h 18m"). Hover one for the exact time.
- Auto-refresh (off, or every 1–60 min), plus a refresh of anything older than a minute when you bring the window back. Ctrl+R / F5 refreshes now.
- Tray icon: hover it to see each account's tightest limit; right-click it to refresh or quit.
- Optional start with Windows (launches minimized to the tray).
- Light, dark or system theme.
- A **Test connection** button before you save a key.

## Development

Requirements: Rust (stable), Node 20+, and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) (on Windows, WebView2 and the MSVC build tools).

```sh
npm install
npm run tauri dev       # run with hot reload
npm run tauri build     # produce installers in target/release/bundle
cargo test --workspace  # provider parsing tests
```

Layout:

```
crates/usage-core/   Provider clients and parsing (pure Rust, unit-tested, no UI)
src-tauri/           Desktop shell: commands, Credential Manager, tray, config
src/                 UI (TypeScript + CSS, no framework, ~25 KB)
```

Adding a provider means one file in `crates/usage-core/src/providers/`, one enum variant, and one entry in `src/providers.ts`.

## Why Tauri + Rust

- **Small and fast:** the installer is a few MB and idles at a fraction of an Electron app's memory. It uses Windows' built-in WebView2.
- **Secure by construction:** the memory-safe Rust backend holds the secrets. The sandboxed UI only gets the permissions it needs.
- **Nice UI:** the web stack makes a polished interface easy, and it still feels native (tray, autostart, single instance).
