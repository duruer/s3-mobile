# UI mode A "Classic Touch"

Status: implemented 2026-11-05 (agent `ui-classic`). Code: `port/crates/s3-mods/src/ui_classic.rs` (mod + hooks) and
`ui_classic_panel.rs` (pure panel model, unit tested). Vision: [mobile-vision.md](mobile-vision.md) (UI decision A).
Framework: [mods.md](mods.md). Game facts used: [game-state-ui.md](../reverse-engineering/game-state-ui.md),
[game-state-units.md](../reverse-engineering/game-state-units.md). Mode B: [ui-hud.md](ui-hud.md).

Idea: keep the original left panel, menus and screens, and make them work with fingers. Nothing is replaced and the
game is not modified on disk. Map touches (select, move, build placement, ✕ cancel, pan, pinch) belong to the
context-aware touch mod (`touch*.rs`, W3), which both UI modes share. Mode A adds only what the panel needs.

## What it does

| Feature | Behaviour | How |
| --- | --- | --- |
| **Panel taps with a magnifier** | A finger on the left panel below the minimap is captured. While it is down, a bubble right of the panel shows the area under the finger enlarged 3x (64x40 px source, 192x120 px) and, below it, the button name in 18 px bold text. The chosen button gets a gold outline on the panel and in the bubble. The finger may slide to change the choice. Lifting it sends one left click to the **centre** of that button. Sliding more than 48 px right of the panel cancels without a click. | `ui_classic_panel::buttons(page, sub, category, race)` lists the known buttons in grid units. `pick()` chooses the button under the finger. In gaps, it snaps to the nearest button within 4 grid cells (12 px at 640x480), but only on pages whose buttons we know completely (0-3, and 7 sub-page 0). On other pages (4 building, 5 units, 8 messages, …) the raw point is clicked and the bubble still magnifies. |
| **Button names from the game** | The game's own strings in the game's language ("Woodcutter's Hut", "Buildings", "Save/Quit", …). English fallback when the table cannot be read. | The hook on the gfx ctor `0x42c820` records `gfx` (ECX). The string table is `[gfx+0x5f6b8]`. `panel::read_text` repeats the lookup `0x5faaa0` does, reading memory only (no guest call). Building names: text `0x80000+type`, plus the special ids for Amazon types. Results are cached. |
| **Floating Menu button** | A 76x38 "Menu" button at the bottom right of the map. It opens the in-game Save/Quit menu (panel page 7). While the menu is open it reads "Close" and returns to the previous page (build, goods, settlers; otherwise none). Replaces ESC, which S3 does not use in game. | The tap queues `Action::OpenMenu`/`CloseMenu`. The hook at `HandleInput` entry `0x524fa0` (logic thread, before input is processed) writes page 7, sub-page 0 and panel dirty, then plays the panel click sound (`0x53f560(0x3d)`, thiscall logic). |
| **Display + FPS toggles** (2026-11-05) | While the menu is open, "Screen: Fit/Fill/Stretch/Wide" and "FPS: off/on/details" buttons sit above "UI: Mobile HUD"; a tap cycles the setting (saved). The floating buttons and the touch ✕ are anchored to the visible part of the frame (Fill mode crops it; the ✕ moves below the FPS counter). Note: in Fill mode the left panel's top/bottom rows are cropped too (e.g. "Quit Game" at 640x480 on a 2.47:1 screen), so Fill suits mode B / Native wide better. | `display::cycle_mode` / `cycle_fps`, `display::visible_game_rect` (see [display.md](display.md)). |
| **Context chip** | Top left of the map: "Place: <building>" in build placement, "N selected" for a unit selection, or the selected building's name. | Read each tick: page `[gd+0x2d284]`, slot `[gd+0x2d28c]`, type `[gd+0x2d2f0]`, selection `[0x7dfd68]`/`[0x7ad620+slot*2]`, building `[0x7dfd6c]` -> building table type. |

When it is active: `settings.ui_mode == ClassicTouch` (the default) or `S3_UI_MODE=A|classic`. `S3_UI_MODE=B|hud|off`
turns it off. It also has to be in game. In-game test: `HandleInput` ran in the last 1.5 s, `gd` has the CGameData
vtable `0x658c18`, and the layout globals `0x730f0c/10` equal the presented frame size. In the 800x600 menus the
mod does nothing, because the menu buttons are already large. All hooks are installed only for the GOG exe (sha256
`00a925cf…`). With any other exe, mode A stays inactive.

Not done, and why:

- **Enlarged panel rendering** (scaling the panel region up when presenting): this needs a presenter crop/scale that
  is shared with mode B. The magnifier already makes small icons readable when needed, and keeps the map area.
  At 640x480 in game (the default), a phone shows the panel at about 2.2x its original size.
- **Own Cancel button**: `touch.rs` already draws the ✕ Cancel button for build placement (top right, for both modes).
  That is why the Menu button sits bottom right.

## Verification (desktop, headless, 2026-11-05)

Recipe (640x480 in game; Menu centre `594 447`):

```sh
D=../scratch/ui-modes
I="3000:shot $D/A-menu.png;1200:tap 165 235;3200:tap 630 467;5200:tap 730 567;7200:tap 730 567;40000:shot $D/A-ingame.png;\
41000:tap 25 167;42500:tap 23 197;44000:longpress 40 233 1500;45000:shot $D/A-build-loupe.png;46500:shot $D/A-build-placing.png;\
47500:tap 612 28;49000:shot $D/A-cancelled.png;50000:tap 594 447;52000:shot $D/A-menu-open.png;53000:longpress 70 230 1200;\
53800:shot $D/A-menu-loupe.png;56000:tap 594 447;57500:shot $D/A-menu-closed.png;58000:down 150 200;58300:move 400 330;\
58600:move 630 470;58800:up 630 470;60500:shot $D/A-units.png;61000:longpress 40 250 1500;62000:shot $D/A-units-loupe.png"
./target/release/s3run -v --headless --game-dir ../game-files/gog-s3-ultimate --data-dir $D/data --max-seconds 66 --inject "$I"
```

Log lines `ui-classic: panel click at 32,167 (page 0)`, `… 23,197 (page 1)`, `… 39,233 (page 1)`, `OpenMenu`,
`CloseMenu`. Screenshots are in `scratch/ui-modes/` (local only, game imagery):

| File | Shows |
| --- | --- |
| `A-menu.png` | Main menu / game setup (800x600): unchanged in mode A |
| `A-ingame.png` | In game: original panel and map; "Menu" button bottom right |
| `A-build-loupe.png` | Build tab -> category 1, finger held on slot 1: gold outline, bubble with the 3x icon and "Woodcutter's Hut" (from the game's string table) |
| `A-build-placing.png` | After lifting the finger: placement mode, chip "Place: Woodcutter's Hut", ghosts on the map, the ✕ of touch.rs top right |
| `A-cancelled.png` | ✕ tapped: slot cleared, chip gone |
| `A-menu-open.png` | Menu tapped: panel page 7 (Save Game / Quit Game); the button reads "Close" |
| `A-menu-loupe.png` | Finger on "Save Game": bubble with the enlarged label |
| `A-menu-closed.png` | Close tapped: back to the build page |
| `A-units.png` | Box selection (mouse inject): units panel and chip "14 selected" |
| `A-units-loupe.png` | Finger on a soldier icon on page 5 (unknown layout): magnified, no snapping, no name |

Tests: `cargo test -p s3-mods` (`ui_classic::panel::tests`: grid rects vs. the integration-log recipe, build list
per race, snapping, layouts, cp1252).

## Known issues / next

- On pages we did not map (4, 5, 8), the bubble has no name. The game's own tooltip id (`snap+0x270`, read through
  `[logic+0x3d0]`) is used when it is set, but it stayed empty in the test. A hover mouse move is sent on touch
  down and move. HYPOTHESIS: the snapshot field is cleared before we read it, or the tooltip pass needs more idle
  ticks. Next step: hook the tooltip draw `0x4b3de1` or read `[logic+0x54]` inside the HandleInput hook.
- Press feedback of `touch.rs` (the long-press ring) also appears under the Menu button, because that mod is on top
  and sees the Down event first (it passes the event, then this mod captures it). Cosmetic. A fix needs touch.rs to
  skip overlay buttons, for example through a shared "UI rects" list in `ModsState`.
- The save slot list (page 7 sub 2) and the quit confirmation (sub 1) are not mapped: taps there are raw clicks
  with a magnifier.
- Android: verified on the emulator in ux-verify1 (panel taps with the magnifier, placement, Menu, mode switch, also
  in a multiplayer game).
- Mode switch: while the Menu (page 7) is open, a "UI: Mobile HUD" button above "Close" switches to mode B (saved
  in `mods-settings.json`). Mode B covers the panel until the next game start, then uses the full-screen map.
