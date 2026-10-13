import type { Meter } from "./api";
import { h } from "./dom";
import { headline, leftPct, level, money, pct, resetLabel, until } from "./format";

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
