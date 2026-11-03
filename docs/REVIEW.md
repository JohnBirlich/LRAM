# RoALT — technical audit of the repository

> **Historical note.** The audit describes the state before the product was renamed to **LRAM**.
> What was renamed and how, what is deliberately left as it was, and how to ship your own build, is in
> `docs/REBRAND.md`; the interface design is in `docs/DESIGN-LRAM.md`.

Date: 1 November 2026. Revision: `901a230` ("Release 0.9.0"), branch `arena/50c7b730-ram`.
Scope: 529 files, ~68 600 lines across `.rs/.ts/.tsx/.css/.json`, 285 Rust tests.

> What I actually ran: `npm ci`, `tsc --noEmit`, `vite build`, `node scripts/check-i18n.mjs` — all pass.
> I could not compile the Rust part: the sandbox has no toolchain and the code is entirely
> Windows-only (`windows` 0.62, `webview2-com`, `Win32_*`, `edition 2024`, `rust-version 1.95`). The
> Rust conclusions come from reading the code, not from running the tests.

---

## 1. What this is

A local Roblox account manager and multi-client launcher for Windows: it stores `.ROBLOSECURITY`
cookies encrypted, shows status/Robux/friends, launches one or many clients into a game, follows the
processes, limits their resources, captures window previews. All offline, no server, no telemetry —
"only Roblox and GitHub".

Stack: **Tauri 2.12 (Rust) + React 19 + TypeScript + Vite 8 + Tailwind 4 + TanStack Query**, SQLite
through **SQLCipher**, `ts-rs` for the IPC types, Inno Setup for the installer.

## 2. Map of the code

```
src-tauri/src/
  main.rs            bootstrap, window, tray/plugins, the App state, data-folder move (770)
  commands.rs        the whole IPC layer (106 commands) (2558)
  model.rs           domain types + Settings/SettingsPatch (1290)
  roblox_api/        typed Roblox client: CSRF, rate limits, cookie rotation (841)
                     endpoints.rs — every URL in one place; tests.rs — a fake HTTP server (1185)
  launcher/          link parsing, client-assertion → ticket, `roblox-player:` URI, bulk launch,
                     client start, Authenticode check of the Roblox installer, FFlags, updates
  instances/         process tracker, multi-instance (named mutex), limits (job objects),
                     QoS, anti-AFK, auto-rejoin, preview capture, resize/ghost windows, log watcher
  vault/             envelope encryption: DPAPI / Argon2id+AES-256-GCM / recovery code / backups
  storage/           SQLCipher database, migrations (9), crash-safe re-encryption of the old database
  webview.rs         Roblox sign-in and session windows on throwaway WebView2 profiles
  network.rs         the "public network guard" through the Network List Manager
  discord.rs / rpc.rs webhook alerts / rich presence
  self_update.rs     updating RoALT itself (ECDSA P-256 signature)
```

Frontend: `src/App.tsx` (shell, navigation, themes, hotkeys) + `features/*` (accounts, launch,
dashboard, servers, performance, monitor, settings, vault, discord, debug, splash) + `ui/*` (Dialog,
Icon, Resizable, TitleBar, Toast, controls) + `i18n/` (18 catalogues) + `bindings/` (types generated
from Rust).

The layers are separated honestly: UI → `api.ts` → Tauri commands → domain modules → `storage`/`roblox_api`.

## 3. How security is built

**Storage.** The database is encrypted whole (SQLCipher, AES-256 per page + HMAC-SHA512). A random
256-bit key sits next to it in `roalt.db.key`, wrapped by the current user's DPAPI; without it the
database file is useless on another PC or account. `PRAGMA secure_delete = ON`, WAL, crash-safe
migrations (export → fingerprint check of every table → atomic renames → `resume` after a crash).

**Envelope encryption of secrets.** One data key encrypts each secret with AES-256-GCM: a random nonce
per value, AAD = the row's identity (`roalt.cookie:<user_id>`), so a blob cannot be moved into another
row. The data key is stored wrapped:
- DPAPI (default) — opens silently;
- master password: `DPAPI(AES-GCM(Argon2id(password)))`, Argon2id 64 MiB / t=3 / p=4;
- recovery code: the same under Argon2id 256 MiB, single use, 9 characters from an alphabet without
  `0 O 1 I L` (~44 bits), the old code is revoked when used.

Switching the mode only re-wraps the key; the secrets are not re-encrypted. `Secret` is a
`Zeroizing<String>`, `Debug` prints `Secret(..)`, and the data key is cleared on lock/auto-lock.

**IPC.** Roblox windows (`login-*`, `session-*`) have no capability at all, and every command is
additionally wrapped in `main_only`, which rejects calls from any window but `main`. Capabilities are
minimal (window, dialogs, clipboard, opener only to `roblox.com` and `github.com/JohnBirlich/*`). A CSP
is set, `frame-ancestors 'none'`, external content never loads into the main window
(`on_navigation` → only `tauri.localhost`/`tauri:`).

**Roblox windows.** Sign-in is the real `roblox.com/login` page in WebView2 on a throwaway incognito
profile under `tmp-webview/<uuid>`, with autofill, devtools and the context menu off; the session
cookie is injected through `ICoreWebView2CookieManager` (not `set_cookie`, which drops the leading
dot) before the first navigation. The profile is deleted after the window closes, and leftovers are
swept on start. `WEBVIEW2_*` variables are stripped from the environment so the profile cannot be
swapped.

**Secrets in logs.** Cookies, tickets, `gameinfo:`, link codes are never logged: tickets and URIs live
in `Secret`, diagnostics (`diagnostics.rs`) scrub lines by the `logwatch::SENSITIVE` list, and
`error.rs` explicitly demands "no cookies in messages". This is checkable and mostly held.

**Clipboard.** Copying a cookie is done from Rust (`write_secret`, no history, no cloud) and cleared
after N seconds or on exit, and only if our secret is still there.

**Network.** Only Roblox, GitHub (update check) and an optional Discord webhook; the HTTP client has no
cookie jar, uses `rustls`, redirects are refused (except in the updater), and the updater has a host
allowlist and size limits. The RoALT update verifies an **ECDSA P-256** signature over
`"RoALT-Setup-X.Y.Z.exe\n" + bytes`, the Roblox installer — **Authenticode Roblox Corporation**.

## 4. Strengths

1. **The cryptography is done properly**, not by eye: envelope encryption, AAD per row, Argon2id with
   RFC 9106 parameters, DPAPI over the master password (a stolen database cannot be brute-forced
   outside this account), a recovery code as a separate wrapper with a different AAD, KDF caps on
   backup import (protection against a hostile file).
2. **The threat model lives in the code, not in the README.** `main_only`, empty capabilities for the
   Roblox windows, CSP, throwaway profiles, `WEBVIEW2_*` cleanup, `icacls` on the data folder,
   `secure_delete`, key zeroing, log redaction. This is above average even for commercial tools of
   this class.
3. **The supply chain is closed better than most**: a separate update-signing key + Authenticode
   verification of the Roblox installer + no redirects to foreign hosts + size limits + refusing to
   install without a click.
4. **Engineering discipline.** 285 tests (including real HTTP tests of the client against a local
   server and Roblox response fixtures in `docs/research/fixtures`), `clippy -D warnings` in CI, an
   in-house i18n completeness checker (keys, placeholders, plural categories), `ts-rs` instead of
   hand-written IPC types, every URL in one file, comments that explain *why* (including `ponytail:`
   marked trade-offs).
5. **Honesty about Roblox.** The documentation records where reality differed from expectations (CSRF
   out of 403, cookie rotation, `x-ratelimit-reset` instead of `Retry-After`, the vanished "last
   online", HBA/challenges, the FFlags allowlist), and the code reflects it.
6. **The client is not modified**: multi-instance through a named mutex/event before start, limits
   through job objects/priority/affinity, previews through Windows.Graphics.Capture. For anti-cheats
   this is a fundamentally safer approach than patching the binary.
7. **Product maturity**: 17 languages plus English, dark/light theme, a custom accent, background,
   density, drag and drop, tray, a global kill-all hotkey, diagnostics, crash history, an Unlock
   screen, a splash, error handling through a single `AppError`.
8. **The frontend is put together carefully**: strict TS without `any`, not a single
   `dangerouslySetInnerHTML`, one `QueryClient` invalidated by backend events, 207 `aria-*`
   attributes, focus-loss handling, an ErrorBoundary.

## 5. Shortcomings and risks

### P1 — what really affects security

1. **The default mode (DPAPI) does not protect against malware.** The database key and the data-key
   wrapper are read silently by any process under the same user. Windows Hello / the master password
   stand only in front of *showing* and *copying* a cookie — launching a client and talking to the API
   go without them. So the master password protects against "someone sat down at my PC", not against a
   stealer. This deserves to be said plainly in the README and in Settings, not just "Encrypted on
   this PC".
2. **`take_over()` is the riskiest place in the project.** To take `ROBLOX_singletonEvent` away from an
   already running client, the code does `OpenProcess(PROCESS_DUP_HANDLE)` into the Roblox process and
   `DuplicateHandle(..., DUPLICATE_CLOSE_SOURCE)`. Hyperion/Byfron generally blocks such operations on
   current clients, and from an anti-cheat's point of view it looks like outside interference in the
   process. The function is not on by default (only behind a confirmation), but it should either go or
   carry an explicit warning about ban risk and it not working.
3. **`protect_shared_login` (on by default) holds `RobloxCookies.dat` and `appStorage.json` with share
   mode 0.** That blocks Roblox itself and any third-party tool; with unlucky timing it can leave a
   client without saved session. The reasoning is in the code (error 773), but the default is
   aggressive.
4. **A cookie is full access to an account, around 2FA.** The user keeps 10+ accounts in one place: one
   leak of a `.lram` backup with a weak password = every account lost. The app warns honestly, but the
   "many cookies in one file" model is toxic by nature.
5. **The installer is not Authenticode-signed** (`HAS_SIGNPATH` in release.yml, SignPath not approved
   yet). For an app that asks for Roblox cookies, the absence of a publisher signature is a serious
   trust minus: SmartScreen complains, and the user cannot check that the binary was built from this
   code.
6. **No LICENSE file.** Legally that is "all rights reserved": forking, redesigning and distributing it
   without the author's permission is not allowed. For the task at hand this is a blocker to settle
   before work.

### P2 — architectural and operational

7. **Hard coupling to Roblox internals.** The singleton event name, LocalStorage file names, client log
   formats, the FFlags allowlist, the endpoint set — all of it can change without warning
   (multi-instance already broke in mid-2025). The app is architecturally ready (the feature drops out
   with a message), but stability cannot be promised.
8. **A fork will update itself from upstream.** `self_update.rs` hardcodes
   `api.github.com/repos/inspectsoftware/RoALT/releases/latest` and verifies the signature with the
   author's public key. With your own design and those constants unchanged, the app would quietly offer
   (and on a click install) the **original** RoALT over your build — the whole redesign gone. The same
   key means you physically cannot ship your own updates until you generate your own key pair and
   rewrite `PUBLIC_KEY`.
9. **There are no tests where they are needed most**: 0 frontend tests, no e2e over Tauri commands. In
   `docs/MILESTONES.md` the UI was "hand-tested by the user". For a project that touches other people's
   accounts that is a bottleneck.
10. **Formatting is not pinned.** There is no `rustfmt.toml`, while 1464 lines are longer than 100
    characters (the style is clearly "120 columns"), and `model.rs` (~line 489) has a broken indent; CI
    does not run `cargo fmt --check`. Diffs will be noisy and contributors will suffer.
11. **Bloated modules**: `commands.rs` — 2558 lines, `instances/tracker.rs` — 2415,
    `instances/window.rs` — 1185, `launcher/bulk.rs` — 1201. `commands.rs` is already asking to be split
    by feature.
12. **Blocking calls.** SQLite queries run synchronously on the tokio thread (marked `ponytail` in the
    code), file reads and writes are wrapped in `blocking` only here and there. With a large account
    list or a slow disk this can hang IPC.
13. **A 2.0 MB frontend bundle (557 KB gzip) in one chunk**, with no code splitting — even though half
    the app (servers, performance, debug, discord) is rarely needed.
14. **The documentation drifted from the code.** `docs/DESIGN.md` still says "proposal, 2026-10-24,
    nothing is built yet" and describes a `QuikSwitch/` structure, `lib.rs`,
    `instances/singleton.rs`, `platform.rs`, `auth.rs`, `roblox_api/users.rs` — none of which exist. The
    README described 0.9.0 and linked to `inspectsoftware/RoALT` while the repository is
    `JohnBirlich/RAM`.
15. **Toolchain fragility.** `edition 2024`, `rust-version = "1.95"`, Tauri 2.12, reqwest 0.13,
    rusqlite 0.40, TS 7, Vite 8 — the build needs the freshest compilers; `stable` on CI will drift and
    the build may stop being reproducible.
16. **`panic = "abort"` in release** plus 790 `.unwrap()`/`.expect()` calls (many in tests, but not
    all). A panic hook writes to the log, but the app simply disappears.
17. **Coupling to the GPU and compositor.** Previews go through Windows.Graphics.Capture + D3D11; on
    virtual machines, RDP, multi-GPU setups and old drivers there can be degradation; many clients ×
    previews = noticeable load (there are preview FPS and "sleeping window" settings, but the default is
    15 FPS per card).
18. **Smaller surface**: `debug_log` lets the UI write arbitrary lines into the log (log injection);
    `background_set` and `accounts_import_ram` take an arbitrary path from the UI (with a compromised
    webview — file reads, bounded by image validation and a 32 MB limit); there is no attempt limit on
    `vault_unlock` (only Argon2 slows it down).

### P3 — taste and product

19. The privacy defaults are debatable: `discord_presence: true` (publishes "Playing RoALT" and the alt
    count into Discord) and `self_update: true` (pings GitHub on start). For a "local-only" position
    both should be off.
20. `allow_public_networks` defaults to `true` (a migration turns it on for everyone), so the "public
    network guard" is effectively an option. It used to be the other way round.
21. `lighter_rendering` and `low_graphics` are on by default — the app out of the box edits
    `ClientAppSettings.json` inside the Roblox installation and the accounts' graphics settings.
    Functionally justified, but "changes someone else's software out of the box" is not for everyone.
22. Localization is incomplete: the checker warns about strings left in English (pt, vi, sv, zh and
    others), and part of the brand is hardcoded.

## 6. Pitfalls in operation

- **Do not run on another Windows account or after moving the folder from another PC** — DPAPI will not
  decrypt; the app says so and suggests renaming the database and importing a `.lram` backup. That is
  not a bug, it is the design.
- **Network folders and OneDrive** for the data folder are a bad idea (DPAPI + WAL + file locks).
- **A disconnected drive** with a moved data folder means a refusal to start with a clear message
  (`location.json`) instead of an "empty new vault" — good, but worth knowing.
- **The Microsoft Store version of Roblox is not supported** (it cannot be started through
  `roblox-player:`).
- **Bloxstrap/Fishstrap**: partly supported — FFlags are not applied (the bootstrapper copies its own
  file) and the Roblox installer is not run over it.
- **If RoALT is killed**, there remain: job objects with CPU limits, a modified
  `ClientAppSettings.json`, "ghost" hidden windows and taken limits. Part of it is restored on the next
  start (`restore`, `sweep`, `recover`); job limits are only released on a normal exit.
- **A game's anti-cheat** may react to the synthetic input of anti-AFK; Roblox does not officially
  support several clients at once.
- **The launch ticket is visible in the Roblox process command line** to any process on the machine
  (that is how Roblox itself works). The app scrubs its own logs; the OS does not.

## 7. Is it safe to use

**Short answer: yes, with caveats, and "safe" is not "without consequences".**

What is protected well: data on disk (SQLCipher + envelope encryption), the network (TLS, a minimal set
of hosts, signed updates), IPC (hard isolation of the Roblox windows), logs and the clipboard.

What remains a risk and does not depend on the code:

1. **The legal/ToS risk.** Storing cookies of several accounts, multi-instance, auto sign-in and
   anti-AFK go against the spirit of Roblox's rules; account bans are possible and do happen with tools
   like this. No cryptography changes that.
2. **The "cookie = account" model.** One stolen cookie = the whole account, 2FA does not save you. The
   default DPAPI mode does not protect against malware under your account.
3. **Trust in the binary.** Install only from official releases (or build it yourself); the installer
   has no publisher signature yet.
4. **Privacy.** Discord rich presence and the GitHub update check are both on by default.

Advice for the user: turn on the master password plus a recovery code (kept offline), turn off
`discord_presence`, set `self_update` as you like, do not keep `.lram` backups in the cloud without a
strong password, do not run it on someone else's or a work machine, and do not use your "main" account
as the first in the list.

Advice for the author: sign the installer, add a `LICENSE`, turn off the debatable defaults, say plainly
in the README that the DPAPI mode does not protect against malware, remove or rework `take_over`, add
`rustfmt.toml` + `cargo fmt --check` to CI and at least smoke tests for the frontend.

**Done after the audit:** a limit on unlock attempts (`vault::Attempts`, 0.11.8) and protection of the
log against forged lines from the UI (`ui_log_text`, 0.11.8: newlines are replaced and the line is cut
at 4 KiB on a character boundary).

## 8. Design: what can be redone

The good news: the visual layer is isolated far better than average.

- **Tokens**: `src/theme.css` — `@theme inline` maps CSS variables to Tailwind utilities (`bg-panel`,
  `text-dim`, `border-line`, `text-accent`, …). In `:root` and `:root[data-theme="light"]` sit
  `--bg/--panel/--raised/--hover/--sel/--line/--text/--dim/--accent`, plus "depth": `--face`,
  `--raise`, `--press`, `--well`, `--primary`, `--shadow`. The "Soft Deck" theme is neomorphic: shadows
  in two directions, sharp corners, a lime accent.
- **The user's accent colour** is generated from one colour through `oklch()`/`color-mix()` with
  automatic contrast correction per theme — that is a ready-made customization mechanism (Settings →
  accent, 9 presets plus an eyedropper).
- **Density** (`Detailed`/`Compact`) and **tab layout** (top/side) are settings too.
- **Components**: `src/ui/*` (Dialog, Toast, controls, Resizable, TitleBar, Icon) — small, styled with
  classes from `theme.css` rather than inline styles.
- **What is not tokenized**: radii (hardcoded in `theme.css` — 16/18/20 px for panels, 10 px for
  controls, 99 px for pills), typography (Geist/Geist Mono, sizes in utilities), ~40 hex literals in
  `.tsx` (group swatches, splash art, the logo), and icons — inline SVG in `ui/Icon.tsx`.

**Conclusion:** changing the visual language (colours, shadows, radii, typography, "flat/glass/terminal")
takes an edit of `theme.css` + `ui/*` — 1–3 files for 80 % of the effect. Reworking the *layout*
(navigation, Home, Accounts) means `App.tsx`, `features/dashboard/*`, `features/accounts/*`.

**What must not be forgotten when rebranding** (or the fork either fails to build "as its own" or
updates itself into the original):

| Place | What is there |
|---|---|
| `package.json`, `src-tauri/Cargo.toml`, `tauri.conf.json` | product name, `identifier`, version |
| `self_update.rs` | `LATEST` (the repository), `PUBLIC_KEY` (the signing key — you need your own pair) |
| `rpc.rs` | the Discord `CLIENT_ID`, the Download/Invite links |
| `capabilities/main.json` | the opener allowlist |
| `installer/*.iss`, `.github/workflows/*` | publisher, AppId, winget id, paths |
| `src/i18n/locales/*.json` (18 files) | occurrences of the old name in English and in the translations |
| `DonateDialog.tsx` | the game-pass id of someone else's donation game |
| icons | `src-tauri/icons/*`, the logo in `ui/Icon.tsx`, the splash art |

---

*The audit was prepared by reading the code; final conclusions about the Rust part need `cargo test` and
`cargo clippy` on a Windows machine (CI does that on every push).*
