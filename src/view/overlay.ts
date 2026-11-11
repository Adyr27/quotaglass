// The hover overlay: read-only details for one service. Every visible account, its limits as pace
// strips with percent, window length and reset text. No buttons, so it never needs focus.

import type { AccountView, BucketView, Service } from "../types";
import { resetText, sentenceCase, stateText, updatedText, usedWhole, visibleAccounts } from "../format";
import { h, syncChildren } from "./dom";
import { createStrip } from "./strip";

export interface Overlay {
  readonly el: HTMLElement;
  update(service: Service, accounts: readonly AccountView[], nowMs: number): void;
}

/** "5 h window" or "7 d window". Empty when the source gives no window length. */
function windowLabel(secs: number | null): string {
  if (secs === null || !Number.isFinite(secs) || secs <= 0) return "";
  if (secs >= 86_400) return `${Math.round(secs / 86_400)} d window`;
  return `${Math.max(1, Math.round(secs / 3600))} h window`;
}

function sourceTag(a: AccountView): string {
  if (a.source === "antigravityApp") return "App";
  return a.profileId !== null ? "Added" : "Login";
}

function bucketRow(b: BucketView, service: Service, nowMs: number): HTMLElement {
  const foot = [windowLabel(b.windowSecs), b.limitReached ? "Limit reached" : resetText(b.resetsAtMs, nowMs)].filter((s) => s !== "");
  return h("div", { class: "ov-bucket" }, [
    h("div", { class: "ov-bucket-head" }, [
      h("span", { class: "ov-bucket-name" }, [b.name]),
      h("span", { class: "ov-pct num" }, [`${usedWhole(b.usedPercent)}%`]),
    ]),
    createStrip(b, service),
    h("div", { class: "ov-bucket-foot num" }, [
      h("span", {}, [foot.join(" · ")]),
      b.fastBurn ? h("span", { class: "burn" }, ["fast burn"]) : null,
    ]),
  ]);
}

function accountBlock(a: AccountView, nowMs: number): HTMLElement {
  const age = a.freshness === "live" ? "" : updatedText(a.ageSecs);
  const reason = stateText(a);
  const dim = a.state === "loginExpired" || a.state === "profileMissing" || a.freshness === "stale";
  return h("section", { class: "ov-account", "data-dim": String(dim) }, [
    h("header", { class: "ov-head" }, [
      h("span", { class: "ov-label" }, [a.label]),
      a.plan === null ? null : h("span", { class: "ov-chip" }, [sentenceCase(a.plan)]),
      h("span", { class: "ov-chip" }, [sourceTag(a)]),
      a.bestNow ? h("span", { class: "best" }, ["Best now"]) : null,
      age === "" ? null : h("span", { class: "ov-age num" }, [age]),
    ]),
    reason === null ? null : h("p", { class: "ov-reason" }, [reason]),
    ...a.buckets.map((b) => bucketRow(b, a.service, nowMs)),
  ]);
}

export function createOverlay(): Overlay {
  const el = h("div", { class: "overlay" });
  return {
    el,
    update(service, accounts, nowMs) {
      const mine = visibleAccounts(accounts).filter((a) => a.service === service);
      const wanted = mine.length === 0 ? [h("p", { class: "ov-empty" }, ["No accounts yet."])] : mine.map((a) => accountBlock(a, nowMs));
      syncChildren(el, wanted);
    },
  };
}
