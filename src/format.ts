// Pure formatting helpers. No DOM, no IPC, no hidden clock: callers pass `nowMs`.

import type { AccountView, BucketView, Freshness, Service, SourceKind, SwitchableSource } from "./types";

export const SERVICE_ORDER: readonly Service[] = ["claude", "codex", "antigravity"];

export const SERVICE_NAME: Record<Service, string> = {
  claude: "Claude",
  codex: "Codex",
  antigravity: "Antigravity",
};

export const SOURCE_ORDER: readonly SwitchableSource[] = ["direct", "orca", "omp"];

/** Short source names for the meta line of a card. */
const SOURCE_TAG: Record<SourceKind, string> = {
  direct: "direct (unofficial)",
  orca: "Orca",
  omp: "OMP",
  sessionKey: "session key",
};

/** Names for the rows of the Sources panel. */
const SOURCE_ROW_NAME: Record<SwitchableSource, string> = {
  direct: "Claude Code and Codex login files",
  orca: "Orca",
  omp: "OMP",
};

export function sourceTag(kind: SourceKind): string {
  return SOURCE_TAG[kind] ?? kind;
}

export function sourceRowName(kind: SwitchableSource): string {
  return SOURCE_ROW_NAME[kind] ?? kind;
}

const HOUR_SECS = 3600;
const DAY_SECS = 86_400;
const WEEK_SECS = 604_800;
const WEEK_MS = WEEK_SECS * 1000;

/** 18000 -> "5 h", 604800 -> "Weekly", 2592000 -> "30 days". Empty string when there is no window. */
export function formatWindow(secs: number | null): string {
  if (secs === null || !Number.isFinite(secs) || secs <= 0) return "";
  if (secs === WEEK_SECS) return "Weekly";
  if (secs % DAY_SECS === 0) {
    const days = secs / DAY_SECS;
    return days === 1 ? "1 day" : `${days} days`;
  }
  if (secs < HOUR_SECS) return `${Math.round(secs / 60)} min`;
  return `${Number((secs / HOUR_SECS).toFixed(1))} h`;
}

// hourCycle h23 keeps midnight as 00:xx. hour12:false gives 24:xx in some locales.
const weekdayTime = new Intl.DateTimeFormat(undefined, {
  weekday: "short",
  hour: "2-digit",
  minute: "2-digit",
  hourCycle: "h23",
});
const monthDay = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" });

/** "Sat 11:00" within 7 days, "Nov 14" after that. Empty string when unknown or already past. */
export function formatReset(resetsAtMs: number | null, nowMs: number): string {
  if (resetsAtMs === null || !Number.isFinite(resetsAtMs) || resetsAtMs <= nowMs) return "";
  const when = new Date(resetsAtMs);
  return resetsAtMs - nowMs <= WEEK_MS ? weekdayTime.format(when) : monthDay.format(when);
}

/** "just now", "5m ago", "3h ago", "2d ago". Empty string when unknown. */
export function formatAge(ageSecs: number | null): string {
  if (ageSecs === null || !Number.isFinite(ageSecs) || ageSecs < 0) return "";
  if (ageSecs < 60) return "just now";
  if (ageSecs < HOUR_SECS) return `${Math.floor(ageSecs / 60)}m ago`;
  if (ageSecs < DAY_SECS) return `${Math.floor(ageSecs / HOUR_SECS)}h ago`;
  return `${Math.floor(ageSecs / DAY_SECS)}d ago`;
}

function clampPercent(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.min(100, Math.max(0, value));
}

/** Whole percent used, 0 to 100. */
export function usedWhole(usedPercent: number): number {
  return Math.round(clampPercent(usedPercent));
}

/** Whole percent left. Always 100 minus the whole percent used, so the two never disagree. */
export function leftWhole(usedPercent: number): number {
  return 100 - usedWhole(usedPercent);
}

/** Bar fill, 0 to 1. */
export function usedRatio(usedPercent: number): number {
  return clampPercent(usedPercent) / 100;
}

/** Plan names arrive as the source wrote them ("pro", "max_5x"). Show them in sentence case. */
export function sentenceCase(value: string): string {
  const spaced = value.replace(/_+/g, " ").trim();
  return spaced.charAt(0).toUpperCase() + spaced.slice(1);
}

export interface MetaParts {
  resetsAtMs: number | null;
  source: SourceKind | null;
  freshness: Freshness;
  ageSecs: number | null;
}

/** "Resets Sat 11:00 · via Orca · 2h ago". The age only appears when the reading is not live. */
export function metaLine(parts: MetaParts, nowMs: number): string {
  const out: string[] = [];
  const reset = formatReset(parts.resetsAtMs, nowMs);
  if (reset !== "") out.push(`Resets ${reset}`);
  if (parts.source !== null) out.push(`via ${sourceTag(parts.source)}`);
  if (parts.freshness !== "live") {
    const age = formatAge(parts.ageSecs);
    if (age !== "") out.push(age);
  }
  return out.join(" \u00b7 ");
}

export interface MostRoom {
  account: AccountView;
  bucket: BucketView;
  left: number;
}

/**
 * The account that still has the most room right now: among accounts with a live, working
 * reading, the one whose tightest bucket is the least used. Null when no account qualifies.
 */
export function mostRoom(accounts: readonly AccountView[]): MostRoom | null {
  let best: MostRoom | null = null;
  for (const account of accounts) {
    const bucket = account.headline;
    if (account.state !== "ok" || account.freshness !== "live" || bucket === null || bucket.limitReached) continue;
    const left = leftWhole(bucket.usedPercent);
    if (left <= 0) continue;
    if (best === null || bucket.usedPercent < best.bucket.usedPercent) best = { account, bucket, left };
  }
  return best;
}

export function findingText(room: MostRoom): string {
  return `Most room: ${room.account.label}, ${room.left}% left on ${room.bucket.name}`;
}
