import { listen } from "@tauri-apps/api/event";
import { disable as disableAutostart, enable as enableAutostart, isEnabled as autostartEnabled } from "@tauri-apps/plugin-autostart";
import { openUrl } from "@tauri-apps/plugin-opener";

import {
  api,
  errorText,
  type Account,
  type AccountInput,
  type AppView,
  type Meter,
  type ProviderKind,
  type Settings,
  type Snapshot,
  type WidgetSlot,
} from "./api";
import { h, icon, logo } from "./dom";
import { ago, headline, leftPct, level, money, pct, resetLabel, until } from "./format";
import { PROVIDERS, provider, type ProviderInfo } from "./providers";

// ---------------------------------------------------------------- state

let view: AppView | null = null;
const snaps = new Map<string, Snapshot>();
const busy = new Set<string>();
let openMenuEl: HTMLElement | null = null;

const root = document.getElementById("app")!;

// ---------------------------------------------------------------- actions

async function refreshOne(id: string) {
  if (busy.has(id)) return;
  busy.add(id);
  render();
  try {
    snaps.set(id, await api.refreshAccount(id));
  } catch (e) {
    snaps.set(id, { accountId: id, fetchedAt: new Date().toISOString(), usage: null, error: errorText(e) });
  } finally {
    busy.delete(id);
    render();
  }
}

async function refreshAll() {
  try {
    await api.refreshAll();
  } catch (e) {
    toast(errorText(e), "error");
  }
}

function applyTheme(theme: Settings["theme"]) {
  if (theme === "system") delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = theme;
}

async function open(url: string) {
  try {
    await openUrl(url);
  } catch (e) {
    toast(errorText(e), "error");
  }
}

async function moveAccount(id: string, delta: number) {
  if (!view) return;
  const list = [...view.accounts];
  const i = list.findIndex((a) => a.id === id);
  const j = i + delta;
  if (i < 0 || j < 0 || j >= list.length) return;
  [list[i], list[j]] = [list[j]!, list[i]!];
  view.accounts = list;
  render();
  try {
    await api.reorderAccounts(list.map((a) => a.id));
  } catch (e) {
    toast(errorText(e), "error");
  }
}

async function removeAccount(a: Account) {
  const ok = await confirmDialog(
    `Remove “${a.label}”?`,
    "Its saved key will be deleted from Windows Credential Manager.",
    "Remove",
  );
  if (!ok || !view) return;
  try {
    await api.removeAccount(a.id);
    view.accounts = view.accounts.filter((x) => x.id !== a.id);
    snaps.delete(a.id);
    render();
    toast(`Removed ${a.label}`);
  } catch (e) {
    toast(errorText(e), "error");
  }
}

// ---------------------------------------------------------------- rendering

function render() {
  if (!view) return;
  closeMenu();
  root.replaceChildren(topbar(), view.accounts.length ? grid() : emptyState());
}

function topbar(): HTMLElement {
  const accounts = view!.accounts;
  const times = accounts.map((a) => snaps.get(a.id)?.fetchedAt).filter((t): t is string => !!t);
  const oldest = times.sort()[0];
  const failing = accounts.filter((a) => snaps.get(a.id)?.error).length;
  const parts: (string | HTMLElement)[] = [];
  if (accounts.length) parts.push(`${accounts.length} account${accounts.length === 1 ? "" : "s"}`);
  if (oldest) parts.push(`updated ${ago(oldest)}`);
  const anyBusy = busy.size > 0;

  return h(
    "header",
    { class: "topbar" },
    h(
      "div",
      { class: "brand" },
      h("div", { class: "logo" }, icon("gauge", 22)),
      h(
        "div",
        null,
        h("h1", null, "UsageCheck"),
        h(
          "p",
          { class: "sub" },
          parts.join(" · ") || "All your AI limits in one place",
          failing ? h("span", { class: "pill danger" }, `${failing} need${failing === 1 ? "s" : ""} attention`) : null,
        ),
      ),
    ),
    h(
      "div",
      { class: "actions" },
      accounts.length
        ? h(
            "button",
            {
              class: `icon-btn${anyBusy ? " spinning" : ""}`,
              title: "Refresh all (Ctrl+R)",
              "aria-label": "Refresh all",
              onclick: refreshAll,
            },
            icon("refresh"),
          )
        : null,
      h("button", { class: "icon-btn", title: "Settings", "aria-label": "Settings", onclick: openSettings }, icon("settings")),
      h("button", { class: "btn primary", onclick: () => openAccountDialog() }, icon("plus", 16), "Add account"),
    ),
  );
}

function emptyState(): HTMLElement {
  const d = view!.detected;
  const quick: HTMLElement[] = [];
  if (d.claude) quick.push(quickAdd("claude_plan", "Claude", "Claude Code sign-in found"));
  if (d.codex) quick.push(quickAdd("codex_plan", "ChatGPT", "Codex CLI sign-in found"));
  if (d.copilot) quick.push(quickAdd("copilot", "GitHub Copilot", "Copilot sign-in found"));
  if (d.antigravity) quick.push(quickAdd("antigravity", "Antigravity", "Antigravity is running"));
  return h(
    "main",
    { class: "empty" },
    h("div", { class: "empty-art" }, icon("gauge", 56)),
    h("h2", null, "Track every AI plan in one place"),
    h(
      "p",
      null,
      "Add your Claude, ChatGPT and API accounts to see how much usage is left, and when it resets. Keys stay encrypted in Windows Credential Manager.",
    ),
    quick.length ? h("div", { class: "quick" }, h("div", { class: "quick-title" }, "Found on this PC"), ...quick) : null,
    h("button", { class: "btn primary lg", onclick: () => openAccountDialog() }, icon("plus", 16), "Add an account"),
  );
}

function quickAdd(kind: ProviderKind, label: string, hint: string): HTMLElement {
  const p = provider(kind);
  return h(
    "button",
    {
      class: "quick-item",
      onclick: async () => {
        try {
          const acc = await api.addAccount({ kind, label, monthlyBudget: null, dataPath: null, secret: null });
          view!.accounts.push(acc);
          render();
          void refreshOne(acc.id);
        } catch (e) {
          toast(errorText(e), "error");
        }
      },
    },
    avatar(p),
    h("span", { class: "quick-text" }, h("strong", null, `Add ${p.name}`), h("small", null, hint)),
    icon("plus", 16),
  );
}

function grid(): HTMLElement {
  return h("main", { class: "grid" }, ...view!.accounts.map(card));
}

function avatar(p: ProviderInfo): HTMLElement {
  return h("div", { class: "avatar", style: { "--accent": p.color }, title: p.name }, logo(p.kind, 20));
}

function card(a: Account): HTMLElement {
  const p = provider(a.kind);
  const snap = snaps.get(a.id);
  const isBusy = busy.has(a.id);
  const usage = snap?.usage ?? null;

  let body: HTMLElement;
  if (!snap) {
    body = h("div", { class: "card-body" }, skeleton(), skeleton());
  } else if (snap.error) {
    body = h(
      "div",
      { class: "card-body" },
      h(
        "div",
        { class: "callout danger" },
        icon("alert", 18),
        h("div", null, h("p", null, snap.error)),
      ),
      h(
        "div",
        { class: "row gap" },
        h("button", { class: "btn small", onclick: () => refreshOne(a.id) }, "Try again"),
        h("button", { class: "btn small ghost", onclick: () => openAccountDialog(a) }, "Edit account"),
      ),
    );
  } else {
    body = h(
      "div",
      { class: "card-body" },
      ...(usage?.meters ?? []).map(meter),
      usage?.notes.length ? h("ul", { class: "notes" }, ...usage.notes.map((n) => h("li", null, n))) : null,
    );
  }

  const menuBtn = h("button", { class: "icon-btn ghost small", "aria-label": "Account options", title: "Options" }, icon("more"));
  menuBtn.addEventListener("click", (e) => {
    e.stopPropagation();
    openMenu(menuBtn, [
      { label: "Refresh", onClick: () => refreshOne(a.id) },
      { label: "Edit", onClick: () => openAccountDialog(a) },
      { label: "Move earlier", onClick: () => moveAccount(a.id, -1) },
      { label: "Move later", onClick: () => moveAccount(a.id, 1) },
      { label: "Remove", danger: true, onClick: () => removeAccount(a) },
    ]);
  });

  return h(
    "article",
    { class: `card${isBusy ? " busy" : ""}`, style: { "--accent": p.color } },
    h(
      "div",
      { class: "card-head" },
      avatar(p),
      h(
        "div",
        { class: "titles" },
        h("div", { class: "title", title: a.label }, a.label),
        h(
          "div",
          { class: "subtitle" },
          a.label === p.name ? p.tagline.split(" · ")[0] : p.name,
          usage?.plan ? h("span", { class: "pill" }, usage.plan) : null,
        ),
      ),
      menuBtn,
    ),
    body,
    h(
      "footer",
      { class: "card-foot" },
      isBusy ? h("span", { class: "spinner" }) : icon("clock", 14),
      h("span", null, isBusy ? "Refreshing…" : snap ? `Updated ${ago(snap.fetchedAt)}` : "Waiting…"),
      h(
        "button",
        { class: "link", onclick: () => refreshOne(a.id), disabled: isBusy },
        "Refresh",
      ),
    ),
  );
}

function meter(m: Meter): HTMLElement {
  const lvl = level(m);
  const { value, caption } = headline(m);
  const left = leftPct(m);
  let of: string | null = null;
  if (m.unit === "percent" && m.used !== null) of = `${pct(m.used)} used`;
  else if (m.limit !== null && m.used !== null) of = `${money(m.unit, m.used)} of ${money(m.unit, m.limit)}`;

  return h(
    "div",
    { class: `meter lvl-${lvl}` },
    h(
      "div",
      { class: "meter-top" },
      h("span", { class: "meter-label" }, m.label),
      m.resetsAt
        ? h("span", { class: "meter-reset", title: `Resets ${resetLabel(m.resetsAt)}` }, `Resets ${until(m.resetsAt)}`)
        : null,
    ),
    h(
      "div",
      { class: "meter-value" },
      h("span", { class: "big" }, value),
      h("span", { class: "cap" }, caption),
      of ? h("span", { class: "of" }, of) : null,
    ),
    left !== null
      ? h(
          "div",
          {
            class: "bar",
            role: "progressbar",
            "aria-valuemin": 0,
            "aria-valuemax": 100,
            "aria-valuenow": Math.round(left),
            "aria-label": `${m.label}: ${pct(left)} left`,
          },
          h("div", { class: "fill", style: { width: `${left}%` } }),
        )
      : null,
    m.detail ? h("div", { class: "meter-detail" }, m.detail) : null,
  );
}

function skeleton(): HTMLElement {
  return h("div", { class: "skeleton" }, h("div", { class: "sk sk-line short" }), h("div", { class: "sk sk-big" }), h("div", { class: "sk sk-bar" }));
}

// ---------------------------------------------------------------- menus & toasts

interface MenuItem {
  label: string;
  danger?: boolean;
  onClick: () => void;
}

function openMenu(anchor: HTMLElement, items: MenuItem[]) {
  closeMenu();
  const menu = h(
    "div",
    { class: "menu", role: "menu" },
    ...items.map((it) =>
      h(
        "button",
        {
          class: `menu-item${it.danger ? " danger" : ""}`,
          role: "menuitem",
          onclick: () => {
            closeMenu();
            it.onClick();
          },
        },
        it.label,
      ),
    ),
  );
  document.body.append(menu);
  const r = anchor.getBoundingClientRect();
  const mw = menu.offsetWidth;
  const mh = menu.offsetHeight;
  menu.style.left = `${Math.max(8, Math.min(r.right - mw, window.innerWidth - mw - 8))}px`;
  menu.style.top = `${r.bottom + 6 + mh > window.innerHeight ? r.top - mh - 6 : r.bottom + 6}px`;
  openMenuEl = menu;
  (menu.querySelector("button") as HTMLButtonElement | null)?.focus();
}

function closeMenu() {
  openMenuEl?.remove();
  openMenuEl = null;
}

function toast(message: string, kind: "info" | "error" = "info") {
  let host = document.querySelector<HTMLElement>(".toasts");
  if (!host) {
    host = h("div", { class: "toasts", "aria-live": "polite" });
    document.body.append(host);
  }
  const t = h("div", { class: `toast ${kind}` }, kind === "error" ? icon("alert", 16) : icon("check", 16), message);
  host.append(t);
  window.setTimeout(() => {
    t.classList.add("out");
    window.setTimeout(() => t.remove(), 250);
  }, kind === "error" ? 6000 : 2800);
}

// ---------------------------------------------------------------- dialogs

function modal(content: HTMLElement[], cls = ""): HTMLDialogElement {
  const d = h("dialog", { class: `modal ${cls}` }, ...content);
  d.addEventListener("close", () => window.setTimeout(() => d.remove(), 200));
  // Click on the backdrop closes the dialog.
  d.addEventListener("mousedown", (e) => {
    if (e.target === d) d.close();
  });
  document.body.append(d);
  d.showModal();
  return d;
}

function modalHead(title: string, d: () => HTMLDialogElement, back?: () => void): HTMLElement {
  return h(
    "div",
    { class: "modal-head" },
    back ? h("button", { class: "icon-btn ghost small", "aria-label": "Back", onclick: back }, icon("arrowLeft")) : null,
    h("h2", null, title),
    h("button", { class: "icon-btn ghost small", "aria-label": "Close", onclick: () => d().close() }, icon("close")),
  );
}

function confirmDialog(title: string, message: string, confirmText: string): Promise<boolean> {
  return new Promise((resolve) => {
    let result = false;
    const d: HTMLDialogElement = modal(
      [
        modalHead(title, () => d),
        h("p", { class: "modal-text" }, message),
        h(
          "div",
          { class: "modal-foot" },
          h("button", { class: "btn ghost", onclick: () => d.close() }, "Cancel"),
          h(
            "button",
            {
              class: "btn danger",
              onclick: () => {
                result = true;
                d.close();
              },
            },
            confirmText,
          ),
        ),
      ],
      "narrow",
    );
    d.addEventListener("close", () => resolve(result));
  });
}

function openAccountDialog(existing?: Account) {
  let d!: HTMLDialogElement;
  const container = h("div", { class: "modal-content" });
  d = modal([container]);

  const showPicker = () => {
    const detected = view!.detected;
    container.replaceChildren(
      modalHead("Add an account", () => d),
      h("p", { class: "modal-text" }, "Choose what you want to keep an eye on."),
      h(
        "div",
        { class: "picker" },
        ...PROVIDERS.map((p) => {
          const found =
            (p.kind === "claude_plan" && detected.claude) ||
            (p.kind === "codex_plan" && detected.codex) ||
            (p.kind === "copilot" && detected.copilot) ||
            (p.kind === "antigravity" && detected.antigravity);
          return h(
            "button",
            { class: "pick", onclick: () => showForm(p) },
            avatar(p),
            h(
              "span",
              { class: "pick-text" },
              h("strong", null, p.name, found ? h("span", { class: "pill good" }, "Detected") : null),
              h("small", null, p.tagline),
            ),
          );
        }),
      ),
    );
  };

  const showForm = (p: ProviderInfo) => {
    const isEdit = !!existing;
    const name = h("input", {
      class: "input",
      id: "f-name",
      maxlength: 60,
      value: existing?.label ?? defaultName(p),
      autocomplete: "off",
      spellcheck: "false",
    });

    const secret = h("input", {
      class: "input mono",
      id: "f-secret",
      type: "password",
      autocomplete: "off",
      spellcheck: "false",
      placeholder: isEdit && existing.hasSecret ? "Saved securely. Leave empty to keep it." : (p.secretPlaceholder ?? ""),
    });
    const eye = h("button", { class: "icon-btn ghost small", type: "button", "aria-label": "Show key" }, icon("eye", 16));
    eye.addEventListener("click", () => {
      const show = secret.type === "password";
      secret.type = show ? "text" : "password";
      eye.replaceChildren(icon(show ? "eyeOff" : "eye", 16));
    });
    const secretField = field(p.secretLabel ?? "API key", "f-secret", h("div", { class: "input-wrap" }, secret, eye));

    const budget = h("input", {
      class: "input",
      id: "f-budget",
      type: "number",
      min: 0,
      step: "1",
      inputmode: "decimal",
      placeholder: p.budget?.placeholder ?? "",
      value: existing?.monthlyBudget ?? "",
    });
    const folder = h("input", {
      class: "input mono",
      id: "f-folder",
      placeholder: p.folder?.placeholder ?? "",
      value: existing?.dataPath ?? "",
      spellcheck: "false",
    });

    const result = h("div", { class: "test-result", "aria-live": "polite" });
    const advanced: HTMLElement[] = [];
    if (p.secret === "optional") advanced.push(secretField);
    if (p.folder) advanced.push(field(p.folder.label, "f-folder", folder));

    const collect = (): AccountInput | null => {
      const b = budget.value.trim();
      const monthlyBudget = p.budget && b !== "" ? Number(b) : null;
      if (monthlyBudget !== null && (!Number.isFinite(monthlyBudget) || monthlyBudget < 0)) {
        showResult(result, { ok: false, text: `${p.budget?.label ?? "Budget"} must be a positive number.` });
        return null;
      }
      return {
        kind: p.kind,
        label: name.value,
        monthlyBudget,
        dataPath: folder.value.trim() || null,
        secret: secret.value.trim() || null,
      };
    };

    const testBtn = h("button", { class: "btn ghost", type: "button" }, "Test connection");
    const saveBtn = h("button", { class: "btn primary", type: "submit" }, isEdit ? "Save changes" : "Add account");

    testBtn.addEventListener("click", async () => {
      const input = collect();
      if (!input) return;
      testBtn.disabled = true;
      showResult(result, { ok: null, text: "Checking…" });
      try {
        const s = await api.testAccount(input, existing?.id ?? null);
        showResult(result, summarize(s));
      } catch (e) {
        showResult(result, { ok: false, text: errorText(e) });
      } finally {
        testBtn.disabled = false;
      }
    });

    const form = h(
      "form",
      { class: "form" },
      h("div", { class: "form-provider" }, avatar(p), h("div", null, h("strong", null, p.name), h("small", null, p.tagline))),
      h(
        "div",
        { class: "callout info" },
        icon("lock", 18),
        h(
          "div",
          null,
          h("p", null, p.help),
          p.link
            ? h("button", { class: "link", type: "button", onclick: () => open(p.link!.url) }, p.link.text, icon("external", 13))
            : null,
        ),
      ),
      field("Display name", "f-name", name),
      p.secret === "required" ? secretField : null,
      p.budget
        ? field(
            p.budget.label,
            "f-budget",
            p.budget.prefix
              ? h("div", { class: "input-wrap prefix" }, h("span", { class: "prefix-sym" }, p.budget.prefix), budget)
              : budget,
            p.budget.hint,
          )
        : null,
      advanced.length ? h("details", { class: "advanced" }, h("summary", null, "Advanced"), ...advanced) : null,
      result,
      h("div", { class: "modal-foot" }, testBtn, saveBtn),
    );

    form.addEventListener("submit", async (e) => {
      e.preventDefault();
      const input = collect();
      if (!input) return;
      saveBtn.disabled = true;
      try {
        const acc = isEdit ? await api.updateAccount(existing.id, input) : await api.addAccount(input);
        secret.value = "";
        if (isEdit) view!.accounts = view!.accounts.map((a) => (a.id === acc.id ? acc : a));
        else view!.accounts.push(acc);
        d.close();
        render();
        toast(isEdit ? "Account updated" : `${acc.label} added`);
        void refreshOne(acc.id);
      } catch (err) {
        showResult(result, { ok: false, text: errorText(err) });
      } finally {
        saveBtn.disabled = false;
      }
    });

    container.replaceChildren(modalHead(isEdit ? "Edit account" : `Add ${p.name}`, () => d, isEdit ? undefined : showPicker), form);
    (p.secret === "required" && !isEdit ? secret : name).focus();
  };

  if (existing) showForm(provider(existing.kind));
  else showPicker();
}

function defaultName(p: ProviderInfo): string {
  const base = p.kind === "claude_plan" ? "Claude" : p.kind === "codex_plan" ? "ChatGPT" : p.name;
  const taken = new Set(view?.accounts.map((a) => a.label));
  if (!taken.has(base)) return base;
  for (let i = 2; ; i++) if (!taken.has(`${base} ${i}`)) return `${base} ${i}`;
}

function field(label: string, id: string, control: HTMLElement, hint?: string): HTMLElement {
  return h("div", { class: "field" }, h("label", { for: id }, label), control, hint ? h("small", { class: "hint" }, hint) : null);
}

function summarize(s: Snapshot): { ok: boolean; text: string } {
  if (s.error || !s.usage) return { ok: false, text: s.error ?? "No data returned." };
  const m = s.usage.meters[0];
  if (!m) return { ok: true, text: "Connected." };
  const { value, caption } = headline(m);
  return { ok: true, text: `Connected · ${m.label}: ${value} ${caption}` };
}

function showResult(el: HTMLElement, r: { ok: boolean | null; text: string }) {
  el.className = `test-result ${r.ok === null ? "pending" : r.ok ? "ok" : "bad"}`;
  el.replaceChildren(r.ok === null ? h("span", { class: "spinner" }) : icon(r.ok ? "check" : "alert", 16), h("span", null, r.text));
}

async function openSettings() {
  if (!view) return;
  const s: Settings = { ...view.settings };
  let d!: HTMLDialogElement;

  const save = async () => {
    try {
      view!.settings = await api.saveSettings(s);
      applyTheme(view!.settings.theme);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  const themeSeg = h(
    "div",
    { class: "segmented", role: "radiogroup", "aria-label": "Theme" },
    ...(["system", "light", "dark"] as const).map((t) => {
      const b = h(
        "button",
        { class: `seg${s.theme === t ? " on" : ""}`, role: "radio", "aria-checked": String(s.theme === t), type: "button" },
        t[0]!.toUpperCase() + t.slice(1),
      );
      b.addEventListener("click", () => {
        s.theme = t;
        themeSeg.querySelectorAll(".seg").forEach((x) => {
          x.classList.toggle("on", x === b);
          x.setAttribute("aria-checked", String(x === b));
        });
        void save();
      });
      return b;
    }),
  );

  const interval = h(
    "select",
    { class: "input", id: "s-interval" },
    ...[0, 1, 2, 5, 10, 15, 30, 60].map((m) =>
      h("option", { value: m, selected: s.refreshMinutes === m }, m === 0 ? "Off" : m === 1 ? "Every minute" : `Every ${m} minutes`),
    ),
  );
  interval.addEventListener("change", () => {
    s.refreshMinutes = Number(interval.value);
    void save();
  });

  const trayToggle = toggle(s.closeToTray, (v) => {
    s.closeToTray = v;
    void save();
  });

  let autostart = false;
  try {
    autostart = await autostartEnabled();
  } catch {
    /* not available in dev */
  }
  const autostartToggle = toggle(autostart, async (v) => {
    try {
      if (v) await enableAutostart();
      else await disableAutostart();
    } catch (e) {
      toast(errorText(e), "error");
    }
  });

  d = modal([
    h(
      "div",
      { class: "modal-content" },
      modalHead("Settings", () => d),
      h(
        "div",
        { class: "settings" },
        settingRow("Theme", null, themeSeg),
        settingRow("Auto-refresh", "How often to check all accounts.", interval),
        settingRow("Keep running in the tray", "Closing the window hides it to the system tray.", trayToggle),
        settingRow("Start with Windows", "Launch minimized to the tray when you sign in.", autostartToggle),
      ),
      widgetSettings(s, save),
      h(
        "div",
        { class: "callout info" },
        icon("lock", 18),
        h(
          "div",
          null,
          h(
            "p",
            null,
            "API keys are encrypted in Windows Credential Manager and are only ever sent to their own provider. The interface never sees them after you save.",
          ),
        ),
      ),
      h(
        "div",
        { class: "modal-foot spread" },
        h("small", { class: "muted" }, `UsageCheck ${view.version}`),
        h("button", { class: "btn ghost", onclick: () => api.quit() }, "Quit app"),
      ),
    ),
  ]);
}

/** "Taskbar widget": on/off, and which two accounts (and gauges) it shows. */
function widgetSettings(s: Settings, save: () => Promise<void>): HTMLElement {
  const accounts = view!.accounts;
  const slotRows = h("div", { class: "slot-rows" });

  const setSlots = (slots: (WidgetSlot | null)[]) => {
    s.widget = { ...s.widget, slots: slots.filter((x): x is WidgetSlot => !!x && !!x.accountId) };
    void save();
  };

  const drawSlots = () => {
    const current: (WidgetSlot | null)[] = [s.widget.slots[0] ?? null, s.widget.slots[1] ?? null];
    slotRows.replaceChildren(
      ...current.map((slot, i) => {
        const acc = h(
          "select",
          { class: "input", "aria-label": `Slot ${i + 1} account` },
          h("option", { value: "", selected: !slot }, "Nothing"),
          ...accounts.map((a) => h("option", { value: a.id, selected: slot?.accountId === a.id }, a.label)),
        );
        const labels = slot ? (snaps.get(slot.accountId)?.usage?.meters ?? []).map((m) => m.label) : [];
        if (slot?.meter && !labels.includes(slot.meter)) labels.push(slot.meter);
        const meter = h(
          "select",
          { class: "input", "aria-label": `Slot ${i + 1} gauge`, disabled: !slot },
          h("option", { value: "", selected: !slot?.meter }, "Tightest limit"),
          ...labels.map((l) => h("option", { value: l, selected: slot?.meter === l }, l)),
        );
        acc.addEventListener("change", () => {
          current[i] = acc.value ? { accountId: acc.value, meter: null } : null;
          setSlots(current);
          drawSlots();
        });
        meter.addEventListener("change", () => {
          if (!current[i]) return;
          current[i] = { ...current[i]!, meter: meter.value || null };
          setSlots(current);
        });
        return h(
          "div",
          { class: "setting" },
          h("div", null, h("div", { class: "setting-title" }, `Slot ${i + 1}`)),
          h("div", { class: "slot-pickers" }, acc, meter),
        );
      }),
    );
  };

  const enabled = toggle(s.widget.enabled, (v) => {
    s.widget = { ...s.widget, enabled: v };
    if (v && s.widget.slots.length === 0) {
      s.widget.slots = accounts.slice(0, 2).map((a) => ({ accountId: a.id, meter: null }));
    }
    slotRows.hidden = !v;
    drawSlots();
    void save();
  });
  slotRows.hidden = !s.widget.enabled;
  drawSlots();

  return h(
    "div",
    { class: "settings-group" },
    h("div", { class: "group-title" }, "Taskbar widget"),
    h(
      "div",
      { class: "settings" },
      settingRow(
        "Show on the taskbar",
        "Up to two accounts next to the clock: icon and how much is left. Drag it to move it; click it to open UsageCheck.",
        enabled,
      ),
      slotRows,
      settingRow(
        "Position",
        "Put the widget back next to the clock after moving it.",
        h(
          "button",
          {
            class: "btn small",
            type: "button",
            onclick: () => api.widgetResetPosition().catch((e) => toast(errorText(e), "error")),
          },
          "Dock to clock",
        ),
      ),
    ),
  );
}

function settingRow(title: string, hint: string | null, control: HTMLElement): HTMLElement {
  return h(
    "div",
    { class: "setting" },
    h("div", null, h("div", { class: "setting-title" }, title), hint ? h("small", { class: "hint" }, hint) : null),
    control,
  );
}

function toggle(initial: boolean, onChange: (v: boolean) => void): HTMLElement {
  const input = h("input", { type: "checkbox", checked: initial });
  input.addEventListener("change", () => onChange(input.checked));
  return h("label", { class: "switch" }, input, h("span", { class: "track" }, h("span", { class: "thumb" })));
}

// ---------------------------------------------------------------- boot

document.addEventListener("click", (e) => {
  if (openMenuEl && !openMenuEl.contains(e.target as Node)) closeMenu();
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") closeMenu();
  const mod = e.ctrlKey || e.metaKey;
  if (e.key === "F5" || (mod && e.key.toLowerCase() === "r")) {
    e.preventDefault();
    void refreshAll();
  } else if (mod && e.key.toLowerCase() === "n" && !document.querySelector("dialog[open]")) {
    e.preventDefault();
    openAccountDialog();
  }
});
// Native-app feel: no browser context menu outside text fields.
document.addEventListener("contextmenu", (e) => {
  const t = e.target as HTMLElement;
  if (!t.closest("input, textarea")) e.preventDefault();
});
window.addEventListener("resize", closeMenu);
// Coming back to the window (e.g. from the tray) refreshes anything older than a minute.
function refreshStale() {
  if (!view || document.visibilityState !== "visible") return;
  const cutoff = Date.now() - 60_000;
  for (const a of view.accounts) {
    const s = snaps.get(a.id);
    if (!s || Date.parse(s.fetchedAt) < cutoff) void refreshOne(a.id);
  }
}
window.addEventListener("focus", refreshStale);
document.addEventListener("visibilitychange", refreshStale);

/** Keeps whichever snapshot is newer (events can race the initial state load). */
function mergeSnapshot(s: Snapshot) {
  const cur = snaps.get(s.accountId);
  if (!cur || Date.parse(s.fetchedAt) >= Date.parse(cur.fetchedAt)) snaps.set(s.accountId, s);
}

async function boot() {
  // The backend refreshes on its own schedule (so the tray and taskbar widget stay
  // current while this window is hidden) and tells us about every result. Listen
  // before loading state so nothing that finishes in between is missed.
  await listen<Snapshot>("snapshot", (e) => {
    mergeSnapshot(e.payload);
    render();
  });
  await listen<{ accountId: string; busy: boolean }>("refresh-state", (e) => {
    if (e.payload.busy) busy.add(e.payload.accountId);
    else busy.delete(e.payload.accountId);
    render();
  });
  await listen<Settings>("settings-changed", (e) => {
    if (!view) return;
    view.settings = e.payload;
    applyTheme(view.settings.theme);
  });

  try {
    view = await api.getState();
  } catch (e) {
    root.replaceChildren(h("main", { class: "empty" }, h("h2", null, "UsageCheck could not start"), h("p", null, errorText(e))));
    return;
  }
  for (const s of view.snapshots) mergeSnapshot(s);
  applyTheme(view.settings.theme);
  render();
  // Keep "updated 3m ago" and reset countdowns current.
  window.setInterval(() => {
    if (!openMenuEl) render();
  }, 30_000);
}

void boot();
