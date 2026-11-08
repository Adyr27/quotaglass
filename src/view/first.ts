// The first screen: nothing is read until the person turns a source on. One button per source;
// a source this PC does not have is disabled with the reason in words.

import type { Connection, Detected } from "../types";
import { CONNECTION_NAME, CONNECTION_ORDER } from "../format";
import { h, setAttr, setText } from "./dom";

const MISSING: Record<Connection, string> = {
  claudeCurrent: "No Claude login found on this PC.",
  codexCurrent: "No Codex login found on this PC.",
  antigravityApp: "",
  orca: "Orca is not installed.",
};

function found(d: Detected, c: Connection): boolean {
  return { claudeCurrent: d.claude, codexCurrent: d.codex, antigravityApp: d.antigravityApp, orca: d.orca }[c];
}

export interface First {
  readonly el: HTMLElement;
  readonly heading: HTMLElement;
  update(detected: Detected | null): void;
}

export function createFirst(onConnect: (c: Connection) => void): First {
  const heading = h("h2", { class: "view-title", id: "first-title", tabindex: -1 }, ["Connect your accounts"]);
  const rows = CONNECTION_ORDER.map((c) => {
    const button = h("button", { class: c === "orca" ? "btn btn-quiet" : "btn btn-primary", type: "button" }, [CONNECTION_NAME[c]]);
    const why = h("span", { class: "first-why" });
    button.addEventListener("click", () => {
      if (button.getAttribute("aria-disabled") !== "true") onConnect(c);
    });
    return { c, button, why, el: h("div", { class: "first-row" }, [button, why]) };
  });
  const el = h("section", { class: "first", "aria-labelledby": "first-title" }, [
    heading,
    h("p", { class: "view-text" }, ["QuotaGlass reads nothing until you pick a source. You can change this later in Settings."]),
    ...rows.map((r) => r.el),
  ]);

  return {
    el,
    heading,
    update(detected) {
      for (const r of rows) {
        const ok = detected === null || found(detected, r.c);
        setAttr(r.button, "aria-disabled", ok ? null : "true");
        setText(r.why, ok ? "" : MISSING[r.c]);
      }
    },
  };
}
