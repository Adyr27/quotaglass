// The left pane: accounts grouped by provider in one listbox (Up/Down move, Home/End jump), the
// Antigravity hint when its app is connected but closed, "+ Add account", and the Settings button.

import type { AccountView, Service, Settings } from "../types";
import { SERVICE_NAME, SERVICE_ORDER, visibleAccounts, worstUsed } from "../format";
import { h, iconSliders, providerMark, setAttr, show, syncChildren } from "./dom";

export interface ListDeps {
  onSelect(id: string): void;
  onAdd(service: Service): void;
  onSettings(): void;
}

export interface List {
  readonly el: HTMLElement;
  readonly listbox: HTMLElement;
  readonly settingsButton: HTMLButtonElement;
  update(accounts: readonly AccountView[], settings: Settings | null, selectedId: string | null): void;
}

export function createList(deps: ListDeps): List {
  let order: AccountView[] = [];
  let selected: string | null = null;

  const listbox = h("div", { class: "listbox", role: "listbox", tabindex: 0, "aria-label": "Accounts" });
  const addButton = h("button", { class: "btn btn-quiet add", type: "button", "aria-expanded": "false" }, ["+ Add account"]);
  const settingsButton = h("button", { class: "icon-btn", type: "button", "aria-label": "Settings" }, [iconSliders()]);
  const addClaude = h("button", { class: "btn", type: "button" }, ["Claude"]);
  const addCodex = h("button", { class: "btn", type: "button" }, ["Codex"]);
  const choice = h("div", { class: "add-choice", hidden: true }, [
    h("p", {}, ["Sign in to"]),
    h("div", { class: "add-row" }, [addClaude, addCodex]),
  ]);
  const foot = h("div", { class: "providers-foot" }, [addButton, settingsButton]);
  const el = h("nav", { class: "providers", "aria-label": "Providers" }, [listbox, choice, foot]);

  const toggleChoice = (open: boolean): void => {
    show(choice, open);
    setAttr(addButton, "aria-expanded", open ? "true" : "false");
  };
  addButton.addEventListener("click", () => {
    const open = choice.hidden;
    toggleChoice(open);
    if (open) addClaude.focus();
  });
  for (const [button, service] of [
    [addClaude, "claude"],
    [addCodex, "codex"],
  ] as const) {
    button.addEventListener("click", () => {
      toggleChoice(false);
      deps.onAdd(service);
    });
  }
  choice.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    event.preventDefault();
    toggleChoice(false);
    addButton.focus();
  });
  settingsButton.addEventListener("click", () => deps.onSettings());

  listbox.addEventListener("keydown", (event) => {
    if (order.length === 0) return;
    const at = Math.max(0, order.findIndex((a) => a.id === selected));
    const next = { ArrowDown: at + 1, ArrowUp: at - 1, Home: 0, End: order.length - 1 }[event.key];
    if (next === undefined) return;
    event.preventDefault();
    deps.onSelect(order[Math.min(order.length - 1, Math.max(0, next))].id);
  });

  return {
    el,
    listbox,
    settingsButton,
    update(accounts, settings, selectedId) {
      selected = selectedId;
      order = visibleAccounts(accounts);
      const groups: HTMLElement[] = [];
      for (const service of SERVICE_ORDER) {
        const mine = order.filter((a) => a.service === service);
        const hint = service === "antigravity" && mine.length === 0 && settings?.connected.includes("antigravityApp") === true;
        if (mine.length === 0 && !hint) continue;
        const head = h("div", { class: "group-head", id: `group-${service}` }, [
          providerMark(service),
          h("span", {}, [SERVICE_NAME[service]]),
          mine.length > 1 ? h("span", { class: "count num" }, [String(mine.length)]) : null,
        ]);
        const rows = mine.map((a) => {
          const used = worstUsed(a);
          const row = h("div", { class: "option", role: "option", id: `acct-${order.indexOf(a)}`, "aria-selected": String(a.id === selectedId), "data-dim": a.state === "loginExpired" || a.state === "profileMissing" || a.freshness === "stale" }, [
            h("span", { class: "option-name" }, [a.label]),
            h("span", { class: "option-pct num" }, [used === null ? "—" : `${used}%`]),
          ]);
          row.addEventListener("click", () => {
            deps.onSelect(a.id);
            listbox.focus();
          });
          return row;
        });
        groups.push(
          h("div", { class: "group", role: "group", "aria-labelledby": `group-${service}` }, [
            head,
            ...rows,
            hint ? h("p", { class: "group-hint" }, ["Open the app"]) : null,
          ]),
        );
      }
      syncChildren(listbox, groups);
      const active = order.findIndex((a) => a.id === selectedId);
      setAttr(listbox, "aria-activedescendant", active < 0 ? null : `acct-${active}`);
      listbox.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: "nearest" });
    },
  };
}
