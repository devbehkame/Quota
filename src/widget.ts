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

/** What one slot shows; slots are only redrawn when this changes. */
interface Look {
  text: string;
  cls: string;
  tip: string;
  fill: number | null;
}

function look(it: WidgetItem): Look {
  const snap = snaps.get(it.accountId);
  const m = snap?.usage ? pickMeter(snap.usage.meters, it.meter) : null;
  if (snap?.error && !snap.usage) return { text: "!", cls: "lvl-danger", tip: `${it.label}: ${snap.error}`, fill: null };
  if (!m) return { text: "…", cls: "lvl-muted", tip: `${it.label}: loading…`, fill: null };
  const text = compact(m);
  const lvl = level(m);
  let tip = `${it.label} · ${m.label}: ${text} left`;
  if (m.resetsAt) tip += ` · resets ${until(m.resetsAt)} (${resetLabel(m.resetsAt)})`;
  const fill = leftPct(m);
  return { text, cls: lvl === "neutral" ? "" : `lvl-${lvl}`, tip, fill: fill === null ? null : Math.round(fill) };
}

/** The elements of one slot, kept so updates change text and colour in place. */
interface SlotEls {
  el: HTMLElement;
  num: HTMLElement;
  mini: HTMLElement;
  bar: HTMLElement;
}

function slot(it: WidgetItem): SlotEls {
  const p = provider(it.kind);
  const num = h("span", { class: "num" });
  const bar = h("span");
  const mini = h("span", { class: "mini" }, bar);
  const el = h(
    "div",
    { class: "item" },
    h("span", { class: "icon", style: { "--accent": p.color } }, logo(it.kind, 13)),
    h("span", { class: "value" }, num, mini),
  );
  // Hovering shows the full details card above this item.
  el.addEventListener("mouseenter", () => {
    void invoke("widget_hover", { accountId: it.accountId, offsetX: el.getBoundingClientRect().left });
  });
  return { el, num, mini, bar };
}

function paint(s: SlotEls, l: Look) {
  s.el.className = `item ${l.cls}`.trim();
  s.el.setAttribute("aria-label", l.tip);
  s.num.textContent = l.text;
  // The bar keeps its space when empty so the widget never changes height.
  s.mini.style.visibility = l.fill === null ? "hidden" : "";
  s.bar.style.width = `${l.fill ?? 0}%`;
}

let slots: SlotEls[] = [];
let shapeKey = "";
let looksKey = "";

function render() {
  if (!view) return;
  document.documentElement.classList.toggle("light", view.lightTaskbar);

  // Rebuild the elements only when the chosen slots change; refreshes just repaint.
  const shape = JSON.stringify(view.items.map((i) => [i.accountId, i.kind]));
  if (shape !== shapeKey) {
    shapeKey = shape;
    looksKey = "";
    slots = view.items.map(slot);
    root.replaceChildren(
      ...(slots.length ? slots.map((s) => s.el) : [h("span", { class: "empty" }, "UsageCheck: pick accounts in Settings")]),
    );
  }
  const looks = view.items.map(look);
  const key = JSON.stringify(looks);
  if (key === looksKey) return;
  looksKey = key;
  slots.forEach((s, i) => paint(s, looks[i]!));

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
