# UsageCheck

A small, fast Windows app that shows how much usage is left on your AI accounts, all in one window and in the system tray.

![UsageCheck](docs/screenshot.png)

The taskbar widget, next to the clock:

![Taskbar widget](docs/widget.png)

| Account | What you see | How it connects |
| --- | --- | --- |
| **Claude plan** (Pro / Max) | 5-hour session and weekly limits, with reset times | Reads the sign-in from [Claude Code](https://claude.com/product/claude-code) on your PC. Nothing to paste. |
| **ChatGPT plan** (Plus / Pro) | Live 5-hour and weekly Codex limits, with reset times | Reuses the ChatGPT sign-in from the [Codex CLI](https://developers.openai.com/codex/cli) on your PC. Nothing to paste. |
| **Anthropic API** | Spend this month, and what's left of your budget | Admin API key (`sk-ant-admin…`) |
| **OpenAI API** | Spend this month, and what's left of your budget | Admin API key (`sk-admin-…`) |
| **OpenRouter** | Credits left | API key (a management key shows the whole account) |
| **DeepSeek** | Prepaid balance | API key |
| **GitHub Copilot** | Premium requests left this month (plus chat and completions on Free), with the reset date | Reuses the GitHub sign-in from Copilot for JetBrains / Visual Studio / Vim or the [GitHub CLI](https://cli.github.com/) (`gh auth login`). You can also paste a token. |
| **Antigravity** | Quota left for each model group, with reset times, plus prompt credits | Reads the quota from [Antigravity](https://antigravity.google/) while it's running on your PC. Nothing to paste, and nothing leaves your computer. |

> **Why the ChatGPT and Claude plans work this way:** neither OpenAI nor Anthropic offers a public API for consumer plan limits. UsageCheck calls the same usage endpoints their own command-line tools use (Claude Code's `/usage`, Codex's `/status`). These endpoints are undocumented, so either company could change them at any time.
>
> GitHub Copilot's quota endpoint is also internal. If it refuses your token (fine-grained tokens usually can't use it), UsageCheck switches to GitHub's official billing API. That needs a fine-grained token with the **Plan (read)** account permission, and only reports requests *used*, so you also enter your monthly allowance (e.g. 300 for Pro).
>
> Antigravity has no web API for quotas. UsageCheck asks the Antigravity app on your PC, the same way the app's own quota display does, so Antigravity has to be open. When it's closed, the card keeps the last values it saw and says so.
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
- **Your Claude, Codex and GitHub sign-ins are only read.** UsageCheck never refreshes or rewrites them, so those tools keep working normally. Each is sent only to its own service (api.anthropic.com, chatgpt.com, api.github.com).
- **Antigravity stays on your PC.** UsageCheck only talks to Antigravity's local server on `127.0.0.1`, using the per-session token Antigravity gives it on its command line. That local server uses a self-signed certificate, so certificate checks are off for that one loopback connection only. Nothing goes over the internet.
- **Network access is locked down:** HTTPS only, no redirects followed, strict timeouts, and each key goes only to its own provider's host.
- **Least-privilege Tauri setup:** a strict Content-Security-Policy, a frozen JS prototype, and only the permissions the UI needs. Links open only for an allow-listed set of provider sites.
- **No telemetry, no analytics, no accounts.**

## Features

- Every account on one dashboard, coloured green, amber or red by how much is left.
- Reset countdowns ("Resets in 2h 18m"). Hover one for the exact time.
- Auto-refresh (off, or every 1–60 min) runs in the background, so the tray and taskbar widget stay current even when the window is closed to the tray. There's also a refresh of anything older than a minute when you bring the window back. Ctrl+R / F5 refreshes now.
- **Taskbar widget:** two accounts sit on the taskbar next to the clock, each as its icon plus how much is left (e.g. `C 65%  G 88%`), coloured green, amber or red. In Settings → Taskbar widget, pick which two accounts to show, and for each one either its tightest limit or a specific gauge (e.g. Claude's weekly limit). Click it to open UsageCheck, or drag it anywhere; "Dock to clock" puts it back. It hides while a full-screen app or game is in front, follows the taskbar's light or dark colour, and can also be toggled from the tray menu.
- Tray icon: hover it to see each account's tightest limit; right-click it to refresh, toggle the taskbar widget, or quit.
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

## Credits

Provider logos come from [LobeHub Icons](https://github.com/lobehub/lobe-icons) (MIT). Claude, ChatGPT, OpenAI, Anthropic, GitHub Copilot, Antigravity, OpenRouter and DeepSeek are trademarks of their owners; their logos are used only to identify each service.
