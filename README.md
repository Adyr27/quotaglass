# QuotaGlass

QuotaGlass is a small Windows desktop widget. It shows how much of each AI plan limit you have used, for every account that is signed in on your PC.

It shows Claude, Codex and Antigravity accounts. Each account shows its 5-hour, weekly and per-model limits, with the time each limit resets.

QuotaGlass reads numbers only from real sources. It never asks you to type a number, and it never guesses one.

## Install

1. Download `QuotaGlass_<version>_x64-setup.exe` and `SHA256SUMS.txt` from the latest release.
2. Open PowerShell in the download folder.
3. Run `Get-FileHash .\QuotaGlass_*_x64-setup.exe -Algorithm SHA256`.
4. Make sure that the hash is the same as the hash in `SHA256SUMS.txt`. If it is different, do not run the file.
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
2. `npm run tauri dev` starts the widget in development mode.
3. `npm run tauri build` makes the installer in `target/release/bundle/nsis/`.
4. `cargo test --workspace` runs the tests.

## Where the numbers come from

No AI vendor publishes an official API for plan limits. Each source below is either another tool on your PC or an endpoint that the vendor does not document. A vendor can change an undocumented endpoint at any time. If a source stops working, its cards show the reason and QuotaGlass uses the next source.

| Source | What it reads | Accounts it covers | Default |
|---|---|---|---|
| Claude Code login file | `~/.claude/.credentials.json`, then `GET api.anthropic.com/api/oauth/usage` | The account signed in to Claude Code | On (unofficial endpoint) |
| Codex login file | `~/.codex/auth.json`, then `GET chatgpt.com/backend-api/wham/usage` | The account signed in to the Codex CLI or the Codex app | On (unofficial endpoint) |
| Orca | The output of `orca account list --json` | Claude and Codex accounts in Orca, and all Antigravity accounts in Orca | On, if Orca is installed |

If two sources report the same account, QuotaGlass shows one card. It uses the email address to match accounts. If a source gives no email address, QuotaGlass does not merge that account with another.

You can turn off each source in **Settings > Sources**.

When two or more sources read the same account, QuotaGlass picks one in this order: direct, then Orca.

## Accounts that QuotaGlass cannot read

| You use only... | Result |
|---|---|
| ChatGPT chat (no Codex) | No card. There is no known source for ChatGPT chat limits. |
| The Antigravity IDE or `agy`, without Orca | No card. Antigravity keeps its login in encrypted app storage. QuotaGlass reads Antigravity limits only through Orca. |
| Claude Desktop or claude.ai, without Claude Code | No card. Sign in to Claude Code, or add the account to Orca. |

QuotaGlass does not have a manual entry field. A typed number becomes wrong as soon as you use the tool, and QuotaGlass does not show numbers that it cannot keep correct.

## Sign in

If a Claude or Codex login expires, the card shows **Sign in**. This button opens a terminal that runs the official sign-in command (`claude auth login` or `codex login`). Your browser opens the vendor sign-in page. QuotaGlass never sees your password.

Antigravity has no sign-in command. To add an Antigravity account, sign in with `agy`. Then add the account in Orca, in **Settings > AI Provider Accounts**.

## Privacy and security

- QuotaGlass connects only to `api.anthropic.com` and `chatgpt.com`, over HTTPS. It does not follow redirects.
- QuotaGlass has no telemetry, no analytics, no crash upload and no update check.
- QuotaGlass reads the login tokens of Claude Code and Codex only in memory, for one request. It does not copy them, store them or show them.
- QuotaGlass never refreshes a login token. A refresh can sign you out of the CLI.
- QuotaGlass opens the files of other apps read-only.
- QuotaGlass keeps its settings in `%APPDATA%\QuotaGlass\settings.json`. This file contains no tokens.
- QuotaGlass does not update itself. To update, download the new release.

## License

MIT. See `LICENSE`.
