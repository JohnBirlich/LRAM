# Renaming to LRAM and cutting ties with the upstream project

The build is no longer called RoALT and does not go to the author's GitHub for updates, donations or
his Discord. This is what changed, what stayed as it was (and why), and what it takes to ship your
own build.

## What is called LRAM now

| Place | Was | Now |
|---|---|---|
| Product name (`package.json`, `tauri.conf.json`, window, page title) | RoALT | LRAM |
| App identifier | `com.inspectsoftware.roalt` | `com.lram.manager` |
| Installer publisher | inspectsoftware | JohnBirlich |
| Installer name | `RoALT-Setup-x.y.z.exe` | `LRAM-Setup-x.y.z.exe` |
| Program folder | `%LOCALAPPDATA%\Programs\RoALT` | `…\LRAM`, file `LRAM.exe` |
| Data folder | `%LOCALAPPDATA%\RoALT` | `%LOCALAPPDATA%\LRAM` |
| Log file | `roalt.log` | `lram.log` |
| Installer AppId | `{F65209C5-…}` | a new `{E647C13A-…}` (installs next to the original) |
| Interface and log text | "RoALT" | "LRAM" (English only) |
| Links in the interface, README, the `capabilities` allowlist | `inspectsoftware/RoALT` | `JohnBirlich/RAM` |

The "Support" button (a donation for the author's game passes) is gone from the sidebar, and so is the
`src/features/donate` component. The Discord button in rich presence is gone too.

## What is deliberately left old

These look like an unfinished rename, but they cannot change without losing data:

- **`b"RoALT/db/v1"` and `b"RoALT/vault/v1"`** — DPAPI entropy (`storage/cipher.rs`,
  `vault/dpapi.rs`). DPAPI mixes it into the key: changing the string means the existing database and
  vault can no longer be decrypted.
- **AADs `roalt.*`** (`roalt.cookie:`, `roalt.vault.key`, `roalt.vault.recovery`,
  `roalt.favourite`, `roalt.discord.webhook`) — same reason.
- **The crate name `roalt`** and the `roalt.exe` binary — visible to the developer only; the
  installer renames the file to `LRAM.exe`.
- **The backup magic `ROALTBK1`** — old backups must keep opening.
- **`ClientAppSettings.json.roalt-before`** — only to restore settings left by the previous build;
  the new one writes `…lram-before`.
- **`localStorage` keys `roalt.*`** — invisible in the interface, and renaming them would drop the
  saved Home layout and panel widths for everyone who already uses the app.

The database file name did change (`roalt.db` → `lram.db`): on start `adopt_legacy_names` moves
`roalt.db`, `roalt.db-wal`, `roalt.db-shm` and `roalt.db.key` to the new names when the new files do
not exist yet. Covered by the `old_database_names_are_taken_over_once` test.

So that data from the previous build is not lost, `default_dir()` takes `%LOCALAPPDATA%\RoALT` when
the `LRAM` folder does not exist yet and the old one holds a database. The data can be moved to the
new path with "Change data folder" in Settings (the move there is safe and checked).

Everything else is renamed: the crate and the binary are `lram` (`lram.exe`), backups and account
profiles are `.lram` with the magic `LRAMBK01` (old `ROALTBK1` files still open), performance
profiles are `lram-performance` (old ones import), temporary files and folders are `lram-*`.

## What was cleaned up in 0.12.1

Two places in the interface still printed the old file name, plus a couple of service names:

- "Backup" in Settings and the recovery-code text said `.roalt` — now `.lram` (the files open the
  same way, and the file dialog still accepts both extensions).
- The Performance section key and the `#perf-roalt` anchor were named after the old project — now
  `performance.section.lram` and `#perf-lram`.
- Test and debug environment variables — `ROALT_TEST_HWND`, `ROALT_TEST_CLIENT`, `ROALT_CDP_PORT` —
  became `LRAM_TEST_HWND`, `LRAM_TEST_CLIENT`, `LRAM_CDP_PORT`. `ROALT_DATA_DIR` stays: it is read as
  a fallback after `LRAM_DATA_DIR`, so old shortcuts and scripts keep working.

What is left is exactly the list above of things incompatible with a rename, plus the `localStorage`
layout keys.

## Updates

The built-in updater is **off**: `PUBLIC_KEY` in `self_update.rs` is empty, so the version check asks
for nothing, `spawn()` returns at once, and `self_update` defaults to `false`, with no switch in
Settings any more. New versions are installed by hand from GitHub Releases.

If it ever needs to come back:

1. `node scripts/update-key.mjs` prints a key pair: the public half into
   `const PUBLIC_KEY: &str = "…";` in `src-tauri/src/self_update.rs`, and the private half (PKCS#8
   PEM) into the repository's `LRAM_UPDATE_KEY` secret.
2. Bring back the Settings switch (`SelfUpdateRow` in `SettingsView.tsx`) and `self_update: true` in
   `models.rs`.
3. Ship releases through `.github/workflows/release.yml`: without the secret it builds the installer
   with no `.sig` (the signing step is skipped), with the secret it signs and attaches the signature.

`LATEST` in `self_update.rs` and `RELEASES`/`DOWNLOAD` in the interface already point at
`JohnBirlich/RAM`.

## What is done and what is not

- **Logo and icons** — our own: `src-tauri/icons/app-icon.svg` (vector) and
  `scripts/make-icons.py` (Pillow builds the png and icon.ico from the same description). The same
  mark is in `Logo` (`src/ui/Icon.tsx`), in the splash and in `docs/images/banner*.png`.
- **Screenshots** — taken from the old look and deleted, so as not to show something untrue. New ones
  are taken from the running app (`Win+Shift+S` or any screenshot tool) and put in `docs/images/`.
- **Discord rich presence** — off in 0.12.1 (`CLIENT_ID` was empty, so no build carried a Discord
  application id), and the whole pipe client was deleted in 0.12.3: `src-tauri/src/rpc.rs`, the
  `LRAM_DISCORD_APP_ID` wiring and the **Settings → Show on Discord** switch are gone. Only the
  webhook alerts in `discord.rs` remain.
- **SignPath signing** — not done: it needs an account and an approved project, which is not possible
  from the sandbox. The workflow is ready; see below.

## SignPath: what is needed from you

1. Sign up at https://signpath.io and apply for the free open-source licence (project `LRAM`,
   repository `JohnBirlich/RAM`).
2. In SignPath: organization → project `LRAM` → signing policy `release-signing` → artifact
   configuration for Inno Setup (`github-ci`).
3. In GitHub: the `SIGNPATH_API_TOKEN` secret and the `SIGNPATH_ORGANIZATION_ID` variable
   (Settings → Secrets and variables → Actions). Nothing else has to change: `release.yml` uploads
   the unsigned installer as an artifact, sends it to be signed, takes the signed one back and only
   then signs it with the self-update key.

## Shipping your own build

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-installer.ps1
```

The version comes from `tauri.conf.json` and must match `package.json` and `src-tauri/Cargo.toml`
(all `0.12.1` at the moment). The installer lands in `dist-installer\LRAM-Setup-0.12.1.exe`. The tag
`v0.12.1` runs the workflow that builds a draft release.
