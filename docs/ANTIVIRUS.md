# Antivirus detections and code signing

A handful of engines on VirusTotal flag the installer. This page says which ones, why, and what
actually removes the flags.

## What the flags are

| Engine | Verdict | What it really is |
|---|---|---|
| Arctic Wolf | `WolfUnsafe` | A static machine-learning verdict on an unsigned binary that starts another process. Not a signature: no malware family, no behaviour. |
| DeepInstinct | `MALICIOUS` | A deep-learning model; it scores unsigned, compressed installers low on reputation alone. |
| SecureAge | `Malicious` | The same kind of reputation model; it is one of the loudest engines on small open-source installers. |
| Skyhigh (SWG) | `BehavesLike.Win32.ObfuscatedPoly.vc` | A heuristic for packed or obfuscated code. An Inno Setup installer with `lzma2/max` and solid compression is one high-entropy blob, which looks exactly like a packer's stub. |

Every one of them is an unsigned-binary or entropy heuristic, not a detection of a known threat. A
typical scan shows 3-6 of about 70 engines, all of them the same names, on every release.

## Windows Defender: `Behavior:Win32/DefenseEvasion.A!ml`

This one is different from the table above. `Behavior:` means Defender judged what the program did,
`!ml` means a cloud machine-learning model made the guess — there is no signature and no family
name. Two consequences matter:

- The verdict is pinned to the exact build. Once a file is convicted, that file stays convicted
  (it is blocked even when it is not running, with `ERROR_VIRUS_INFECTED` / "virus or potentially
  unwanted software"), while a rebuilt file has a different hash and is judged again from scratch.
- Rebuilding, renaming or repacking does not clear the old hash. Nothing clears a hash except
  Microsoft.

Why an unsigned account manager looks like this at all: it reads and DPAPI-decrypts another
application's cookie store, starts and stops other processes, moves and resizes their windows, and
installs a low-level mouse hook for free resizing. Each of those is ordinary for this program and
ordinary for an infostealer, and the model cannot tell the difference without a publisher name and a
reputation.

The one piece that was genuinely tampering-shaped is gone: with Roblox already running, LRAM used to
read the system's whole handle table, open the Roblox process with `PROCESS_DUP_HANDLE` and close
`ROBLOX_singletonEvent` inside it. That is handle manipulation inside another process, and it also
risked a ban under Hyperion, so it was removed rather than kept. Multi-instance now only holds the
name before Roblox starts; a client that is already running has to be closed first. LRAM no longer
touches another process's handles or memory: the only cross-process handles left are read-only ones
for metrics and the job object for the CPU cap.

To clear it:

1. Windows Security → **Virus & threat protection → Protection history**, open the entry →
   **Actions → Restore**. If the file was already deleted, install the current release again.
2. If you are the one building it, rebuild — the new hash is judged on its own.
3. Report it to Microsoft as the developer: <https://www.microsoft.com/en-us/wdsi/filesubmission>,
   choose **Software developer**, attach the file, name the detection
   (`Behavior:Win32/DefenseEvasion.A!ml`), link the repository and say what the app does and that
   the handle-taking code was removed. This is the only thing that clears a hash, and it clears it
   for everyone, usually within a day or two.
4. On your own machine, if you trust the build: **Windows Security → Virus & threat protection →
   Manage settings → Exclusions → Add an exclusion → File** for `LRAM.exe`. Excluding it globally is
   not something to recommend to other users, and it is not needed once Microsoft clears the hash
   or the build is signed.

Signing matters more here than for the static engines: the behaviour model weighs publisher
reputation heavily, and a signed binary from a known publisher stops looking like an unknown that
decrypts cookies and kills processes.

## What removes them

1. **Code signing with a trusted certificate.** This is the only thing that removes them at the
   root: the file gets a publisher, SmartScreen gets reputation, and the reputation models stop
   treating the binary as an unknown object. Signing the inner `LRAM.exe` matters as much as signing
   the installer around it.
2. **Reputation over time.** Downloads, an unchanged Publisher name, and the same hashes published in
   `SHA256SUMS.txt` move the models; this is slow and needs signing first.
3. **A false-positive report to each vendor.** They do read these and usually clear the file within
   a few days. Details and a template below.

What does **not** help: rebuilding, renaming the file, changing the icon, uploading it again, or
wrapping it in another archive. A different packer can even make it worse.

Behaviour verdicts (the Defender one below) are a separate matter: they need the behaviour gone, not
a signature.

## Where this project stands

- The release workflow already has the signing steps wired (`signpath/github-action-submit-signing-request`
  in `.github/workflows/release.yml`); they are skipped while `SIGNPATH_API_TOKEN` is not set.
- Signing needs a certificate. Three routes:

| Route | Cost | Notes |
|---|---|---|
| [SignPath Foundation](https://signpath.org/apply) | Free for open source | Needs an OSI-approved licence for the whole project, existing releases, CI-built binaries, 2FA on the accounts, and a published code-signing policy. Publisher shown as "SignPath Foundation". Approval depends on public visibility. |
| [Certum Open Source](https://shop.certum.eu/open-source-code-signing-on-simplysign.html) | about €25-49 | Cheapest paid route, requires an OSI licence too; reputation builds over the first releases. |
| [Azure Trusted Signing](https://azure.microsoft.com/pricing/details/artifact-signing/) | about $10/month | Cloud signing from CI, own organisation name, US/Canada only. |

- **Blocker today:** the repository has no `LICENSE` file, so the two free-or-cheap open-source
  routes do not qualify yet. Adding one (MIT, or whatever the fork's upstream allows) is the first
  step.
- Signing is not instant reputation: SmartScreen keeps warning until the certificate has seen a few
  downloads. The vendor models usually stop flagging the file at the first signed release.

## Reporting a false positive

Send one report per engine, with the same evidence:

- Arctic Wolf: <https://arcticwolf.com/company/contact-us/>
- DeepInstinct: <https://www.deepinstinct.com/contact-us>
- SecureAge: <https://www.secureage.com/support/report-false-positive>
- Skyhigh (SWG) / Trellix: through the support portal of the product that flagged it

Template:

```text
Subject: False positive — LRAM <version> installer (open-source Roblox account manager)

File:     LRAM-Setup-<version>.exe
SHA-256:  <hash from SHA256SUMS.txt>
Scan:     <VirusTotal permalink>
Download: https://github.com/JohnBirlich/RAM/releases/tag/v<version>

LRAM is an open-source Windows app (Tauri 2 + React) that manages Roblox accounts on the
user's own PC. The installer is built from the public source by GitHub Actions on windows-latest
and is unsigned, which is why it scores low on reputation. It does not download or run anything
hidden: the only network hosts are *.roblox.com, *.rbxcdn.com and GitHub (update check).

Please re-check the file and remove the detection, or tell me what in it looks malicious.
```

## What a user can check

- Compare the file with `SHA256SUMS.txt` from the same release: `Get-FileHash .\LRAM-Setup-x.y.z.exe -Algorithm SHA256`.
- The build is reproducible from the public source by `.github/workflows/release.yml`; the installer
  script is `installer/LRAM.iss`.
- The scans happen because the binary is new to the engines, not because of what it does: nothing in
  LRAM reads or writes the Roblox client's memory, and the only files it touches are named in the
  README.
