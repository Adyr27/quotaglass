// IPC contract shared with the Rust side. All payloads are camelCase JSON.

export type Service = "claude" | "codex" | "antigravity";

export type SourceKind = "direct" | "antigravityApp";

export type Freshness = "live" | "aged" | "stale";

export type CardState = "ok" | "noReadingYet" | "error" | "noSource" | "loginExpired" | "profileMissing" | "rateLimited";

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
  hidden: boolean;
}

/** One pill slot: the best reading over the visible accounts of a service. */
export interface ServiceView {
  service: Service;
  usedPercent: number | null;
  allExhausted: boolean;
  accounts: number;
  rateLimited: boolean;
}

export type FlyoutView = "settings" | "first";

export interface FlyoutMode {
  mode: "hover" | "popover" | "hidden";
  service: Service | null;
  view: FlyoutView | null;
}

export interface Snapshot {
  accounts: AccountView[];
  services: ServiceView[];
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
export type Connection = "claudeCurrent" | "codexCurrent" | "antigravityApp";

/** An account signed in through "+ Add account". Its login stays in `profileDir`. */
export interface AddedAccount {
  id: string;
  service: Service;
  profileDir: string;
}

export type ToastSound = "default" | "silent";

/** Which reset toasts the person wants, and with what sound. */
export interface ResetToasts {
  enabled: boolean;
  claude: boolean;
  codex: boolean;
  antigravity: boolean;
  sound: ToastSound;
}

export interface Settings {
  version: 3;
  pillPosition: WindowRect | null;
  /** Pill size factor, 0.65 to 1.6. Changed only with `setPillScale`. */
  pillScale: number;
  /** Window lock: no drag, no resize. Changed only with `setPillLocked`. */
  pillLocked: boolean;
  alwaysOnTop: boolean;
  autostart: boolean;
  refreshMinutes: number;
  theme: Theme;
  material: Material;
  connected: Connection[];
  addedAccounts: AddedAccount[];
  hiddenAccounts: string[];
  /** Account ID to label, at most 40 characters each. */
  labels: Record<string, string>;
  resetToasts: ResetToasts;
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
    | "resetToasts"
    | "hiddenAccounts"
    | "labels"
  >
>;

/** Which one-click sources apply on this PC. `antigravityApp` is always true: its fetch decides. */
export interface Detected {
  claude: boolean;
  codex: boolean;
  antigravityApp: boolean;
}

/** A page the app may open in the browser. The UI names it; the app holds the URL. */
export type UrlTarget = "openAiUsage";

export interface BackdropPayload {
  acrylic: boolean;
}
