# QuotaGlass — Technical Design Document

This document describes the architecture, data sources, security model, and implementation details of QuotaGlass.

## Purpose and Scope

QuotaGlass is a desktop widget for Windows 10 and Windows 11 (x64). It monitors subscription plan limits across artificial intelligence development accounts.

The application displays remaining quota for:
- Claude (Anthropic)
- Codex (OpenAI)
- Antigravity (Google)

QuotaGlass reads usage metrics only from existing local tools and vendor HTTP endpoints. It does not track API token expenditure, financial balance, or ChatGPT consumer web chat limits. It provides no automated updater and no manual entry forms.

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

### Component Roles

1. `crates/core`
   Contains business logic and provider integrations without Tauri dependencies.
   - Discovers local configuration files and command-line binaries.
   - Performs network requests to authorized vendor endpoints.
   - Parses vendor metrics into normalized bucket records.
   - Merges metrics across sources and calculates data freshness.
   - Protects access tokens using non-serializable memory containers.

2. `src-tauri`
   Hosts the operating system application shell and desktop integration.
   - Manages asynchronous polling schedules and snapshot generation.
   - Persists user preferences to `%APPDATA%\QuotaGlass\settings.json`.
   - Manages the frameless translucent window, Acrylic blur, and system tray.
   - Exposes safe IPC commands to the user interface.

3. `src/`
   Renders the visual presentation layer.
   - Implemented in TypeScript using standard DOM APIs and Vite.
   - Renders service groups, account cards, quota bars, and flyout overlays.
   - Communicates with the backend exclusively through typed IPC wrappers.

## Data Sources and Priority

QuotaGlass gathers metrics using three primary source tiers:

| Priority | Source | Mechanism | Accounts Covered |
|---|---|---|---|
| 1 (Highest) | Direct | Reads local CLI credentials, then queries vendor endpoints | Active Claude Code login; active Codex CLI or desktop login |
| 2 | Orca | Executes `orca account list --json` | Managed Claude, Codex, and all Antigravity accounts |
| 3 (Lowest) | OMP | Reads `%USERPROFILE%\.omp\agent\agent.db` SQLite database | Anthropic and Antigravity accounts logged into OMP |

### Priority and Selection Rules

- If multiple sources provide data for the same account, Direct wins over Orca, and Orca wins over OMP (`Direct -> Orca -> OMP`).
- A live response from the current refresh cycle always takes precedence over cached historical readings.
- If a source answers with an empty bucket list while another source has metrics, the source with metrics takes precedence.
- If Orca reports a non-standard window length, QuotaGlass overrides it with the vendor window definition from Direct readings when present.
- `settings.json` can hold per-account overrides: pin one source, or turn off sources, for one account. v0.1.0 has no UI for these overrides.

## Account Merging by Email

QuotaGlass merges records across sources with an account key. The key is the service plus the email address, trimmed and lower-cased (for example `claude:user@example.com`).

### Merging Rules

- An account record must contain a valid email address to merge with another source.
- If a reading lacks an email address, QuotaGlass generates an isolated unmerged card (e.g., `Claude (CLI login)` or `Codex (Orca)`).
- Two readings without email addresses never merge, preventing cross-account contamination.

## Freshness Rules

Snapshots update internally whenever new data arrives. In addition, the background scheduler rebuilds the visible snapshot every 30 seconds to update age indicators and expiration states between network polls.

Each usage bucket is classified into one of three freshness tiers:

| Tier | Condition | Presentation |
|---|---|---|
| Live | Age is within two refresh intervals, and reset time is in the future. | Normal full-contrast progress bar. |
| Aged | Reading is older than two refresh intervals, but reset time is in the future. | Normal progress bar with an age label (e.g., `2h ago`). |
| Stale | The reset timestamp has already passed, or reading age exceeds the bucket window duration. | Dimmed progress bar; never shows "Limit reached". |

If an account reading becomes stale because its window reset, QuotaGlass dims the card to avoid presenting historical exhaustion as current state.

## IPC Surface

The web view communicates with the Rust backend through an explicit IPC boundary.

### Commands

| Command | Arguments | Return Type | Description |
|---|---|---|---|
| `get_snapshot` | None | `Snapshot` | Returns the current aggregated view model. |
| `refresh_now` | None | `()` | Triggers an immediate poll across all active sources. |
| `detect_sources` | None | `Vec<SourceStatus>` | Detects availability of local tools and files. |
| `get_settings` | None | `Settings` | Retrieves current application configuration. |
| `update_settings` | `{ patch: SettingsPatch }` | `Result<Settings, String>` | Updates configuration and triggers window or scheduler updates. |
| `connect` | `{ service: Service }` | `Result<(), String>` | Spawns a visible terminal running official login commands. |

### Events

| Event | Payload | Description |
|---|---|---|
| `snapshot` | `Snapshot` | Emitted when refreshed metrics or freshness calculations change. |
| `settings` | `Settings` | Emitted when configuration settings change. |
| `backdrop` | `{ acrylic: bool }` | Notifies the UI if native Acrylic transparency succeeds or fails. |
| `open-settings` | None | Emitted from system tray menu to open the settings view. |

## Security Model

QuotaGlass handles authentication artifacts under strict isolation constraints.

- **Network Allowlist**: Network requests connect exclusively to `https://api.anthropic.com` and `https://chatgpt.com`. No arbitrary URL fetching is permitted.
- **No HTTP Redirects**: The HTTP client refuses redirects to prevent credential forwarding.
- **Response Size Cap**: Incoming HTTP responses and CLI standard output streams have a hard 1 MiB consumption limit (`MAX_BODY = 1024 * 1024`).
- **Memory Protection**: Access tokens are wrapped in a non-serializable `Secret` container backed by `zeroize::Zeroizing`. Memory is zeroed on drop. Tokens never pass across Tauri IPC.
- **No Token Refresh**: OAuth access tokens are read without refreshing. Refresh tokens rotate upon use; refreshing inside QuotaGlass would invalidate the user's primary CLI sessions.
- **Read-Only Access**: QuotaGlass opens the files and databases of other apps read-only. It never writes to them.
- **Settings Isolation**: Settings are stored at `%APPDATA%\QuotaGlass\settings.json`. The file stores user preferences, window geometry, and source toggles. It never stores secrets.
- **Tauri Capability Confinement**: The web view capability grants only window dragging, events, and the six app commands. It grants no filesystem, shell, HTTP, opener or dialog access.
- **Content Security Policy**: The application enforces a strict CSP:
  `default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src ipc: http://ipc.localhost`
- **Telemetry and Updates**: QuotaGlass contains zero telemetry, analytics, crash reporting, or automated update mechanisms.

## UI and Visual Design

QuotaGlass displays a borderless floating widget measuring 360 by 520 pixels (minimum 300 by 360 pixels).

- **Backdrop**: Windows 11 native Acrylic translucency applied via platform APIs. If unsupported, the window falls back to CSS backdrop blur (`rgba(12, 13, 15, 0.86)` with `backdrop-filter: blur(24px)`).
- **Service Hues**: Distinct brand hues identify services on progress bars, indicators, and headers:
  - Claude: Clay (`#D97757`)
  - Codex: Emerald (`#10A37F`)
  - Antigravity: Blue (`#4C8DF6`)
- **Interaction**: Drag the window by its header. **Pin** (header button or tray menu) locks the position and turns off dragging. Click an account card to open an overlay with all limit buckets for that account.

## Known Deviations from Original Plan

The as-built v0.1.0 release includes the following deliberate deviations from earlier design explorations:

1. **TLS Implementation**: The network stack uses `native-tls` (Windows SChannel) instead of `rustls`. This avoids the `aws-lc-rs` native build on Windows and uses the TLS stack that Windows already updates.
2. **Keyring and Session Key Deferred**: Operating system keyring storage (`keyring` crate) and the manual `claude.ai` web session key source are deferred and omitted from v0.1.0. No session key settings exist in the application.
3. **Claude Color Exception**: The warm clay accent hue (`#D97757`) is retained as a deliberate design exception for visual service recognition.
4. **Glass Styling Retained**: Translucent acrylic glass styling is retained for the widget form factor despite general frontend design preferences favoring opaque panels.
5. **Synthetic Test Fixtures**: All test fixtures use synthetic tokens and identities (`user@example.com`, unsigned `alg: none` JWTs). They contain no real credentials or personal data.
6. **Orca Managed Account Labeling**: When an Orca-managed account is active, QuotaGlass does not infer or guess an email address. The card is labeled `Claude (Orca)` or `Codex (Orca)` because managed-account schema properties are unverified.
