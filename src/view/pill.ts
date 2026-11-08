// The pill: up to four pinned accounts, or the single worst one, each as dot, name, % used and a
// compact strip. "Best now" marks the account with the most room; when that account is not shown it
// joins as a fifth, compact item. The chevron opens and closes the panel.

import type { AccountView, Settings } from "../types";
import { usedWhole, visibleAccounts, worstBucket, worstUsed } from "../format";
import { h, iconChevron, setAttr, syncChildren } from "./dom";
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

function item(a: AccountView, compact: boolean): HTMLElement {
  // The pill answers "how close is this account to a limit", so it shows the worst limit.
  const worst = worstBucket(a);
  const used = worst === null ? null : usedWhole(worst.usedPercent);
  const top = h("span", { class: "pill-top" }, [
    h("span", { class: "dot", "data-service": a.service, "aria-hidden": "true" }),
    h("span", { class: "pill-name" }, [a.label]),
    a.bestNow ? h("span", { class: "best" }, ["Best now"]) : null,
    compact ? null : h("span", { class: "pill-pct num" }, [used === null ? "—" : `${used}%`]),
  ]);
  return h("span", { class: compact ? "pill-item pill-item-compact" : "pill-item", "data-dim": dimmed(a) }, [
    top,
    worst !== null && !compact ? createStrip(worst, a.service, true) : null,
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
      const shown = pillAccounts(accounts, settings?.pinnedAccounts ?? []);
      const best = accounts.find((a) => a.bestNow && !a.hidden);
      const wanted = shown.map((a) => item(a, false));
      if (best !== undefined && !shown.includes(best)) wanted.push(item(best, true));
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
