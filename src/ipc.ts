// Typed wrappers around the Tauri commands and events. Nothing here throws:
// a failed command becomes one short problem line that the UI shows inline.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  BackdropPayload,
  Connection,
  Detected,
  FlyoutMode,
  FlyoutView,
  Service,
  Settings,
  SettingsPatch,
  Snapshot,
  UrlTarget,
} from "./types";

export type Result<T> = { ok: true; value: T } | { ok: false };

// One problem line per failing command or event. A later success of the same command clears it.
const problems = new Map<string, string>();
let notify: (message: string | null) => void = () => undefined;

export function setProblemListener(listener: (message: string | null) => void): void {
  notify = listener;
}

function publish(): void {
  let latest: string | null = null;
  for (const message of problems.values()) latest = message;
  notify(latest);
}

function clearProblems(...keys: string[]): void {
  let changed = false;
  for (const key of keys) {
    if (problems.delete(key)) changed = true;
  }
  if (changed) publish();
}

function raise(key: string, failure: string, error: unknown): void {
  const detail = describe(error);
  const sentence = detail === "" || detail.endsWith(".") ? detail : `${detail}.`;
  problems.delete(key); // delete first so the newest problem is the last one in the map
  problems.set(key, sentence === "" ? failure : `${failure} ${sentence}`);
  publish();
}

function describe(error: unknown): string {
  let text = "";
  if (typeof error === "string") {
    text = error;
  } else if (error instanceof Error) {
    text = error.message;
  } else if (typeof error === "object" && error !== null && "message" in error && typeof error.message === "string") {
    text = error.message;
  }
  text = text.trim();
  return text.length > 140 ? `${text.slice(0, 137)}...` : text;
}

async function call<T>(command: string, failure: string, args?: Record<string, unknown>): Promise<Result<T>> {
  try {
    const value = await invoke<T>(command, args);
    clearProblems(command);
    return { ok: true, value };
  } catch (error) {
    raise(command, failure, error);
    return { ok: false };
  }
}

export function getSnapshot(): Promise<Result<Snapshot>> {
  return call<Snapshot>("get_snapshot", "Could not read the account list.");
}

export function refreshNow(): Promise<Result<void>> {
  return call<void>("refresh_now", "Could not refresh.");
}

export function detect(): Promise<Result<Detected>> {
  return call<Detected>("detect", "Could not check this PC for sources.");
}

export function getSettings(): Promise<Result<Settings>> {
  return call<Settings>("get_settings", "Could not load settings.");
}

export function updateSettings(patch: SettingsPatch): Promise<Result<Settings>> {
  return call<Settings>("update_settings", "Could not save that setting.", { patch });
}

export function connect(service: Service): Promise<Result<void>> {
  return call<void>("connect", "Could not open the sign-in window.", { service });
}

export function addAccount(service: Service): Promise<Result<Settings>> {
  return call<Settings>("add_account", "Could not add the account.", { service });
}

export function openCli(accountId: string): Promise<Result<void>> {
  return call<void>("open_cli", "Could not open the console.", { accountId });
}

export function removeAccount(accountId: string): Promise<Result<Settings>> {
  return call<Settings>("remove_account", "Could not remove the account.", { accountId });
}

export function setHidden(accountId: string, hidden: boolean): Promise<Result<Settings>> {
  return call<Settings>("set_hidden", "Could not change that account.", { accountId, hidden });
}

export function setLabel(accountId: string, label: string | null): Promise<Result<Settings>> {
  return call<Settings>("set_label", "Could not save the name.", { accountId, label });
}

export function connectSource(conn: Connection, on: boolean): Promise<Result<Settings>> {
  return call<Settings>("connect_source", "Could not change that source.", { conn, on });
}

export function openUrl(target: UrlTarget): Promise<Result<void>> {
  return call<void>("open_url", "Could not open the browser.", { target });
}

async function subscribe<T>(event: string, handler: (payload: T) => void): Promise<void> {
  try {
    await listen<T>(event, (incoming) => handler(incoming.payload));
  } catch (error) {
    raise(`listen:${event}`, "Could not listen for updates.", error);
  }
}

export function onSnapshot(handler: (snapshot: Snapshot) => void): Promise<void> {
  return subscribe<Snapshot>("snapshot", handler);
}

export function onBackdrop(handler: (payload: BackdropPayload) => void): Promise<void> {
  return subscribe<BackdropPayload>("backdrop", handler);
}

export function setZoom(zoom: number): Promise<Result<void>> {
  return call<void>("set_zoom", "Could not resize the window.", { zoom });
}

export function setPillScale(scale: number): Promise<Result<void>> {
  return call<void>("set_pill_scale", "Could not resize the window.", { scale });
}

export function setPillSlots(count: number): Promise<Result<void>> {
  return call<void>("set_pill_slots", "Could not resize the window.", { count });
}

export function setPillLocked(locked: boolean): Promise<Result<void>> {
  return call<void>("set_pill_locked", "Could not pin the window.", { locked });
}

export function flyoutHover(service: Service, inside: boolean, anchorX: number): Promise<Result<void>> {
  return call<void>("flyout_hover", "Could not show the details.", { service, inside, anchorX });
}

export function flyoutPointer(inside: boolean): Promise<Result<void>> {
  return call<void>("flyout_pointer", "Could not show the details.", { inside });
}

export function flyoutOpenPopover(service: Service | null, view: FlyoutView | null): Promise<Result<void>> {
  return call<void>("flyout_open_popover", "Could not open the details.", { service, view });
}

export function flyoutClose(): Promise<Result<void>> {
  return call<void>("flyout_close", "Could not close the details.");
}

export function flyoutSize(heightCss: number): Promise<Result<void>> {
  return call<void>("flyout_size", "Could not resize the details.", { heightCss });
}

export function onFocusPill(handler: () => void): Promise<void> {
  return subscribe<null>("focus-pill", () => handler());
}

export function onFlyoutMode(handler: (mode: FlyoutMode) => void): Promise<void> {
  return subscribe<FlyoutMode>("flyout-mode", handler);
}

export function onPillLocked(handler: (locked: boolean) => void): Promise<void> {
  return subscribe<boolean>("pill-locked", handler);
}

export function onTheme(handler: (theme: "light" | "dark") => void): Promise<void> {
  return subscribe<"light" | "dark">("theme", handler);
}
