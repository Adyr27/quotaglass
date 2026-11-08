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

export interface Settings {
  version: 1;
  window: WindowRect | null;
  pinned: boolean;
  alwaysOnTop: boolean;
  autostart: boolean;
  refreshMinutes: number;
  sources: SourceSwitches;
  collapsed: Service[];
  hiddenServices: Service[];
  firstRunDone: boolean;
}

export type SettingsPatch = Partial<
  Pick<
    Settings,
    | "pinned"
    | "alwaysOnTop"
    | "autostart"
    | "refreshMinutes"
    | "sources"
    | "collapsed"
    | "hiddenServices"
    | "firstRunDone"
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
