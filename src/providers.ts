import type { ProviderKind } from "./api";

export interface ProviderInfo {
  kind: ProviderKind;
  name: string;
  tagline: string;
  color: string;
  glyph: string;
  /** "required": must paste a key; "optional": advanced override; "none": no key at all. */
  secret: "required" | "optional" | "none";
  secretLabel?: string;
  secretPlaceholder?: string;
  /** Optional monthly limit the user can enter (USD budget, or a request allowance). */
  budget?: { label: string; hint: string; prefix?: string; placeholder: string };
  folder?: { label: string; placeholder: string };
  help: string;
  link?: { text: string; url: string };
}

export const PROVIDERS: ProviderInfo[] = [
  {
    kind: "claude_plan",
    name: "Claude plan",
    tagline: "Pro / Max · session and weekly limits",
    color: "#d97757",
    glyph: "C",
    secret: "optional",
    secretLabel: "Access token override",
    secretPlaceholder: "Leave empty to use Claude Code's sign-in",
    folder: { label: "Claude config folder", placeholder: "Default: %USERPROFILE%\\.claude" },
    help:
      "Uses the sign-in from Claude Code on this PC. Install Claude Code and run `claude` once. The token is only read, never changed, and is sent only to Anthropic.",
    link: { text: "Get Claude Code", url: "https://claude.com/product/claude-code" },
  },
  {
    kind: "codex_plan",
    name: "ChatGPT plan",
    tagline: "Plus / Pro · live Codex limits",
    color: "#10a37f",
    glyph: "G",
    secret: "none",
    folder: { label: "Codex folder", placeholder: "Default: %USERPROFILE%\\.codex" },
    help:
      "Uses the ChatGPT sign-in from the Codex CLI on this PC (run `codex login` once) to read live limits, the same ones Codex shows in /status. The sign-in is only read, never changed, and is sent only to chatgpt.com.",
    link: { text: "Get Codex CLI", url: "https://developers.openai.com/codex/cli" },
  },
  {
    kind: "anthropic_api",
    name: "Anthropic API",
    tagline: "Monthly spend · Admin key",
    color: "#c96442",
    glyph: "A",
    secret: "required",
    secretLabel: "Admin API key",
    secretPlaceholder: "sk-ant-admin01-…",
    budget: { label: "Monthly budget (USD)", hint: "Optional. Used to show how much is left this month.", prefix: "$", placeholder: "e.g. 100" },
    help: "Needs an Admin key (not a normal API key) to read your organization's cost report.",
    link: { text: "Create an Admin key", url: "https://platform.claude.com/settings/admin-keys" },
  },
  {
    kind: "openai_api",
    name: "OpenAI API",
    tagline: "Monthly spend · Admin key",
    color: "#1f2328",
    glyph: "O",
    secret: "required",
    secretLabel: "Admin API key",
    secretPlaceholder: "sk-admin-…",
    budget: { label: "Monthly budget (USD)", hint: "Optional. Used to show how much is left this month.", prefix: "$", placeholder: "e.g. 100" },
    help: "Needs an organization Admin key to read the Costs API.",
    link: { text: "Create an Admin key", url: "https://platform.openai.com/settings/organization/admin-keys" },
  },
  {
    kind: "openrouter",
    name: "OpenRouter",
    tagline: "Credit balance",
    color: "#8b5cf6",
    glyph: "R",
    secret: "required",
    secretLabel: "API key",
    secretPlaceholder: "sk-or-v1-…",
    help: "A management key shows your whole account balance. A regular key shows that key's own limit.",
    link: { text: "Manage keys", url: "https://openrouter.ai/settings/keys" },
  },
  {
    kind: "deepseek",
    name: "DeepSeek",
    tagline: "Prepaid balance",
    color: "#4d6bfe",
    glyph: "D",
    secret: "required",
    secretLabel: "API key",
    secretPlaceholder: "sk-…",
    help: "Any DeepSeek API key can read the account balance.",
    link: { text: "Manage keys", url: "https://platform.deepseek.com/api_keys" },
  },
  {
    kind: "copilot",
    name: "GitHub Copilot",
    tagline: "Premium requests · monthly quota",
    color: "#2f81f7",
    glyph: "Co",
    secret: "optional",
    secretLabel: "GitHub token",
    secretPlaceholder: "Leave empty to use your Copilot or GitHub CLI sign-in",
    budget: {
      label: "Monthly premium requests",
      hint: "Only needed if you paste a fine-grained token: e.g. 300 for Pro, 1500 for Pro+.",
      placeholder: "e.g. 300",
    },
    folder: { label: "Copilot config folder", placeholder: "Default: %LOCALAPPDATA%\\github-copilot" },
    help:
      "Uses the sign-in from Copilot in JetBrains, Visual Studio or Vim, or the GitHub CLI (`gh auth login`). You can also paste a token from `gh auth token`. Only read, and only sent to GitHub.",
    link: { text: "Get the GitHub CLI", url: "https://cli.github.com/" },
  },
  {
    kind: "antigravity",
    name: "Antigravity",
    tagline: "Per-model quotas · Google",
    color: "#4285f4",
    glyph: "Ag",
    secret: "none",
    help:
      "Reads the quotas Antigravity itself shows, from the app running on this PC. Nothing is sent over the internet and no Google sign-in is touched. Antigravity must be open to update; otherwise the last values stay on screen.",
    link: { text: "Get Antigravity", url: "https://antigravity.google/" },
  },
];

export function provider(kind: ProviderKind): ProviderInfo {
  return PROVIDERS.find((p) => p.kind === kind) ?? PROVIDERS[0]!;
}
