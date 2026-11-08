// Pure formatting helpers. No DOM, no IPC, no hidden clock: callers pass `nowMs`.

import type { AccountView, BucketView, Connection, Service } from "./types";

export const SERVICE_ORDER: readonly Service[] = ["claude", "codex", "antigravity"];

export const SERVICE_NAME: Record<Service, string> = {
  claude: "Claude",
  codex: "Codex",
  antigravity: "Antigravity",
};

export const CONNECTION_ORDER: readonly Connection[] = ["claudeCurrent", "codexCurrent", "antigravityApp"];

export const CONNECTION_NAME: Record<Connection, string> = {
  claudeCurrent: "Claude login",
  codexCurrent: "Codex login",
  antigravityApp: "Antigravity app",
};

const MIN_MS = 60_000;
const HOUR_MS = 60 * MIN_MS;
const DAY_MS = 24 * HOUR_MS;

// hourCycle h23 keeps midnight as 00:xx. hour12:false gives 24:xx in some locales.
const weekdayTime = new Intl.DateTimeFormat(undefined, {
  weekday: "short",
  hour: "2-digit",
  minute: "2-digit",
  hourCycle: "h23",
});

/** "resets 3 h 30 min" under 24 h, else "resets Sat 11:00". Empty when unknown or past. */
export function resetText(resetsAtMs: number | null, nowMs: number): string {
  if (resetsAtMs === null || !Number.isFinite(resetsAtMs) || resetsAtMs <= nowMs) return "";
  const left = resetsAtMs - nowMs;
  if (left >= DAY_MS) return `resets ${weekdayTime.format(new Date(resetsAtMs))}`;
  const hours = Math.floor(left / HOUR_MS);
  const minutes = Math.max(1, Math.ceil((left % HOUR_MS) / MIN_MS));
  if (hours === 0) return `resets in ${minutes} min`;
  return minutes === 60 ? `resets ${hours + 1} h` : `resets ${hours} h ${minutes} min`;
}

/** "just now", "4 min ago", "2 h ago", "3 d ago". Empty when unknown. */
export function updatedText(ageSecs: number | null): string {
  if (ageSecs === null || !Number.isFinite(ageSecs) || ageSecs < 0) return "";
  if (ageSecs < 60) return "just now";
  if (ageSecs < 3600) return `${Math.floor(ageSecs / 60)} min ago`;
  if (ageSecs < 86_400) return `${Math.floor(ageSecs / 3600)} h ago`;
  return `${Math.floor(ageSecs / 86_400)} d ago`;
}

/** Whole percent used, 0 to 100. Floored, so a limit never reads as reached before it is. */
export function usedWhole(usedPercent: number): number {
  if (!Number.isFinite(usedPercent)) return 0;
  return Math.floor(Math.min(100, Math.max(0, usedPercent)));
}

/** Plan names arrive as the source wrote them ("pro", "max_5x"). Show them in sentence case. */
export function sentenceCase(value: string): string {
  const spaced = value.replace(/_+/g, " ").trim();
  return spaced.charAt(0).toUpperCase() + spaced.slice(1);
}

/** The limit closest to running out; null without readings. */
export function worstBucket(account: AccountView): BucketView | null {
  let worst: BucketView | null = null;
  for (const b of account.buckets) if (worst === null || b.usedPercent > worst.usedPercent) worst = b;
  return worst;
}

/** Highest whole percent used over an account's limits; null without readings. */
export function worstUsed(account: AccountView): number | null {
  const b = worstBucket(account);
  return b === null ? null : usedWhole(b.usedPercent);
}

/** Accounts in panel order: service, current login first, then label. Hidden accounts are left out. */
export function visibleAccounts(accounts: readonly AccountView[]): AccountView[] {
  return accounts
    .filter((a) => !a.hidden)
    .sort(
      (a, b) =>
        SERVICE_ORDER.indexOf(a.service) - SERVICE_ORDER.indexOf(b.service) ||
        Number(b.currentLogin) - Number(a.currentLogin) ||
        a.label.localeCompare(b.label),
    );
}
