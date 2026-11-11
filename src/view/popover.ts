// The click popover: accounts grouped by service (clicked service first), one expanded row at a
// time, a footer (Add account, Pin pill, Settings), and the Settings and First pages in place.

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

export function createPopover(deps: PopoverDeps): Popover {
  let page: Page = "list";
  let service: Service | null = null;
  let expandedId: string | null = null;
  let last: PopoverData | null = null;
  const rows = new Map<string, { row: HTMLButtonElement; pct: HTMLElement; name: HTMLElement; best: HTMLElement }>();
  const heads = new Map<Service, HTMLElement>();

  const detail = createDetail({
    onRefresh: deps.onRefresh,
    onRemove: deps.onRemove,
    onHide: deps.onHide,
    onSignIn: deps.onSignIn,
    onRedeem: deps.onRedeem,
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
  const listPage = h("div", { class: "pop-page" }, [list, choice, foot]);
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
    entry.row.dataset.dim = String(a.state === "loginExpired" || a.state === "profileMissing" || a.freshness === "stale");
    setAttr(entry.row, "title", stateText(a));
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
