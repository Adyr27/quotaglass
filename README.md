# QuotaGlass

QuotaGlass is a Windows desktop widget that shows plan-limit usage for Claude, Codex and Antigravity. It sits on the desktop as a small pill with one icon per service. Provider names and icons identify the service only. They do not imply endorsement by Anthropic, OpenAI or Google, and the marks belong to their owners.

## The pill

- Each icon sits inside a thin ring. The ring fills with the percent used of the best account of that service, the one with the most room left. The ring turns amber from 80 percent and red when every account of the service is at 100 percent. A dashed, empty ring means there is no reading.
- Hover an icon to see an overlay with every account of that service and each limit: bar, percent, window length and reset time. The overlay is read-only and never takes focus.
- Click an icon, or press Enter on it, to open the popover. It lists the accounts of every service. Select an account to refresh, rename, hide, remove or sign in again. The footer has "+ Add account", Pin and Settings. Esc or a click outside closes it.
- Tab moves between icons. The tray menu has "Focus QuotaGlass".
- Drag the pill by any icon. A short click still opens the popover. Drag the grip at the right end to resize the pill. The size and place are remembered.
- Pin (tray menu or popover footer) locks the pill: no drag, no resize. The place is restored at the next start.
- The pill stays above the taskbar. It hides while a full-screen app or game runs, and returns afterwards. If you hid it from the tray, it stays hidden.

## Pace strip

Each limit shows a pace strip. The solid fill is the percentage of the limit used. A marker shows the share of the time window that has passed. When usage runs ahead of time (`used >= 1.5 x passed` and `used >= 20`), the strip shows "fast burn" in red.

"Best now" marks the account with the most room left across its limits. If accounts tie on room left, the one whose worst limit resets sooner wins. This mark appears when at least two accounts of a service are readable.

## Accounts

Nothing is read until the person connects a source on the first screen.

- "Use my current Claude login" and "Use my current Codex login" read `~/.claude` and `~/.codex` read-only.
- "+ Add account" makes a profile folder in `%APPDATA%\QuotaGlass\profiles\<id>` and opens the real CLI sign-in in a console.
- Remove signs that profile out on this PC and deletes its folder.
- The current CLI login cannot be removed; it can only be hidden.

## Antigravity

Antigravity quota is read from the running Antigravity app's local server on this PC (`127.0.0.1`), only while the app is open. No Google sign-in is used.

## Warnings and reset reminders

Usage warnings fire at 50, 80 and 90 percent, once per limit window, stored in `%APPDATA%\QuotaGlass\warnings.json`.

- The 50 and 80 percent warnings are silent toasts.
- The 90 percent toast plays a sound. It is a normal toast (the notification plugin cannot keep it on screen).

Reset reminders are off by default. Turn them on in Settings. A toast tells you when a limit window has reset. There is a switch for each service, and the sound can be Default or Silent. Pending resets are stored in `%APPDATA%\QuotaGlassesets.json`. A reset that is more than 15 minutes late when the app starts gets no toast. Focus Assist or Do Not Disturb can mute the sound. Clicking a toast opens the popover for that service, but only while the toast is still on screen.

## Rate limits

When Claude answers "too many requests", the account keeps its last reading and shows "Rate limited. Retrying in N min." in a neutral colour. Claude accounts are read at most every 5 minutes, whatever the refresh setting says. The wait after a rate limit is at most 15 minutes. Retry skips the wait.

## Window and settings

Sizes follow Windows text size.

Settings:
- Theme (System, Light, Dark)
- Glass (Acrylic, Mica, Clear)
- Always on top (on by default)
- Start with Windows
- Refresh every 1, 2 or 5 min
- Reset reminders (on or off, per service, sound)
- Sources
- Account names
- Hidden accounts

Glass shows as a solid colour while Windows "Transparency effects" is off (Settings > Personalization > Colors). Mica looks nearly solid by design.

## Known limits

The 5-minute Claude floor is a guess. Anthropic does not document the limits of this endpoint.

A CLI installed under a folder whose path has `&`, `|`, `<`, `>`, `^`, `%`, `!`, `"`, `(`, or `)` is refused. This includes `C:\Program Files (x86)`. Normal npm installs work.

## Privacy and safety

- No emails or tokens in labels, logs or notifications. Settings keys hold a hash of the email, which is not anonymous.
- Vendor login files are read-only.
- Tokens are never refreshed.
- No Windows Credential Manager or Google session use.
- No telemetry, analytics, crash uploads or auto-updates.
- The app writes only to `%APPDATA%\QuotaGlass\`: `settings.json` (window, look, sources, hidden accounts, names), `warnings.json`, `resets.json`, and `profiles\<id>\` (the CLI sign-ins that you add).

## Install

1. Download `QuotaGlass_<version>_x64-setup.exe` and `SHA256SUMS.txt` from the latest release.
2. Open PowerShell in the download folder.
3. Run `Get-FileHash .\QuotaGlass_*_x64-setup.exe -Algorithm SHA256`.
4. Make sure that the hash matches the hash in `SHA256SUMS.txt`. If it is different, do not run the file.
5. Run the installer.

The installer is not code-signed. Windows SmartScreen can show "Windows protected your PC". To continue, select **More info**, then **Run anyway**.

## Build from source

Prerequisites:
- Windows 10 or 11, x64.
- Rust stable for MSVC (`rustup default stable-x86_64-pc-windows-msvc`).
- Visual Studio 2022 Build Tools with the "Desktop development with C++" workload.
- Microsoft Edge WebView2 Runtime. Windows 11 includes it.
- Node.js 22 or later.

Commands:
1. `npm install`
2. `cargo test --workspace`
3. `npm run build`
4. `npm run tauri dev` starts the widget in development mode.
5. `npm run tauri build` makes the installer in `target/release/bundle/nsis/`.

## License

MIT. See `LICENSE`.
