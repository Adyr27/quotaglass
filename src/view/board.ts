// The main view: finding line, service filter, and one collapsible group of cards per service.

import { SERVICE_NAME, SERVICE_ORDER, findingText, mostRoom } from "../format";
import type { AccountView, Service, Settings, Snapshot } from "../types";
import { createCard, type Card } from "./card";
import { h, iconChevron, setText, show, syncChildren } from "./dom";

export interface BoardDeps {
  onOpen(accountId: string, opener: HTMLElement): void;
  onRetry(): void;
  onSignIn(service: Service): void;
  onToggleService(service: Service): void;
  onToggleGroup(service: Service): void;
}

export interface Board {
  readonly el: HTMLElement;
  update(snapshot: Snapshot, settings: Settings, nowMs: number, refreshing: boolean): void;
  focusCard(accountId: string): boolean;
}

interface Chip {
  el: HTMLButtonElement;
  count: HTMLElement;
}

interface Group {
  el: HTMLElement;
  toggle: HTMLButtonElement;
  count: HTMLElement;
  list: HTMLElement;
}

export function createBoard(deps: BoardDeps): Board {
  const cards = new Map<string, Card>();
  const chips = new Map<Service, Chip>();
  const groups = new Map<Service, Group>();

  const finding = h("p", { class: "finding", role: "status" });
  const chipRow = h("div", { class: "chips", role: "group", "aria-label": "Show services" });
  const groupsEl = h("div", { class: "groups" });
  const empty = h("div", { class: "empty" }, [
    h("p", { class: "empty-title" }, ["No signed-in accounts found"]),
    h("p", { class: "empty-text" }, ["QuotaGlass shows Claude, Codex and Antigravity accounts that are signed in on this PC."]),
  ]);
  const el = h("div", { class: "board" }, [finding, chipRow, groupsEl, empty]);

  for (const service of SERVICE_ORDER) {
    const name = SERVICE_NAME[service];

    const count = h("span", { class: "chip-count" });
    const chipEl = h("button", { class: "chip", type: "button", "data-service": service, "aria-pressed": "true" }, [
      h("span", { class: "chip-dot", "aria-hidden": "true" }),
      h("span", {}, [name]),
      count,
    ]);
    chipEl.addEventListener("click", () => deps.onToggleService(service));
    chips.set(service, { el: chipEl, count });
    chipRow.append(chipEl);

    const groupCount = h("span", { class: "group-count" });
    const toggle = h("button", { class: "group-toggle", type: "button", "aria-expanded": "true" }, [
      h("span", { class: "group-name" }, [name]),
      groupCount,
      iconChevron("chev"),
    ]);
    toggle.addEventListener("click", () => deps.onToggleGroup(service));
    const list = h("div", { class: "group-list" });
    const groupEl = h("section", { class: "group", "data-service": service, "aria-label": name }, [
      h("h2", { class: "group-head" }, [toggle]),
      list,
    ]);
    groups.set(service, { el: groupEl, toggle, count: groupCount, list });
    groupsEl.append(groupEl);
  }

  function update(snapshot: Snapshot, settings: Settings, nowMs: number, refreshing: boolean): void {
    const ctx = { nowMs, refreshing, onOpen: deps.onOpen, onRetry: deps.onRetry, onSignIn: deps.onSignIn };
    const byService = new Map<Service, AccountView[]>();
    for (const account of snapshot.accounts) {
      const list = byService.get(account.service);
      if (list === undefined) byService.set(account.service, [account]);
      else list.push(account);
    }

    const seen = new Set<string>();
    const shownAccounts: AccountView[] = [];
    let entering = 0;

    for (const service of SERVICE_ORDER) {
      const accounts = byService.get(service) ?? [];
      const chip = chips.get(service);
      const group = groups.get(service);
      if (chip === undefined || group === undefined) continue;

      const hidden = settings.hiddenServices.includes(service);
      const collapsed = settings.collapsed.includes(service);

      show(chip.el, accounts.length > 0);
      setText(chip.count, String(accounts.length));
      chip.el.setAttribute("aria-pressed", hidden ? "false" : "true");

      show(group.el, accounts.length > 0 && !hidden);
      setText(group.count, String(accounts.length));
      group.toggle.setAttribute("aria-expanded", collapsed ? "false" : "true");
      show(group.list, !collapsed);

      const els: Element[] = [];
      for (const account of accounts) {
        seen.add(account.id);
        let card = cards.get(account.id);
        if (card === undefined) {
          card = createCard(entering++);
          cards.set(account.id, card);
        }
        card.update(account, ctx);
        els.push(card.el);
        if (!hidden) shownAccounts.push(account);
      }
      syncChildren(group.list, els);
    }

    for (const id of Array.from(cards.keys())) {
      if (!seen.has(id)) cards.delete(id);
    }

    const room = mostRoom(shownAccounts);
    const text = room === null ? "" : findingText(room);
    setText(finding, text);
    show(finding, text !== "");
    show(chipRow, snapshot.accounts.length > 0);
    show(empty, snapshot.accounts.length === 0);
  }

  return {
    el,
    update,
    focusCard: (accountId) => cards.get(accountId)?.focus() ?? false,
  };
}
