import type { Meter, Unit } from "./api";

export function money(unit: Unit, v: number): string {
  const sym = unit === "cny" ? "¥" : "$";
  const abs = Math.abs(v);
  const digits = abs >= 1000 ? 0 : 2;
  return `${v < 0 ? "-" : ""}${sym}${abs.toLocaleString(undefined, {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  })}`;
}

export function pct(v: number): string {
  return `${Math.round(v)}%`;
}

function span(ms: number): string {
  const m = Math.max(1, Math.round(ms / 60000));
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return m % 60 ? `${h}h ${m % 60}m` : `${h}h`;
  const d = Math.floor(h / 24);
  return h % 24 ? `${d}d ${h % 24}h` : `${d}d`;
}

export function until(iso: string, now = Date.now()): string {
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "";
  if (t <= now) return "now";
  return `in ${span(t - now)}`;
}

export function ago(iso: string, now = Date.now()): string {
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "";
  const d = now - t;
  if (d < 45_000) return "just now";
  return `${span(d)} ago`;
}

export function resetLabel(iso: string): string {
  const t = new Date(iso);
  const sameDay = t.toDateString() === new Date().toDateString();
  const time = t.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  return sameDay ? time : `${t.toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" })}, ${time}`;
}

/** Remaining share of the limit, 0–100, if the meter has a limit. */
export function leftPct(m: Meter): number | null {
  return m.percentUsed === null ? null : Math.max(0, 100 - m.percentUsed);
}

export type Level = "good" | "warn" | "danger" | "neutral";

export function level(m: Meter): Level {
  const left = leftPct(m);
  if (left === null) return "neutral";
  if (left < 15) return "danger";
  if (left < 40) return "warn";
  return "good";
}

/** The headline number and caption for a meter. */
export function headline(m: Meter): { value: string; caption: string } {
  if (m.unit === "percent") {
    return { value: pct(leftPct(m) ?? 0), caption: "left" };
  }
  if (m.remaining !== null) {
    return { value: money(m.unit, m.remaining), caption: "left" };
  }
  return { value: money(m.unit, m.used ?? 0), caption: "spent" };
}
