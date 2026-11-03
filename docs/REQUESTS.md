# User requests: what is actually worth doing

Complaints and suggestions from the chat, against the code as of 0.11.0. For each item: the verdict,
why, what exactly to change and what it costs. Size: S — an evening, M — a few evenings, L — a feature
for a week or more.

| # | Request | Verdict | Size |
|---|---------|---------|------|
| 1 | "An account shows as disconnected / closed while the client is right there" | Yes, a bug, fixable | M |
| 2 | "Windows didn't let LRAM bring that client to the front", though it did | Yes, a false error | S |
| 3 | Launching clients at volume 0 | Done in 0.11.5 | M |
| 4 | Detecting the state inside a game (Tapping Simulator and the like) | No | — |
| 5a | Window size after F11 and after minimizing | Done in 0.11.6 | M |
| 5b | Laying windows out in a grid from the top-left corner | Almost there | S |
| 6 | AFK that does not break a treadmill | Done in 0.11.4 | S–M |
| 7 | The first launch says "failed" though the client runs | Partly, by the same fix as #1 | M |
| 8 | "Make the quality worse so it doesn't lag" | Almost everything is there | S |
| 9 | 403 = send the user to check the account on the site | Already detected, needs words | S |
| 10 | Discord presence does not work | The build carries no app id | S |
| 11 | An auto-clicker or macros configured in the app | Debatable: only with an explicit risk notice | L |
| 12 | Grouping accounts on screen | Done in 0.11.7 | S |
| 13 | Adding accounts right in the app | Already there | — |
| 14 | Accepting and sending friend requests from the app | Yes, done in 0.11.3 | M |

## 1. False "Disconnected" and "Closed unexpectedly"

How it works now:

- The state comes from the client's log (`src-tauri/src/instances/logwatch.rs`, `parse_line`) and from
  process liveness (`tracker.rs`, `observe` → `settle`).
- The "disconnect" list includes the line `Time to disconnect replication data` (`DISCONNECTS`). It is
  written on a teleport and on leaving a game, and the `disconnected` flag is cleared only by the next
  `! Joining game` line. If Roblox reconnects without writing a new join line, the "Disconnected"
  badge stays forever.
- "Exited" is set when the process is gone and nothing suggests it is coming back
  (`!alive && !may_appear && !written`). But an entry that became `Exited` is never polled again
  (`poll` takes only non-`Exited` ones), so one failed process check is irreversible. There is a
  guard (`process::unreadable_client`), but it needs the process to be visible in the list under
  `RobloxPlayerBeta.exe`.
- A second source of confusion with several clients: a closed instance and a live client of the same
  account are two different entries; the history shows "Closed unexpectedly" while the account is
  playing.

What to do (both possible and worth it):

1. Confirmations before `Exited`: 2–3 empty polls in a row (about 3 s) plus a window re-check.
2. "Revival": if the entry is already `Exited` but a process with its `browserTrackerId` is alive (its
   log grows), bring it back instead of calling it closed.
3. Clear `disconnected` when the log shows signs of play after the disconnect line (`serverId:`,
   `Replicator created`) or keeps growing with a live window for more than ~30 s.
4. Show the reason from the log in the tooltip (`facts.reason` is already collected), so "Disconnected"
   is not mute.

`tracker.rs` has the test harness with fake processes and logs — all four points are covered by unit
tests without Windows.

## 2. "Windows didn't let LRAM bring that client to the front" while it did

`instances/afk.rs::to_front` checks `GetForegroundWindow() == hwnd` right after `SetForegroundWindow`
and a mouse nudge. Window activation is asynchronous, so the check sees the old window and reports an
error although the client is already in front 5–20 ms later. The second case is `SendInput` blocked by
UIPI when the client runs with other rights.

What to do: poll `GetForegroundWindow()` for up to ~300 ms after the attempt; if the window is visible
and not minimized while another one is in front, that is a log line, not an error. About 30 lines.

## 3. Volume 0 for a client (done in 0.11.5)

Done through Windows Core Audio (`instances/audio.rs`): a thread with `CoInitializeEx`, enumerating
audio sessions (`IAudioSessionManager2` → `IAudioSessionControl2::GetProcessId`) and `SetMute` on
`ISimpleAudioVolume` for the right pid.

- `Settings → Mute clients`: new clients start muted, and the switch also changes the ones already
  running.
- One client is switched separately: Home → Limits → Sound, or the client's menu in the dashboard.
- Every 5 seconds the thread re-checks the muted clients: Roblox creates its session a few seconds
  after the launch and may recreate it (a reconnect, a graphics change). Unmuting is applied once, so
  it does not fight the Windows mixer.

Limits: this mutes the process, not the in-game volume; checked by hand on a live client.

## 4. Detecting the state inside games

No. LRAM deliberately does not read client memory and does not inject anything into it
(`docs/DESIGN.md`); Hyperion (Byfron) flags even a long-held process handle. "Is it hatching or not"
is data of a particular game's scripts, available only from its memory.

What can be done instead is what already exists or is cheap: the place (`placeId`), its name (a name
cache), `jobId`, the server, time in game. A useful addition is an "N minutes in this game" timer and a
notice when the place changes.

## 5. Windows

**a) Size after a fullscreen (done in 0.11.6).** LRAM does not touch the fullscreen mode, so after F11
Roblox leaves it with its own previous layout, not the size it had. Now `window::sweep` remembers the
last "normal" client-area size and, when the window leaves fullscreen with a different size, puts the
old one back (`note_size`/`restore_size`). Fullscreen is recognized as "no caption" + "client area
covers the whole monitor", so a maximized or tiled window is not mistaken for it. Guard: if the user
dragged the window himself (the `EVENT_SYSTEM_MOVESIZEEND` hook), his size wins; "tiny" clients have
their own size and `keep_tiny` answers for them. Check by hand on a live client.

**b) Laying out in a grid.** Already there as a button: `instances_tile` → `instances/window.rs::tile`
(from the top-left corner, "tiny" included). What is left is a "lay out on launch" setting and calling
`tile` after a successful launch or entry into a game — about 40 lines.

## 6. AFK that does not break a treadmill (done in 0.11.4)

It was: anti-AFK presses **Space** (a jump) in the Focus mode and **Tab** in the Background mode
(`instances/afk.rs`), and the jump breaks the treadmill.

It is: the `Settings → Anti-AFK → What it does` setting (Focus mode only; Background keeps Tab, since
messages sent to a window that is not in front may not count as activity for Roblox):

- **Jump** — Space, as before and by default;
- **Camera** — the right button for 3 pixels and back: the camera turns slightly, the character neither
  jumps nor walks;
- **Mouse only** — the cursor goes into the middle of the client and back: no buttons, no camera,
  nothing in the game changes at all.

The cursor and focus go back after every press. Everything is still `SendInput` only, and only when the
user has not touched the keyboard or mouse for 5 seconds.

## 7. The first launch says "failed" though the client runs

In a batch launch "failed" is set by the frontend from the instance state (`src/features/launch/Batch.tsx`:
a sign-in problem, a disconnect, a close). The first launch is the longest: Roblox downloads or updates
the client, the log appears later, and the `REBIND_MS` window (120 s) may not be enough — the instance
goes to `Exited` and gets "failed".

What to do: (1) give the first launch after an install or update more time and do not count the client
as closed while the Roblox installer runs; (2) before showing "failed", re-check whether a live client of
that account has appeared; (3) show the real reason in the toast, not a generic text. Part of it is
closed by fix #1. To stop guessing, a log (`lram.log` plus the autopsy report) is needed from whoever
sees it repeat.

## 8. "Worse quality so it doesn't lag"

Already there: `fpsCap`, `lowGraphics`, `lighterRendering` with the `renderFlags` set (MSAA x1, lowest
textures, no grass, lowest FRM, voxelizer paused), tiny windows, `mem_restart_mb` and `mem_throttle`,
`autoSpread`, `fairCap`, the adaptive limiter.

What is missing: a one-button "weak PC" preset (fps 30 + low graphics + lighter rendering + all flags +
tiny on launch) and short explanations next to the options. About 50 lines in Settings.

## 9. 403 — send the user to check the account

403 is already recognized (`logwatch::AUTH_FAILED`), shown as the note "Roblox refused the sign-in
(error 403)" and slows batches down (`bulk.refused()`). Only the wording is missing: open roblox.com with
this account, pass the "I'm not a robot" check and try again. A change in `src/i18n/locales/*.json`.

## 10. Discord presence does not work

The reason is in the code, not on the user's side. `src-tauri/src/rpc.rs` takes the application id from
`option_env!("LRAM_DISCORD_APP_ID")`, and the release build never gets that variable (`.github/workflows/release.yml`
does not set it). So `spawn()` logs `discord presence is off: this build carries no Discord application
id` and returns: the switch in Settings exists but does nothing.

To make it work:

1. Create an application in the Discord Developer Portal and note its Application ID.
2. Upload a picture named `lram` under Rich Presence → Art Assets (the activity uses
   `large_image: "lram"`), or there will be no icon.
3. Pass the ID into the build: the `LRAM_DISCORD_APP_ID` repository variable and `$env:LRAM_DISCORD_APP_ID`
   before the build in `release.yml` (locally this is described in `docs/REBRAND.md`).
4. Show in the interface that the build carries no ID: today the switch silently lies.

Not to be confused with webhook alerts (`discord.rs`): they work independently of presence.

## Requests from the second batch

### 14. Friend requests from the app (done in 0.11.3)

A new "Friends & requests" window in the account menu: incoming requests with Accept/Decline and a field
to send a request by username. Everything goes through the same cookie and the same CSRF dance as on the
site: `GET friends.roblox.com/v1/users/{id}/friends/requests`,
`POST .../users/{targetUserId}/request-friendship` with `{"friendshipOriginSourceType":0}`,
`POST .../users/{requesterUserId}/accept-friend-request` and `.../decline-friend-request`. If Roblox asks
for a captcha (`isCaptchaRequired`), the app does not stay silent: it says a request has to be sent once
on the site and retried. Nothing is automated: every request is a human action.

### 13. Adding accounts in the app

Already there and working: signing in through the Roblox window (the cookie is captured), pasting one or
several cookies, importing RAM's `AccountData.json`. The only thing missing is a bulk `login:password`
import: Roblox does not hand out a cookie for a password, so every line still needs a sign-in window or a
captcha. The item stays as it is.

### 12. Accounts by group (done in 0.11.7)

Groups exist since 0.10: coloured chips, a filter by group, adding many to a group, a group colour. Now
what was missing compared to RAM is added:

- **Sorting by the numeric prefix** (`model::group_order`): `1Main` goes before `007 Bank`, then by the
  number and the rest of the name; groups without a number keep the creation order. The same order is
  used in the interface (`byGroupOrder` in `data.ts`) and inside the account row.
- **Groups in the dashboard**: a running client's card shows its group chips (in the compact mode,
  coloured dots with a tooltip), so the account's group is visible both in Accounts and in the grid of
  running clients.

Group chips on a client card do not take width from the picture and wrap when there are many.

### 11. Auto-clickers and macros

There is nothing like that in the code. Technically it is done in `src-tauri/src/instances/` through
`SendInput` into the client window (like `afk.rs`), with a configurable action: a click at a point, a key,
an interval, start/stop from the account menu, off by default. But that is exactly the class of software
Roblox bans for: any input automation in the client breaks the rules, and the app cannot hide it. So it
is only worth taking if the author accepts a large notice in the interface ("this interferes with the
client, the ban risk is yours"), and it is never presented as a safe feature. Not done.

## Batch 4 — complaints after 0.11.8 (0.11.9)

| # | Complaint | What was done |
|---|-----------|---------------|
| 15 | Account statuses update slowly | `accounts_validate` emits `accounts://changed` after every account: the list fills row by row instead of all at the end. On the first `RateLimited` the pass stops instead of waiting out the same window on every next account |
| 16 | With Roblox already running, "it doesn't see the account" / "Closed unexpectedly" | A client that already showed its window gets two silent polls (`DEAD_POLLS`): one failed process or window check no longer ends it. Every such ending is now logged with evidence (`window work done`, `polls without it`, …) |
| 17 | Frequent 429 on launch | The ticket is requested at most once per 2 s (`ticket_gap`): simultaneous launches queue up instead of firing together |
| 18 | Some windows minimized, some full screen; the size does not change | The grid is laid out for clients that are hidden later too (`arranges` no longer depends on `minimize_on_launch`): hiding is the last step, otherwise the client falls out of the grid. `free_resize` checks that the window took the size; if not, it drops maximization and asks again, and if that fails too, it logs a warning |

## Batch 5 — complaints after 0.11.9 (0.11.10)

| # | Complaint | What was done |
|---|-----------|---------------|
| 19 | Client windows overlap each other | `grid` no longer restarts the layout from the first cell when there are more windows than fit the chosen size: such windows get cells fitted to the screen instead of overlapping the first ones |
| 20 | LRAM says "8 in game" while 10 clients run; some accounts "were not restarted" | A client whose tracker-id line appears late (Roblox sits in the tray first) is no longer counted closed after 120 s: if the client's log appeared after the launch, the entry waits up to 10 minutes. The log is searched at both the start and the end of the file, so a late `websiteBTId` line also binds the client |
| 22 | How to make windows eat almost no CPU | `2 %` and `1 %` were added to the CPU cap scale (the client menu on Home, the limits dialog, "Background CPU limit" in Performance). Below them the interface says plainly that under 5 % a client starts to lag and may be dropped from the game, and that the cap is a hard Windows limit, not a wish |
| 21 | The percentage in the interface does not match Task Manager | It is being pinned down which number is meant: the dials on Home count only LRAM clients (and only tracked ones), while the "Working flat out" caption is the whole PC load. Part of the difference goes away with #20 |

| 23 | After four windows the rest open full screen | `tile` laid out only the windows visible at that moment. Clients hidden after getting into a game fell out of the layout, and the next client ended up alone in the grid — that is, full screen. Now a cell is computed for every tracked client, hidden ones get theirs and return to it when shown |

## Batch 6 — 0.12.0

| # | Request | What was done |
|---|---------|---------------|
| 24 | A bug report in one click, not buried in Debug | The report is available from Home: "Save a report…" in the "⋮" menu next to "Tile windows", and a button on the card of a stuck or crashed client. A `Clients` section was added to the report: how many clients run, and for each — its state, pid, uptime, relaunch count, grid cell, window size and position (visible, hidden, minimized, not responding) and the limits in force. This answers "why is the window full screen" and "why is the client not counted as running". Redaction of cookies, names, IPs and paths is untouched |
| 25 | A watchdog for "hung / sitting in the tray" and a readable restart policy | A client that is alive but has not reached a game in 15 minutes (`STUCK_MS`) now counts as stuck: the grid labels it "Stuck before reaching a game", and with auto-restart on it is relaunched the same way a hung one is. Such a client is relaunched at most 3 times (`STUCK_TRIES`), otherwise a client that will never load would be relaunched every 15 minutes forever; after the third it gets the note "Gave up: never reached a game" and the "Try now" button. The breaker threshold went from 3 to 5 relaunches per 10 minutes: the typical case is "Roblox dropped all three", not a crash loop. The whole policy is in `docs/RESTARTS.md` |
| 26 | Lowering process priorities | The adaptive manager is on by default and with no CPU cap: a client that is not in the front window gets efficiency mode (EcoQoS + `BELOW_NORMAL_PRIORITY_CLASS` + very low memory priority). The client in the front window, one still loading and an account with "Full quality" are left alone. In Performance it is still configurable and can be turned off entirely, and the 25/50 % cap stays in the presets. Settings nobody touched (the old default: off, 25 % cap) are moved to the new one on the first start of 0.12.0 |
| 27 | Importing and exporting Roblox account profiles | The account list gained "Export profiles…" for the selection and "Import profiles…" next to "Add account". Export puts the chosen accounts — cookie, names, notes, colour, groups and launch settings — into one `.lram` file under a password you choose; the master password is asked separately, because the file carries cookies. Import opens such a file (or a plain backup) with a "keep existing / replace" choice and reports how many accounts were added, updated, skipped and failed the check. A profile from one PC opens on another; a single account can be selected too |
| 28 | Traces of the previous name in the app | Settings and the recovery-code text said `.roalt` instead of `.lram`; the Performance section key and anchor were named after the old project; so were the test environment variables. All renamed; the list of what cannot change (DPAPI entropy, AADs, the old backup magic, the names used to move old files) is in `docs/REBRAND.md` |

## Work order

1. **Batch 1 (cheap, no risk):** #2, #9, #8, #5b, step 4 of #10.
2. **Batch 2 (state bugs):** #1 and #7 — points 1–3 of #1 first, since they also remove most of the
   "failed" on the first launch.
3. **Batch 3 (features):** #6 — 0.11.4, #3 — 0.11.5, #5a — 0.11.6. The batch is closed. Outside the
   batches: #12 (group sorting by number and groups in the dashboard) — 0.11.7.
4. **#4 is not taken**; at most a timer in game and a notice when the place changes.
5. **#8 is rejected** by the author: the fine-grained settings already exist, presets are not needed.

Done: #2, #9, #5b — in 0.11.1; #1 (false "Disconnected" and "Closed unexpectedly") and the part of #7
that depends on the first launch — in 0.11.2. Details below.

What changed in #1: a client whose process cannot be read but that has its own window behind it (a hidden
one included) is no longer counted closed; the disconnect line in the log stops applying as soon as the
log shows replication restored. In #7: while the Roblox installer runs, the entry waits for the client up
to 15 minutes instead of 120 seconds, so a first launch with a download no longer turns into "failed".
