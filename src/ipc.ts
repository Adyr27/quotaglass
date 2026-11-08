// Typed wrappers around the Tauri commands and events. Nothing here throws:
// a failed command becomes one short problem line that the UI shows inline.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { BackdropPayload, Detected, Service, Settings, SettingsPatch, Snapshot } from "./types";

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

export function clearProblems(...keys: string[]): void {
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

export function onSettings(handler: (settings: Settings) => void): Promise<void> {
  return subscribe<Settings>("settings", handler);
}

export function onBackdrop(handler: (payload: BackdropPayload) => void): Promise<void> {
  return subscribe<BackdropPayload>("backdrop", handler);
}

export function onOpenSettings(handler: () => void): Promise<void> {
  return subscribe<null>("open-settings", () => handler());
}
