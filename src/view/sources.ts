// The Sources rows. The first-run panel and the settings panel both use them.

import { SOURCE_ORDER, sourceRowName } from "../format";
import type { SourceStatus, SwitchableSource } from "../types";
import { createSwitch, h, setText, show, type Switch } from "./dom";

export interface SourceRows {
  readonly el: HTMLElement;
  /** Shows one row per status. A source missing from `rows` is hidden. */
  update(rows: readonly SourceStatus[]): void;
}

interface Row {
  el: HTMLElement;
  statusText: HTMLElement;
  unofficial: HTMLElement;
  toggle: Switch;
}

/** `prefix` keeps element ids unique when two panels are in the page at once. */
export function createSourceRows(
  prefix: string,
  onToggle: (kind: SwitchableSource, next: boolean) => void,
): SourceRows {
  const rows = new Map<SwitchableSource, Row>();
  const el = h("div", { class: "src-rows" });

  for (const kind of SOURCE_ORDER) {
    const nameId = `${prefix}-${kind}-name`;
    const statusId = `${prefix}-${kind}-status`;
    const statusText = h("span");
    const unofficial = h("span", { class: "pill" }, ["Unofficial endpoint"]);
    const status = h("div", { class: "src-status", id: statusId }, [statusText, unofficial]);
    const text = h("div", { class: "src-text" }, [h("span", { class: "src-name", id: nameId }, [sourceRowName(kind)]), status]);
    const toggle = createSwitch({
      labelledBy: nameId,
      describedBy: statusId,
      onChange: (next) => onToggle(kind, next),
    });
    const row = h("div", { class: "src-row" }, [text, toggle.el]);
    rows.set(kind, { el: row, statusText, unofficial, toggle });
    el.append(row);
  }

  function update(statuses: readonly SourceStatus[]): void {
    for (const [kind, row] of rows) {
      const found = statuses.find((s) => s.kind === kind);
      show(row.el, found !== undefined);
      if (found === undefined) continue;
      setText(row.statusText, found.detected ? "Found" : "Not found");
      show(row.unofficial, found.unofficial);
      row.toggle.set(found.enabled);
    }
  }

  return { el, update };
}
