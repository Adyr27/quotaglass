// The click popover: accounts grouped by service (clicked service first), one expanded row at a
// time, a Resets block (Codex first, then Claude), a footer (Add account, Pin pill, Settings), and
// the Settings and First pages in place.

import type { AccountView, Connection, Detected, FlyoutView, Service, Settings, SettingsPatch } from "../types";
import { SERVICE_NAME, SERVICE_ORDER, stateText, usedWhole, visibleAccounts, worstUsed } from "../format";
import { createDetail } from "./account";
import { h, iconPin, iconSliders, providerMark, setAttr, setText, show, syncChildren } from "./dom";
import { createFirst } from "./first";
import { createSettings } from "./settings";

export interface PopoverDeps {
  onRefresh(): void;
  onRemove(account: AccountView): void;
  onHide(account: AccountView): void;
  onSignIn(account: AccountView): void;
  onRedeem(): void;
  onClaudeUsage(): void;
  onAdd(service: Service): void;
  onPatch(patch: SettingsPatch): void;
  onSource(c: Connection, on: boolean): void;
  onUnhide(id: string): void;
  onLabel(id: string, label: string | null): void;
  onConnectFirst(c: Connection): void;
  onTogglePin(): void;
}

export interface PopoverData {
  accounts: readonly AccountView[];
  settings: Settings | null;
  detected: Detected | null;
  locked: boolean;
  refreshing: boolean;
  nowMs: number;
}

export interface Popover {
  readonly el: HTMLElement;
  /** Called when the flyout opens (or changes) in popover mode: picks the page and the first service. */
  open(service: Service | null, view: FlyoutView | null): void;
  update(data: PopoverData): void;
  /** Steps back one level. Returns false when there is nothing to step back from. */
  back(): boolean;
}

type Page = "list" | "settings" | "first";

/** Rows for an account whose numbers may be old: same rule for the account list and the Resets block. */
function dimmed(a: AccountView): boolean {
  return a.state === "loginExpired" || a.state === "profileMissing" || a.freshness === "stale";
}

export function createPopover(deps: PopoverDeps): Popover {
  let page: Page = "list";
  let service: Service | null = null;
  let expandedId: string | null = null;
  let last: PopoverData | null = null;
  const rows = new Map<string, { row: HTMLButtonElement; pct: HTMLElement; name: HTMLElement; best: HTMLElement }>();
  const resetRows = new Map<string, { row: HTMLElement; name: HTMLElement; count: HTMLElement; note: HTMLElement; redeem: HTMLElement }>();
  const heads = new Map<Service, HTMLElement>();

  const detail = createDetail({
    onRefresh: deps.onRefresh,
    onRemove: deps.onRemove,
    onHide: deps.onHide,
    onSignIn: deps.onSignIn,
  });
  const first = createFirst(deps.onConnectFirst);
  const settingsView = createSettings({
    onPatch: deps.onPatch,
    onSource: deps.onSource,
    onUnhide: deps.onUnhide,
    onLabel: deps.onLabel,
    onBack: () => void back(),
  });

  const list = h("div", { class: "pop-list" });
  // Codex does not say what a reset restores, so every count gets the same check line.
  const claudeUsage = h("button", { class: "btn", type: "button" }, ["Check limit resets"]);
  const claudeReset = h("div", { class: "reset-row" }, [providerMark("claude"), h("span", { class: "option-name" }, [SERVICE_NAME.claude]), claudeUsage]);
  const resetList = h("div", { class: "reset-list" });
  const resets = h("section", { class: "resets", "aria-labelledby": "resets-title" }, [
    h("div", { class: "group-head", id: "resets-title" }, ["Resets"]),
    resetList,
  ]);
  const addButton = h("button", { class: "btn btn-quiet", type: "button", "aria-expanded": "false" }, ["+ Add account"]);
  const addClaude = h("button", { class: "btn", type: "button" }, ["Claude"]);
  const addCodex = h("button", { class: "btn", type: "button" }, ["Codex"]);
  const choice = h("div", { class: "add-choice", hidden: true }, [
    h("p", {}, ["Sign in to"]),
    h("div", { class: "add-row" }, [addClaude, addCodex]),
  ]);
  const pinText = h("span", {}, ["Pin pill"]);
  const pinButton = h("button", { class: "btn btn-quiet", type: "button", "aria-pressed": "false" }, [iconPin(), pinText]);
  const settingsButton = h("button", { class: "btn btn-quiet", type: "button" }, [iconSliders(), "Settings"]);
  const foot = h("div", { class: "pop-foot" }, [addButton, pinButton, settingsButton]);
  const listPage = h("div", { class: "pop-page" }, [list, resets, choice, foot]);
  const el = h("div", { class: "popover" }, [listPage, first.el, settingsView.el]);

  const toggleChoice = (open: boolean): void => {
    show(choice, open);
    setAttr(addButton, "aria-expanded", open ? "true" : "false");
  };
  addButton.addEventListener("click", () => {
    const open = choice.hidden;
    toggleChoice(open);
    if (open) addClaude.focus();
  });
  for (const [button, svc] of [
    [addClaude, "claude"],
    [addCodex, "codex"],
  ] as const) {
    button.addEventListener("click", () => {
      toggleChoice(false);
      deps.onAdd(svc);
    });
  }
  choice.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    event.preventDefault();
    event.stopPropagation();
    toggleChoice(false);
    addButton.focus();
  });
  pinButton.addEventListener("click", () => deps.onTogglePin());
  claudeUsage.addEventListener("click", () => deps.onClaudeUsage());
  settingsButton.addEventListener("click", () => go("settings"));

  function go(next: Page): void {
    page = next;
    render();
    (next === "settings" ? settingsView.heading : next === "first" ? first.heading : settingsButton).focus({ preventScroll: true });
  }

  function back(): boolean {
    if (page !== "list") {
      go("list");
      return true;
    }
    if (expandedId !== null) {
      const id = expandedId;
      expandedId = null;
      render();
      rows.get(id)?.row.focus({ preventScroll: true });
      return true;
    }
    return false;
  }

  function rowFor(a: AccountView): HTMLElement {
    let entry = rows.get(a.id);
    if (entry === undefined) {
      const name = h("span", { class: "option-name" });
      const pct = h("span", { class: "option-pct num" });
      const best = h("span", { class: "best" }, ["Best now"]);
      const row = h("button", { class: "pop-row", type: "button" }, [name, best, pct]);
      const id = a.id;
      row.addEventListener("click", () => {
        expandedId = expandedId === id ? null : id;
        render();
      });
      entry = { row, pct, name, best };
      rows.set(a.id, entry);
    }
    const used = worstUsed(a);
    setText(entry.name, a.label);
    setText(entry.pct, used === null ? "—" : `${usedWhole(used)}%`);
    show(entry.best, a.bestNow);
    setAttr(entry.row, "aria-expanded", String(a.id === expandedId));
    entry.row.dataset.dim = String(dimmed(a));
    setAttr(entry.row, "title", stateText(a));
    return entry.row;
  }

  /** One Codex account in the Resets block. Built once per account, so a refresh keeps focus on Redeem. */
  function resetRowFor(a: AccountView, count: number): HTMLElement {
    let entry = resetRows.get(a.id);
    if (entry === undefined) {
      const name = h("span", { class: "option-name" });
      const countText = h("span", { class: "num" });
      const redeem = h("button", { class: "btn", type: "button" }, ["Redeem"]);
      redeem.addEventListener("click", () => deps.onRedeem());
      const note = h("p", { class: "set-note" }, ["Check what this reset restores before you use it."]);
      const row = h("div", { class: "reset-row" }, [providerMark("codex"), name, countText, redeem, note]);
      entry = { row, name, count: countText, note, redeem };
      resetRows.set(a.id, entry);
    }
    setText(entry.name, a.label);
    setText(entry.count, count === 0 ? "No resets" : count === 1 ? "1 reset" : `${count} resets`);
    show(entry.note, count > 0);
    show(entry.redeem, count > 0);
    entry.row.dataset.dim = String(dimmed(a));
    return entry.row;
  }

  function render(): void {
    if (last === null) return;
    const { accounts, settings } = last;
    const visible = visibleAccounts(accounts);
    const firstRun = settings !== null && settings.connected.length === 0 && settings.addedAccounts.length === 0;
    if (page === "first" && settings !== null && !firstRun) page = "list";
    // Settings not loaded yet (or failed to load): show the list instead of a blank popover.
    show(listPage, page === "list" || (page === "settings" && settings === null));
    show(first.el, page === "first");
    show(settingsView.el, page === "settings" && settings !== null);

    setAttr(pinButton, "aria-pressed", String(last.locked));
    setText(pinText, last.locked ? "Unpin pill" : "Pin pill");

    const ids = new Set(visible.map((a) => a.id));
    for (const id of rows.keys()) if (!ids.has(id)) rows.delete(id);
    if (expandedId !== null && !ids.has(expandedId)) expandedId = null;
    const order = service === null ? SERVICE_ORDER : [service, ...SERVICE_ORDER.filter((s) => s !== service)];
    const wanted: HTMLElement[] = [];
    for (const svc of order) {
      const mine = visible.filter((a) => a.service === svc);
      if (mine.length === 0) continue;
      let head = heads.get(svc);
      if (head === undefined) {
        head = h("div", { class: "group-head" }, [providerMark(svc), h("span", {}, [SERVICE_NAME[svc]])]);
        heads.set(svc, head);
      }
      wanted.push(head);
      for (const a of mine) {
        wanted.push(rowFor(a));
        if (a.id === expandedId) wanted.push(detail.el);
      }
    }
    if (wanted.length === 0) wanted.push(h("p", { class: "set-note" }, ["No accounts yet."]));
    detail.update(visible.find((a) => a.id === expandedId) ?? null, last.nowMs, last.refreshing);
    syncChildren(list, wanted);

    const codex = visible.filter((a) => a.service === "codex" && a.bankedResets !== null);
    for (const id of resetRows.keys()) if (!codex.some((a) => a.id === id)) resetRows.delete(id);
    const resetWanted = codex.map((a) => resetRowFor(a, a.bankedResets ?? 0));
    if (visible.some((a) => a.service === "claude")) resetWanted.push(claudeReset);
    show(resets, resetWanted.length > 0);
    syncChildren(resetList, resetWanted);

    first.update(last.detected);
    if (settings !== null) settingsView.update(settings, last.detected, accounts);
  }

  return {
    el,
    open(svc, view) {
      service = svc;
      expandedId = null;
      toggleChoice(false);
      page = view ?? "list";
      render();
    },
    update(data) {
      last = data;
      render();
    },
    back,
  };
}
