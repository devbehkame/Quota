import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import type { ProviderKind, Settings, Snapshot } from "./api";
import { h, icon, logo } from "./dom";
import { ago, tickTimes } from "./format";
import { meter, resetsRow } from "./meterview";
import { provider } from "./providers";

interface CardView {
  account: { id: string; kind: ProviderKind; label: string } | null;
  snapshot: Snapshot | null;
  theme: Settings["theme"];
}

const root = document.getElementById("card")!;
let currentId: string | null = null;
let view: CardView | null = null;

function applyTheme(theme: Settings["theme"]) {
  if (theme === "system") delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = theme;
}

function render() {
  if (!view?.account) return;
  const a = view.account;
  const p = provider(a.kind);
  const snap = view.snapshot;
  const usage = snap?.usage ?? null;
  applyTheme(view.theme);
  root.style.setProperty("--accent", p.color);

  let body: HTMLElement;
  if (!snap) {
    body = h("div", { class: "card-body" }, h("p", { class: "muted" }, "Checking…"));
  } else if (snap.error && !usage) {
    body = h("div", { class: "card-body" }, h("div", { class: "callout danger" }, icon("alert", 18), h("div", null, h("p", null, snap.error))));
  } else {
    body = h(
      "div",
      { class: "card-body" },
      ...(usage?.meters ?? []).map(meter),
      usage?.resets ? resetsRow(usage.resets, true) : null,
      usage?.notes.length ? h("ul", { class: "notes" }, ...usage.notes.map((n) => h("li", null, n))) : null,
    );
  }

  root.replaceChildren(
    h(
      "div",
      { class: "card-head" },
      h("div", { class: "avatar", style: { "--accent": p.color } }, logo(a.kind, 18)),
      h(
        "div",
        { class: "titles" },
        h(
          "div",
          { class: "title-row" },
          h("div", { class: "title" }, a.label),
          usage?.signedInAs ? h("div", { class: "who", title: usage.signedInAs }, usage.signedInAs) : null,
        ),
        h("div", { class: "subtitle" }, p.name, usage?.plan ? h("span", { class: "pill" }, usage.plan) : null),
      ),
    ),
    body,
    h(
      "footer",
      { class: "card-foot" },
      icon("clock", 14),
      snap
        ? h("span", { "data-ago": snap.fetchedAt, "data-prefix": "Updated " }, `Updated ${ago(snap.fetchedAt)}`)
        : h("span", null, "Waiting…"),
      h("span", { class: "muted" }, "Click to open"),
    ),
  );

  // Report the full size (card plus the body's shadow padding) so the backend
  // can size, place and show the window.
  const r = root.getBoundingClientRect();
  void invoke("card_layout", { width: r.width + 20, height: r.height + 20 });
}

async function load() {
  const id = currentId;
  if (!id) return;
  const next = await invoke<CardView>("card_state", { accountId: id });
  // Moving quickly between items: ignore an answer for an item already left.
  if (id !== currentId) return;
  view = next;
  render();
}

root.addEventListener("mouseenter", () => void invoke("card_hover", { inside: true }));
root.addEventListener("mouseleave", () => void invoke("card_hover", { inside: false }));
root.addEventListener("click", () => void invoke("widget_open_main"));
document.addEventListener("contextmenu", (e) => e.preventDefault());

async function start() {
  await listen<string>("card-show", (e) => {
    currentId = e.payload;
    void load();
  });
  await listen<Snapshot>("snapshot", (e) => {
    if (view && e.payload.accountId === currentId) {
      view.snapshot = e.payload;
      render();
    }
  });
  await listen<Settings>("settings-changed", (e) => {
    if (view) {
      view.theme = e.payload.theme;
      render();
    }
  });
}

window.setInterval(() => tickTimes(root), 30_000);

void start();
