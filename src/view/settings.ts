// The settings panel. Every change saves at once through the callbacks.

import type { Connection, Detected, Settings } from "../types";
import { createSwitch, h, type Switch } from "./dom";
import { createSourceRows } from "./sources";

export interface SettingsDeps {
  onRefreshMinutes(minutes: number): void;
  onToggle(key: "alwaysOnTop" | "autostart", next: boolean): void;
  onSource(kind: Connection, next: boolean): void;
  onClose(): void;
}

export interface SettingsPanel {
  readonly el: HTMLElement;
  /** Receives focus when the panel opens. */
  readonly heading: HTMLElement;
  update(settings: Settings, detected: Detected | null): void;
}

const MINUTE_CHOICES = [1, 2, 3, 4, 5] as const;

export function createSettings(deps: SettingsDeps): SettingsPanel {
  const heading = h("h2", { class: "panel-title", id: "settings-title", tabindex: -1 }, ["Settings"]);

  // Refresh interval: a radio group with one tab stop. Arrow keys move and select.
  const radios = MINUTE_CHOICES.map((minutes) =>
    h("button", { class: "seg-item", type: "button", role: "radio", "aria-checked": "false" }, [`${minutes} min`]),
  );
  radios.forEach((radio, index) => {
    radio.addEventListener("click", () => deps.onRefreshMinutes(MINUTE_CHOICES[index]));
    radio.addEventListener("keydown", (event) => {
      const step = event.key === "ArrowRight" || event.key === "ArrowDown" ? 1 : event.key === "ArrowLeft" || event.key === "ArrowUp" ? -1 : 0;
      if (step === 0) return;
      event.preventDefault();
      const next = (index + step + radios.length) % radios.length;
      radios[next].focus();
      deps.onRefreshMinutes(MINUTE_CHOICES[next]);
    });
  });
  const refreshGroup = h("div", { class: "seg", role: "radiogroup", "aria-labelledby": "settings-refresh-label" }, radios);

  function switchRow(id: string, text: string, key: "alwaysOnTop" | "autostart"): { row: HTMLElement; toggle: Switch } {
    const toggle = createSwitch({ labelledBy: id, onChange: (next) => deps.onToggle(key, next) });
    return { row: h("div", { class: "set-row" }, [h("span", { id }, [text]), toggle.el]), toggle };
  }
  const onTop = switchRow("settings-ontop-label", "Always on top", "alwaysOnTop");
  const autostart = switchRow("settings-autostart-label", "Start with Windows", "autostart");

  const sources = createSourceRows("settings", deps.onSource);

  const close = h("button", { class: "btn", type: "button" }, ["Close"]);
  close.addEventListener("click", () => deps.onClose());

  const el = h("section", { class: "panel", "aria-labelledby": "settings-title" }, [
    heading,
    h("p", { class: "panel-sub", id: "settings-refresh-label" }, ["Refresh every"]),
    refreshGroup,
    onTop.row,
    autostart.row,
    h("p", { class: "panel-sub" }, ["Sources"]),
    sources.el,
    h("div", { class: "panel-foot" }, [close]),
  ]);

  function update(settings: Settings, detected: Detected | null): void {
    const selected = MINUTE_CHOICES.findIndex((minutes) => minutes === settings.refreshMinutes);
    radios.forEach((radio, index) => {
      radio.setAttribute("aria-checked", index === selected ? "true" : "false");
      radio.tabIndex = index === Math.max(selected, 0) ? 0 : -1;
    });
    onTop.toggle.set(settings.alwaysOnTop);
    autostart.toggle.set(settings.autostart);
    sources.update(detected, settings.connected);
  }

  return { el, heading, update };
}
