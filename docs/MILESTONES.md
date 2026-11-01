# RoALT milestones

What each milestone built, how to test it, and where Roblox behaved differently from the brief.
Design: [DESIGN.md](DESIGN.md). Every change gets its own setup in `installers/` (gitignored).

## M1 — vault, storage, accounts (v0.1.0)

**Built:** Tauri 2 app with the approved sharp dark/light UI; envelope vault (Windows DPAPI by
default, optional Argon2id + AES-256-GCM master password with auto-lock); SQLite storage; add
accounts through Roblox's own sign-in page in a throw-away WebView2 profile or by pasting a
cookie (validated first); validation on start and on demand; groups, alias, notes, colour,
favourite, drag order, search and filters; reveal/copy behind Windows Hello or the master
password (copy kept out of clipboard history, cleared after 30 s); encrypted `.lram` backups;
Inno Setup installer, CI, and a tag → draft GitHub release workflow.

**Test:** add an account both ways, validate, reveal/copy, set a master password and lock,
export and re-import a backup. Hand-tested by the user: all worked.

**Roblox differences:** CSRF comes from the real request's 403 (the logout trick ends the
session); `.ROBLOSECURITY` rotates via `Set-Cookie`, so rotated values are stored; hardware-bound
sessions exist but are not enforced for cookie use (staff, 2026-05).

## M2 — account info panel

**Built:** avatars (cached 6 h); presence column (each on-screen account asks about itself,
once a minute, only while visible); info panel with Robux, Plus / Premium (legacy), friend /
follower / following counts, join date and age, email verified, 2-step methods, presence with
game, job ID and "Open game page"; "Open on roblox.com" (signed-in throw-away window; the cookie
it ends with is kept only if it still signs in as the same user); info cache (Settings → Data).

**Test:** select an account: panel fills; Refresh; Open on roblox.com shows the account signed in;
close it; an expired account shows "Cookie expired" instead of a half-empty panel.

**Roblox differences:** Premium no longer sells (Roblox Plus via `user-profile-api`);
`/presence/last-online` is gone; friends-only presence is hidden from other viewers, so each
account checks itself; public endpoints answer anonymously with a dead cookie, so Robux
(cookie-only) is the dead-cookie probe; the friends list endpoint returns empty names.

## M3 — single launch

**Built:** Launch view: paste anything (game link or place ID, `roblox://` link, job ID with
place, classic private server link, `roblox.com/share?code=…` share link incl. v2, universe,
profile link, `@username`) and the type is detected locally; Join someone (friends online, then
your other accounts, each checked with its own cookie); favourites (name + target, encrypted at
rest, sidebar list, rename / reorder / delete, used-last time); account picker (valid cookies
only); Ctrl+Enter. The launch itself works like the website: client-assertion, then
authentication-ticket with cookie + CSRF + Referer, then a `roblox-player:` URI with a fresh
browser tracker id, started through the installed client (or the registered bootstrapper).
A dead or challenged cookie found during launch is recorded on the account.

**Test:** pick an account, paste a game link, Launch: Roblox opens in that game. Paste a job
ID, enter the place ID: joins that server. Paste a private server link or a Server share link:
joins the private server (the account must have access). Join someone → a friend in a game:
joins their server. Save as favourite, click it in the sidebar, Launch again.

**Roblox differences:** the web client sends a `client-assertion` body with the ticket request
(sent here too; an empty body still works); `launchexp:InApp` and no `channel` parameter;
share links resolve per account through `sharelinks/v1/resolve-link` and only Valid invites
join their exact server (Expired ones still open the game, with a warning); launch data over
200 bytes is refused by Roblox, so it is dropped with a warning; usernames resolve through
`users/v1/usernames/users` (banned users excluded).

## M4 — multi-instance and process tracking

**Built:** a multi-instance switch (sidebar and Settings → Launching). When on, RoALT holds a
mutex named exactly like the event Roblox's client creates, so a new client can't tell the old
one to quit. If Roblox is already running unprotected, the switch says so and offers "Close
Roblox". The first time it is turned on, Roblox's stance on multiple clients is shown once.
While RoALT's clients run (and while one is launching), Roblox's shared sign-in files are held
locked so clients can't overwrite each other's session; they are released when the last client
ends. The Launch view lists the clients RoALT started: account, state (starting, joining, in
game, disconnected, ended, with problems such as a human check or a wrong account shown), job
ID and uptime; close one (graceful, then forced after 5 s), close all, clear ended. Clients are
tied to accounts through their own log file (by the launch's browser tracker id), followed
through Roblox's updater relaunch and through bootstrappers such as Bloxstrap.

**Test:** turn multi-instance on with Roblox closed; launch two accounts: both clients stay
open, each row shows its account in game. Close one from its row. Turn protection off and on in
Settings. Start Roblox normally first, then turn multi-instance on: the sidebar explains and
"Close Roblox" fixes it.

**Roblox differences:** the lock is now `ROBLOX_singletonEvent` (an event in client 0.744), not
the old `ROBLOX_singletonMutex`; a mutex with that exact name blocks it. Roblox's tray process can
already hold the name, so it counts as running. Logs are the only reliable source of the job id
and of which account a client signed in as; presence lags.

## M5 — bulk launch and same server

**Built:** the Launch view picks several accounts (checkboxes). Two or more launch as a batch:
strictly one after another, each waiting until the previous client's log is bound, then the
stagger (Settings → Launching, default 6 s, ±1 s). "Same server" puts everyone in one server:
"Pick a server" chooses one with room for all of them from the server list (Recommended for the
first account, else the fullest that fits) before anyone starts; "Follow the first" reads the
first client's server from its log (its own presence as a fallback, only when it shows that
game). The Batch panel shows each account's steps (ticket, run, join, in game), the count in
game, a Cancel (after the current step), and why a batch stopped: a human check, a wrong account
in a client, a locked vault or multi-instance turned off stop it; one dead cookie fails only its
account. After a stop, one click picks the accounts that didn't launch. Single launches wait
while a batch runs.

**Test:** turn on multi-instance, pick three accounts, paste a game link, turn on Same server,
Launch 3 accounts: all three end up in the same job ID. Cancel a batch midway: the rest show
cancelled. Try Follow the first.

**Roblox differences:** the server list (v2, `orderBy`) is the website's; Recommended and Best
ping need a signed-in account, and a guest gets 3 list calls a minute. Picking the server before
launching avoids detecting it afterwards; presence alone is too slow to follow reliably.

## M6 — server browser

**Built:** a Servers view. Paste a game link, place ID, game ID link or share link (share links
open as the first picked account); the header shows the game's icon, name, creator, place and
universe IDs and how many play. Public servers: Most full, Most empty, Recommended and Best
ping (the last two only with an account picked), Hide full, 25 per page with Load next
(duplicates across pages removed), Refresh, copy a job ID. Private servers the first picked
account can see (joinable ones carry an access code that is never shown). Pick a server, choose
the accounts in the bottom bar, Join: one account launches directly, several as a batch. It warns
when the server lacks room and refuses accounts Roblox restricts.

**Test:** load a popular game, sort and page through servers, pick one and join with two
accounts: both land in that job ID. Open the Private tab with an account that owns or was
invited to a private server.

**Roblox differences:** the guest flag for selling private servers is false for games that
still have them, so the Private tab is always offered and the account's own list decides; game
info, icons and the place → universe lookup work without a cookie; icons are only accepted from
https *.rbxcdn.com.

## M7 — quality of life

**Built:** auto-rejoin (Settings → Launching, and next to Same server in Launch): a client Roblox
disconnects (kicked, server shut down, connection lost) is closed after the launch delay and its
account relaunched into the same target, within the crash breaker (since Batch 1; first it was 3
tries in a row); leaving on purpose (reason 285) or a duplicate-login kick (273) is not rejoined, and in
master-password mode the vault stays unlocked while auto-rejoin has clients to watch. Tray icon:
show, favourites (launch with their stored accounts; several as a batch), close all clients,
lock vault, quit; optional close to tray. Settings also show the Roblox client RoALT will start
and the data folder, which can be opened or moved (copied, RoALT restarts in the new place, the
old database is deleted after the restart). Shortcuts Ctrl+1..4 switch views. Backend notices
(rejoins, tray launches) show as toasts; errors stay until dismissed.

**Test:** turn on auto-rejoin, launch an account, get it kicked (idle 20 min or a server
shutdown): its client closes and comes back in the same game. Leave a game normally: no rejoin.
Turn on Close to tray, close the window, use the tray menu to show it and launch a favourite.
Move the data folder to another drive: RoALT restarts and the accounts are still there.

**Roblox differences:** closing Roblox's window leaves no trace in its log, so plain exits can't
be told from crashes and aren't rejoined; a kicked client stays open on its error dialog, which
is why rejoin closes it first.

## Batch 1 — crash handling and launch control

**Built:** auto-restart (Settings → Launching, off by default): a client that ends without RoALT
closing it, or whose window stops responding for 30 s, is closed and relaunched into the same
target. A crash breaker replaces the old 3-tries rule for rejoins and restarts alike: at most N
relaunches of one account in M minutes (default 3 in 10), each after a growing backoff, then
"Gave up after N" and an error toast. Per-account "Allow disconnect" (detail panel and right-click
menu): never rejoined or restarted, and a batch counts its disconnect as done. Ended clients that
crashed, hung, were disconnected or restarted for memory are kept (newest 200) with uptime and the
log's reason; the Running list expands them and opens the log. Every launch waits out the launch
delay since the previous one. Custom launch arguments (Settings default, per-account override),
checked before use and never logged. Kill all: a hold-to-fire button in the sidebar and a tray
item end every Roblox process and stop the batch and pending rejoins. Close grace is a setting.

**Test:** turn on auto-restart, launch an account, end its process in Task Manager: it comes back
in the same game. Do it 4 times within 10 minutes: the fourth gives up. Mark an account Allow
disconnect, get it kicked: it stays closed. Set `--foo` as launch arguments, then `a & b`: refused.
Hold Kill all: every client ends and nothing relaunches.

**Roblox differences:** a window closed by hand still looks exactly like a crash in the log, which
is why auto-restart is opt-in; a hung client's window answers `IsHungAppWindow` without opening
the process.

## Batch 2 — metrics and performance

**Built:** a Monitor tab (Ctrl+5): CPU, memory, disk/network IO, uptime, state and restarts per
client with sparklines and totals, refreshed every 2 s. Memory restart (off by default): a client
over the limit for 3 samples 10 s apart is restarted, counted by the breaker. Per-client limits
gain a working-set trim (every N minutes, or Trim now) and CPU core checkboxes; Settings →
Performance adds a default trim, auto-spread of cores, the adaptive manager (clients not in front
get a background CPU cap and efficiency mode; manual limits win) and the bandwidth limit (all
Roblox clients together, outbound only, applied through an admin prompt, cleared by a restart).

**Test:** launch two clients, open Monitor: both show CPU and memory. Trim now: working set drops.
Untick cores: Task Manager's affinity matches. Turn on the adaptive manager, switch between the
clients: the one in front loses its cap. Set 1000 kbps and Apply (UAC prompt), then Remove.

**Roblox differences:** Roblox's working set grows back as used after a trim; private bytes, not
the working set, is what a leak grows, so the memory limit uses that.

## Batch 3 — anti-AFK and Discord alerts

**Built:** anti-AFK (Settings → off by default, every 1–19 minutes; per-client switch in the
running client's limits): while the user has been idle 5 s, each due client is brought to the
front, gets one Space press and the previous window gets the focus back; the round stops if the
user comes back. Discord tab (Ctrl+6): a webhook kept encrypted in the vault, a test message, and
toggles for crashes, disconnects, hangs, restarts (memory restarts separately), the rejoin
breaker giving up, finished batches and an optional status summary.

**Test:** turn on anti-AFK at 1 minute, launch a client, leave the PC: about a minute later the
client flashes to the front and back, and "Last key press" resets. Touch the mouse during a
round: it stops. Set a webhook, Send test, then kill a client: a crash message arrives.

## Batch 4 — Roblox updates and installer (v0.4.0)

**Built:** Settings shows the installed Roblox version next to Roblox's live one (checked at start
and every 30 minutes) and flags it when out of date; a bulk launch warns first, since Roblox's own
update can end the other clients. "Install / Update Roblox" downloads Roblox's installer from
`setup.rbxcdn.com`, runs it only if its Authenticode signature is valid and Roblox Corporation's,
then finds the client again and deletes the download; progress shows in the row, and
roblox.com/download is the fallback. Auto-update (off by default) does the same once no client,
launch, batch or rejoin runs. Neither ever runs over a bootstrapper such as Bloxstrap.

**Test:** with an older Roblox installed, Settings says out of date; Install / Update: the
progress runs through downloading, verifying, installing, and the version matches. Try it with a
client running: refused. Turn on auto-update, close all clients: it updates on the next check.

**Roblox differences:** the live version is `clientVersionUpload` from
`clientsettingscdn.roblox.com/v2/client-version/WindowsPlayer`, and the installed one is only
known from the `Versions\version-…` folder name; bootstrapper installs keep their own versions.
