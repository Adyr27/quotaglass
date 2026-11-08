// The pace strip: ticks every 10%, a fill at % used (red on fast burn), and a 2 px ink marker at
// the share of the window that has passed. The marker is left out when the source gives no window.

import type { BucketView, Service } from "../types";
import { usedWhole } from "../format";
import { h } from "./dom";

export function createStrip(b: BucketView, service: Service, compact = false): HTMLElement {
  const used = usedWhole(b.usedPercent);
  const fill = h("span", { class: "strip-fill" });
  fill.style.setProperty("--p", String(used / 100));
  const ticks = h("span", { class: "strip-ticks", "aria-hidden": "true" }, Array.from({ length: 9 }, () => h("i")));
  const children: HTMLElement[] = [fill, ticks];
  if (b.passedPercent !== null && !compact) {
    const pace = h("span", { class: "strip-pace", "aria-hidden": "true" });
    pace.style.setProperty("--t", String(Math.min(100, Math.max(0, b.passedPercent)) / 100));
    children.push(pace);
  }
  return h(
    "span",
    {
      class: compact ? "strip strip-compact" : "strip",
      role: "meter",
      "aria-valuemin": 0,
      "aria-valuemax": 100,
      "aria-valuenow": used,
      "aria-label": `${b.name} ${used}% used${b.fastBurn ? ", fast burn" : ""}`,
      "data-service": service,
      "data-burn": b.fastBurn,
      "data-stale": b.freshness === "stale",
    },
    children,
  );
}
