# LRAM design

The interface as it stands: one direction, built from two references (`image-1.png` and
`Screenshot 2026-11-01 164554.png`). The three earlier variants (slate / studio / grid) are gone —
they existed only to pick one.

## What the direction is

A dark, instrumental interface: dense, rectangular, thin borders, a table in the middle. No rounded
corners, no glows, no gradients — only layers of surfaces, 1 px lines, and colour as a signal
(green / red / amber), never as decoration.

The palette comes from the reference:

| Role | Colour |
|---|---|
| Window background (`--frame`) | `#0c1017` |
| Working surface (`--bg`) | `#111720` |
| Panel, title bar, sidebar (`--panel`) | `#171e2a` |
| Inputs, log | `#0f131b` |
| Lines and borders (`--line`) | `#2a3446` |
| Pressed / segment (`--hover`) | `#232e3f` |
| Accent, buttons (`--accent`) | `#437beb` (hover `#5b91f5`, edge `#2f62cc`) |
| Headings, micro labels | `#70a4ff` / `#93bdff` |
| Main text | `#f0f2f6` |
| Secondary text | `#b1bccf` |
| OK / error / warning | `#9ad5b0` / `#ff8074` / `#ffc670` |

## What the screen is made of

1. **Title bar, 58 px.** Logo, `LRAM`, the `v0.10.0` chip and `Local Roblox Account Manager` on the
   left; service buttons (refresh, theme, lock, window) on the right. The space between drags the
   window.
2. **Sidebar.** A rectangular panel with a 1 px border; above the section list a micro heading in
   caps (`SECTIONS`), items without fill: the active one is marked with a border and blue text.
3. **Working area.** `--bg` inside the frame, 6 px from the sidebar and the edges.
4. **Counters strip.** Six cells in one outline, the value coloured: total, in game, online (green),
   offline, signed out (red), not checked (amber). Every cell is a table filter.
5. **Table.** Caps header, separators between columns, 46 px rows (38 px in the dense mode), a
   hairline between rows, selection as a blue highlight without fill.
6. **Bottom log panel.** A collapsed full-width strip: caps heading, line count and the last line;
   it expands into a log well with mono lines (warn amber, error red) and Copy, Clear and Log folder
   buttons.

## How it is built in the code

- `src/styles/directions.css` — the whole look under `:root[data-direction="lram"]`: tokens, shapes
  of buttons/fields/chips/segments, tables, dialogs, menus, toasts, the counters strip, the log
  panel. Without the `data-direction` attribute (or with `soft`) the file changes nothing, which is
  handy for A/B work.
- `src/theme.css` — structural classes that work in any direction (`.brand`, `.nav-heading`,
  `.counters`, `.counter`, `.logdock`), so the markup does not depend on the theme.
- `src/ui/TitleBar.tsx` — exports `PRODUCT`/`TAGLINE` and draws the brand block.
- The mark: `src-tauri/icons/app-icon.svg` and `scripts/make-icons.py` (png and icon.ico come from
  them), `Logo` in `src/ui/Icon.tsx`, the tile in the splash.
- `src/ui/LogDock.tsx` — the bottom log panel (live `log://log` lines plus the tail of `lram.log`).
- `src/ui/DirectionSwitcher.tsx` — a dev switch `LRAM ↔ Soft Deck`; built only in dev and never
  shipped.

## Following a client across a pid change

Roblox moves its client to a new process on an update, and a launcher hands over to the real client
a second after the launch. The tracker keeps the record of a client alive as long as any of these
holds: its pid still reads back with the same start time, Windows refuses to tell (`unreadable`), or
a window of that pid is on screen.

When none of them holds, the record may still be waiting for a client to appear — 120 s after the
launch, 10 min when a launch log of that client turned up, 15 min while the Roblox installer runs.
Inside that window a new pid is claimed through the launch log, the updater relaunch or a growing log.

Past the window, one last check runs before the record is written off: if the client had shown a
window and an unclaimed Roblox process started within 10 min of the launch is still there, the record
rebinds to it and the preview comes back. Nothing is claimed twice: every pid already followed by a
live record is out of reach, and a record being closed never rebinds. The log names what happened —
`rebound from process N to M (it is still running)`, then, when the client is really gone,
`gone: no process and no window, N s after its launch`.

## Checked

`npx tsc --noEmit` is clean; `npx vite build` builds; `node scripts/check-i18n.mjs` finds every key
the code asks for in `en.json`. Rust does not build in this environment (Windows-only crates), so the
`src-tauri` changes were reviewed by reading, not by compiling.
