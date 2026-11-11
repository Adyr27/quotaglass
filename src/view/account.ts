// The right pane: the selected account. Name, plan and freshness, one pace strip per limit, the
// Codex banked resets, and the actions. Buttons are built once and updated in place, so a refresh
// never takes focus away; the strips are rebuilt because nothing in them takes focus.

import type { AccountView, BucketView } from "../types";
import { resetText, sentenceCase, SERVICE_NAME, stateText, updatedText, usedWhole } from "../format";
import { h, iconRefresh, setAttr, setText, show, syncChildren } from "./dom";
import { createStrip } from "./strip";

export interface DetailDeps {
  onRefresh(): void;
  onRemove(account: AccountView): void;
  onHide(account: AccountView): void;
  /** Login expired or not finished: sign in again (current login) or open the CLI with the profile. */
  onSignIn(account: AccountView): void;
  onRedeem(): void;
}

export interface Detail {
  readonly el: HTMLElement;
  update(account: AccountView | null, nowMs: number, refreshing: boolean): void;
}

function limitRow(b: BucketView, account: AccountView, nowMs: number): HTMLElement {
  const used = usedWhole(b.usedPercent);
  const reset = resetText(b.resetsAtMs, nowMs);
  return h("div", { class: "limit" }, [
    h("div", { class: "limit-head" }, [h("span", { class: "limit-name" }, [b.name]), h("span", { class: "limit-pct num" }, [`${used}%`])]),
    createStrip(b, account.service),
    h("div", { class: "limit-foot" }, [
      h("span", { class: "num" }, [b.limitReached ? "Limit reached" : reset]),
      b.fastBurn ? h("span", { class: "burn" }, ["fast burn"]) : null,
    ]),
  ]);
}

/** Label for the sign-in button, or null when the account has none. */
function signInLabel(a: AccountView): string | null {
  if (a.service === "antigravity") return null;
  const needs = a.state === "loginExpired" || (a.state === "noReadingYet" && a.profileId !== null);
  if (!needs) return null;
  return a.profileId === null ? "Sign in" : `Open ${SERVICE_NAME[a.service]}`;
}

export function createDetail(deps: DetailDeps): Detail {
  let current: AccountView | null = null;

  const name = h("h2", { class: "detail-name", id: "detail-title" });
  const meta = h("p", { class: "detail-meta num" });
  const state = h("p", { class: "detail-state" });
  const signIn = h("button", { class: "btn btn-primary", type: "button" });
  const stateRow = h("div", { class: "detail-state-row" }, [state, signIn]);
  const limits = h("div", { class: "limits" });
  const bankedText = h("span", { class: "num" });
  const redeem = h("button", { class: "btn", type: "button" }, ["Redeem"]);
  const banked = h("p", { class: "banked" }, [bankedText, redeem]);

  const best = h("span", { class: "best" }, ["Best now"]);
  const refresh = h("button", { class: "icon-btn", type: "button", "aria-label": "Refresh", title: "Refresh" }, [iconRefresh()]);
  // The destructive action goes last, on its own.
  const remove = h("button", { class: "btn btn-quiet", type: "button" });
  const actions = h("div", { class: "actions" }, [remove]);

  const confirmYes = h("button", { class: "btn btn-danger", type: "button" }, ["Remove"]);
  const confirmNo = h("button", { class: "btn", type: "button" }, ["Cancel"]);
  const confirm = h("div", { class: "confirm", role: "group", "aria-labelledby": "confirm-text", hidden: true }, [
    h("p", { id: "confirm-text" }, ["Signs out and deletes this profile."]),
    h("div", { class: "confirm-actions" }, [confirmYes, confirmNo]),
  ]);

  const empty = h("p", { class: "detail-empty" }, ["Pick an account."]);
  const body = h("div", { class: "detail-body" }, [h("header", { class: "detail-head" }, [
    h("div", { class: "detail-title" }, [h("div", { class: "detail-title-row" }, [name, best]), meta]),
    refresh,
  ]), stateRow, limits, banked, actions, confirm]);
  const el = h("section", { class: "detail", "aria-labelledby": "detail-title" }, [empty, body]);

  const closeConfirm = (): void => {
    show(confirm, false);
    show(actions, true);
  };

  refresh.addEventListener("click", () => deps.onRefresh());
  signIn.addEventListener("click", () => current !== null && deps.onSignIn(current));
  redeem.addEventListener("click", () => deps.onRedeem());
  remove.addEventListener("click", () => {
    if (current === null) return;
    if (current.profileId === null) {
      deps.onHide(current);
      return;
    }
    show(actions, false);
    show(confirm, true);
    confirmNo.focus();
  });
  confirmNo.addEventListener("click", () => {
    closeConfirm();
    remove.focus();
  });
  confirmYes.addEventListener("click", () => {
    if (current !== null) deps.onRemove(current);
    closeConfirm();
  });
  confirm.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    event.preventDefault();
    closeConfirm();
    remove.focus();
  });

  return {
    el,
    update(account, nowMs, refreshing) {
      if (account?.id !== current?.id) closeConfirm();
      current = account;
      show(empty, account === null);
      show(body, account !== null);
      if (account === null) return;

      el.dataset.dim = String(account.state === "loginExpired" || account.state === "profileMissing" || account.freshness === "stale");
      setText(name, account.label);
      setText(meta, [account.plan === null ? "" : sentenceCase(account.plan), updatedText(account.ageSecs)].filter((s) => s !== "").join(" · "));

      const text = stateText(account);
      setText(state, text ?? "");
      const label = signInLabel(account);
      setText(signIn, label ?? "");
      show(signIn, label !== null);
      show(stateRow, text !== null || label !== null);

      syncChildren(limits, account.buckets.map((b) => limitRow(b, account, nowMs)));

      show(banked, account.service === "codex" && account.bankedResets !== null);
      setText(bankedText, `Banked resets: ${account.bankedResets ?? 0}`);

      show(best, account.bestNow);
      refresh.classList.toggle("is-busy", refreshing);
      setAttr(refresh, "aria-busy", refreshing ? "true" : null);
      setText(remove, account.profileId === null ? "Hide" : "Remove");
    },
  };
}
