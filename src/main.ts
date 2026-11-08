// QuotaGlass window: the pill, and the panel it opens into. Holds the state and re-renders on every change.

import {
  addAccount,
  connect,
  connectSource,
  detect,
  getSettings,
  getSnapshot,
  onBackdrop,
  onOpenSettings,
  onSettings,
  onSnapshot,
  onTheme,
  openCli,
  openUrl,
  refreshNow,
  removeAccount,
  setHidden,
  setLabel,
  setPanelOpen,
  setZoom,
  setPinned,
  setProblemListener,
  updateSettings,
  type Result,
} from "./ipc";
import { visibleAccounts } from "./format";
import type { AccountView, Detected, Settings, Snapshot } from "./types";
import { createDetail } from "./view/account";
import { h, show } from "./view/dom";
import { createFirst } from "./view/first";
import { createList } from "./view/panel";
import { createPill } from "./view/pill";
import { createSettings } from "./view/settings";

let snapshot: Snapshot | null = null;
let settings: Settings | null = null;
let detected: Detected | null = null;
let open = false;
/** True while the panel extends above the pill, so the pill row sits at the bottom. */
let up = true;
let toggling = false;
let settingsOpen = false;
/** True when Settings opened the panel (from the tray), so Back closes it again. */
let settingsOpenedPanel = false;
let selectedId: string | null = null;
let refreshing = false;

const accounts = (): AccountView[] => snapshot?.accounts ?? [];
const isFirstRun = (): boolean => settings !== null && settings.connected.length === 0 && settings.addedAccounts.length === 0;

// Views

const pill = createPill(() => void setOpen(!open));
const list = createList({
  onSelect: (id) => {
    selectedId = id;
    render();
  },
  onAdd: (service) => void take(addAccount(service)),
  onSettings: () => openSettings(),
});
const detail = createDetail({
  onPin: (a) => void take(setPinned(a.id, !a.pinned)),
  onRefresh: () => void refresh(),
  onRemove: (a) => void take(removeAccount(a.id)).then(() => list.listbox.focus()),
  onHide: (a) => void take(setHidden(a.id, true)).then(() => list.listbox.focus()),
  onSignIn: (a) => void (a.profileId === null ? connect(a.service) : openCli(a.id)),
  onRedeem: () => void openUrl("openAiUsage"),
});
const first = createFirst((c) => void take(connectSource(c, true)));
const settingsView = createSettings({
  onPatch: (patch) => void take(updateSettings(patch)),
  onSource: (c, on) => void take(connectSource(c, on)),
  onUnhide: (id) => void take(setHidden(id, false)),
  onLabel: (id, label) => void take(setLabel(id, label)),
  onBack: () => settingsBack(),
});

const problem = h("p", { class: "problem", role: "status", hidden: true });
const area = h("div", { class: "area" }, [detail.el, first.el, settingsView.el]);
const panes = h("div", { class: "panes" }, [list.el, area]);
const body = h("div", { class: "body", hidden: true }, [problem, panes]);
const app = h("div", { class: "app" }, [pill.el, body]);
document.getElementById("app")?.append(app);

// Rendering

function render(): void {
  const nowMs = Date.now();
  const all = accounts();
  const visible = visibleAccounts(all);
  if (!visible.some((a) => a.id === selectedId)) selectedId = visible[0]?.id ?? null;
  const firstRun = isFirstRun();

  app.dataset.open = String(open);
  app.dataset.up = String(up);
  if (settings !== null) document.documentElement.dataset.material = settings.material;
  pill.update(all, settings, open);
  show(body, open);
  show(list.el, !firstRun && !settingsOpen);
  panes.dataset.full = String(firstRun || settingsOpen);
  show(first.el, firstRun && !settingsOpen);
  show(settingsView.el, settingsOpen && settings !== null);
  show(detail.el, !firstRun && !settingsOpen);

  list.update(all, settings, selectedId);
  detail.update(visible.find((a) => a.id === selectedId) ?? null, nowMs, refreshing);
  first.update(detected);
  if (settings !== null) settingsView.update(settings, detected, all);
}

// Panel

async function setOpen(next: boolean): Promise<void> {
  if (toggling || next === open) return;
  toggling = true;
  if (!next) {
    // Hide the panel's content first, so it is never squeezed into the pill's height.
    open = false;
    settingsOpen = false;
    render();
  }
  const result = await setPanelOpen(next);
  if (result.ok && next) {
    up = result.value;
    open = true;
    render();
    if (isFirstRun()) {
      await loadDetected();
      first.heading.focus({ preventScroll: true });
    } else {
      list.listbox.focus({ preventScroll: true });
    }
  }
  toggling = false;
}

function openSettings(): void {
  if (!settingsOpen) settingsOpenedPanel = !open;
  settingsOpen = true;
  void (open ? Promise.resolve() : setOpen(true)).then(() => {
    settingsOpen = true;
    render();
    settingsView.heading.focus({ preventScroll: true });
    void loadDetected();
  });
}

/** Back to the previous view: the panel Settings was opened from, or the pill when Settings opened the panel. */
function settingsBack(): void {
  if (settingsOpenedPanel) {
    void setOpen(false).then(() => pill.chevron.focus({ preventScroll: true }));
    return;
  }
  settingsOpen = false;
  render();
  (isFirstRun() ? first.heading : list.settingsButton).focus({ preventScroll: true });
}

async function loadDetected(): Promise<void> {
  const result = await detect();
  if (!result.ok) return;
  detected = result.value;
  render();
}

// Commands

/** Takes the settings a command returns; a failed command re-renders the stored values. */
async function take(pending: Promise<Result<Settings>>): Promise<void> {
  const result = await pending;
  if (result.ok) settings = result.value;
  render();
}

async function refresh(): Promise<void> {
  if (refreshing) return;
  refreshing = true;
  render();
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
  // A new install opens straight onto the first screen.
  if (isFirstRun()) await setOpen(true);
}

// Wiring

setProblemListener((message) => {
  problem.textContent = message ?? "";
  show(problem, message !== null);
});

document.addEventListener("keydown", (event) => {
  if (event.key !== "Escape" || event.defaultPrevented || !open) return;
  event.preventDefault();
  if (settingsOpen) {
    settingsBack();
    return;
  }
  void setOpen(false).then(() => pill.chevron.focus({ preventScroll: true }));
});

/** Windows text size zooms the web view; the window is sized from this zoom, and follows changes. */
function watchZoom(): void {
  void setZoom(window.devicePixelRatio);
  matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`).addEventListener("change", watchZoom, { once: true });
}
watchZoom();

// Listeners go first so an event that fires while the first load runs is not missed.
void Promise.all([
  onSnapshot((next) => takeSnapshot(next)),
  onSettings((next) => {
    settings = next;
    render();
  }),
  onBackdrop((payload) => document.documentElement.classList.toggle("no-acrylic", !payload.acrylic)),
  onOpenSettings(() => openSettings()),
  onTheme((theme) => (document.documentElement.dataset.theme = theme)),
]).then(() => load());

// Reset times count down between snapshots.
setInterval(render, 30_000);
