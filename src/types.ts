// IPC contract shared with the Rust side. All payloads are camelCase JSON.

export type Service = "claude" | "codex" | "antigravity";

export type SourceKind = "direct" | "antigravityApp" | "orca";

/** The sources a person can switch on or off in the Sources rows. */
export type SwitchableSource = "direct" | "orca";

export type Freshness = "live" | "aged" | "stale";

export type CardState = "ok" | "noReadingYet" | "error" | "noSource";

export interface BucketView {
  name: string;
  usedPercent: number;
  windowSecs: number | null;
  resetsAtMs: number | null;
  limitReached: boolean;
  freshness: Freshness;
}

export interface AccountView {
  id: string;
  service: Service;
  label: string;
  plan: string | null;
  source: SourceKind | null;
  /** Every source with a reading for this account, in display order. */
  sources: SourceKind[];
  freshness: Freshness;
  ageSecs: number | null;
  headline: BucketView | null;
  extraBuckets: number;
  buckets: BucketView[];
  state: CardState;
  reason: string | null;
  connect: Service | null;
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

export type SourceSwitches = Record<SwitchableSource, boolean>;

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
    | "pillPosition"
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

export interface SourceStatus {
  kind: SwitchableSource;
  detected: boolean;
  enabled: boolean;
  unofficial: boolean;
}

export interface BackdropPayload {
  acrylic: boolean;
}
