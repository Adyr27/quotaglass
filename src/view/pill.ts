// The pill: one progress ring per service, and a grip that resizes it. Everything scales with
// the CSS variable --s, so the viewport is exactly base x scale CSS px and slot rectangles are true anchors.

import type { Service, ServiceView, FlyoutView } from "../types";
import { h, syncChildren } from "./dom";
import { createSlot, updateSlot } from "./ring";

const SCALE_MIN = 0.65;
const SCALE_MAX = 1.6;
/** Pointer travel (screen px) that turns a press into a drag. */
const DRAG_PX = 4;
/** Fewest ms between two scale commands while the grip moves. */
const SCALE_EVERY_MS = 40;

export interface PillDeps {
  hover(service: Service, inside: boolean, anchorX: number): void;
  open(service: Service | null, view: FlyoutView | null): void;
  close(): void;
  /** `final` is true for the last value of one grip drag. */
  scale(value: number, final: boolean): void;
  startDrag(): void;
}

export interface Pill {
  readonly el: HTMLElement;
  /** The first slot, for the tray's "Focus QuotaGlass". */
  focusFirst(): void;
  update(services: readonly ServiceView[], locked: boolean, scale: number): void;
}

const clamp = (value: number, lo: number, hi: number): number => Math.min(hi, Math.max(lo, value));

export function createPill(deps: PillDeps): Pill {
  const slots = new Map<Service, HTMLButtonElement>();
  const neutral = createSlot(null);
  const grip = h("div", { class: "grip", "aria-hidden": "true" }, [h("i"), h("i"), h("i")]);
  const el = h("div", { class: "pill", role: "group", "aria-label": "QuotaGlass usage" });
  let locked = false;
  let gripping = false;

  const serviceOf = (slot: Element | null): Service | null => (slot?.getAttribute("data-service") as Service | null) ?? null;
  const centre = (slot: HTMLElement): number => {
    const r = slot.getBoundingClientRect();
    return r.left + r.width / 2;
  };

  function slotFor(service: Service): HTMLButtonElement {
    let slot = slots.get(service);
    if (slot === undefined) {
      slot = createSlot(service);
      slots.set(service, slot);
      wire(slot, service);
    }
    return slot;
  }

  // Slot events

  let dragged = false;
  // True from the drag start until the pointer is seen again with no button down. WebView2 can fire
  // pointerenter inside Windows' move loop, and the loop swallows pointerup.
  let dragging = false;
  const enter = (slot: HTMLElement, service: Service): void => {
    if (!dragging) deps.hover(service, true, centre(slot));
  };
  const leave = (service: Service): void => deps.hover(service, false, 0);

  function wire(slot: HTMLButtonElement, service: Service): void {
    slot.addEventListener("pointerenter", () => enter(slot, service));
    slot.addEventListener("pointerleave", () => leave(service));
    slot.addEventListener("focus", () => enter(slot, service));
    slot.addEventListener("blur", () => leave(service));
    // A button turns Enter and Space into a click, so one handler covers mouse and keyboard.
    slot.addEventListener("click", () => {
      if (dragged) {
        dragged = false;
        return;
      }
      deps.open(service, null);
    });
  }

  neutral.addEventListener("click", () => {
    if (dragged) {
      dragged = false;
      return;
    }
    deps.open(null, "first");
  });

  el.addEventListener("keydown", (event) => {
    if (event.key === "Escape") deps.close();
  });

  // Drag: a press that moves more than DRAG_PX is a drag, a short press is a click.
  // Windows runs its own move loop, so pointerup can be lost: `dragged` resets on the next press or click.

  let armed = false;
  let startX = 0;
  let startY = 0;

  function endDrag(event: PointerEvent): void {
    if (!dragging) return;
    dragging = false;
    const slot = (event.target as Element).closest<HTMLElement>(".slot");
    const service = serviceOf(slot);
    if (slot !== null && service !== null) enter(slot, service);
  }

  el.addEventListener("pointerdown", (event) => {
    dragged = false;
    endDrag(event);
    if (event.button !== 0 || locked || (event.target as Element).closest(".grip") !== null) return;
    armed = true;
    startX = event.screenX;
    startY = event.screenY;
  });
  el.addEventListener("pointermove", (event) => {
    if (event.buttons === 0) endDrag(event);
    if (!armed || Math.hypot(event.screenX - startX, event.screenY - startY) <= DRAG_PX) return;
    armed = false;
    dragged = true;
    dragging = true;
    const under = serviceOf((event.target as Element).closest(".slot"));
    if (under !== null) deps.hover(under, false, 0);
    deps.close();
    deps.startDrag();
  });
  const disarm = (event: PointerEvent): void => {
    armed = false;
    endDrag(event);
  };
  el.addEventListener("pointerup", disarm);
  el.addEventListener("pointercancel", disarm);

  // Grip: dragging it right grows the pill. screenX, not clientX, because the window grows under the pointer.

  let x0 = 0;
  let w0 = 1;
  let s0 = 1;
  let next = 1;
  let lastSent = 0;

  grip.addEventListener("pointerdown", (event) => {
    if (event.button !== 0 || locked) return;
    event.stopPropagation();
    grip.setPointerCapture(event.pointerId);
    gripping = true;
    x0 = event.screenX;
    w0 = window.innerWidth;
    s0 = Number(el.style.getPropertyValue("--s")) || 1;
    next = s0;
  });
  grip.addEventListener("pointermove", (event) => {
    if (!gripping) return;
    next = clamp((s0 * (w0 + event.screenX - x0)) / w0, SCALE_MIN, SCALE_MAX);
    el.style.setProperty("--s", String(next));
    const now = performance.now();
    if (now - lastSent >= SCALE_EVERY_MS) {
      lastSent = now;
      deps.scale(next, false);
    }
  });
  const release = (event: PointerEvent): void => {
    if (!gripping) return;
    gripping = false;
    if (grip.hasPointerCapture(event.pointerId)) grip.releasePointerCapture(event.pointerId);
    deps.scale(next, true);
  };
  grip.addEventListener("pointerup", release);
  grip.addEventListener("pointercancel", release);

  return {
    el,
    focusFirst() {
      (el.querySelector(".slot") as HTMLElement | null)?.focus();
    },
    update(services, isLocked, scale) {
      locked = isLocked;
      el.dataset.locked = String(isLocked);
      if (!gripping) el.style.setProperty("--s", String(scale));
      const wanted = services.length === 0 ? [neutral] : services.map((sv) => slotFor(sv.service));
      services.forEach((sv) => updateSlot(slotFor(sv.service), sv));
      syncChildren(el, [...wanted, grip]);
    },
  };
}
