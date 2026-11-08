// IPC contract shared with the Rust side. All payloads are camelCase JSON.

export type Service = "claude" | "codex" | "antigravity";

export type SourceKind = "direct" | "antigravityApp" | "orca";

export type Freshness = "live" | "aged" | "stale";

export type CardState = "ok" | "noReadingYet" | "error" | "noSource" | "loginExpired" | "profileMissing";

export interface BucketView {
  name: string;
  usedPercent: number;
  windowSecs: number | null;
  resetsAtMs: number | null;
  limitReached: boolean;
  freshness: Freshness;
  /** Share of the window that has passed, 0..100. Null without a window length and a reset time. */
  passedPercent: number | null;
  fastBurn: boolean;
}

export interface AccountView {
  id: string;
  service: Service;
  label: string;
  plan: string | null;
  source: SourceKind | null;
  freshness: Freshness;
  ageSecs: number | null;
  headline: BucketView | null;
  buckets: BucketView[];
  state: CardState;
  reason: string | null;
  /** 100 minus the highest used % over the limits that still hold. */
  roomLeft: number | null;
  bestNow: boolean;
  /** Codex only. Null when the source does not say. */
  bankedResets: number | null;
  /** The added account (profile) that reads this account. */
  profileId: string | null;
  currentLogin: boolean;
  pinned: boolean;
  hidden: boolean;
}

export interface Snapshot {
  accounts: AccountView[];
  generatedAtMs: number;
}

export interface WindowRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export type Theme = "system" | "light" | "dark";

export type Material = "acrylic" | "mica" | "clear";

/** A one-click source the person switched on. */
export type Connection = "claudeCurrent" | "codexCurrent" | "antigravityApp" | "orca";

/** An account signed in through "+ Add account". Its login stays in `profileDir`. */
export interface AddedAccount {
  id: string;
  service: Service;
  profileDir: string;
}

export interface Settings {
  version: 2;
  pillPosition: WindowRect | null;
  alwaysOnTop: boolean;
  autostart: boolean;
  refreshMinutes: number;
  theme: Theme;
  material: Material;
  connected: Connection[];
  addedAccounts: AddedAccount[];
  /** Account IDs in display order, at most 4. */
  pinnedAccounts: string[];
  hiddenAccounts: string[];
  /** Account ID to label, at most 40 characters each. */
  labels: Record<string, string>;
}

export type SettingsPatch = Partial<
  Pick<
    Settings,
    | "alwaysOnTop"
    | "autostart"
    | "refreshMinutes"
    | "theme"
    | "material"
    | "connected"
    | "pinnedAccounts"
    | "hiddenAccounts"
    | "labels"
  >
>;

/** Which one-click sources apply on this PC. `antigravityApp` is always true: its fetch decides. */
export interface Detected {
  claude: boolean;
  codex: boolean;
  antigravityApp: boolean;
  orca: boolean;
}

export interface BackdropPayload {
  acrylic: boolean;
}
