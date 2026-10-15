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

/** Banked limit resets: the count on the card, each one's expiry date on hover. */
export function resetsRow(r: LimitResets): HTMLElement {
  const lines = r.items.map((i) => {
    const what = `${i.count} ${i.count === 1 ? "reset" : "resets"}${i.label ? ` · ${i.label}` : ""}`;
    return i.expiresAt ? `${what} · expires ${dateLabel(i.expiresAt)} (${until(i.expiresAt)})` : `${what} · no expiry date`;
  });
  const tip = lines.length
    ? lines.join("\n")
    : r.available
      ? "The expiry dates weren't reported."
      : "None right now. When you get one, it shows here with its expiry date.";
  const next = r.items.find((i) => i.expiresAt)?.expiresAt ?? null;
  return h(
    "div",
    { class: `resets${r.available ? "" : " none"}`, title: tip },
    h("span", { class: "meter-label" }, "Limit resets"),
    h(
      "span",
      { class: "resets-value" },
      h("span", { class: "resets-count" }, r.available ? `${r.available} available` : "None"),
      next ? h("span", { class: "meter-reset", "data-until": next, "data-prefix": "· next expires " }, `· next expires ${until(next)}`) : null,
    ),
  );
}
