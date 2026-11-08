# UI mode B "Mobile HUD" (W4)

Status: implemented 2026-11-05 (agent `ui-hud`), desktop-verified with `s3run --inject` touch taps; verified on the
Android emulator (also in a multiplayer game) in ux-verify1 ([integration-log.md](integration-log.md)). Vision: [mobile-vision.md](mobile-vision.md); framework: [mods.md](mods.md); game state used:
[game-state-units.md](../reverse-engineering/game-state-units.md), [game-state-ui.md](../reverse-engineering/game-state-ui.md).
The other mode (A "Classic Touch") is [ui-classic.md](ui-classic.md).

Code (`port/crates/s3-mods/src`): `ui_hud.rs` (the `MobileHud` mod: layout, drawing, touch), `ui_hud_game.rs`
(hooks and game-memory access for the GOG exe), `ui_hud_data.rs` (build-menu table, goods ids), `ui_hud_minimap.rs`
(own minimap widget + full-screen map, 2026-11-05 hud-polish). Installed by
`s3_mods::install` (`ui_hud::install(m, gog_patches)`); hooks only with the GOG patch set.

## What the player sees

**2026-11-05 (hud-polish)**, screenshots `scratch/hud-polish/` (local): the game's own minimap, its frame art and its
white camera rectangle are gone in full-screen layout (nothing of the left panel is drawn any more); the resource
strip moved to the **top left** (the FPS counter, `fps.rs`, draws at the top centre in this mode since verify-fps1;
at the top right it covered the full-map "Close" button); our **minimap widget** sits bottom
left above the bar (dark card, the game's minimap image in the same sheared shape, white camera rectangle, an orange
corner mark hinting "expand"). Drag on it = pan (camera follows the finger), tap = **full-screen map** (whole map
scaled to the visible screen, camera rectangle, "Close" button, Android Back / Esc closes; tap a point = camera
jumps there and the overlay closes). Game menu: Resume, Save game, Pause, Settings..., Quit. **Settings** page:
Screen (display mode: Fit / Fill / Stretch / Native wide) + Filter (Pixel / Smooth / Sharp), FPS counter (Off / FPS /
FPS + details) — all `crate::display` settings of the display agent, persisted in `mods-settings.json` —, Zoom - /
level / Zoom +, HUD size, Full-screen map on/off, UI mode switch, Back. Every HUD element is laid out inside
`display::visible_game_rect` (in `Fill` mode the frame is cropped; the HUD stays on screen).

Older shots (before hud-polish; B-01 still shows the game's minimap, B-08 the old one-page menu):

Screenshots (local only, game pixels 640x480 unless noted): `scratch/ui-modes/B-*.png`.

| Shot | Content |
| --- | --- |
| `B-01-ingame` | Full-screen map (original left panel gone), the game's minimap in the top-left corner with a thin frame, resource strip top right (Boards / Stone / Logs / Food = bread+fish+meat, with the game's own goods icons; Pop = settlers), bottom bar: Basic, Food, Military, Special (icons = first building of each category), Settlers, Stats, Menu |
| `B-02-build-basic` | Bottom sheet "Basic buildings": 4 columns of large cards, icon (sampled from the game's panel) + the game's own building name (text table 8). Only the buildings of the player's race are listed |
| `B-03-build-military` | Same for Military |
| `B-04-placement` | After choosing Guard Tower: banner with icon + name + "Tap the map to place" + red Cancel; the game shows its placement markers; the next map tap places the site (VERIFIED in earlier runs: site created, banner closes) |
| `B-05-unit-card` | Box-selected soldiers: card "14 selected: 10 x Swordsmen, 2 x Bowmen, 2 x Soldiers (3rd)" with "Move to..." (next map tap = right click = move/attack) and "Deselect" |
| `B-06-settlers` | Settlers sheet: total settlers and buildings, counts by settler type, and four buttons with the game's own names (Population, Occupations, Recruitment, Production statistics) that open those original pages (**replaced 2026-11-08**, see "No original panel") |
| `B-07-stats` | Goods sheet: stock per good (icon + name + amount, own land only), buttons Inventory / Tools and Weapons / Priority of Buildings / Priority of Goods (original pages; **removed 2026-11-08**) |
| `B-08-menu` | Game menu: Resume, Save game, Pause, Quit game, UI mode switch (Mobile HUD > Classic Touch), Full-screen map on/off, HUD size -/+ |
| `B-09-save-peek` | (obsolete since 2026-11-08: native save sheet) "Save game" opened the original Save/Quit page in the restored panel ("panel peek") |
| `B-10-building-card` | Tapped Guard Tower: card with the name, Details (since 2026-11-08 the native building sheet), Destroy (tap twice: "Sure?"), Close |
| `B-11-800x600-ingame`, `B-12-800x600-build` | Same HUD at 800x600 (registry `Resolution=1`): everything scales with the frame (factor = height/480 x `hud_scale`) |

Touch targets: bar buttons ~86x46 game px, cards ~146x56, card/menu buttons >= 36 px high (x2.25 on a 1080p phone).
Taps outside an open sheet close it and do not reach the map. HUD buttons capture their finger (W1 pointer capture), so
a press that slides off a button does nothing.

## How it works

**Window layer (2026-11-08 overlay-window)**: the HUD is drawn into the mods' window-resolution layer
([mods.md](mods.md) "Window layer"), not into the game frame: sizes come from the window (factor = window height /
480 x `hud_scale`, same look as a 600 px frame shown full screen before), so the bar, sheets, cards and text keep
their size and stay sharp at every zoom level (now down to 200 px render height). Touches hit-test with their window
position. Icons are still sampled from the game's frame; the icon harvest needs a frame >= 480 px high, so while it
runs the zoom keeps the render size at least 480 (`HudShared::harvesting`, hidden behind the "Preparing touch HUD"
screen). The minimap is a shared RGBA image drawn with the game's shear (`Layer::blit_map`).

### Hiding the panel: full-screen map (layout patch)

Instead of covering the panel, the game is made to draw the map over the whole screen:

| Hook | Where / thread | What |
| --- | --- | --- |
| `0x4c6d70` return (layout setter) | display setup | if `ui_mode == MobileHud` (and full-screen not turned off): `[0x730f24]` (panel width) = 0, `[0x730f18]` (map view width) = screen width |
| `0x54bec0` entry (landscape ring buffer init, thiscall `[gfx+0x5f6cc]`, arg0 = width) | display setup | arg0 = screen width + margin when it equals map width + margin (`[0x730f44]` = 32 in our runs) |
| `0x4b4d00` entry (panel renderer) | render thread, every in-game frame | heartbeat ("in game"), records `gfx` (ecx); applies or undoes the layout; skips the panel (`return_from_function(0, 1)`) while full screen |
| `0x4ff170` entry (`gd::ScreenToMap`) | logic thread | while full screen: `sx += original panel width` (its offsets are constants for the original layout) |

Findings (VERIFIED in s3run, 2026-11-05):

- The landscape is rendered into a ring buffer (`gfx+0x5f6cc`, width at `+0x50`, height `+0x54`) of width
  `map view width + [0x730f44]`, allocated by the display setup (`0x42ef..0x42f204`, three calls to `0x54bec0`). The
  blitter `0x54c9d0` wraps at that width. With only the globals changed at runtime the map showed a wrapped black
  zigzag column at x ~ 515 (504 + margin). So the layout must be in place before the display setup; the HUD then only
  uses full screen when the buffer is wide enough (`HudShared::buffer_ok`), otherwise it covers the panel ("cover
  mode", below).
- The renderer places the map at `x = [0x730f24] - (camX & 15) - 32`; units, buildings and the on-screen hit list
  (`0x7ade08`) follow, so `gd::0x4ff0a0` (hover / placement) and the hit list need no change; only `ScreenToMap` does.
- The minimap is drawn by the frame renderer `0x4b26c0`, not the panel. Since hud-polish the HUD **skips its two
  minimap blocks** while the full-screen layout is active (hooks `ui-hud:minimap-skip`: `0x4b2b34` -> jump
  `0x4b2ce5` = incremental minimap copy; `0x4b366a` -> jump `0x4b3cf3` = minimap part right of the panel + frame art
  resources `0x40001`/`0x40003` + white camera rectangle; the selection-box drawing after `0x4b3cf3` stays). Render
  thread only, no guest-visible game state changes (lockstep safe). VERIFIED 2026-11-05 (screenshot: top left shows
  map). In cover mode / panel peek / Classic mode the blocks run as before (VERIFIED: cover mode shows the full game
  minimap again). Its hit test in HandleInput needs `x < [0x730f24] + d`, so with panel width 0 the game ignores
  taps there; the HUD widget queues `gd::CenterOn` `0x51b560` (thiscall gd, `(tx, ty)`) on the logic thread.
  Writing the camera from the host instead left stale ring-buffer columns after a big jump.
- **Custom resolutions** (res-zoom): the layout hook records `(res index, panel width, map width)` as the game's
  layout setter wrote them (`HudShared::orig_layout`) before applying the full-screen layout, and restores/compensates
  with those values (`current_original_layout`), so a patched `0x4c6d70` case for a new resolution works without a
  table entry. The landscape-buffer widening accepts screens up to 4096 px wide (was 1024).
- The panel is only redrawn when `[gd+0x2d27c]` (panel dirty) is set: after restoring the layout the HUD sets it,
  otherwise the panel area kept map pixels.
- The panel renderer does not use `[0x730f24]` (all panel art is at fixed positions), so it can draw on top of the
  full-screen map for the icon sampling (the former "panel peek" used this too); panel clicks need the original
  layout (HandleInput tests `x < [0x730f24]`).
- Runtime resolution switch (F1-F3) in HUD mode: the 800x600 frame after F2 showed a black band with a zigzag edge
  around x ~ 515..650 (screenshots `scratch/ui-hud/r9-*.png`); starting directly at 800x600 (registry
  `Resolution=1`) is correct. HYPOTHESIS: another buffer (fog of war / visibility) is sized at game start from the
  map view width. Open.
- Camera clamp: the clamp `0x51b4d0` is unchanged, so near the west map edge the void outside the (sheared) world is
  visible where the panel used to hide it. Cosmetic.

### Minimap widget and full-screen map (`ui_hud_minimap.rs`)

Source: the game's own minimap image, a square RGB565 `mw x mw` image (`mw = [0x730f28]`, 128 at 640x480) at
`[snap+0x4c]`, where `snap` is the render snapshot (the panel renderer's argument, recorded in the panel hook).
Image column `c`, row `r` = tile `(c*W/mw, r*W/mw)`; the game draws it sheared (`x = mx + (mw-r-1)/2 + c`,
`y = my + r`), we draw it the same way at any scale (`MapGeom`). Camera rectangle in image coordinates with the
game's own formula (`0x4b3a3c`): `k = (mw<<8)/[snap+0x2c]`, `x = ([snap+0x24]+11)*k/256`, `y = ([snap+0x28]+16)*k/256`,
`w = [snap+0x30]*k/256`, `h = ([snap+0x34]-40)*k/256` (VERIFIED: the rectangle follows drags and jumps; 128 px image,
384 world: `cam (58, 85, 16, 17)` at the start). Refreshed every 250 ms into a host copy (one 32 KiB read). Player
territory colours come with the image; unexplored land is black. Touch: Down on the widget captures the finger;
moving more than ~7 HUD px turns it into a drag (each new tile under the finger replaces a still queued `CenterOn`);
Up without a drag opens the overlay. Overlay tap inside the map's bounding box -> tile (clamped into the shape) ->
`CenterOn`, close. Back / Esc: new `Mod::on_key` (lib.rs, additive): the HUD consumes VK_ESCAPE while a sheet is open
(Settings -> Menu, others -> close); otherwise Esc reaches the game as before. All VERIFIED on desktop 2026-11-05.

### Settings and zoom

Display entries call `crate::display::{cycle_mode, cycle_filter, cycle_fps}` (display agent, docs/porting/display.md).
VERIFIED: "FPS counter: FPS" turned the counter on (then top right; top centre since verify-fps1) and was saved in `mods-settings.json`.
Zoom - / + is still a placeholder (open as of 2026-11-05 wrap-up; pinch already uses res-zoom's `zoom.rs`) (`ui_hud::zoom_step` / `zoom_label`): it sends F1..F3 like
the pinch in touch.rs (`touch_logic::pinch_zoom_key`) and shows the game resolution as the level. Known issue: the
runtime F1-F3 switch in HUD mode shows a black band (below) — res-zoom's zoom replaces this.

### Running game actions

UI state changes are queued (`ui_hud_game::queue`) and run in a hook at `CLogicThread::HandleInput` entry
`0x524fa0` (logic thread, the place where the game changes the same state), so no state is written mid-tick:

| HUD action | Game side | Status |
| --- | --- | --- |
| Building card -> placement | `[gd+0x2d284]=1`, cat, slot, type, panel dirty, then `gd::0x4ff3f0` (new placement-marker generation, what the panel click does at `0x526feb`). Without `0x4ff3f0` no markers are drawn and map taps cannot place (`0x4ff0a0` picks the nearest marker from the hit list) | VERIFIED (site placed) |
| Cancel placement | slot = 0, page 0 | VERIFIED |
| Move units | `s3_mods::set_right_next` -> the next map tap is a quick right click (passes the 15-step check at `0x53437d`) | VERIFIED (unit walked) |
| Deselect | UI selection count 0 + dirty, `[0x7dfd6c]` = 0, page 0 | STATIC |
| Destroy building | command `0x06` `{0x21, 0x06, 0x08, 0, id_hi, id_lo, player, 0}` posted with `Queue::Post 0x4309c0` on `[logic+0x3c4]` (`ndwords` 2, flag 1) | VERIFIED (Guard Tower destroyed, game reported "You've Lost") |
| Minimap widget drag / full map tap | `gd::CenterOn 0x51b560` | VERIFIED (view jumped / followed the drag, 2026-11-05) |
| Save | slot list `CLogicThread::0x50b670`, save `0x50ac40(0, slot)` (multiplayer: command 0xfa), no panel page | VERIFIED (slot 1 written and listed, 2026-11-08) |
| Quit | `0x53e720` like the panel's YES | VERIFIED |
| Building sheet | commands 0x04 (stop/resume), 0xf5/0xf6 (site priority), 0x0c (garrison), work area `0x523840` + sub-page 2 | VERIFIED except work area (STATIC) |
| Recruitment (settlers sheet) | command 0x10 at the view-centre tile, then `0x542630` | VERIFIED (geologist, pioneers) |
| Pause | key `P` (0x50) down/up into `m.input` | STATIC |
| Goods tab 1 (icon harvest only) | page 2 + `[gd+0x2d290]` = 1 behind the "Preparing" cover | VERIFIED |
| UI mode switch | `settings.ui_mode` saved with `s3_mods::set_settings`; the HUD goes inactive and restores the layout + panel | VERIFIED (panel back, Classic mode's Menu button shown) |

Load game: there is no in-game load in the original panel; the menu says "load: main menu" (quit, then Load).

### Reading game state (every 200 ms, `ui_hud_game::snapshot`)

`gd = [0x7e3408]` (checked: committed memory + vtable `0x658c18`); panel page/category/slot/type; local player and
race (`gd+0x11943e0 + p*0x148`); UI selection (`0x7dfd68`, `0x7ad620`, `0x7ad634`) with settler types from the settler
table; selected building `[0x7dfd6c]` + type; settlers per type of the local player; buildings of the player; goods
summed over the pile table.

- **Goods: only piles on the player's own land** (tile `+8` owner == local player). Without this filter a game against
  the AI showed exactly twice the Storage Area numbers (both start stocks). With it the numbers matched the panel's
  Inventory page (VERIFIED, one map).
- The panel's Inventory numbers are **not** a live sum of piles: writing pile amounts (debug poke of every good-1 pile)
  changed our sum but not the panel (VERIFIED). The panel keeps its own counters (not located; re-ui also did not find
  them).
- Good ids matched to the Inventory page by unique start amounts (`ui_hud_data::GOODS`): 4 Bread, 5 Coal, 7 Iron ore,
  8 Fish, 12 Shovel, 13 Hammer, 14 Axe, 15 Pickaxe, 16 Saw, 24 Meat (VERIFIED by amounts). 1/2 = Boards/Stone and
  17/32 = Scythe/Fishing rod have equal amounts at the start: order HYPOTHESIS. 3 = Logs HYPOTHESIS.
- Settler type names: 0 Carriers, 5 Swordsmen, 8 Bowmen, 15 third soldier kind (HYPOTHESIS, part 1); others are
  shown as "Type N".
- Names come from the game's text tables through a hook on the string lookup `0x5faaa0` (records the table object,
  `ecx`), read host-side like `tools/re-ui/strings.py` (`ui_hud_game::text`). So they follow the game's language.
  Only tables 7, 8 and 9 are loaded in game. `S3_HUD_TEXT_DUMP=1` logs every string once (game data: local logs only).

### Icons sampled from the running game (nothing shipped)

On the first in-game frames (and when the race changes) the HUD lets the original panel draw each build category once
(page 1, cat 1..4) and then the goods Inventory page, and copies pixels from the presented frame before drawing:
build slot `2k+1` / `2k+2` = grid cols 1..19 / 21..39, rows `22+12k .. 31+12k` (game-state-ui.md), goods icon
`(row, col)` at `x = 14 + 18*col`, `y = 247 + 27*row`, 16x16 at 640x480 (measured; scaled by the grid cell for other
resolutions, which worked at 800x600). It takes 5 x 3 panel frames (well under a second, inside the start-up period);
a "Preparing touch HUD" cover is drawn meanwhile. The icons live only in memory.

### Cover mode

When the full-screen map is off (menu toggle) or not possible yet (buffer too narrow, e.g. switching to HUD mode from
Classic mid-game until the next display setup), the HUD covers the panel below the minimap with a dark sidebar and
keeps the game's minimap usable.

## No original panel (hud-native, 2026-11-08)

User report: "Details", "Save game" and the goods / settlers tab buttons opened the original left panel ("panel
peek"): not in wide mode, and scaled by the zoom. Since 2026-11-08 the original panel is **never** shown in this
mode (only behind the "Preparing touch HUD" cover during the icon harvest). The peek (`start_peek`, `PeekDone`,
`HudShared::peeking`, the zoom's peek minimum height) is gone; RE in
[game-state-ui.md](../reverse-engineering/game-state-ui.md) section 8.

| Former peek | Now | Status |
| --- | --- | --- |
| Building card "Details" (page 4) | **Building sheet**: name, state ("Construction site - stopped - highest priority", "Working" / "Stopped", "Military building"), buttons with the game's own tooltip texts: Stop / Resume Working (or Building), Highest Priority / Normal priority (sites), Set new working area (woodcutter, stonecutter, forester, fisher, farms, winery, waterworks, beekeeper: next map tap), Garrison: Add / Remove Swordsman / Bowman / Spearman + Maximum / Minimum Garrison (towers, castle), link Recruitment (barracks) / Population (residences) to the settlers sheet, Crush building (tap twice), Close. Only for own buildings. Closes when the game deselects the building | VERIFIED: stop building, priority on, Maximum Garrison (building bytes changed); work area STATIC |
| Menu "Save game" (page 7) | **Save sheet**: 8 slots in two columns, each with the game's own description (map name, date/time + race from `0x50b670`) or "Empty slot"; tap = save (`0x50ac40`), an occupied slot needs a second tap ("Tap again to overwrite"); "Saving..." then the toast "Game saved (slot N)"; multiplayer: one "Save multiplayer game" button (command 0xfa); Back = game menu | VERIFIED single player (slot 1 written, listed afterwards); multiplayer STATIC |
| Settlers sheet tabs (page 3) | Counts per type stay (Population / Occupations); **Recruitment** natively: 1 / 5 New Geologists, Thieves, Pioneers, Pioneers to Carriers (game texts; Amazon texts for race 3) | VERIFIED (geologist + 5 pioneers created) |
| Goods sheet tabs (page 2) | The sheet itself is the Inventory (title = the game's "Inventory"); the other tabs are dropped | – |

Left (not native yet; no button any more): goods tabs Tools and Weapons, Priority of Buildings, Priority of Goods
(economy commands 0x07/0x08/0x0a/0x0b/0x11, not RE'd), settlers tab Production statistics, soldier recruitment and
carrier / digger / builder limits on the Recruitment page, building-specific pages (siege workshops "Build
Catapult...", shipyard / harbour waypoints and ships, storage area "Setting", trade), the building picture and the
workers / goods in-out numbers of the original page 4 (renderer page-4 block not analysed), garrison occupancy numbers.

Screenshots (local, 2316x904 window): `scratch/hud-native/` — `bs1.png` Guard Tower sheet, `l4.png` construction
site sheet, `l5.png` after Highest Priority, `l6.png` after Stop Building, `st1.png` / `st2.png` settlers sheet
before / after recruiting, `sv1.png` empty slot list, `sv2.png` "Game saved (slot 1)", `sv3.png` slot 1 described.

Recipe (window 2316x904, start taps as below, game ~30 s): tower `wtap 1148 383` (centre of the start view),
Details `wtap 1923 747`; Settlers `wtap 1486 855`, Menu `wtap 2146 855`, then Save game `wtap 1013 374`, slot 1
`wtap 899 240`. A fixed map: load the save (`tap 165 355; tap 300 126` from the main menu, in game at ~20 s).
`S3_HUD_DEBUG=1` logs the building entry bytes on every building-sheet action.

## Testing on desktop

```sh
mkdir -p scratch/ui-hud/data && echo '{"ui_mode":"MobileHud"}' > scratch/ui-hud/data/mods-settings.json
cd port
I="1200:tap 165 235;3200:tap 630 467;5200:tap 730 567;7200:tap 730 567;32000:shot ../scratch/ui-hud/a.png;33000:tap 47 455;34500:shot ../scratch/ui-hud/b.png;35000:tap 90 250;36500:shot ../scratch/ui-hud/c.png"
./target/release/s3run --headless --game-dir ../game-files/gog-s3-ultimate --data-dir ../scratch/ui-hud/data --max-seconds 38 --inject "$I"
```

Bar button centres at 640x480: x = 47, 137, 227, 317, 407, 497, 587 (y 455). "Back to HUD" = (575, 27).
Minimap widget (640x480, scale 100%): (60, 380) (card x 6..160, y 314..420). Menu: Settings... = (320, 245);
Settings rows (centre y): 80 screen/filter, 127 FPS, 174 zoom, 221 HUD size, 268 full-screen map, 315 UI mode,
362 Back. hud-polish recipe (after the start taps above): `33000:drag 60 380 130 340 400;35000:tap 60 380;36000:key
0x1b;37500:tap 587 455;38500:tap 320 245;40000:tap 320 127` (pan, open map, Back, menu, settings, FPS on). Random
maps differ per run, so unit/building positions must be read from a screenshot first. Note: switching the UI mode in
the HUD menu saves `mods-settings.json` in the data dir. `S3_HUD_DEBUG=1` logs the landscape buffer widths.

Unit tests (`cargo test -p s3-mods`): build table, slot rectangles, hit rectangles, minimap geometry (shear round
trip, tiles, camera formula).

## Open / next

- ~~Runtime F1-F3 switch in HUD mode (black band)~~: fixed 2026-11-05 by res-zoom (cached landscape view rectangle
  re-synced every frame + redraw on a matching snapshot, [resolution-zoom.md](resolution-zoom.md)). With the custom
  resolution on (default) the HUD runs at the window aspect (e.g. 1480x600) and pinch zooms.
- Touch-context (W3) mod and the HUD both take taps; the HUD must stay on top (it is added after the framework's
  own mods; if another mod is added later it gets events first). Taps on HUD elements are consumed by the HUD.
- Settler type and remaining good names; tooltips for building cards (cost) — needs the panel's goods counters / cost
  tables (not RE'd).
- ~~Building card: more actions~~: done 2026-11-08 (building sheet). Left: see "No original panel" below.
- ~~Try on the Android emulator~~: done (ux-verify1). Note: map taps in the HUD are handled by touch-context
  (tap = select / move / place); that needed `touch_game::layout` to accept panel width 0. In multiplayer
  `[0x7e3408]` is 0, so `gd()` falls back to `[logic+0x3a0]` (ux-verify1).
- touch.rs' ✕ placement cancel is not drawn in this mode (the banner has Cancel), but its hit area top right is
  still active during placement.

## Game speed

Single-player game: the ">> Nx" pill right of the resource strip switches between normal speed and the chosen fast
speed (accent colour while fast). Settings -> "Fast speed: Nx" cycles 2x..8x (`crate::speed`).
Settings -> "Game time: on/off" shows the game time (`crate::game_time`) below the resource strip (default on).
