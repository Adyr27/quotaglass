// One account card. The DOM is built once and patched in place on every snapshot,
// so focus, hover and the bar animation survive a refresh.

import { leftWhole, metaLine, sentenceCase, usedRatio } from "../format";
import type { AccountView } from "../types";
import { h, isVisible, setAttr, setText, show } from "./dom";

export interface CardContext {
  /** One instant for the whole render, so every card agrees on "now". */
  nowMs: number;
  /** True while a refresh is running. Retry then ignores clicks. */
  refreshing: boolean;
  onOpen(accountId: string, opener: HTMLElement): void;
  onRetry(): void;
  onSignIn(account: AccountView): void;
}

export interface Card {
  readonly el: HTMLElement;
  update(view: AccountView, ctx: CardContext): void;
  /** Moves focus to the card's main control. False when nothing in the card can take focus. */
  focus(): boolean;
}

const WARN_AT_PERCENT = 80;
const MAX_ENTER_STAGGER = 8;

/**
 * An expired Claude or Codex login can be renewed from the card: a current login through the vendor's
 * sign-in, an added account (profileId set) by opening its CLI with that profile.
 */
function canSignIn(view: AccountView): boolean {
  return view.state === "loginExpired" && view.service !== "antigravity" && (view.currentLogin || view.profileId !== null);
}

function signInText(view: AccountView): string {
  if (view.profileId === null) return "Sign in";
  return view.service === "claude" ? "Open Claude" : "Open Codex";
}

export function createCard(enterIndex: number): Card {
  let view: AccountView | null = null;
  let ctx: CardContext | null = null;
  let openable = false;
  let barStarted = false;

  // The stretched button is the keyboard and screen reader way in. Mouse clicks anywhere on the
  // card are caught by the article, so the label can keep its own tooltip.
  const openButton = h("button", { class: "card-open", type: "button" });

  const label = h("span", { class: "card-label" });
  const plan = h("span", { class: "pill plan" });
  const head = h("div", { class: "card-head" }, [label, plan]);

  const bucketName = h("span", { class: "bucket-name" });
  const more = h("button", { class: "pill pill-button", type: "button" });
  const figureNumber = h("span", { class: "figure-number" });
  const figure = h("span", { class: "figure" }, [figureNumber, " ", h("span", { class: "figure-unit" }, ["left"])]);
  const limitPill = h("span", { class: "pill pill-neg" }, ["Limit reached"]);
  const line = h("div", { class: "card-line" }, [bucketName, more, figure, limitPill]);

  const fill = h("div", { class: "bar-fill" });
  const bar = h("div", { class: "bar", "aria-hidden": "true" }, [fill]);

  const reading = h("div", { class: "card-reading" }, [head, line, bar]);
  const stateTitle = h("p", { class: "card-state" }, ["No reading yet"]);
  const meta = h("p", { class: "card-meta" });

  const note = h("p", { class: "card-note" });
  const retry = h("button", { class: "btn btn-sm", type: "button" }, ["Retry"]);
  const signIn = h("button", { class: "btn btn-sm btn-primary", type: "button" }, ["Sign in"]);
  const actions = h("div", { class: "card-actions" }, [retry, signIn]);
  const foot = h("div", { class: "card-foot" }, [note, actions]);

  const el = h("article", { class: "card is-new" }, [openButton, reading, stateTitle, meta, foot]);
  el.style.setProperty("--i", String(Math.min(enterIndex, MAX_ENTER_STAGGER)));

  // The entrance animation must not replay when a hidden group is shown again.
  el.addEventListener("animationend", (event) => {
    if (event.target === el) el.classList.remove("is-new");
  });

  el.addEventListener("click", (event) => {
    if (view === null || ctx === null || !openable) return;
    const button = event.target instanceof Element ? event.target.closest("button") : null;
    if (button !== null && button !== openButton && button !== more) return;
    ctx.onOpen(view.id, button === more ? more : openButton);
  });
  retry.addEventListener("click", () => {
    if (ctx !== null && !ctx.refreshing) ctx.onRetry();
  });
  signIn.addEventListener("click", () => {
    if (ctx !== null && view !== null && canSignIn(view)) ctx.onSignIn(view);
  });

  function update(next: AccountView, nextCtx: CardContext): void {
    view = next;
    ctx = nextCtx;
    const headline = next.headline;
    openable = headline !== null || next.buckets.length > 0;

    el.dataset.service = next.service;
    el.dataset.state = next.state;
    el.classList.toggle("is-openable", openable);
    // Without a headline the backend reports "stale" only because there is nothing to age.
    // Dim a card only when it shows a number that no longer holds.
    el.classList.toggle("is-dim", headline !== null && next.freshness === "stale");

    setText(label, next.label);
    setAttr(label, "title", next.label);
    const planText = next.plan === null ? "" : sentenceCase(next.plan);
    setText(plan, planText);
    show(plan, planText !== "");

    show(line, headline !== null);
    show(bar, headline !== null);
    if (headline !== null) {
      setText(bucketName, headline.name);
      setAttr(bucketName, "title", headline.name);
      // The headline is stale only when every bucket is; then "Limit reached" no longer holds.
      const limit = headline.limitReached && headline.freshness !== "stale";
      show(figure, !limit);
      show(limitPill, limit);
      setText(figureNumber, `${leftWhole(headline.usedPercent)}%`);
      figure.classList.toggle("is-warn", !limit && headline.freshness !== "stale" && headline.usedPercent >= WARN_AT_PERCENT);
      if (!barStarted) {
        // Commit a zero-width start once, so the first fill grows from the left.
        fill.style.setProperty("--p", "0");
        void fill.offsetWidth;
        barStarted = true;
      }
      fill.style.setProperty("--p", String(usedRatio(headline.usedPercent)));
    }

    const extra = Math.max(next.buckets.length - 1, 0);
    show(more, headline !== null && extra > 0);
    setText(more, `+${extra}`);
    setAttr(more, "aria-label", `+${extra} more ${extra === 1 ? "limit" : "limits"} for ${next.label}`);

    show(openButton, openable);
    setAttr(openButton, "aria-label", `Show all limits for ${next.label}`);

    const metaText =
      headline === null
        ? ""
        : metaLine(
            { resetsAtMs: headline.resetsAtMs, source: next.source, freshness: next.freshness, ageSecs: next.ageSecs },
            nextCtx.nowMs,
          );
    setText(meta, metaText);
    show(meta, metaText !== "");

    show(stateTitle, next.state === "noReadingYet");

    const reason = next.reason ?? "";
    const showNote = next.state !== "ok" && reason !== "";
    const canRetry = next.state === "error" || next.state === "noSource" || next.state === "loginExpired";
    setText(note, reason);
    show(note, showNote);
    note.classList.toggle("is-warn", next.state === "error");
    show(actions, canRetry);
    show(signIn, canSignIn(next));
    setText(signIn, signInText(next));
    show(foot, showNote || canRetry);
    retry.classList.toggle("is-disabled", nextCtx.refreshing);
    setAttr(retry, "aria-disabled", nextCtx.refreshing ? "true" : null);
  }

  function focus(): boolean {
    for (const candidate of [openButton, more, retry, signIn]) {
      if (isVisible(candidate)) {
        candidate.focus({ preventScroll: true });
        return true;
      }
    }
    return false;
  }

  return { el, update, focus };
}
