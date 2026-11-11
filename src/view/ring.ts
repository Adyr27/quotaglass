// One pill slot: a service mark inside a progress ring. The ring shows the best reading over the
// service's visible accounts. A slot with no service is the neutral "Connect an account" slot.

import type { Service, ServiceView } from "../types";
import { SERVICE_NAME, usedWhole } from "../format";
import { h, providerMark, setAttr, svg } from "./dom";

const RADIUS = 10.75;
const CIRCUMFERENCE = 2 * Math.PI * RADIUS;

export function createSlot(service: Service | null): HTMLButtonElement {
  const track = svg("circle", { cx: 12, cy: 12, r: RADIUS, fill: "none", "stroke-width": 2.5, class: "ring-track" });
  const fill = svg("circle", {
    cx: 12,
    cy: 12,
    r: RADIUS,
    fill: "none",
    "stroke-width": 2.5,
    class: "ring-fill",
    transform: "rotate(-90 12 12)",
  });
  const ring = svg("svg", { viewBox: "0 0 24 24", "aria-hidden": "true" }, [track, fill]);
  const slot = h(
    "button",
    {
      type: "button",
      class: "slot",
      "data-service": service,
      "aria-haspopup": "dialog",
      "aria-label": "Connect an account",
      title: "Connect an account",
    },
    [ring, service === null ? null : providerMark(service, "mark slot-mark")],
  );
  if (service === null) {
    track.classList.add("ring-track--dashed");
    slot.dataset.dim = "true";
  }
  return slot;
}

function label(sv: ServiceView, used: number | null): string {
  const name = SERVICE_NAME[sv.service];
  let text = used === null ? `${name}, no reading` : `${name}, ${used}% used`;
  if (used !== null && sv.accounts > 1) text += `, best of ${sv.accounts} accounts`;
  if (sv.rateLimited) text += ", rate limited";
  return text;
}

export function updateSlot(el: HTMLButtonElement, sv: ServiceView | null): void {
  if (sv === null) return; // the neutral slot never changes
  const track = el.querySelector(".ring-track");
  const fill = el.querySelector(".ring-fill");
  if (track === null || fill === null) return;

  const p = sv.usedPercent === null ? null : Math.min(100, Math.max(0, sv.usedPercent));
  fill.setAttribute("stroke-dasharray", p === null ? `0 ${CIRCUMFERENCE}` : `${(CIRCUMFERENCE * p) / 100} ${CIRCUMFERENCE}`);
  track.classList.toggle("ring-track--dashed", p === null);
  setAttr(el, "data-dim", p === null ? "true" : null);
  setAttr(el, "data-level", sv.allExhausted ? "full" : p !== null && p >= 80 ? "warn" : "ok");
  setAttr(el, "data-rate-limited", sv.rateLimited ? "true" : null);

  const text = label(sv, sv.usedPercent === null ? null : usedWhole(sv.usedPercent));
  setAttr(el, "aria-label", text);
  setAttr(el, "title", text);
}
