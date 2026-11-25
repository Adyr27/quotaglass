# QuotaGlass

QuotaGlass is a Windows desktop widget that tracks plan-limit usage for Claude, Codex, and Antigravity. It sits on your desktop as a small pill with one icon per service. Provider names and marks identify the service only; they don't imply endorsement by Anthropic, OpenAI, or Google, and belong to their owners.

## The pill and popover

- Each service icon sits inside a thin ring filled to the percentage used on that service's best account (the one with the most room left). The ring turns amber at 80 percent and red when every account for that service reaches 100 percent; a dashed, empty ring means there is no reading yet.
- Click any icon on the pill to open the compact popover (there is no hover overlay). Provider tabs (`All`, `Claude`, `Codex`, `Antigravity`) and a 1-click `Refresh` button sit across the top, while the footer holds `+ Add account`, `Pin`, `FAQ`, and `Settings`. Press `Esc` or click outside to close it.
- Click an account card inside the popover to expand every limit, inline reset actions (Codex banked resets with `Redeem`, Claude `Check limit resets`), and controls to refresh, rename, hide, remove (with 2-step confirmation), or sign in again.
- While unpinned, drag the pill by any icon to move it; a short click still opens the popover. Drag the grip at the right edge to resize the pill between `0.85x` and `2.0x`.
- `Pin` (in the popover footer or the tray menu) locks the pill against dragging and resizing and saves a taskbar-relative anchor per monitor setup, so it returns to the taskbar when you switch displays.
- The pill stays above the taskbar, hides while a full-screen app or game runs, and comes back afterwards. If you hide it from the tray menu, it stays hidden until you bring it back. The tray menu also gives quick access to `Focus QuotaGlass`, `Pin pill`, `Refresh now`, and `Settings`.

## Pace strip

Each limit has a pace strip where the solid fill shows the percent used and a vertical marker shows how much of the time window has passed. When usage runs ahead of time (`used >= 1.5 x passed` and `used >= 20`), the strip flags `fast burn` in red.

`Best now` marks the account with the most room left across its limits. If two accounts tie on room left, the one whose worst limit resets sooner wins; the badge appears whenever at least two accounts of a service are readable.

## Accounts

Nothing is read until you connect a source on the first screen.

- "Use my current Claude login" and "Use my current Codex login" read `~/.claude` and `~/.codex` read-only.
- `+ Add account` creates a profile folder under `%APPDATA%\QuotaGlass\profiles\<id>` and opens the real CLI sign-in in a console window.
- Removing an added account asks for confirmation, signs that profile out on this PC, and deletes its profile folder.
- Your current CLI login can't be removed; you can only hide it.

## Antigravity

Antigravity quota is read from the local Antigravity server on this PC (`127.0.0.1`) while Antigravity is running. If Antigravity is closed, QuotaGlass keeps the last reading visible until you open it again. No Google sign-in is used.

## Warnings and reset reminders

Usage warnings fire at 50, 80, and 90 percent, once per limit window, and are saved in `%APPDATA%\QuotaGlass\warnings.json`. The 50 and 80 percent warnings are silent Windows toasts; the 90 percent toast plays a sound and notes when a Codex banked reset is available.

Reset reminders start off by default, and you can turn them on in Settings. They send a toast when a limit window resets or when a Codex banked reset becomes available, with per-service switches and a `Default` or `Silent` sound option. Pending resets live in `%APPDATA%\QuotaGlass\resets.json`. Any reset that is more than 15 minutes late when QuotaGlass launches gets no toast, and Windows Focus Assist or Do Not Disturb can mute notification sounds. Clicking a toast opens the popover on that service while the toast is still on screen.

## Rate limits

When Claude returns "too many requests", the account keeps its last reading and shows "Rate limited. Retrying in N min." in a neutral colour. Claude accounts are polled at most every 5 minutes regardless of the refresh setting, and multiple Claude accounts are staggered across the window so they don't burst together. The cooldown after a rate limit caps at 15 minutes; clicking `Refresh` skips the wait.

## Update checks

Once a day and at startup, QuotaGlass sends one anonymous HTTPS GET request to `api.github.com/repos/Adyr27/quotaglass/releases/latest` (nothing about you or your accounts is sent). When a newer release exists, the popover footer shows `Update to vX.Y.Z`, which opens the GitHub Releases page in your browser. QuotaGlass never downloads or installs binaries automatically. The check is on by default, and you can turn it off on the first screen or in Settings (`Check for updates`) so no update request leaves your PC.

## Window and settings

Settings:
- Theme (`System`, `Light`, `Dark`)
- Glass (`Liquid`, `Clear`, `Frosted`)
- Refresh every `1`, `2`, or `5` min
- Always on top (on by default)
- Start with Windows
- Check for updates (on by default)
- Reset reminders (on or off, per service, sound)
- Sources
- Hidden accounts

Glass falls back to a solid surface when Windows "Transparency effects" is off (Settings > Personalization > Colors).

## Known limits

The 5-minute Claude floor is a guess; Anthropic doesn't document the rate limits of that endpoint.

A CLI installed under a folder whose path contains `&`, `|`, `<`, `>`, `^`, `%`, `!`, `"`, `(`, or `)` is refused, including `C:\Program Files (x86)`. Standard npm installs work.

## Privacy and safety

- No emails or tokens appear in labels, logs, or notifications. Settings keys store a hash of the email rather than the address itself, though a hash isn't anonymous.
- Vendor login files are opened read-only, and OAuth tokens are never refreshed.
- No Windows Credential Manager or Google session access.
- No telemetry, analytics, or crash uploads. Outside Anthropic, OpenAI, and localhost, the only network request is the daily GitHub Releases check, which you can switch off.
- QuotaGlass writes only inside `%APPDATA%\QuotaGlass\`: `settings.json`, `readings.json`, `snapshot_cache.json`, `warnings.json`, `resets.json`, and `profiles\<id>\` (the CLI sign-ins you add).

## Install

QuotaGlass ships as a portable single-file `.exe`. You don't need an installer or WebView2.

1. Download `QuotaGlass_<version>_x64.exe` and `SHA256SUMS.txt` from the latest release.
2. Open PowerShell in the download folder.
3. Run `Get-FileHash .\QuotaGlass_*_x64.exe -Algorithm SHA256`.
4. Confirm the hash matches `SHA256SUMS.txt`. If it doesn't match, delete the file and don't run it.
5. Put `QuotaGlass_<version>_x64.exe` wherever you want to keep it and launch it.

The binary isn't code-signed by Microsoft, so Windows SmartScreen may show "Windows protected your PC". Choose **More info**, then **Run anyway**.

## Build from source

Prerequisites:
- Windows 10 or 11, x64.
- Rust stable for MSVC (`rustup default stable-x86_64-pc-windows-msvc`).
- Visual Studio Build Tools with the "Desktop development with C++" workload.

No Node.js or WebView2 is needed for the native v0.5.0 app.

Commands:
1. `cargo test --workspace`
2. `cargo run -p quotaglass-app`
3. `cargo build --release -p quotaglass-app` (outputs `target/release/quotaglass-app.exe`)

## License

MIT. See `LICENSE`.
