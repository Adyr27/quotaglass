// The pill: up to four pinned accounts, or the single worst one, each as provider icon, % used and a
// compact strip. No names: the icon says the provider, and the full name is in the tooltip and for
// screen readers. "Best now" shows only in the open panel. The chevron opens and closes the panel.

import type { AccountView, Settings } from "../types";
import { SERVICE_NAME, usedWhole, visibleAccounts, worstBucket, worstUsed } from "../format";
import { h, iconChevron, providerMark, setAttr, syncChildren } from "./dom";
import { createStrip } from "./strip";

export interface Pill {
  readonly el: HTMLElement;
  readonly chevron: HTMLButtonElement;
  update(accounts: readonly AccountView[], settings: Settings | null, open: boolean): void;
}

/** What the pill shows: pinned accounts in order, else the single worst account. */
export function pillAccounts(accounts: readonly AccountView[], pinned: readonly string[]): AccountView[] {
  const visible = visibleAccounts(accounts);
  const chosen = pinned.map((id) => visible.find((a) => a.id === id)).filter((a): a is AccountView => a !== undefined);
  if (chosen.length > 0) return chosen.slice(0, 4);
  let worst: AccountView | null = null;
  for (const a of visible) {
    const used = worstUsed(a);
    if (used !== null && (worst === null || used > (worstUsed(worst) ?? -1))) worst = a;
  }
  return worst === null ? [] : [worst];
}

function dimmed(a: AccountView): boolean {
  return a.state === "loginExpired" || a.state === "profileMissing" || a.freshness === "stale";
}

function item(a: AccountView): HTMLElement {
  // The pill answers "how close is this account to a limit", so it shows the worst limit.
  const worst = worstBucket(a);
  const used = worst === null ? null : usedWhole(worst.usedPercent);
  const name = a.label === SERVICE_NAME[a.service] ? a.label : `${SERVICE_NAME[a.service]} · ${a.label}`;
  const top = h("span", { class: "pill-top" }, [
    providerMark(a.service),
    h("span", { class: "pill-pct num" }, [used === null ? "—" : `${used}%`]),
  ]);
  return h("span", { class: "pill-item", "data-dim": dimmed(a), role: "img", "aria-label": `${name} ${used === null ? "no reading" : `${used}% used`}`, title: name }, [
    top,
    worst !== null ? createStrip(worst, a.service, true) : null,
  ]);
}

/** Tauri drags only from elements that carry the attribute themselves, so every non-button part gets it. */
function draggable(root: HTMLElement): void {
  for (const el of [root, ...Array.from(root.querySelectorAll<HTMLElement>("*"))]) {
    if (el.closest("button") === null) el.setAttribute("data-tauri-drag-region", "");
  }
}

export function createPill(onToggle: () => void): Pill {
  const items = h("span", { class: "pill-items" });
  const empty = h("span", { class: "pill-empty" });
  const chevron = h("button", { class: "icon-btn pill-chevron", type: "button", "aria-expanded": "false", "aria-label": "Open panel" }, [
    iconChevron("chevron"),
  ]);
  chevron.addEventListener("click", onToggle);
  const el = h("div", { class: "pill" }, [items, empty, chevron]);

  return {
    el,
    chevron,
    update(accounts, settings, open) {
      const wanted = pillAccounts(accounts, settings?.pinnedAccounts ?? []).map(item);
      syncChildren(items, wanted);
      empty.textContent = accounts.length === 0 ? "Connect an account" : "No readings yet";
      empty.hidden = wanted.length > 0;
      items.hidden = wanted.length === 0;
      setAttr(chevron, "aria-expanded", open ? "true" : "false");
      setAttr(chevron, "aria-label", open ? "Close panel" : "Open panel");
      draggable(el);
    },
  };
}
