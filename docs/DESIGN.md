# QuotaGlass — Technical design document

For the approved v0.2.0 design specification, see [docs/superpowers/specs/2026-11-08-v0.2.0-design.md](superpowers/specs/2026-11-08-v0.2.0-design.md).

This document describes the architecture, data sources, security model, and implementation details of QuotaGlass.

## Purpose and scope

QuotaGlass is a desktop widget for Windows 10 and Windows 11 (x64). It monitors subscription plan limits across artificial intelligence development accounts.

The application displays remaining quota for:
- Claude (Anthropic)
- Codex (OpenAI)
- Antigravity (Google)

QuotaGlass reads usage metrics only from local tools and vendor HTTP endpoints. It does not track API token expenditure, financial balance, or ChatGPT consumer web chat limits. It provides no automated updater and no manual entry forms.

## Architecture

QuotaGlass uses a two-crate Rust workspace and a lightweight web view frontend.

```
+-------------------------------------------------------------+
|                         src/ (UI)                           |
|         Plain TypeScript + Vite (No UI framework)           |
+-------------------------------------------------------------+
                              |
                     Tauri IPC (Commands / Events)
                              |
+-----------------------------v-------------------------------+
|                      src-tauri (App)                        |
|  Scheduler, Settings store, Window, System tray, IPC router |
+-------------------------------------------------------------+
                              |
                 Rust internal crate boundary
                              |
+-----------------------------v-------------------------------+
|                    crates/core (Engine)                     |
|  Token handling, Sources, Merge logic, Freshness evaluation |
+-------------------------------------------------------------+
```

### Component roles

1. `crates/core`
   Contains business logic and provider integrations without Tauri dependencies.
   - Discovers local configuration files, CLI tools, and the running Antigravity app.
   - Queries vendor endpoints for Claude and Codex.
   - Queries the local language server on loopback (`127.0.0.1`) for Antigravity.
   - Queries Orca when enabled for Antigravity.
   - Parses vendor metrics into normalized bucket records.
   - Computes pace strips, fast burn, room left, and the best account now.
   - Evaluates limit warnings at 50, 80, and 90 percent.
   - Merges metrics across sources and calculates data freshness.
   - Protects access tokens using non-serializable memory containers.
   - Hashes email addresses into safe account identifiers using FNV-1a.

2. `src-tauri`
   Hosts the operating system application shell and desktop integration.
   - Manages asynchronous polling schedules and snapshot generation.
   - Sends native warning toasts via `tauri-plugin-notification`.
   - Persists user preferences and profiles to `%APPDATA%\QuotaGlass\settings.json`.
   - Persists warning state to `%APPDATA%\QuotaGlass\warnings.json`.
   - Manages the frameless translucent window, pill and panel geometry, system tray, and backdrop materials.
   - Exposes safe IPC commands to the user interface.
   - Spawns visible console windows for vendor CLI sign-in and account actions.

3. `src/`
   Renders the visual presentation layer.
   - Implemented in TypeScript using standard DOM APIs and Vite.
   - Renders the Pace Band layout with a compact pill and expandable panel.
   - Renders account cards, pace strips, fast burn indicators, and the Best now chip.
   - Communicates with the backend exclusively through typed IPC wrappers.

## Data sources and priority

QuotaGlass gathers metrics using three source tiers:

| Priority | Source | Mechanism | Accounts covered |
|---|---|---|---|
| 1 (Highest) | Direct | Reads local CLI credentials, then queries vendor endpoints | Active Claude Code or Codex login (`~/.claude`, `~/.codex`), or added profile folders (`%APPDATA%\QuotaGlass\profiles\<id>`) |
| 2 | Antigravity app | Queries the running Antigravity IDE language server on loopback (`127.0.0.1`) | Active Antigravity accounts while the IDE is running |
| 3 (Lowest) | Orca | Executes `orca account list --json` | Antigravity accounts in Orca (optional) |

### Priority and selection rules

- Nothing is read until the user connects a source on the initial screen or in Settings.
- Claude and Codex use Direct sources only (current CLI login or added profile folders).
- Antigravity reads from the running Antigravity app's local server. If Orca is connected, it adds the Antigravity accounts it knows; accounts are merged into one card per account ID. Orca is never used for Claude or Codex.
- OMP support has been removed.
- The per-account source picker has been removed. Each account uses its detected and assigned source.
- A live response from the current refresh cycle takes precedence over cached historical readings.
- If an account reading becomes stale because its window reset, QuotaGlass dims the card to avoid presenting historical exhaustion as current state.

## Account identification and merging

QuotaGlass merges records across sources with an account key (`service:email`).

### Identity and privacy rules

- To prevent email leakage, internal store keys and UI card IDs hash the email address with FNV-1a 64-bit (for example `claude:0123456789abcdef`).
- Raw email addresses are never sent across IPC, stored in settings files, printed in logs, or shown in UI labels or notifications.
- An account record must contain an email address to merge with another source.
- If a reading lacks an email address, QuotaGlass generates an isolated card using a local identifier.
- Two readings without email addresses never merge, preventing cross-account contamination.

## Freshness rules

Snapshots update internally whenever new data arrives. In addition, the background scheduler rebuilds the visible snapshot every 30 seconds to update age indicators and expiration states between network polls.

Each usage bucket is classified into one of three freshness tiers:

| Tier | Condition | Presentation |
|---|---|---|
| Live | Age is within two refresh intervals, and reset time is in the future. | Normal full-contrast progress bar. |
| Aged | Reading is older than two refresh intervals, but reset time is in the future. | Normal progress bar with an age label (e.g., `2h ago`). |
| Stale | The reset timestamp has already passed, or reading age exceeds the bucket window duration. | Dimmed progress bar; never shows "Limit reached". |

If an account reading becomes stale because its window reset, QuotaGlass dims the card to avoid presenting historical exhaustion as current state.

## IPC surface

The web view communicates with the Rust backend through an explicit IPC boundary.

### Commands

| Command | Arguments | Return type | Description |
|---|---|---|---|
| `get_snapshot` | None | `Snapshot` | Returns the current aggregated view model. |
| `refresh_now` | None | `()` | Triggers an immediate poll across all active sources. |
| `detect` | None | `Detected` | Detects availability of local tools and files. |
| `get_settings` | None | `Settings` | Retrieves current application configuration. |
| `update_settings` | `{ patch: SettingsPatch }` | `Result<Settings, String>` | Updates configuration and triggers window or scheduler updates. |
| `connect` | `{ service: Service }` | `Result<(), String>` | Spawns a visible terminal running vendor login commands for the current login. |
| `add_account` | `{ service: Service }` | `Result<Settings, String>` | Creates a profile folder and spawns the vendor CLI login in a console. |
| `open_cli` | `{ account_id: String }` | `Result<(), String>` | Spawns the vendor CLI in a console for the given account. |
| `remove_account` | `{ account_id: String }` | `Result<Settings, String>` | Signs out an added profile, deletes its folder, and removes the settings entry. |
| `set_hidden` | `{ account_id: String, hidden: bool }` | `Result<Settings, String>` | Hides or unhides a current-login account. |
| `set_pinned` | `{ account_id: String, pinned: bool }` | `Result<Settings, String>` | Pins or unpins an account on the pill (maximum 4). |
| `set_label` | `{ account_id: String, label: Option<String> }` | `Result<Settings, String>` | Sets or clears a custom account display name. |
| `connect_source` | `{ conn: Connection, on: bool }` | `Result<Settings, String>` | Toggles an individual connection source. |
| `open_url` | `{ target: UrlTarget }` | `Result<(), String>` | Opens an allowlisted URL in the default browser. |
| `set_panel_open` | `{ open: bool }` | `Result<bool, String>` | Expands or collapses the panel window. |
| `set_zoom` | `{ zoom: f64 }` | `Result<(), String>` | Synchronizes the window geometry with the web view zoom factor. |

### Events

| Event | Payload | Description |
|---|---|---|
| `snapshot` | `Snapshot` | Emitted when refreshed metrics or freshness calculations change. |
| `settings` | `Settings` | Emitted when configuration settings change. |
| `backdrop` | `{ acrylic: bool }` | Notifies the UI if native Acrylic transparency succeeds or fails. |
| `theme` | `{ dark: bool }` | Emitted when the effective application theme changes. |
| `open-settings` | None | Emitted from system tray menu to open the settings view. |

## Settings version 2

Settings are stored at `%APPDATA%\QuotaGlass\settings.json`.

Schema fields:
- `version`: format version (current value: 2).
- `pill_position`: optional saved window coordinates `{ x, y, width, height }`.
- `panel_size`: optional saved panel dimensions `{ width, height }`.
- `pill_width`: optional pill width in CSS pixels (clamped between 320 and 960).
- `always_on_top`: boolean, keeps the widget above other windows (defaults to true).
- `autostart`: boolean, launches QuotaGlass with Windows.
- `refresh_minutes`: integer refresh interval (1, 2, or 5 minutes).
- `theme`: `system`, `light`, or `dark`.
- `material`: `acrylic`, `mica`, or `clear`.
- `connected`: list of active connections (`claudeCurrent`, `codexCurrent`, `antigravityApp`, `orca`).
- `added_accounts`: list of profile records `{ id, service, profile_dir }`.
- `pinned_accounts`: list of up to 4 pinned account IDs.
- `hidden_accounts`: list of hidden account IDs.
- `labels`: map of account ID to user-defined display label (maximum 40 characters).

### Migration from version 1

When upgrading from v0.1.0:
- Window position is migrated from `window` to `pill_position`.
- `refresh_minutes` and `autostart` are retained.
- `always_on_top` is set to true by default.
- All v0.1.0 per-account settings (`overrides`, `sources`, `collapsed`, `hidden_services`) and `first_run_done` are dropped.
- Upgraded users see the first-run connection screen and reconnect accounts in a few clicks.

## Security model

QuotaGlass handles authentication artifacts and system processes under strict isolation constraints.

- **Process execution safety**: All process invocations use absolute tool paths discovered on PATH. Arguments are fixed per action. No user input or UI text is passed as a command argument.
- **Path character validation**: Tool paths containing characters parsed as syntax by `cmd.exe` (`&`, `|`, `<`, `>`, `^`, `%`, `!`, `"`, `(`, `)`) are refused. This prevents path-injection vulnerabilities when spawning console windows.
- **Profile folder containment**: Profile folders must reside strictly within `%APPDATA%\QuotaGlass\profiles\`. Paths are canonicalised and checked before launching sign-in sessions or deleting profiles.
- **Profile deletion**: Removing an added account runs the vendor CLI sign-out, deletes the canonical profile folder, and removes the settings entry. Folders outside the profile directory are never deleted.
- **Network allowlist**: Network requests connect exclusively to `https://api.anthropic.com` and `https://chatgpt.com` over HTTPS, and `127.0.0.1` for the local Antigravity server. No redirects are followed.
- **Response size cap**: Incoming HTTP bodies and CLI output streams have a hard consumption limit (1 MiB for HTTP, 4 MiB for CLI output).
- **Memory protection**: Access tokens are wrapped in a non-serializable `Secret` container backed by `zeroize::Zeroizing`. Memory is zeroed on drop. Tokens never pass across Tauri IPC.
- **No token refresh**: OAuth access tokens are read without refreshing. Refresh tokens rotate upon use; refreshing inside QuotaGlass would invalidate the user's primary CLI sessions.
- **Read-only access**: QuotaGlass opens vendor configuration files (`~/.claude`, `~/.codex`) read-only. It never writes to them.
- **No credential store access**: QuotaGlass never touches Windows Credential Manager or Google session credentials.
- **Settings isolation**: Settings are stored at `%APPDATA%\QuotaGlass\settings.json`, and limit warnings are stored at `%APPDATA%\QuotaGlass\warnings.json`. Neither file stores secrets or emails.
- **Tauri capability confinement**: The web view capability grants only window dragging, events, and declared app commands. It grants no unrestricted filesystem, shell, HTTP, opener or dialog access.
- **Content security policy**: The application enforces a strict CSP:
  `default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src ipc: http://ipc.localhost`
- **Telemetry and updates**: QuotaGlass contains zero telemetry, analytics, crash reporting, or automated update mechanisms.

## UI and visual design

QuotaGlass uses a two-state window: a compact pill and an expandable panel.

- **Pill**: A floating bar docked above the taskbar by default. It can be dragged to any screen position and resized horizontally (320 to 960 CSS pixels). It displays pinned accounts, or the account with the highest limit usage when none are pinned.
- **Panel**: Expands upward or downward from the pill depending on available screen space. The panel is resizable (minimum 440 by 344 CSS pixels).
- **Pace strips**: Each limit displays as a pace strip with tick marks at 10% increments. The solid bar represents percentage used. An ink line marker indicates the elapsed share of the time window.
- **Fast burn**: When usage exceeds expected pace (`used >= 1.5 x passed` and `used >= 20`), the strip turns red and displays a "fast burn" label.
- **Best now**: The account with the highest remaining room across its readable limits receives an inverted ink chip. Ties are broken by the earliest reset time.
- **Backdrop materials**: Acrylic (default), Mica, and Clear. On systems where OS glass is disabled or unavailable, a solid background tint is used.
- **Accessibility**: High contrast text over wallpapers, full keyboard navigation (`Esc` closes flyouts and the panel), and respect for Windows animation settings.

## Known deviations from original plan

The as-built release includes the following deliberate deviations from earlier design explorations:

1. **TLS implementation**: The network stack uses `native-tls` (Windows SChannel) instead of `rustls`. This avoids the `aws-lc-rs` native build on Windows and uses the TLS stack that Windows already updates.
2. **Keyring and session key omitted**: Operating system keyring storage and manual web session keys are omitted.
3. **Glass styling retained**: Translucent glass styling is retained for the desktop widget form factor.
4. **Synthetic test fixtures**: All test fixtures use synthetic tokens and identities without personal data.
5. **Orca scoped to Antigravity**: Orca is restricted to Antigravity metrics; Claude and Codex rely on direct CLI sources.
6. **Notification persistence**: The 90 percent warning uses a standard Windows toast with sound because the notification plugin cannot keep a toast on screen indefinitely.
