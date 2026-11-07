// The all-limits dialog for one account. Focus stays inside while it is open.

import { formatReset, formatWindow, metaLine, sentenceCase, usedRatio, usedWhole } from "../format";
import type { AccountView, SourceKind } from "../types";
import { h, setText, show } from "./dom";

const WARN_AT_PERCENT = 80;
const FOCUSABLE = "button:not([disabled]), [href], input, select, textarea, [tabindex]:not([tabindex='-1'])";

export interface Overlay {
  readonly el: HTMLElement;
  /** Id of the account on show, or null when closed. */
  readonly openId: string | null;
  open(view: AccountView, nowMs: number, pin: SourceKind | null): void;
  update(view: AccountView, nowMs: number, pin: SourceKind | null): void;
  close(): void;
}

const SOURCE_SHORT: Record<SourceKind, string> = { direct: "Direct", orca: "Orca", omp: "OMP", sessionKey: "Session key" };

/** `onPin(id, null)` returns the account to automatic source choice. */
export function createOverlay(onClose: () => void, onPin: (accountId: string, kind: SourceKind | null) => void): Overlay {
  let openId: string | null = null;
  let shown = "";
  let pickerShown = "";

  const title = h("h2", { class: "dialog-title", id: "overlay-title" });
  const plan = h("span", { class: "pill" });
  const meta = h("p", { class: "dialog-meta" });
  const list = h("div", { class: "ov-list" });
  const pickerGroup = h("div", { class: "seg", role: "radiogroup", "aria-labelledby": "overlay-source-label" });
  const picker = h("div", { class: "set-row ov-source", hidden: true }, [
    h("span", { id: "overlay-source-label" }, ["Source"]),
    pickerGroup,
  ]);
  const closeButton = h("button", { class: "btn", type: "button" }, ["Close"]);
  const dialog = h("div", { class: "dialog", role: "dialog", "aria-modal": "true", "aria-labelledby": "overlay-title" }, [
    h("div", { class: "dialog-head" }, [title, plan]),
    meta,
    picker,
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

  // Source picker: shown only when more than one source knows the account. One tab stop; arrows move and select.
  function paintPicker(view: AccountView, pin: SourceKind | null): void {
    const choices: (SourceKind | null)[] = view.sources.length > 1 ? [null, ...view.sources] : [];
    const key = JSON.stringify([view.id, choices, pin]);
    if (key === pickerShown) return;
    pickerShown = key;
    show(picker, choices.length > 0);
    const selected = Math.max(choices.indexOf(pin), 0);
    const id = view.id;
    const radios = choices.map((kind, index) =>
      h(
        "button",
        { class: "seg-item", type: "button", role: "radio", "aria-checked": index === selected ? "true" : "false", tabindex: index === selected ? 0 : -1 },
        [kind === null ? "Auto" : SOURCE_SHORT[kind]],
      ),
    );
    radios.forEach((radio, index) => {
      radio.addEventListener("click", () => onPin(id, choices[index]));
      radio.addEventListener("keydown", (event) => {
        const step = event.key === "ArrowRight" || event.key === "ArrowDown" ? 1 : event.key === "ArrowLeft" || event.key === "ArrowUp" ? -1 : 0;
        if (step === 0) return;
        event.preventDefault();
        const next = (index + step + radios.length) % radios.length;
        radios[next].focus();
        onPin(id, choices[next]);
      });
    });
    const hadFocus = pickerGroup.contains(document.activeElement);
    pickerGroup.replaceChildren(...radios);
    if (hadFocus && radios.length > 0) radios[selected].focus({ preventScroll: true });
  }

  function paint(view: AccountView, nowMs: number, pin: SourceKind | null): void {
    dialog.dataset.service = view.service;
    setText(title, view.label);
    const planText = view.plan === null ? "" : sentenceCase(view.plan);
    setText(plan, planText);
    show(plan, planText !== "");
    const metaText = metaLine({ resetsAtMs: null, source: view.source, freshness: view.freshness, ageSecs: view.ageSecs }, nowMs);
    setText(meta, metaText);
    show(meta, metaText !== "");
    paintPicker(view, pin);

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
    open(view, nowMs, pin) {
      openId = view.id;
      shown = "";
      pickerShown = "";
      paint(view, nowMs, pin);
      show(el, true);
      closeButton.focus({ preventScroll: true });
    },
    update(view, nowMs, pin) {
      if (openId === view.id) paint(view, nowMs, pin);
    },
    close() {
      openId = null;
      shown = "";
      pickerShown = "";
      show(el, false);
    },
  };
}
