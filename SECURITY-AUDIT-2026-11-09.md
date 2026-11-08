# Security audit - QuotaGlass - 2026-11-09

Scope: commit `a5801a3` on `v0.2.0-design` (owner bug pass and in-process Antigravity discovery) and pre-public CI hardening. Focus on `src-tauri/src/window.rs` (Win32 subclass FFI), `crates/core/src/sources/antigravity_app.rs` (Toolhelp and `GetExtendedTcpTable` FFI, loopback calls), `src-tauri/src/settings.rs` (load, unknown-source drop), `crates/core/src/warnings.rs`, and `.github/workflows/`.

Scanner: Ghost `scan-code` 1.1.1, depth quick (primary). Supporting: Ghost `poltergeist` (secrets), Ghost `wraith` with `osv-scanner` (dependencies), `npm audit --omit=dev`, manual CI workflow review.

Result:
- `scan-code`: 2 projects, 6 vectors (ssrf, unsafe-deserialization x2, path-traversal, missing-input-validation, race-condition), 18 of 18 file analyses complete, 0 findings.
- `scan-code` frontend (`. (frontend)`): 3 vectors (dom-xss, css-injection, client-authz-bypass), 7 of 7 file analyses complete, 0 findings. Account data reaches the DOM only as text nodes or fixed-name attributes; no `innerHTML`, `outerHTML`, `insertAdjacentHTML`, `document.write`, `eval` or `new Function` in `src/`. The only data written into CSS is two numeric custom properties set with `style.setProperty`. URL targets and account ids are resolved and allowlisted on the Rust side, so no frontend check guards a privilege.
- Secrets: 186 raw matches. 181 are in `node_modules/` and `target/` (third-party and build output, not tracked). The 5 in tracked files are synthetic test data (placeholder tokens made of the words "access token" and "refresh token", a JWT for `user@example.com` signed with the literal `sig`) and one test function name. 0 real secrets.
- Dependencies: npm 0 vulnerabilities. `Cargo.lock`: 6 advisories in 5 crates, all Linux-only and absent from the Windows build (`cargo tree -i <crate> --target x86_64-pc-windows-msvc` prints nothing).

Fixed:
- CI workflows (`.github/workflows/ci.yml`, `.github/workflows/release.yml`): pinned all GitHub Actions (`actions/checkout`, `dtolnay/rust-toolchain`, `actions/setup-node`, `actions/cache`, `actions/attest-build-provenance`) to 40-character commit SHAs and passed `github.ref_name` through `env:` in `release.yml`.
- Docs: replaced the local user profile path in `docs/superpowers/` with `$HOME\.cargo\bin`.

Accepted risk:
- RUSTSEC-2024-0429 / GHSA-wrw7-89jp-8q8g (`glib` 0.18.5, unsoundness): GTK dependency of Tauri on Linux; not compiled for Windows, the only target.
- RUSTSEC-2024-0370 (`proc-macro-error` 1.0.4), RUSTSEC-2026-0371 (`zbus_names` 4.3.5), RUSTSEC-2026-0370 (`zvariant` 5.15.1), RUSTSEC-2026-0372 (`zvariant_derive` 5.15.1): "unmaintained" notices, Linux-only, not compiled for Windows.

Not checked:
- `cargo audit` was not run locally (not installed); CI runs `cargo-audit` 0.22.2 on every push.
- The `osv-scanner` read of `package-lock.json` listed 0 packages; `npm audit` covers that lockfile instead.
- Ghost `scan-secrets` and `scan-deps` orchestrators were not used because they spawn harness subagents; their scanner binaries were run directly with the same commands.

Evidence:
- `~/.ghost/repos/<repo-id>/scans/a5801a3/code/` (`plan.md`, `nominations.md`, `analyses.md`, `no-findings.md`)
- `~/.ghost/repos/<repo-id>/scans/a5801a3/code-frontend/` (same files, frontend run)
- `~/.ghost/repos/<repo-id>/scans/a5801a3/secrets/scan-output.json`
- `~/.ghost/repos/<repo-id>/scans/a5801a3/deps/scan-Cargo.lock.json`, `scan-package-lock.json.json`
- `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `npm run build`: pass. Live Antigravity fetch through the new discovery: 4 buckets, 0.05 s.
