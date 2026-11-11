// The pill window: shows one ring per service and sends hover, click, drag and resize to Rust.

import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  flyoutClose,
  flyoutHover,
  flyoutOpenPopover,
  getSettings,
  getSnapshot,
  onFocusPill,
  onPillLocked,
  onSnapshot,
  setPillScale,
  setPillSlots,
} from "./ipc";
import type { ServiceView, Snapshot } from "./types";
import { createPill } from "./view/pill";

export function startPill(): void {
  let services: readonly ServiceView[] = [];
  let locked = false;
  let scale = 1;
  let lastSlots = 0;
  let lastGeneratedAtMs = -1;

  const pill = createPill({
    hover: (service, inside, anchorX) => void flyoutHover(service, inside, anchorX),
    open: (service, view) => void flyoutOpenPopover(service, view),
    close: () => void flyoutClose(),
    scale: (value) => {
      scale = value;
      void setPillScale(value);
    },
    startDrag: () => void getCurrentWindow().startDragging(),
  });
  document.getElementById("app")?.append(pill.el);

  function render(): void {
    pill.update(services, locked, scale);
    const slots = Math.max(1, services.length);
    if (slots !== lastSlots) {
      lastSlots = slots;
      void setPillSlots(slots);
    }
  }

  function take(snapshot: Snapshot): void {
    // A slow answer to an older request must not replace a newer snapshot.
    if (snapshot.generatedAtMs < lastGeneratedAtMs) return;
    lastGeneratedAtMs = snapshot.generatedAtMs;
    services = snapshot.services;
    render();
  }

  // Listeners go first so an event that fires during the first load is not missed.
  void Promise.all([
    onSnapshot(take),
    onPillLocked((next) => {
      locked = next;
      render();
    }),
    onFocusPill(() => pill.focusFirst()),
  ]).then(async () => {
    const [settings, snapshot] = await Promise.all([getSettings(), getSnapshot()]);
    if (settings.ok) {
      locked = settings.value.pillLocked;
      scale = settings.value.pillScale;
    }
    if (snapshot.ok) take(snapshot.value);
    else render();
    // A new install opens straight onto the first screen.
    if (settings.ok && settings.value.connected.length === 0 && settings.value.addedAccounts.length === 0) {
      void flyoutOpenPopover(null, "first");
    }
  });
}
