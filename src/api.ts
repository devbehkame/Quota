import { invoke } from "@tauri-apps/api/core";

export type ProviderKind =
  | "claude_plan"
  | "codex_plan"
  | "anthropic_api"
  | "openai_api"
  | "openrouter"
  | "deepseek"
  | "copilot"
  | "antigravity"
  | "cursor";

export type Unit = "percent" | "usd" | "cny" | "count";

export interface Account {
  id: string;
  kind: ProviderKind;
  label: string;
  monthlyBudget: number | null;
  dataPath: string | null;
  hasSecret: boolean;
}

export interface Meter {
  label: string;
  unit: Unit;
  used: number | null;
  limit: number | null;
  remaining: number | null;
  percentUsed: number | null;
  resetsAt: string | null;
  detail: string | null;
}

export interface Usage {
  plan: string | null;
  meters: Meter[];
  notes: string[];
  /** The signed-in account (email or username), when the sign-in says. */
  signedInAs: string | null;
  /** Banked limit resets (Claude and ChatGPT plans), when the service reports them. */
  resets: LimitResets | null;
  /** Tokens used this week, from the local Claude Code / Codex logs. */
  tokens: TokenUsage | null;
}

export interface TokenUsage {
  /** Start of the weekly window the counts cover. */
  since: string;
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
  /** This PC's share of the weekly limit (estimated), in percentage points. */
  share: WeeklyShare | null;
}

export interface WeeklyShare {
  used: number;
  thisPc: number;
  other: number;
  unsplit: number;
  /** Part of thisPc is estimated from tokens logged while Quota wasn't watching. */
  estimated: boolean;
  trackedSince: string;
}

export interface LimitResets {
  available: number;
  /** Soonest expiry first; empty when only the count is known. */
  items: { label: string | null; count: number; expiresAt: string | null }[];
}

export interface Snapshot {
  accountId: string;
  fetchedAt: string;
  usage: Usage | null;
  error: string | null;
}

export interface Settings {
  refreshMinutes: number;
  closeToTray: boolean;
  startWithWindows: boolean;
  theme: "system" | "light" | "dark";
  widget: WidgetSettings;
  autoUpdate: boolean;
}

export interface UpdateStatus {
  state: "idle" | "checking" | "upToDate" | "available" | "downloading" | "installing" | "error";
  currentVersion: string;
  newVersion: string | null;
  message: string | null;
  checkedAt: string | null;
}

export interface WidgetSlot {
  accountId: string;
  /** Meter label to show; null = the tightest one. */
  meter: string | null;
}

export interface WidgetSettings {
  enabled: boolean;
  slots: WidgetSlot[];
  /** A second, thinner bar under each slot for its weekly limit. */
  weeklyBar: boolean;
}

export interface AppView {
  accounts: Account[];
  settings: Settings;
  snapshots: Snapshot[];
  detected: {
    claude: boolean;
    codex: boolean;
    copilot: boolean;
    copilotNeedsSignIn: boolean;
    antigravity: boolean;
    cursor: boolean;
    cursorNeedsSignIn: boolean;
  };
  version: string;
}

export interface AccountInput {
  kind: ProviderKind;
  label: string;
  monthlyBudget: number | null;
  dataPath: string | null;
  /** Only sent when the user typed a new key; never read back from the backend. */
  secret: string | null;
}

export const api = {
  getState: () => invoke<AppView>("get_state"),
  addAccount: (input: AccountInput) => invoke<Account>("add_account", { input }),
  updateAccount: (id: string, input: AccountInput) => invoke<Account>("update_account", { id, input }),
  removeAccount: (id: string) => invoke<void>("remove_account", { id }),
  reorderAccounts: (ids: string[]) => invoke<void>("reorder_accounts", { ids }),
  refreshAccount: (id: string) => invoke<Snapshot>("refresh_account", { id }),
  refreshAll: () => invoke<void>("refresh_all"),
  widgetResetPosition: () => invoke<void>("widget_reset_position"),
  testAccount: (input: AccountInput, existingId: string | null) =>
    invoke<Snapshot>("test_account", { input, existingId }),
  saveSettings: (settings: Settings) => invoke<Settings>("save_settings", { settings }),
  quit: () => invoke<void>("quit_app"),
  githubSignIn: () => invoke<void>("github_sign_in"),
  updateStatus: () => invoke<UpdateStatus>("update_status"),
  checkForUpdates: () => invoke<UpdateStatus>("check_for_updates"),
};

export function errorText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return "Something went wrong.";
}
