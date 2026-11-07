// The all-limits dialog for one account. Focus stays inside while it is open.

import { formatReset, formatWindow, metaLine, sentenceCase, usedRatio, usedWhole } from "../format";
import type { AccountView } from "../types";
import { h, setText, show } from "./dom";

const WARN_AT_PERCENT = 80;
const FOCUSABLE = "button:not([disabled]), [href], input, select, textarea, [tabindex]:not([tabindex='-1'])";

export interface Overlay {
  readonly el: HTMLElement;
  /** Id of the account on show, or null when closed. */
  readonly openId: string | null;
  open(view: AccountView, nowMs: number): void;
  update(view: AccountView, nowMs: number): void;
  close(): void;
}

export function createOverlay(onClose: () => void): Overlay {
  let openId: string | null = null;
  let shown = "";

  const title = h("h2", { class: "dialog-title", id: "overlay-title" });
  const plan = h("span", { class: "pill" });
  const meta = h("p", { class: "dialog-meta" });
  const list = h("div", { class: "ov-list" });
  const closeButton = h("button", { class: "btn", type: "button" }, ["Close"]);
  const dialog = h("div", { class: "dialog", role: "dialog", "aria-modal": "true", "aria-labelledby": "overlay-title" }, [
    h("div", { class: "dialog-head" }, [title, plan]),
    meta,
    list,
    h("div", { class: "dialog-foot" }, [closeButton]),
  ]);
  const el = h("div", { class: "scrim", hidden: true }, [dialog]);

  closeButton.addEventListener("click", () => onClose());
  el.addEventListener("click", (event) => {
    if (event.target === el) onClose();
  });
  el.addEventListener("keydown", (event) => {
    if (event.key !== "Tab") return;
    const items = Array.from(dialog.querySelectorAll<HTMLElement>(FOCUSABLE));
    if (items.length === 0) return;
    const first = items[0];
    const last = items[items.length - 1];
    const active = document.activeElement;
    if (event.shiftKey && (active === first || !dialog.contains(active))) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && (active === last || !dialog.contains(active))) {
      event.preventDefault();
      first.focus();
    }
  });

  function bucketRow(view: AccountView, index: number, nowMs: number): HTMLElement {
    const bucket = view.buckets[index];
    const used = usedWhole(bucket.usedPercent);
    const fill = h("div", { class: "bar-fill" });
    fill.style.setProperty("--p", String(usedRatio(bucket.usedPercent)));
    // A stale bucket's window already reset or its reading is too old: dim it and never claim the limit is hit.
    const stale = bucket.freshness === "stale";
    const status = bucket.limitReached && !stale
      ? h("span", { class: "pill pill-neg" }, ["Limit reached"])
      : h("span", { class: !stale && used >= WARN_AT_PERCENT ? "ov-used is-warn" : "ov-used" }, [`${used}% used`]);
    const reset = formatReset(bucket.resetsAtMs, nowMs);
    const detail = [formatWindow(bucket.windowSecs), reset === "" ? "" : `Resets ${reset}`].filter((part) => part !== "").join(" \u00b7 ");
    return h("div", { class: stale ? "ov-bucket is-dim" : "ov-bucket" }, [
      h("div", { class: "ov-row" }, [h("span", { class: "ov-name", title: bucket.name }, [bucket.name]), status]),
      h("div", { class: "bar", "aria-hidden": "true" }, [fill]),
      detail === "" ? null : h("p", { class: "ov-meta" }, [detail]),
    ]);
  }

  function paint(view: AccountView, nowMs: number): void {
    dialog.dataset.service = view.service;
    setText(title, view.label);
    const planText = view.plan === null ? "" : sentenceCase(view.plan);
    setText(plan, planText);
    show(plan, planText !== "");
    const metaText = metaLine({ resetsAtMs: null, source: view.source, freshness: view.freshness, ageSecs: view.ageSecs }, nowMs);
    setText(meta, metaText);
    show(meta, metaText !== "");

    // Rebuild the rows only when something on them changed, so a refresh does not replay the bars.
    const rowsKey = JSON.stringify([view.buckets, Math.floor(nowMs / 60_000)]);
    if (rowsKey === shown) return;
    shown = rowsKey;
    list.replaceChildren(...view.buckets.map((_, index) => bucketRow(view, index, nowMs)));
  }

  return {
    el,
    get openId() {
      return openId;
    },
    open(view, nowMs) {
      openId = view.id;
      shown = "";
      paint(view, nowMs);
      show(el, true);
      closeButton.focus({ preventScroll: true });
    },
    update(view, nowMs) {
      if (openId === view.id) paint(view, nowMs);
    },
    close() {
      openId = null;
      shown = "";
      show(el, false);
    },
  };
}
