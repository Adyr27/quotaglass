# Security audit - QuotaGlass - 2026-11-08

Scope: the whole repository (`crates/core`, `src-tauri`, `src`, CI), before the first commit.

Scanners:
- Code review: one full review pass, then a manual pass over the ghost scan-code vectors that apply to a desktop app (command injection, SQL injection, sensitive logging, DOM XSS, console leaks, SSRF, sourcemaps, unsafe deserialization). The ghost scan-code loop could not run, because it needs the Claude Code CLI.
- Secrets: ghost poltergeist v2.0.5.
- Dependencies: ghost wraith v2.0.2 with osv-scanner, on `Cargo.lock` and `package-lock.json`. Also `npm audit --omit=dev`.
- Scanner binaries: downloaded from the GitHub releases. The SHA-256 of each file matches its Sigstore bundle, and the signing certificate names the vendor's release workflow. This was not a full Sigstore chain check.

Result: 0 Critical, 0 High. The review found 6 Medium and 9 Low findings. All Medium findings are fixed. Low findings are fixed or accepted below.

## Fixed

- M1: A source that has an account but no numbers (for example Antigravity before its first reading) no longer hides numbers from another source for the same account.
- M2: When the vendor's window length is used, the vendor's bucket name is used too, so the name and the length agree ("30 days").
- M3: The headline skips a bucket whose window has already reset.
- M4: A bucket whose window reset, or whose reading is older than its window, is dimmed. It never shows "Limit reached".
- M5: When a login file or program disappears, its stored readings are removed too.
- M6: The snapshot is rebuilt every 30 seconds, so freshness and age labels follow the clock between fetches.
- L1: The HTTP body and the Orca output are read with a running size cap (1 MiB). They are no longer read in full first.
- L2: The formatted `Bearer` header string is wiped on drop. The header value is marked sensitive.
- L3: "Connect" runs the sign-in tool by its absolute path, as found on PATH. A relative path is refused, so `cmd.exe` cannot pick up a file from the working directory.
- L4: The Claude email is read from `.claude.json` inside a custom `CLAUDE_CONFIG_DIR` first.
- L6: One bad OMP row is skipped. It no longer hides every other OMP account.
- L7: An Orca reading with numbers but no time stamp is dropped.
- L8: Orca uses the system-login email only when no Orca-managed account is active.

## Accepted risk

- L2 (part): The Codex `id_token` stays a plain string. It holds identity claims only. The usage request does not send it.
- L5: OMP readings use one time stamp per account, not one per bucket. The effect is on the age label only.
- L9: Unreachable code path. No effect.
- RUSTSEC-2024-0429 / GHSA-wrw7-89jp-8q8g (`glib` 0.18.5) and RUSTSEC-2024-0370 (`proc-macro-error`, unmaintained): Linux GTK dependencies of Tauri. `cargo tree --target x86_64-pc-windows-msvc` shows neither one in the Windows build.
- Secrets scan: 180 matches. All of them are in `node_modules`, `target` or the test fixtures. The two fixtures (`claude_credentials.json`, `codex_auth.json`) hold `SYNTHETIC` tokens and unsigned (`alg: none`) JWTs for `user@example.com`.

## Not checked

- `cargo audit` is not installed. wraith covered `Cargo.lock` instead.
- No fuzzing of the JSON parsers.
- No full Sigstore chain verification of the scanner binaries (no cosign on this PC).
- The Orca-managed account format, because no managed account exists on the test PC.

## Evidence

- `cargo test --workspace`: 86 passed, 0 failed.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean. `cargo fmt --all --check`: clean. `npm run build`: built.
- `npm audit --omit=dev`: 0 vulnerabilities. wraith `package-lock.json`: no issues. wraith `Cargo.lock`: the 3 advisories above, all outside the Windows build.
- `npx cc-safety-net status`: ready.
- Search of the repository for real emails, user paths and key prefixes: only the public GitHub handle in `LICENSE` and the bundle identifier.
- Smoke run with real accounts: 5 cards and no duplicate card; the pin round-trip works; the 30-second rebuild was measured at 29 979 ms.
- Resource use (debug build, 60 seconds): app working set 42.6 MB (9.4 MB private); CPU 0.08 s.
