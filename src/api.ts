import { invoke } from "@tauri-apps/api/core";

export type ProviderKind =
  | "claude_plan"
  | "codex_plan"
  | "anthropic_api"
  | "openai_api"
  | "openrouter"
  | "deepseek"
  | "copilot"
  | "antigravity";

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
  theme: "system" | "light" | "dark";
}

export interface AppView {
  accounts: Account[];
  settings: Settings;
  snapshots: Snapshot[];
  detected: { claude: boolean; codex: boolean; copilot: boolean; antigravity: boolean };
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
  testAccount: (input: AccountInput, existingId: string | null) =>
    invoke<Snapshot>("test_account", { input, existingId }),
  saveSettings: (settings: Settings) => invoke<Settings>("save_settings", { settings }),
  quit: () => invoke<void>("quit_app"),
};

export function errorText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return "Something went wrong.";
}
