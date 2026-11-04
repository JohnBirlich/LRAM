# Restart policy and the stuck-client watchdog (0.12.0)

What LRAM does when a client crashes, hangs, never gets into a game or goes over its memory limit,
and how to turn it off. Everything here is what the code actually does (`instances/tracker.rs`,
`instances/rejoin.rs`), not a wish list.

## What counts as trouble

| Trouble | How LRAM knows | Setting that enables the relaunch |
|---|---|---|
| Disconnect | a disconnect line in the client's log, or the `Disconnected` state | `auto_rejoin` |
| Crash | the client ended by itself and LRAM did not close it, and its log does not say it left | `auto_restart_on_crash` |
| Hang | the window does not answer for 30 polls in a row (about a minute) | `auto_restart_on_crash` |
| Stuck | the client is alive but has not reached a game in 15 minutes (sitting in the tray) | `auto_restart_on_crash` |
| Memory | working set over `mem_restart_mb` for several polls in a row | `mem_restart_mb` > 0 |

A player who left (Leave/Teleport), a client the user closed, Close all/Kill all, a client with a
sign-in problem (403, "signed in as another account", a captcha) and a client whose log cannot be
read are never relaunched: that is a decision, not a failure.

## How long it waits and how many times it tries

1. **Pause before the relaunch** — `launch_delay_ms × 2^relaunches` (from 6 s by default, with
   jitter), then 15 s → 30 s → 60 s → 120 s. The jitter keeps five clients from hitting Roblox at
   once.
2. **Breaker**: no more than `breaker_max` relaunches of one account per `breaker_window_min`
   minutes (5 per 10 minutes by default). After that the note reads "Gave up after N restarts", a
   notice goes out and, if it is on, so does a Discord message.
3. **Patient reasons** — no network, Roblox down, a batch running, a locked vault — do not cancel
   the relaunch: it is retried for up to an hour (`RETRY_FOR`), after which the client gets "Gave up
   after N minutes of trying".
4. **A stuck client** is relaunched at most 3 times: a client that will never open would otherwise
   be relaunched every 15 minutes forever. After the third: "Gave up: never reached a game".

## The buttons

- **Try now** — skip the pause and try again (also after "Gave up").
- **Stop trying** — stop the attempts for this client.
- **Close all** — cancels every relaunch that is waiting.

## Where it shows

- In the grid on Home: the note under the account name and the relaunch count.
- The trouble card above the grid: what it waits for, and Try now / Stop trying / Save a report….
- In the log (`lram.log` and the Debug tab): every attempt, every pause and every reason it gave up,
  with evidence.

## What LRAM does not do

- It does not relaunch a client whose account signed out or hit a sign-in problem: fix the account
  first.
- It does not relaunch clients while a Roblox update is being installed.
- It does not raise process priorities and never touches input in the game: the only thing it does
  to a client is the CPU/memory limits and efficiency mode (`instances/limits.rs`).
