import type { LimitResets, Meter, TokenUsage, WeeklyShare } from "./api";
import { h } from "./dom";
import { dateLabel, headline, leftPct, level, money, pct, resetLabel, tokens, until } from "./format";

/** One gauge as drawn on an account card (main window and widget hover card). */
export function meter(m: Meter): HTMLElement {
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
        ? h(
            "span",
            { class: "meter-reset", title: `Resets ${resetLabel(m.resetsAt)}`, "data-until": m.resetsAt, "data-prefix": "Resets " },
            `Resets ${until(m.resetsAt)}`,
          )
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

/** One line per reset, soonest first, up to `max`: what it is (bold) and when it expires. */
function resetList(r: LimitResets, max: number): HTMLElement {
  if (!r.items.length) {
    const msg = r.available
      ? "The expiry dates weren't reported."
      : "None right now. When you get one, it shows here with its expiry date.";
    return h("p", { class: "reset-empty" }, msg);
  }
  const more = r.items.length - max;
  return h(
    "ul",
    { class: "reset-list" },
    ...r.items.slice(0, max).map((i) =>
      h(
        "li",
        null,
        h("span", { class: "reset-name" }, i.label ?? "Limit reset", i.count > 1 ? h("span", { class: "reset-times" }, ` ×${i.count}`) : null),
        h(
          "span",
          { class: "reset-when" },
          i.expiresAt ? `Expires ${dateLabel(i.expiresAt)} · ${until(i.expiresAt)}` : "No expiry date",
        ),
      ),
    ),
    more > 0 ? h("li", { class: "reset-more" }, `+${more} more`) : null,
  );
}

let tipEl: HTMLElement | null = null;

function hideTip() {
  tipEl?.remove();
  tipEl = null;
}

/** A styled popover under (or above) `anchor`, kept inside the window. */
function showTip(anchor: HTMLElement, content: HTMLElement) {
  hideTip();
  const tip = h("div", { class: "tip", role: "tooltip" }, content);
  document.body.append(tip);
  tipEl = tip;
  const r = anchor.getBoundingClientRect();
  const w = tip.offsetWidth;
  const hgt = tip.offsetHeight;
  tip.style.left = `${Math.max(8, Math.min(r.right - w, window.innerWidth - w - 8))}px`;
  tip.style.top = `${r.bottom + 6 + hgt > window.innerHeight - 8 ? Math.max(8, r.top - hgt - 6) : r.bottom + 6}px`;
  // A refresh can replace the card under the pointer without a mouseleave.
  const check = () => {
    if (tipEl !== tip) return;
    if (!anchor.isConnected) hideTip();
    else setTimeout(check, 400);
  };
  setTimeout(check, 400);
}

addEventListener("scroll", hideTip, true);
addEventListener("blur", hideTip);

/**
 * Banked limit resets: the count on the card, each one's expiry date on hover.
 * `inline` lists up to two, soonest first, under the row instead, for the
 * widget's hover card, whose small window would cut a popover off.
 */
export function resetsRow(r: LimitResets, inline = false): HTMLElement {
  const next = r.items.find((i) => i.expiresAt)?.expiresAt ?? null;
  const row = h(
    "div",
    { class: `resets${r.available ? "" : " none"}${inline ? " inline" : ""}`, tabindex: inline ? null : 0 },
    h("span", { class: "meter-label" }, "Limit resets"),
    h(
      "span",
      { class: "resets-value" },
      h("span", { class: "resets-count" }, r.available ? `${r.available} available` : "None"),
      next ? h("span", { class: "meter-reset", "data-until": next, "data-prefix": "· next expires " }, `· next expires ${until(next)}`) : null,
    ),
  );
  if (inline) {
    // At most the two expiring first.
    return r.items.length ? h("div", { class: "resets-block" }, row, resetList(r, 2)) : row;
  }
  const open = () => showTip(row, resetList(r, Infinity));
  row.addEventListener("mouseenter", open);
  row.addEventListener("focus", open);
  row.addEventListener("mouseleave", hideTip);
  row.addEventListener("blur", hideTip);
  return row;
}

/**
 * Tokens used in a limit window (from the local logs): the total, a bar split
 * into input / output / cache, each part's share of the whole, and this PC's
 * share of the limit.
 */
export function tokensView(t: TokenUsage, source: string): HTMLElement {
  const name = t.period === "session" ? "session" : "week";
  const limit = t.period === "session" ? "5-hour session" : "weekly";
  const cache = t.cacheRead + t.cacheWrite;
  const total = t.input + t.output + cache;
  const parts = [
    { key: "in", label: "Input", value: t.input },
    { key: "out", label: "Output", value: t.output },
    { key: "cache", label: "Cache", value: cache },
  ];
  const share = (v: number) => (total ? (v / total) * 100 : 0);
  const shareText = (v: number) => {
    const p = share(v);
    if (p > 0 && p < 1) return "<1%";
    if (p > 99 && p < 100) return ">99%";
    return `${Math.round(p)}%`;
  };
  const cacheTip = t.cacheWrite ? ` (${tokens(t.cacheRead)} read, ${tokens(t.cacheWrite)} written)` : " (read from the prompt cache)";
  return h(
    "div",
    {
      class: "tokens",
      title: `Since ${dateLabel(t.since)}, when this ${limit} limit started (or was last reset). Counted from ${source} on this PC.`,
    },
    h(
      "div",
      { class: "meter-top" },
      h("span", { class: "meter-label" }, `Tokens this ${name}`),
      h("span", { class: "tokens-total" }, `${tokens(total)} total`),
    ),
    h(
      "div",
      { class: "tokens-bar", role: "img", "aria-label": parts.map((p) => `${p.label} ${shareText(p.value)}`).join(", ") },
      ...parts.filter((p) => p.value > 0).map((p) => h("span", { class: `seg ${p.key}`, style: { width: `${share(p.value)}%` } })),
    ),
    h(
      "div",
      { class: "tokens-legend" },
      ...parts.map((p) =>
        h(
          "span",
          { class: "tk", title: p.key === "cache" ? `Cache: ${tokens(cache)}${cacheTip}` : `${p.label}: ${p.value.toLocaleString()} tokens` },
          h("i", { class: `dot ${p.key}` }),
          h("span", { class: "tk-label" }, p.label),
          h("b", null, tokens(p.value)),
          h("span", { class: "tk-share" }, shareText(p.value)),
        ),
      ),
    ),
    t.share ? shareView(t.share, name) : null,
  );
}

/** This PC's estimated share of a limit (session or week), next to other devices. */
function shareView(s: WeeklyShare, name: string): HTMLElement {
  const p = (v: number) => (v > 0 && v < 1 ? "<1%" : `${Math.round(v)}%`);
  const parts = [
    { key: "pc", label: "This PC", value: s.thisPc },
    { key: "other", label: "Other devices", value: s.other },
    { key: "unsplit", label: "Not split yet", value: s.unsplit },
  ].filter((x) => x.key !== "unsplit" || x.value > 0);
  const tip = [
    `Estimated. Your ${name}'s usage that went up while this PC was in use (within half an hour) counts as this PC; ` +
      "what went up while it was idle was another device, the website or the phone app.",
    s.estimated
      ? `Usage from before Quota started watching (${dateLabel(s.trackedSince)}) is split by the tokens this PC logged then, at the rate seen since.`
      : "",
    s.unsplit > 0
      ? `"Not split yet" is usage from before Quota started watching (${dateLabel(s.trackedSince)}) or from a long gap between checks. It gets split once Quota has seen this PC at work.`
      : "",
  ]
    .filter(Boolean)
    .join("\n\n");
  return h(
    "div",
    { class: "share", title: tip },
    h(
      "div",
      { class: "meter-top" },
      h("span", { class: "meter-label" }, `This PC's share of the ${name}`),
      h("span", { class: "tokens-total" }, `≈ ${p(s.thisPc)} of ${p(s.used)} used`),
    ),
    h(
      "div",
      { class: "tokens-bar share-bar", role: "img", "aria-label": parts.map((x) => `${x.label} ${p(x.value)}`).join(", ") },
      ...parts.filter((x) => x.value > 0).map((x) => h("span", { class: `seg ${x.key}`, style: { width: `${Math.min(100, x.value)}%` } })),
    ),
    h(
      "div",
      { class: "tokens-legend" },
      ...parts.map((x) =>
        h("span", { class: "tk" }, h("i", { class: `dot ${x.key}` }), h("span", { class: "tk-label" }, x.label), h("b", null, p(x.value))),
      ),
    ),
  );
}

/** The tab picked in each card (session or week), kept across refreshes and restarts. */
const picked = new Map<string, TokenUsage["period"]>();
const PICKED_KEY = "quota.tokensTab";
try {
  for (const [k, v] of Object.entries(JSON.parse(localStorage.getItem(PICKED_KEY) ?? "{}"))) {
    if (v === "session" || v === "week") picked.set(k, v);
  }
} catch {
  /* nothing saved yet */
}

/**
 * The token sections of a card, one at a time behind a Session | Week switch.
 * With only one window to show, there's no switch.
 */
export function tokensTabs(cardId: string, list: TokenUsage[], source: string): HTMLElement | null {
  if (!list.length) return null;
  if (list.length === 1) return h("div", { class: "tokens-panel" }, tokensView(list[0]!, source));
  const label = (p: TokenUsage["period"]) => (p === "session" ? "Session" : "Week");
  let current = list.find((t) => t.period === picked.get(cardId)) ?? list[0]!;
  const body = h("div", { class: "tokens-tab-body", role: "tabpanel" }, tokensView(current, source));
  const tabs = list.map((t) =>
    h(
      "button",
      {
        class: "seg-tab",
        type: "button",
        role: "tab",
        "aria-selected": String(t === current),
        onclick: (e: Event) => {
          e.stopPropagation();
          current = t;
          picked.set(cardId, t.period);
          try {
            localStorage.setItem(PICKED_KEY, JSON.stringify(Object.fromEntries(picked)));
          } catch {
            /* not saved; fine */
          }
          tabs.forEach((b, i) => b.setAttribute("aria-selected", String(list[i] === t)));
          body.replaceChildren(tokensView(t, source));
        },
      },
      label(t.period),
    ),
  );
  return h(
    "div",
    { class: "tokens-panel" },
    h(
      "div",
      { class: "tokens-panel-head" },
      h("span", { class: "meter-label" }, "On this PC"),
      h("div", { class: "seg-tabs", role: "tablist", "aria-label": "Limit window" }, ...tabs),
    ),
    body,
  );
}
