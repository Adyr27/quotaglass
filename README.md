# QuotaGlass

QuotaGlass is a Windows desktop widget that shows plan-limit usage for Claude, Codex and Antigravity. It sits on the desktop as a pill that opens into a panel.

Each limit appears as a pace strip. The solid fill shows the percentage of the limit used. A marker shows the share of the time window that has passed. When usage runs ahead of time (`used >= 1.5 x passed` and `used >= 20`), the strip shows "fast burn" in red.

"Best now" marks the account with the most room left across its limits. If accounts tie on room left, the one whose worst limit resets sooner wins. This mark appears when at least two accounts are readable.

## Accounts

Nothing is read until the person connects a source on the first screen.

- "Use my current Claude login" and "Use my current Codex login" read `~/.claude` and `~/.codex` read-only.
- "+ Add account" makes a profile folder in `%APPDATA%\QuotaGlass\profiles\<id>` and opens the real CLI sign-in in a console.
- Remove signs that profile out on this PC and deletes its folder.
- The current CLI login cannot be removed; it can only be hidden.

## Antigravity

Antigravity quota is read from the running Antigravity app's local server on this PC (`127.0.0.1`), only while the app is open. No Google sign-in is used.

Orca is optional and only for Antigravity.

## Warnings

Warnings fire at 50, 80 and 90 percent, once per limit window, stored in `%APPDATA%\QuotaGlass\warnings.json`.

- The 50 and 80 percent warnings are silent toasts.
- The 90 percent toast plays a sound. It is a normal toast (the notification plugin cannot keep it on screen).

## Window and settings

You can drag the pill anywhere. You can stretch it sideways and resize the panel. Sizes are remembered across runs. The window follows Windows text size.

Settings:
- Theme (System, Light, Dark)
- Glass (Acrylic, Mica, Clear)
- Always on top (on by default)
- Start with Windows
- Refresh every 1, 2 or 5 min
- Sources
- Account names
- Hidden accounts

Glass shows as a solid colour while Windows "Transparency effects" is off (Settings > Personalization > Colors). Mica looks nearly solid by design.

## Known limits

A CLI installed under a folder whose path has `&`, `|`, `<`, `>`, `^`, `%`, `!`, `"`, `(`, or `)` is refused. This includes `C:\Program Files (x86)`. Normal npm installs work.

## Privacy and safety

- No emails or tokens in labels, logs, notifications or settings keys.
- Vendor login files are read-only.
- Tokens are never refreshed.
- No Windows Credential Manager or Google session use.
- No telemetry, analytics, crash uploads or auto-updates.
- Settings are stored in `%APPDATA%\QuotaGlass\settings.json`.

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
