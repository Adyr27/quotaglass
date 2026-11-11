// The flyout window. Hover mode shows the read-only overlay for one service; popover mode shows
// the account list, Settings and the first screen. The pointer calls keep the Rust hover protocol informed.

import {
  addAccount,
  connect,
  connectSource,
  detect,
  flyoutClose,
  flyoutPointer,
  flyoutSize,
  getSettings,
  getSnapshot,
  onFlyoutMode,
  onPillLocked,
  onSnapshot,
  openCli,
  openUrl,
  refreshNow,
  removeAccount,
  setHidden,
  setLabel,
  setPillLocked,
  setProblemListener,
  updateSettings,
  type Result,
} from "./ipc";
import type { Detected, FlyoutMode, Settings, Snapshot } from "./types";
import { h, show } from "./view/dom";
import { createOverlay } from "./view/overlay";
import { createPopover } from "./view/popover";

const RESIZE_GAP_MS = 50;
const REFRESH_MS = 30_000;

export function startFlyout(): void {
  let snapshot: Snapshot | null = null;
  let mode: FlyoutMode = { mode: "hidden", service: null, view: null };
  let lastGeneratedAtMs = -1;
  let tick: number | undefined;
  let lastSent = 0;
  let pending: number | undefined;
  let settings: Settings | null = null;
  let detected: Detected | null = null;
  let locked = false;
  let refreshing = false;

  /** Takes the settings a command returns; a failed command re-renders the stored values. */
  async function takeSettings(result: Promise<Result<Settings>>): Promise<void> {
    const r = await result;
    if (r.ok) {
      settings = r.value;
      locked = r.value.pillLocked;
    }
    render();
  }

  async function loadDetected(): Promise<void> {
    const r = await detect();
    if (!r.ok) return;
    detected = r.value;
    render();
  }

  async function refresh(): Promise<void> {
    if (refreshing) return;
    refreshing = true;
    render();
    await refreshNow();
    const latest = await getSnapshot();
    if (latest.ok) take(latest.value);
    refreshing = false;
    render();
  }

  const overlay = createOverlay();
  const popover = createPopover({
    onRefresh: () => void refresh(),
    onRemove: (a) => void takeSettings(removeAccount(a.id)),
    onHide: (a) => void takeSettings(setHidden(a.id, true)),
    onSignIn: (a) => void (a.profileId === null ? connect(a.service) : openCli(a.id)),
    onRedeem: () => void openUrl("openAiUsage"),
    onAdd: (service) => void takeSettings(addAccount(service)),
    onPatch: (patch) => void takeSettings(updateSettings(patch)),
    onSource: (c, on) => void takeSettings(connectSource(c, on)),
    onUnhide: (id) => void takeSettings(setHidden(id, false)),
    onLabel: (id, label) => void takeSettings(setLabel(id, label)),
    onConnectFirst: (c) => void takeSettings(connectSource(c, true)),
    onTogglePin: () => void setPillLocked(!locked),
  });
  const problem = h("p", { class: "problem", role: "status", hidden: true });
  setProblemListener((message) => {
    problem.textContent = message ?? "";
    show(problem, message !== null);
  });
  const popoverBox = h("div", { class: "popover-box" }, [problem, popover.el]);
  const content = h("div", { class: "flyout-content" }, [overlay.el, popoverBox]);
  const root = h("div", { class: "flyout-root" }, [content]);
  root.addEventListener("mouseenter", () => void flyoutPointer(true));
  root.addEventListener("mouseleave", () => void flyoutPointer(false));
  document.getElementById("app")?.append(root);

  document.addEventListener("keydown", (event) => {
    if (event.key !== "Escape" || event.defaultPrevented || mode.mode !== "popover") return;
    event.preventDefault();
    if (!popover.back()) void flyoutClose();
  });

  function render(): void {
    const hovering = mode.mode === "hover" && mode.service !== null;
    show(overlay.el, hovering);
    show(popoverBox, mode.mode === "popover");
    if (mode.mode === "popover" && snapshot !== null) {
      popover.update({ accounts: snapshot.accounts, settings, detected, locked, refreshing, nowMs: Date.now() });
    }
    if (hovering && snapshot !== null && mode.service !== null) {
      overlay.update(mode.service, snapshot.accounts, Date.now());
    }
    // Reset countdowns need a redraw even when no snapshot arrives.
    if (mode.mode !== "hidden" && tick === undefined) tick = window.setInterval(render, REFRESH_MS);
    if (mode.mode === "hidden" && tick !== undefined) {
      window.clearInterval(tick);
      tick = undefined;
    }
  }

  function sendSize(): void {
    pending = undefined;
    lastSent = Date.now();
    void flyoutSize(content.scrollHeight);
  }

  // At most one size message every 50 ms; the last change always goes out.
  new ResizeObserver(() => {
    if (pending !== undefined) return;
    const wait = Math.max(0, RESIZE_GAP_MS - (Date.now() - lastSent));
    pending = window.setTimeout(sendSize, wait);
  }).observe(content);

  function take(next: Snapshot): void {
    // A slow answer to an older request must not replace a newer snapshot.
    if (next.generatedAtMs < lastGeneratedAtMs) return;
    lastGeneratedAtMs = next.generatedAtMs;
    snapshot = next;
    render();
  }

  // Listeners go first so an event that fires during the first load is not missed.
  void Promise.all([
    onSnapshot(take),
    onPillLocked((next) => {
      locked = next;
      render();
    }),
    onFlyoutMode((next) => {
      if (next.mode !== mode.mode || next.service !== mode.service) root.scrollTop = 0;
      mode = next; // the snapshot is data, not view state: keep it so the next open has numbers at once
      if (next.mode === "popover") {
        popover.open(next.service, next.view);
        void loadDetected();
      }
      render();
    }),
  ]).then(async () => {
    const [loaded, first] = await Promise.all([getSettings(), getSnapshot()]);
    if (loaded.ok) {
      settings = loaded.value;
      locked = loaded.value.pillLocked;
    }
    if (first.ok) take(first.value);
    else render();
  });
}
