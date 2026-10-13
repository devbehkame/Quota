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
  budget: boolean;
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
    budget: false,
    folder: { label: "Claude config folder", placeholder: "Default: %USERPROFILE%\\.claude" },
    help:
      "Uses the sign-in from Claude Code on this PC. Install Claude Code and run `claude` once. The token is only read, never changed, and is sent only to Anthropic.",
    link: { text: "Get Claude Code", url: "https://claude.com/product/claude-code" },
  },
  {
    kind: "codex_plan",
    name: "ChatGPT plan",
    tagline: "Plus / Pro · Codex limits",
    color: "#10a37f",
    glyph: "G",
    secret: "none",
    budget: false,
    folder: { label: "Codex folder", placeholder: "Default: %USERPROFILE%\\.codex" },
    help:
      "OpenAI has no public API for ChatGPT plan limits. UsageCheck reads the limits the Codex CLI saves on this PC after each message. Nothing is sent anywhere.",
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
    budget: true,
    help: "Needs an Admin key (not a normal API key) to read your organization's cost report.",
    link: { text: "Create an Admin key", url: "https://platform.claude.com/settings/admin-keys" },
  },
  {
    kind: "openai_api",
    name: "OpenAI API",
    tagline: "Monthly spend · Admin key",
    color: "#6b7cff",
    glyph: "O",
    secret: "required",
    secretLabel: "Admin API key",
    secretPlaceholder: "sk-admin-…",
    budget: true,
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
    budget: false,
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
    budget: false,
    help: "Any DeepSeek API key can read the account balance.",
    link: { text: "Manage keys", url: "https://platform.deepseek.com/api_keys" },
  },
];

export function provider(kind: ProviderKind): ProviderInfo {
  return PROVIDERS.find((p) => p.kind === kind) ?? PROVIDERS[0]!;
}
