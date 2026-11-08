// Settings. Every control saves itself through the callbacks; a failed save re-renders the stored value.

import type { AccountView, Connection, Detected, Material, Settings, SettingsPatch, Theme } from "../types";
import { CONNECTION_NAME, CONNECTION_ORDER, SERVICE_NAME } from "../format";
import { createSwitch, h, setAttr, setText, show, syncChildren, type Switch } from "./dom";

export interface SettingsDeps {
  onPatch(patch: SettingsPatch): void;
  onSource(c: Connection, on: boolean): void;
  onUnhide(id: string): void;
  onLabel(id: string, label: string | null): void;
  onClose(): void;
}

export interface SettingsView {
  readonly el: HTMLElement;
  readonly heading: HTMLElement;
  update(settings: Settings, detected: Detected | null, accounts: readonly AccountView[]): void;
}

interface Seg<T> {
  readonly el: HTMLElement;
  set(value: T): void;
}

/** A radio group of buttons. Arrow keys move and pick, as native radios do. */
function seg<T extends string | number>(labelId: string, choices: readonly [T, string][], onPick: (v: T) => void): Seg<T> {
  const buttons = choices.map(([value, text]) => {
    const b = h("button", { class: "seg-item", type: "button", role: "radio", "aria-checked": "false", tabindex: -1 }, [text]);
    b.addEventListener("click", () => onPick(value));
    return b;
  });
  const el = h("div", { class: "seg", role: "radiogroup", "aria-labelledby": labelId }, buttons);
  el.addEventListener("keydown", (event) => {
    const at = buttons.findIndex((b) => b === document.activeElement);
    const step = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[event.key];
    if (at < 0 || step === undefined) return;
    event.preventDefault();
    const next = (at + step + buttons.length) % buttons.length;
    buttons[next].focus();
    onPick(choices[next][0]);
  });
  return {
    el,
    set(value) {
      choices.forEach(([v], i) => {
        setAttr(buttons[i], "aria-checked", v === value ? "true" : "false");
        setAttr(buttons[i], "tabindex", v === value ? "0" : "-1");
      });
    },
  };
}

function row(id: string, title: string, control: HTMLElement, note?: string): HTMLElement {
  return h("div", { class: "set-row" }, [
    h("div", { class: "set-label" }, [h("span", { id }, [title]), note === undefined ? null : h("span", { class: "set-note" }, [note])]),
    control,
  ]);
}

function found(d: Detected | null, c: Connection): boolean {
  if (d === null) return true;
  return { claudeCurrent: d.claude, codexCurrent: d.codex, antigravityApp: d.antigravityApp, orca: d.orca }[c];
}

export function createSettings(deps: SettingsDeps): SettingsView {
  const heading = h("h2", { class: "view-title", id: "settings-title", tabindex: -1 }, ["Settings"]);
  const back = h("button", { class: "btn", type: "button" }, ["Done"]);
  back.addEventListener("click", () => deps.onClose());

  const theme = seg<Theme>("set-theme", [["system", "System"], ["light", "Light"], ["dark", "Dark"]], (v) => deps.onPatch({ theme: v }));
  const glass = seg<Material>("set-glass", [["acrylic", "Acrylic"], ["mica", "Mica"], ["clear", "Clear"]], (v) => deps.onPatch({ material: v }));
  const minutes = seg<number>("set-refresh", [[1, "1 min"], [2, "2 min"], [5, "5 min"]], (v) => deps.onPatch({ refreshMinutes: v }));
  const onTop = createSwitch({ labelledBy: "set-top", onChange: (v) => deps.onPatch({ alwaysOnTop: v }) });
  const autostart = createSwitch({ labelledBy: "set-start", onChange: (v) => deps.onPatch({ autostart: v }) });

  const sources = CONNECTION_ORDER.map((c) => {
    const sw: Switch = createSwitch({ labelledBy: `set-src-${c}`, onChange: (v) => deps.onSource(c, v) });
    return { c, sw, el: row(`set-src-${c}`, CONNECTION_NAME[c], sw.el) };
  });

  const hiddenList = h("div", { class: "set-list" });
  const namesList = h("div", { class: "set-list" });
  const inputs = new Map<string, HTMLInputElement>();
  /** The saved labels as of the last update; the inputs compare against these, not a stale copy. */
  let labels: Record<string, string> = {};

  const el = h("section", { class: "settings", "aria-labelledby": "settings-title" }, [
    h("div", { class: "view-head" }, [heading, back]),
    h("div", { class: "set-group" }, [
      row("set-theme", "Theme", theme.el),
      row("set-glass", "Glass", glass.el, "Shows as solid while Windows transparency effects are off."),
      row("set-top", "Always on top", onTop.el),
      row("set-start", "Start with Windows", autostart.el),
      row("set-refresh", "Refresh every", minutes.el),
    ]),
    h("h3", { class: "set-title" }, ["Sources"]),
    h("div", { class: "set-group" }, sources.map((s) => s.el)),
    h("h3", { class: "set-title" }, ["Account names"]),
    namesList,
    h("h3", { class: "set-title" }, ["Hidden accounts"]),
    hiddenList,
  ]);

  return {
    el,
    heading,
    update(settings, detected, accounts) {
      labels = settings.labels;
      theme.set(settings.theme);
      glass.set(settings.material);
      minutes.set(settings.refreshMinutes);
      onTop.set(settings.alwaysOnTop);
      autostart.set(settings.autostart);
      for (const s of sources) {
        const on = settings.connected.includes(s.c);
        s.sw.set(on);
        // A source this PC lacks can still be switched off, never on.
        setAttr(s.sw.el, "aria-disabled", on || found(detected, s.c) ? null : "true");
        s.sw.el.disabled = !on && !found(detected, s.c);
      }

      const names = accounts.map((a) => {
        let input = inputs.get(a.id);
        if (input === undefined) {
          input = h("input", { class: "field", type: "text", maxlength: 40, "aria-label": `Name for ${SERVICE_NAME[a.service]} account` });
          const field = input;
          const id = a.id;
          field.addEventListener("change", () => {
            const value = field.value.trim();
            if (value !== (labels[id] ?? "")) deps.onLabel(id, value === "" ? null : value);
          });
          inputs.set(a.id, input);
        }
        if (document.activeElement !== input) input.value = settings.labels[a.id] ?? "";
        input.placeholder = a.label;
        return h("label", { class: "name-row" }, [h("span", { class: "dot", "data-service": a.service, "aria-hidden": "true" }), input]);
      });
      syncChildren(namesList, names.length > 0 ? names : [h("p", { class: "set-note" }, ["No accounts yet."])]);

      const hidden = accounts.filter((a) => a.hidden);
      const rows = hidden.map((a) => {
        const unhide = h("button", { class: "btn", type: "button" }, ["Unhide"]);
        unhide.addEventListener("click", () => deps.onUnhide(a.id));
        return h("div", { class: "hidden-row" }, [h("span", {}, [`${SERVICE_NAME[a.service]} · ${a.label}`]), unhide]);
      });
      // Hidden IDs whose account is not on screen right now can still be unhidden.
      for (const id of settings.hiddenAccounts) {
        if (hidden.some((a) => a.id === id)) continue;
        const unhide = h("button", { class: "btn", type: "button" }, ["Unhide"]);
        unhide.addEventListener("click", () => deps.onUnhide(id));
        rows.push(h("div", { class: "hidden-row" }, [h("span", {}, ["Account not connected now"]), unhide]));
      }
      const none = h("p", { class: "set-note" });
      setText(none, "No hidden accounts.");
      syncChildren(hiddenList, rows.length > 0 ? rows : [none]);
      show(hiddenList, true);
    },
  };
}
