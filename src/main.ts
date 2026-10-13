import { listen } from "@tauri-apps/api/event";
import { disable as disableAutostart, enable as enableAutostart, isEnabled as autostartEnabled } from "@tauri-apps/plugin-autostart";
import { openUrl } from "@tauri-apps/plugin-opener";

import {
  api,
  errorText,
  type Account,
  type AccountInput,
  type AppView,
  type ProviderKind,
  type Settings,
  type Snapshot,
  type UpdateStatus,
  type WidgetSlot,
} from "./api";
import { h, icon, logo } from "./dom";
import { ago, headline, tickTimes } from "./format";
import { meter } from "./meterview";
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

async function saveOrder(ids: string[]) {
  if (!view) return;
  const byId = new Map(view.accounts.map((a) => [a.id, a]));
  view.accounts = ids.map((id) => byId.get(id)).filter((a): a is Account => !!a);
  try {
    await api.reorderAccounts(ids);
  } catch (e) {
    toast(errorText(e), "error");
  }
}

/**
 * Drag a card by any non-interactive part of it to reorder the grid. A floating
 * copy follows the pointer while the other cards slide into place (FLIP).
 */
function startCardDrag(e: PointerEvent) {
  const card = (e.currentTarget as HTMLElement | null)?.closest<HTMLElement>(".card");
  if (!card || e.button !== 0 || dragging) return;
  if ((e.target as HTMLElement).closest("button, a, input, select, textarea, .callout p")) return;
  const grid = card.parentElement!;
  const startX = e.clientX;
  const startY = e.clientY;
  let ghost: HTMLElement | null = null;
  let offX = 0;
  let offY = 0;

  const cards = () => [...grid.querySelectorAll<HTMLElement>(".card[data-id]")];

  /** Animates every card from where it was to where the layout now puts it. */
  const flip = (mutate: () => void) => {
    const before = new Map(cards().map((c) => [c, c.getBoundingClientRect()]));
    mutate();
    for (const c of cards()) {
      const a = before.get(c);
      if (!a || c === card) continue;
      const b = c.getBoundingClientRect();
      const dx = a.left - b.left;
      const dy = a.top - b.top;
      if (!dx && !dy) continue;
      c.animate([{ transform: `translate(${dx}px, ${dy}px)` }, { transform: "none" }], {
        duration: 180,
        easing: "cubic-bezier(0.2, 0.8, 0.2, 1)",
      });
    }
  };

  const begin = () => {
    dragging = true;
    closeMenu();
    const r = card.getBoundingClientRect();
    offX = startX - r.left;
    offY = startY - r.top;
    ghost = card.cloneNode(true) as HTMLElement;
    ghost.classList.add("drag-ghost");
    ghost.classList.remove("enter");
    Object.assign(ghost.style, { width: `${r.width}px`, height: `${r.height}px`, left: "0", top: "0" });
    document.body.append(ghost);
    card.classList.add("placeholder");
    document.body.classList.add("dragging");
  };

  const move = (ev: PointerEvent) => {
    if (!ghost) {
      if (Math.hypot(ev.clientX - startX, ev.clientY - startY) < 6) return;
      begin();
    }
    ghost!.style.transform = `translate(${ev.clientX - offX}px, ${ev.clientY - offY}px) rotate(1deg) scale(1.02)`;
    // Hit-test against where cards are laid out, not where their slide
    // animation currently draws them, so the order can't flip back and forth.
    const origin = grid.getBoundingClientRect();
    const px = ev.clientX - origin.left;
    const py = ev.clientY - origin.top;
    const over = cards().find((c) => {
      if (c === card) return false;
      const x = c.offsetLeft - grid.offsetLeft;
      const y = c.offsetTop - grid.offsetTop;
      return px >= x && px <= x + c.offsetWidth && py >= y && py <= y + c.offsetHeight;
    });
    if (!over) return;
    const list = cards();
    const before = list.indexOf(card) > list.indexOf(over);
    flip(() => (before ? over.before(card) : over.after(card)));
  };

  const end = () => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", end);
    window.removeEventListener("pointercancel", end);
    if (!ghost) return; // it was a click, not a drag
    const r = card.getBoundingClientRect();
    const g = ghost;
    g.animate([{ transform: g.style.transform }, { transform: `translate(${r.left}px, ${r.top}px)` }], {
      duration: 150,
      easing: "ease-out",
    }).onfinish = () => {
      g.remove();
      card.classList.remove("placeholder");
      document.body.classList.remove("dragging");
      dragging = false;
      void saveOrder(cards().map((c) => c.dataset.id!));
      if (renderPending) {
        renderPending = false;
        render();
      }
    };
  };

  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", end);
  window.addEventListener("pointercancel", end);
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

/** Cards that have already been on screen (they only slide in the first time). */
const shown = new Set<string>();

/** Set while a card is being dragged; redraws wait until it's dropped. */
let dragging = false;
let renderPending = false;

function render() {
  if (!view) return;
  if (dragging) {
    renderPending = true;
    return;
  }
  closeMenu();
  root.replaceChildren(topbar(), view.accounts.length ? grid() : emptyState());
  for (const a of view.accounts) shown.add(a.id);
}

function topbar(): HTMLElement {
  const accounts = view!.accounts;
  const times = accounts.map((a) => snaps.get(a.id)?.fetchedAt).filter((t): t is string => !!t);
  const oldest = times.sort()[0];
  const failing = accounts.filter((a) => snaps.get(a.id)?.error).length;
  const count = accounts.length ? `${accounts.length} account${accounts.length === 1 ? "" : "s"}` : "";
  const anyBusy = busy.size > 0;

  return h(
    "header",
    { class: "topbar" },
    h(
      "div",
      { class: "brand" },
      h("img", { class: "logo", src: "/logo.svg", alt: "" }),
      h(
        "div",
        null,
        h("h1", null, "Quota"),
        h(
          "p",
          { class: "sub" },
          count || "All your AI limits in one place",
          oldest
            ? h("span", { "data-ago": oldest, "data-prefix": " · updated " }, ` · updated ${ago(oldest)}`)
            : null,
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
  if (d.copilot)
    quick.push(
      quickAdd("copilot", "GitHub Copilot", d.copilotNeedsSignIn ? "Copilot found (sign in once with GitHub)" : "Copilot found"),
    );
  if (d.antigravity) quick.push(quickAdd("antigravity", "Antigravity", "Antigravity found"));
  if (d.cursor) quick.push(quickAdd("cursor", "Cursor", d.cursorNeedsSignIn ? "Cursor found (run cursor-agent login once)" : "Cursor found"));
  return h(
    "main",
    { class: "empty" },
    h("div", { class: "empty-art" }, h("img", { src: "/logo.svg", alt: "" })),
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
  return h("div", { class: "avatar", style: { "--accent": p.color }, title: p.name }, logo(p.kind, 18));
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
    {
      class: `card${isBusy ? " busy" : ""}${shown.has(a.id) ? "" : " enter"}`,
      style: { "--accent": p.color },
      "data-id": a.id,
      onpointerdown: (e: PointerEvent) => startCardDrag(e),
    },
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
      isBusy || !snap
        ? h("span", null, isBusy ? "Refreshing…" : "Waiting…")
        : h("span", { "data-ago": snap.fetchedAt, "data-prefix": "Updated " }, `Updated ${ago(snap.fetchedAt)}`),
      h(
        "button",
        { class: "link", onclick: () => refreshOne(a.id), disabled: isBusy },
        "Refresh",
      ),
    ),
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
  document.querySelectorAll('.dd[aria-expanded="true"]').forEach((b) => b.setAttribute("aria-expanded", "false"));
}

interface Choice {
  value: string;
  label: string;
}

/**
 * A dropdown styled like the rest of the app. Native <select> lists are drawn
 * by Windows and can't be themed, so this renders its own list. Keyboard:
 * Enter/Space/arrows open it, arrows move, Enter picks, Escape closes.
 */
function dropdown(choices: Choice[], value: string, onChange: (v: string) => void, label: string): HTMLButtonElement {
  let current = value;
  const text = h("span", { class: "dd-text" }, choices.find((c) => c.value === value)?.label ?? "");
  const btn = h(
    "button",
    { class: "input dd", type: "button", "aria-haspopup": "listbox", "aria-expanded": "false", "aria-label": label },
    text,
  );

  const close = (refocus: boolean) => {
    closeMenu();
    btn.setAttribute("aria-expanded", "false");
    if (refocus) btn.focus();
  };

  const pick = (c: Choice) => {
    const changed = c.value !== current;
    current = c.value;
    text.textContent = c.label;
    close(true);
    if (changed) onChange(c.value);
  };

  const open = () => {
    closeMenu();
    const options = choices.map((c) => {
      const selected = c.value === current;
      const opt = h(
        "button",
        {
          class: `menu-item option${selected ? " selected" : ""}`,
          type: "button",
          role: "option",
          "aria-selected": String(selected),
        },
        h("span", { class: "option-label" }, c.label),
        selected ? icon("check", 14) : null,
      );
      opt.addEventListener("click", (e) => {
        e.stopPropagation();
        pick(c);
      });
      return opt;
    });
    const list = h("div", { class: "menu listbox", role: "listbox", "aria-label": label }, ...options);
    list.addEventListener("keydown", (e) => {
      const i = options.indexOf(document.activeElement as HTMLButtonElement);
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        const next = e.key === "ArrowDown" ? Math.min(options.length - 1, i + 1) : Math.max(0, i - 1);
        options[next]?.focus();
      } else if (e.key === "Home" || e.key === "End") {
        e.preventDefault();
        options[e.key === "Home" ? 0 : options.length - 1]?.focus();
      } else if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation(); // don't close the dialog too
        close(true);
      } else if (e.key === "Tab") {
        close(false);
      }
    });
    // Inside a modal dialog the list must live in the dialog to appear above it.
    (btn.closest("dialog") ?? document.body).append(list);
    const r = btn.getBoundingClientRect();
    list.style.minWidth = `${r.width}px`;
    const lh = list.offsetHeight;
    const below = r.bottom + 4 + lh <= window.innerHeight - 8;
    list.style.left = `${Math.max(8, Math.min(r.left, window.innerWidth - list.offsetWidth - 8))}px`;
    list.style.top = `${below ? r.bottom + 4 : Math.max(8, r.top - lh - 4)}px`;
    openMenuEl = list;
    btn.setAttribute("aria-expanded", "true");
    (options.find((o) => o.classList.contains("selected")) ?? options[0])?.focus();
  };

  btn.addEventListener("click", (e) => {
    e.stopPropagation();
    if (btn.getAttribute("aria-expanded") === "true") close(false);
    else open();
  });
  btn.addEventListener("keydown", (e) => {
    if (["ArrowDown", "ArrowUp", "Enter", " "].includes(e.key)) {
      e.preventDefault();
      open();
    }
  });
  return btn;
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
            (p.kind === "antigravity" && detected.antigravity) ||
            (p.kind === "cursor" && detected.cursor);
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
      p.kind === "copilot" ? githubSignIn(result) : null,
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

/** One-time GitHub CLI sign-in, for Copilot in VS Code. */
function githubSignIn(result: HTMLElement): HTMLElement {
  const needs = view!.detected.copilotNeedsSignIn;
  return h(
    "div",
    { class: "signin-row" },
    h(
      "div",
      null,
      h("strong", null, needs ? "Sign in once with GitHub" : "GitHub sign-in"),
      h(
        "small",
        { class: "hint" },
        needs
          ? "Opens GitHub's official sign-in in your browser. Use the same account as Copilot in VS Code."
          : "Already signed in. Use this to switch GitHub accounts.",
      ),
    ),
    h(
      "button",
      {
        class: `btn small${needs ? " primary" : ""}`,
        type: "button",
        onclick: async () => {
          try {
            await api.githubSignIn();
            showResult(result, { ok: null, text: "Finish signing in in the window that opened, then click Test connection." });
          } catch (e) {
            showResult(result, { ok: false, text: errorText(e) });
          }
        },
      },
      "Sign in with GitHub",
    ),
  );
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

  const interval = dropdown(
    [0, 1, 2, 3, 4, 5, 10, 15, 30, 60].map((m) => ({
      value: String(m),
      label: m === 0 ? "Off" : m === 1 ? "Every minute" : `Every ${m} minutes`,
    })),
    String(s.refreshMinutes),
    (v) => {
      s.refreshMinutes = Number(v);
      void save();
    },
    "Auto-refresh",
  );


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
      await updateSettings(),
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
        h("small", { class: "muted" }, `Quota ${view.version}`),
        h("button", { class: "btn ghost", onclick: () => api.quit() }, "Quit app"),
      ),
    ),
  ]);
}

function updateText(u: UpdateStatus): string {
  switch (u.state) {
    case "checking":
      return "Checking for updates…";
    case "upToDate":
      return `You're on the latest version (${u.currentVersion})${u.checkedAt ? `, checked ${ago(u.checkedAt)}` : ""}.`;
    case "available":
      return `Version ${u.newVersion} is ready. ${u.message ?? ""}`.trim();
    case "downloading":
      return `Downloading version ${u.newVersion}…`;
    case "installing":
      return `Installing version ${u.newVersion}. Quota will restart.`;
    case "error":
      return u.message ?? "Couldn't check for updates.";
    default:
      return `You're on the latest version (${u.currentVersion}).`;
  }
}

/** "Updates": always automatic; shows the status and a Check now button. */
async function updateSettings(): Promise<HTMLElement> {
  let status: UpdateStatus | null = null;
  try {
    status = await api.updateStatus();
  } catch {
    /* not available in dev */
  }
  const line = h("div", { class: "update-line", "aria-live": "polite" }, status ? updateText(status) : "");
  updateLine = line;
  const checkBtn = h(
    "button",
    {
      class: "btn small",
      type: "button",
      onclick: async () => {
        checkBtn.disabled = true;
        try {
          line.textContent = updateText(await api.checkForUpdates());
        } catch (e) {
          line.textContent = errorText(e);
        } finally {
          checkBtn.disabled = false;
        }
      },
    },
    "Check now",
  );
  return h(
    "div",
    { class: "settings-group" },
    h("div", { class: "group-title" }, "Updates"),
    h(
      "div",
      { class: "settings" },
      h(
        "div",
        { class: "setting" },
        h("div", null, line),
        checkBtn,
      ),
    ),
  );
}

/** The open Settings dialog's update status line, kept live by "update-status" events. */
let updateLine: HTMLElement | null = null;

/** "Taskbar widget": on/off, and which two accounts (and gauges) it shows. */
const MAX_SLOTS = 8;

function widgetSettings(s: Settings, save: () => Promise<void>): HTMLElement {
  const accounts = view!.accounts;
  const slotRows = h("div", { class: "slot-rows" });

  const setSlots = (slots: WidgetSlot[]) => {
    s.widget = { ...s.widget, slots: slots.filter((x) => !!x.accountId) };
    void save();
    drawSlots();
  };

  const drawSlots = () => {
    const current = [...s.widget.slots];
    const rows = current.map((slot, i) => {
      const acc = dropdown(
        accounts.map((a) => ({ value: a.id, label: a.label })),
        slot.accountId,
        (v) => {
          current[i] = { accountId: v, meter: null };
          setSlots(current);
        },
        `Slot ${i + 1} account`,
      );
      const labels = (snaps.get(slot.accountId)?.usage?.meters ?? []).map((m) => m.label);
      if (slot.meter && !labels.includes(slot.meter)) labels.push(slot.meter);
      const meter = dropdown(
        [{ value: "", label: "Tightest limit" }, ...labels.map((l) => ({ value: l, label: l }))],
        slot.meter ?? "",
        (v) => {
          current[i] = { ...current[i]!, meter: v || null };
          setSlots(current);
        },
        `Slot ${i + 1} gauge`,
      );
      const remove = h(
        "button",
        { class: "icon-btn ghost small", type: "button", "aria-label": `Remove slot ${i + 1}`, title: "Remove" },
        icon("close", 16),
      );
      remove.addEventListener("click", () => setSlots(current.filter((_, j) => j !== i)));
      return h(
        "div",
        { class: "setting" },
        h("div", null, h("div", { class: "setting-title" }, `Slot ${i + 1}`)),
        h("div", { class: "slot-pickers" }, acc, meter, remove),
      );
    });

    const canAdd = accounts.length > 0 && current.length < MAX_SLOTS;
    const add = h(
      "button",
      { class: "btn small", type: "button", disabled: !canAdd },
      icon("plus", 14),
      "Add slot",
    );
    add.addEventListener("click", () => {
      // Suggest the first account not shown yet.
      const used = new Set(current.map((x) => x.accountId));
      const next = accounts.find((a) => !used.has(a.id)) ?? accounts[0];
      if (next) setSlots([...current, { accountId: next.id, meter: null }]);
    });
    slotRows.replaceChildren(
      ...rows,
      h(
        "div",
        { class: "setting" },
        h(
          "div",
          null,
          h("div", { class: "setting-title" }, current.length ? `${current.length} shown` : "Nothing shown yet"),
          h("small", { class: "hint" }, `Show as many as you like (up to ${MAX_SLOTS}). The same account can appear twice with different gauges.`),
        ),
        add,
      ),
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
        "Your chosen accounts next to the clock: icon and how much is left. Hover for details, drag to move, click to open Quota.",
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
// No refresh-on-focus here: the backend refreshes on the schedule in Settings
// (and paces requests), so opening or clicking the window never re-fetches.

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
  await listen<UpdateStatus>("update-status", (e) => {
    if (updateLine?.isConnected) updateLine.textContent = updateText(e.payload);
  });
  await listen<Settings>("settings-changed", (e) => {
    if (!view) return;
    view.settings = e.payload;
    applyTheme(view.settings.theme);
  });

  try {
    view = await api.getState();
  } catch (e) {
    root.replaceChildren(h("main", { class: "empty" }, h("h2", null, "Quota could not start"), h("p", null, errorText(e))));
    return;
  }
  for (const s of view.snapshots) mergeSnapshot(s);
  applyTheme(view.settings.theme);
  render();
  // Keep "updated 3m ago" and reset countdowns current by updating just those
  // labels; redrawing the cards would make the page jump.
  window.setInterval(() => tickTimes(), 30_000);
}

void boot();
