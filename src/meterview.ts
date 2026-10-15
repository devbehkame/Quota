import type { LimitResets, Meter } from "./api";
import { h } from "./dom";
import { dateLabel, headline, leftPct, level, money, pct, resetLabel, until } from "./format";

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

/** One line per reset: what it is (bold) and when it expires. */
function resetList(r: LimitResets): HTMLElement {
  if (!r.items.length) {
    const msg = r.available
      ? "The expiry dates weren't reported."
      : "None right now. When you get one, it shows here with its expiry date.";
    return h("p", { class: "reset-empty" }, msg);
  }
  return h(
    "ul",
    { class: "reset-list" },
    ...r.items.map((i) =>
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
 * `inline` shows the soonest-expiring one under the row instead, for the widget's
 * hover card, whose small window would cut a popover off.
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
    // Only the one expiring first; the row already says how many there are.
    return r.items.length ? h("div", { class: "resets-block" }, row, resetList({ ...r, items: r.items.slice(0, 1) })) : row;
  }
  const open = () => showTip(row, resetList(r));
  row.addEventListener("mouseenter", open);
  row.addEventListener("focus", open);
  row.addEventListener("mouseleave", hideTip);
  row.addEventListener("blur", hideTip);
  return row;
}
