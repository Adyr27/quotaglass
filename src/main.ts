// QuotaGlass window. Holds the state, wires the panels together, and re-renders on every change.

import {
  connect,
  detectSources,
  getSettings,
  getSnapshot,
  onBackdrop,
  onOpenSettings,
  onSettings,
  onSnapshot,
  refreshNow,
  setProblemListener,
  updateSettings,
} from "./ipc";
import type { AccountView, Connection, Service, Settings, SettingsPatch, Snapshot, SourceStatus, SourceSwitches, SwitchableSource } from "./types";
import { createBoard, type BoardView } from "./view/board";
import { h, iconPin, iconRefresh, iconSliders, markRing, setAttr, show } from "./view/dom";
import { createOverlay } from "./view/overlay";
import { createSettings } from "./view/settings";
import { createSourceRows } from "./view/sources";

let snapshot: Snapshot | null = null;
let settings: Settings | null = null;
let detected: SourceStatus[] | null = null;
/** First run only: the switches as the person sets them, saved when they press Done. */
let draft: SourceSwitches | null = null;
/** Set once Done is pressed, so an empty `connected` list does not bring the first-run panel back this session. */
let firstRunDismissed = false;
/** Chip and group state for this session only. The UI rebuild replaces it. */
const boardView: { hiddenServices: Service[]; collapsed: Service[] } = { hiddenServices: [], collapsed: [] };
let settingsOpen = false;
let refreshing = false;
let saving = false;
let opener: HTMLElement | null = null;

// Header

const refreshButton = h("button", { class: "icon-btn", type: "button", "aria-label": "Refresh" }, [iconRefresh()]);
const pinButton = h("button", { class: "icon-btn", type: "button", "aria-label": "Always on top", "aria-pressed": "false" }, [iconPin()]);
const settingsButton = h(
  "button",
  { class: "icon-btn", type: "button", "aria-label": "Settings", "aria-expanded": "false", "aria-controls": "settings-panel" },
  [iconSliders()],
);
const headerActions = h("div", { class: "header-actions" }, [refreshButton, pinButton, settingsButton]);
const header = h("header", { class: "header", "data-tauri-drag-region": true }, [
  h("div", { class: "brand" }, [markRing("brand-mark"), h("span", {}, ["QuotaGlass"])]),
  headerActions,
]);
const problem = h("p", { class: "problem", role: "status", hidden: true });

// Views

const board = createBoard({
  onOpen: (accountId, from) => openOverlay(accountId, from),
  onRetry: () => void refresh(),
  onSignIn: (service) => void connect(service),
  onToggleService: (service) => toggleIn("hiddenServices", service),
  onToggleGroup: (service) => toggleIn("collapsed", service),
});

const firstRunRows = createSourceRows("first", (kind: SwitchableSource, next: boolean) => {
  if (draft !== null) draft = { ...draft, [kind]: next };
  render();
});
const firstRunDone = h("button", { class: "btn btn-primary", type: "button" }, ["Done"]);
const firstRunHeading = h("h2", { class: "panel-title", id: "first-title", tabindex: -1 }, ["Sources"]);
const firstRun = h("section", { class: "panel", "aria-labelledby": "first-title" }, [
  firstRunHeading,
  h("p", { class: "panel-text" }, ["QuotaGlass reads these on this PC. Turn off any you do not want."]),
  firstRunRows.el,
  h("div", { class: "panel-foot" }, [firstRunDone]),
]);

const settingsPanel = createSettings({
  onRefreshMinutes: (minutes) => void apply({ refreshMinutes: minutes }),
  onToggle: (key, next) => void apply({ [key]: next }),
  onSource: (kind, next) => {
    if (settings !== null) void apply({ connected: connectionsFrom({ ...switchesOf(settings.connected), [kind]: next }, settings.connected) });
  },
  onClose: () => closeSettings(),
});
settingsPanel.el.id = "settings-panel";

const overlay = createOverlay(() => closeOverlay());

const views = h("div", { class: "views" }, [board.el, firstRun, settingsPanel.el]);
const shell = h("div", { class: "shell" }, [header, problem, views]);
document.getElementById("app")?.append(shell, overlay.el);

// Rendering

function render(): void {
  const nowMs = Date.now();
  const firstRunMode = isFirstRun();
  const boardMode = !firstRunMode && snapshot !== null && settings !== null && !settingsOpen;
  const settingsMode = !firstRunMode && settings !== null && settingsOpen;

  show(board.el, boardMode);
  show(firstRun, firstRunMode);
  show(settingsPanel.el, settingsMode);
  show(headerActions, !firstRunMode);

  const onTop = settings?.alwaysOnTop === true;
  setAttr(pinButton, "aria-pressed", onTop ? "true" : "false");
  setAttr(settingsButton, "aria-expanded", settingsMode ? "true" : "false");
  refreshButton.classList.toggle("is-busy", refreshing);
  setAttr(refreshButton, "aria-busy", refreshing ? "true" : null);

  if (snapshot !== null && settings !== null) board.update(snapshot, boardView satisfies BoardView, nowMs, refreshing);
  if (settings !== null) {
    settingsPanel.update(settings, withEnabled(detected, switchesOf(settings.connected)));
    firstRunRows.update(withEnabled(detected, draft ?? switchesOf(settings.connected)));
  }
  firstRunDone.classList.toggle("is-disabled", saving);
  setAttr(firstRunDone, "aria-disabled", saving ? "true" : null);

  if (overlay.openId !== null) {
    const account = findAccount(overlay.openId);
    if (account === null) closeOverlay();
    else overlay.update(account, nowMs);
  }
}

function withEnabled(rows: SourceStatus[] | null, switches: SourceSwitches): SourceStatus[] {
  return (rows ?? []).map((row) => ({ ...row, enabled: switches[row.kind] }));
}

function isFirstRun(): boolean {
  return settings !== null && settings.connected.length === 0 && !firstRunDismissed;
}

/** Bridge from the v0.1.0 switches to `connected` until the UI rebuild. Antigravity is kept as it is. */
function switchesOf(connected: readonly Connection[]): SourceSwitches {
  return {
    direct: connected.includes("claudeCurrent") || connected.includes("codexCurrent"),
    orca: connected.includes("orca"),
  };
}

function connectionsFrom(switches: SourceSwitches, current: readonly Connection[]): Connection[] {
  const out: Connection[] = current.filter((c) => c === "antigravityApp");
  if (switches.direct) out.push("claudeCurrent", "codexCurrent");
  if (switches.orca) out.push("orca");
  return out;
}

function findAccount(id: string): AccountView | null {
  return snapshot?.accounts.find((account) => account.id === id) ?? null;
}

// Overlay

function openOverlay(accountId: string, from: HTMLElement): void {
  const account = findAccount(accountId);
  if (account === null) return;
  opener = from;
  shell.inert = true;
  overlay.open(account, Date.now());
}

function closeOverlay(): void {
  const account = overlay.openId;
  overlay.close();
  shell.inert = false;
  const back = opener;
  opener = null;
  if (back !== null && back.isConnected && back.getClientRects().length > 0) back.focus({ preventScroll: true });
  else if (account === null || !board.focusCard(account)) settingsButton.focus({ preventScroll: true });
}

// Settings

function openSettings(): void {
  if (settings === null || isFirstRun() || settingsOpen) return;
  if (overlay.openId !== null) closeOverlay();
  settingsOpen = true;
  render();
  settingsPanel.heading.focus({ preventScroll: true });
  void loadSources();
}

function closeSettings(): void {
  settingsOpen = false;
  render();
  settingsButton.focus({ preventScroll: true });
}

async function loadSources(): Promise<void> {
  const result = await detectSources();
  if (!result.ok) return;
  detected = result.value;
  if (draft === null && isFirstRun()) draft = switchesFrom(result.value);
  render();
}

function switchesFrom(rows: readonly SourceStatus[]): SourceSwitches {
  const out: SourceSwitches = { direct: false, orca: false };
  for (const row of rows) out[row.kind] = row.enabled;
  return out;
}

async function apply(patch: SettingsPatch): Promise<void> {
  const result = await updateSettings(patch);
  if (result.ok) settings = result.value;
  render(); // on a failure this also puts every switch back to the saved value
}

function toggleIn(key: "hiddenServices" | "collapsed", service: Service): void {
  const current = boardView[key];
  boardView[key] = current.includes(service) ? current.filter((s) => s !== service) : [...current, service];
  render();
}

async function finishFirstRun(): Promise<void> {
  if (saving) return;
  saving = true;
  render();
  const patch: SettingsPatch = {};
  if (draft !== null) patch.connected = connectionsFrom(draft, settings?.connected ?? []);
  const result = await updateSettings(patch);
  saving = false;
  if (result.ok) {
    settings = result.value;
    firstRunDismissed = true;
    draft = null;
    render();
    void refresh();
    return;
  }
  render();
}

// Commands

async function refresh(): Promise<void> {
  if (refreshing) return;
  refreshing = true;
  render();
  if (settings === null || snapshot === null) await load();
  await refreshNow();
  const latest = await getSnapshot();
  if (latest.ok) takeSnapshot(latest.value);
  refreshing = false;
  render();
}

function takeSnapshot(next: Snapshot): void {
  // A slow answer to an older request must not replace a newer snapshot.
  if (snapshot !== null && next.generatedAtMs < snapshot.generatedAtMs) return;
  snapshot = next;
  render();
}

async function load(): Promise<void> {
  const [settingsResult, snapshotResult] = await Promise.all([getSettings(), getSnapshot()]);
  if (settingsResult.ok) settings = settingsResult.value;
  if (snapshotResult.ok) takeSnapshot(snapshotResult.value);
  render();
  if (isFirstRun()) {
    await loadSources();
    firstRunHeading.focus({ preventScroll: true });
  }
}

// Wiring

setProblemListener((message) => {
  if (message === null) {
    show(problem, false);
    return;
  }
  problem.textContent = message;
  show(problem, true);
});

refreshButton.addEventListener("click", () => void refresh());
pinButton.addEventListener("click", () => {
  if (settings !== null) void apply({ alwaysOnTop: !settings.alwaysOnTop });
});
settingsButton.addEventListener("click", () => {
  if (settingsOpen) closeSettings();
  else openSettings();
});
firstRunDone.addEventListener("click", () => void finishFirstRun());

document.addEventListener("keydown", (event) => {
  if (event.key !== "Escape" || event.defaultPrevented) return;
  if (overlay.openId !== null) closeOverlay();
  else if (settingsOpen) closeSettings();
});

// Listeners go first so an event that fires while the first load runs is not missed.
void Promise.all([
  onSnapshot((next) => takeSnapshot(next)),
  onSettings((next) => {
    settings = next;
    render();
  }),
  onBackdrop((payload) => document.documentElement.classList.toggle("no-acrylic", !payload.acrylic)),
  onOpenSettings(() => openSettings()),
]).then(() => load());
