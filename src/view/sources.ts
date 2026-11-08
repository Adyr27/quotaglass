// The Sources rows, one per `connected` entry. The first-run panel and the settings panel both use them.

import { CONNECTION_ORDER, sourceRowName } from "../format";
import type { Connection, Detected } from "../types";
import { createSwitch, h, setText, type Switch } from "./dom";

export interface SourceRows {
  readonly el: HTMLElement;
  /** `detected` is null until the first check answers; `on` is the list of switched-on connections. */
  update(detected: Detected | null, on: readonly Connection[]): void;
}

interface Row {
  statusText: HTMLElement;
  toggle: Switch;
}

const DETECTED_KEY: Record<Connection, keyof Detected> = {
  claudeCurrent: "claude",
  codexCurrent: "codex",
  antigravityApp: "antigravityApp",
  orca: "orca",
};

function statusOf(kind: Connection, detected: Detected | null): string {
  if (detected === null) return "";
  if (kind === "antigravityApp") return "Read while the app runs";
  return detected[DETECTED_KEY[kind]] ? "Found" : "Not found";
}

/** `prefix` keeps element ids unique when two panels are in the page at once. */
export function createSourceRows(prefix: string, onToggle: (kind: Connection, next: boolean) => void): SourceRows {
  const rows = new Map<Connection, Row>();
  const el = h("div", { class: "src-rows" });

  for (const kind of CONNECTION_ORDER) {
    const nameId = `${prefix}-${kind}-name`;
    const statusId = `${prefix}-${kind}-status`;
    const statusText = h("span");
    const status = h("div", { class: "src-status", id: statusId }, [statusText]);
    const text = h("div", { class: "src-text" }, [h("span", { class: "src-name", id: nameId }, [sourceRowName(kind)]), status]);
    const toggle = createSwitch({
      labelledBy: nameId,
      describedBy: statusId,
      onChange: (next) => onToggle(kind, next),
    });
    el.append(h("div", { class: "src-row" }, [text, toggle.el]));
    rows.set(kind, { statusText, toggle });
  }

  function update(detected: Detected | null, on: readonly Connection[]): void {
    for (const [kind, row] of rows) {
      setText(row.statusText, statusOf(kind, detected));
      row.toggle.set(on.includes(kind));
    }
  }

  return { el, update };
}
