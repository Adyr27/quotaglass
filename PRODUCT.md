# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

The UI is HTML, CSS and TypeScript in a Tauri 2 WebView2 window on Windows 10 and 11 (x64). It is not a website. The window is frameless, transparent and blurred by the OS (Acrylic through `window_vibrancy`, with a CSS fallback). Windows desktop habits apply: tray icon, native notifications, Segoe UI Variable, and the user's Windows animation setting.

## Users

Everyone who installs QuotaGlass. The repo `Adyr27/quotaglass` is private today, but the product is designed for public users. The owner's own setup counts, but no more than anyone else's (owner decision, 2026-11-08). These are developers who have one or more Claude, Codex or Antigravity plan accounts signed in on their PC, use AI coding tools all day, and lose work when a plan limit cuts them off mid-task.

Their setups differ: some have one account, some several; some use Orca, most do not. A screen that only makes sense on the owner's setup is a failure.

## Product Purpose

The main job is to switch before a limit hits. One glance shows which account is close to a limit and which one has the most room left, so the user moves their work there before anything stops.

Success: the user never hits a limit by surprise, and never has to open a vendor website to check one.

## Positioning

QuotaGlass shows only numbers it reads from real sources: the user's own CLI logins and the tools already on their PC. It never asks the user to type a number, never estimates one, and never signs in or renews a login itself. When it cannot read something, it says so plainly instead of guessing.

## Operating Context

- A small window that sits on the desktop next to the IDE and terminal. It is looked at for about a second, many times a day.
- Data sources: the Claude Code login (`~/.claude`), the Codex login (`~/.codex`), and the running Antigravity app for Antigravity. Orca is an optional extra source; most users have only the native tools. QuotaGlass opens other apps' files read-only.
- The limits are the 5-hour, weekly and per-model windows, each with its reset time.
- Planned in v0.2.0 (brainstorm, not built yet): a pill docked above the taskbar that can be dragged anywhere and stays on top; a two-pane panel (providers on the left, account detail on the right); native Windows warnings at 50, 80 and 90% of each limit; a "Best Now" badge; a fast-burn badge.

## Capabilities and Constraints

- Providers: Claude, Codex and Antigravity only. Cursor is out of scope.
- No manual entry, no telemetry, no analytics, no auto-update, no crash upload.
- Never refresh or create OAuth tokens. Never print tokens or email addresses.
- Antigravity is read from the running Antigravity app (Orca optional). QuotaGlass never touches Google sign-in and never uses the Antigravity Google session.
- Orca is never required.
- Vendor endpoints are undocumented and can change. Every card must handle a source that fails, with the reason shown.
- Open: whether Codex exposes a count of banked resets.

## Brand Commitments

- The name is QuotaGlass. Glass is part of the identity, and the window is translucent by design.
- Provider colours already in use: Claude `#d97757`, Codex `#10a37f`, Antigravity `#4c8df6`.
- Voice: plain and factual. The word "unofficial" is removed from the UI in v0.2.0.

## Evidence on Hand

- Real data comes only from the user's own accounts at run time. No sample screenshots of strangers' accounts exist.
- Behaviour references (owner's screenshots, outside the repo): the collapsed pill, the expanded panel, a Windows warning notification, the "Connect an AI Account" dialog, and account cards with Pin, Refresh and Remove.
- No testimonials, user counts, press or benchmarks exist. Do not invent them.

## Product Principles

1. Real numbers or none. A missing number is better than a wrong one.
2. Hands off the login. Read only, never sign in, never renew.
3. One glance, one answer: which account to use right now.
4. Always findable, never in the way. A small pill that stays on top.
5. Works on any user's setup, and says plainly what it cannot read.

## Accessibility & Inclusion

- Glanceable: the state of every visible limit reads in about one second over any wallpaper and any window behind the glass.
- WCAG AA text contrast, measured against the worst case behind the translucent window, including the CSS fallback.
- Full keyboard use: open and close the panel (`Esc` closes it), move between accounts, and run every action.
- Respect the Windows "Animation effects" setting: no motion when it is off.
- Never use colour alone to show a state: pair it with a number, a label or a shape.
