import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

import type { ProviderKind, Snapshot } from "./api";
import { h, logo } from "./dom";
import { compact, leftPct, level, pickMeter, resetLabel, until } from "./format";
import { provider } from "./providers";

interface WidgetItem {
  accountId: string;
  kind: ProviderKind;
  label: string;
  meter: string | null;
}

interface WidgetView {
  items: WidgetItem[];
  snapshots: Snapshot[];
  lightTaskbar: boolean;
}

const root = document.getElementById("widget")!;
let view: WidgetView | null = null;
const snaps = new Map<string, Snapshot>();
let lastSize = "";

function item(it: WidgetItem): HTMLElement {
  const p = provider(it.kind);
  const snap = snaps.get(it.accountId);
  const m = snap?.usage ? pickMeter(snap.usage.meters, it.meter) : null;

  let text = "…";
  let cls = "lvl-muted";
  let tip = `${it.label}: loading…`;
  let fill: number | null = null;
  if (snap?.error && !snap.usage) {
    text = "!";
    cls = "lvl-danger";
    tip = `${it.label}: ${snap.error}`;
  } else if (m) {
    text = compact(m);
    const lvl = level(m);
    cls = lvl === "neutral" ? "" : `lvl-${lvl}`;
    fill = leftPct(m);
    tip = `${it.label} · ${m.label}: ${text} left`;
    if (m.resetsAt) tip += ` · resets ${until(m.resetsAt)} (${resetLabel(m.resetsAt)})`;
  }

  const el = h(
    "div",
    { class: `item ${cls}`, "aria-label": tip },
    h("span", { class: "icon", style: { "--accent": p.color } }, logo(it.kind, 13)),
    h(
      "span",
      { class: "value" },
      h("span", { class: "num" }, text),
      fill !== null ? h("span", { class: "mini" }, h("span", { style: { width: `${fill}%` } })) : null,
    ),
  );
  // Hovering shows the full details card above this item.
  el.addEventListener("mouseenter", () => {
    void invoke("widget_hover", { accountId: it.accountId, offsetX: el.getBoundingClientRect().left });
  });
  return el;
}

function render() {
  if (!view) return;
  document.documentElement.classList.toggle("light", view.lightTaskbar);
  root.replaceChildren(
    ...(view.items.length ? view.items.map(item) : [h("span", { class: "empty" }, "UsageCheck: add an account")]),
  );
  // Tell the backend how big we are so it can size and dock the window.
  const r = root.getBoundingClientRect();
  const size = `${Math.ceil(r.width)}x${Math.ceil(r.height)}`;
  if (size !== lastSize) {
    lastSize = size;
    void invoke("widget_layout", { width: r.width, height: r.height });
  }
}

function merge(s: Snapshot) {
  const cur = snaps.get(s.accountId);
  if (!cur || Date.parse(s.fetchedAt) >= Date.parse(cur.fetchedAt)) snaps.set(s.accountId, s);
}

async function load() {
  view = await invoke<WidgetView>("widget_state");
  for (const s of view.snapshots) merge(s);
  render();
}

// Click opens the app; press-and-drag moves the widget (its position is remembered).
let down: { x: number; y: number } | null = null;
root.addEventListener("mousedown", (e) => {
  if (e.button === 0) down = { x: e.screenX, y: e.screenY };
});
window.addEventListener("mousemove", (e) => {
  if (down && Math.hypot(e.screenX - down.x, e.screenY - down.y) > 4) {
    down = null;
    void invoke("widget_unhover");
    void getCurrentWindow().startDragging();
  }
});
window.addEventListener("mouseup", () => {
  if (down) void invoke("widget_open_main");
  down = null;
});
document.addEventListener("contextmenu", (e) => e.preventDefault());
document.documentElement.addEventListener("mouseleave", () => void invoke("widget_unhover"));

async function start() {
  await listen<Snapshot>("snapshot", (e) => {
    merge(e.payload);
    if (view?.items.some((i) => i.accountId === e.payload.accountId)) render();
  });
  await listen("widget-update", () => void load());
  await listen("settings-changed", () => void load());
  await load();
}

void start();
