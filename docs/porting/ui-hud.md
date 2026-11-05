# UI mode B "Mobile HUD" (W4)

Status: implemented 2026-11-05 (agent `ui-hud`), desktop-verified with `s3run --inject` touch taps; verified on the
Android emulator (also in a multiplayer game) in ux-verify1 ([integration-log.md](integration-log.md)). Vision: [mobile-vision.md](mobile-vision.md); framework: [mods.md](mods.md); game state used:
[game-state-units.md](../reverse-engineering/game-state-units.md), [game-state-ui.md](../reverse-engineering/game-state-ui.md).
The other mode (A "Classic Touch") is [ui-classic.md](ui-classic.md).

Code (`port/crates/s3-mods/src`): `ui_hud.rs` (the `MobileHud` mod: layout, drawing, touch), `ui_hud_game.rs`
(hooks and game-memory access for the GOG exe), `ui_hud_data.rs` (build-menu table, goods ids). Installed by
`s3_mods::install` (`ui_hud::install(m, gog_patches)`); hooks only with the GOG patch set.

## What the player sees

Screenshots (local only, game pixels 640x480 unless noted): `scratch/ui-modes/B-*.png`.

| Shot | Content |
| --- | --- |
| `B-01-ingame` | Full-screen map (original left panel gone), the game's minimap in the top-left corner with a thin frame, resource strip top right (Boards / Stone / Logs / Food = bread+fish+meat, with the game's own goods icons; Pop = settlers), bottom bar: Basic, Food, Military, Special (icons = first building of each category), Settlers, Stats, Menu |
| `B-02-build-basic` | Bottom sheet "Basic buildings": 4 columns of large cards, icon (sampled from the game's panel) + the game's own building name (text table 8). Only the buildings of the player's race are listed |
| `B-03-build-military` | Same for Military |
| `B-04-placement` | After choosing Guard Tower: banner with icon + name + "Tap the map to place" + red Cancel; the game shows its placement markers; the next map tap places the site (VERIFIED in earlier runs: site created, banner closes) |
| `B-05-unit-card` | Box-selected soldiers: card "14 selected: 10 x Swordsmen, 2 x Bowmen, 2 x Soldiers (3rd)" with "Move to..." (next map tap = right click = move/attack) and "Deselect" |
| `B-06-settlers` | Settlers sheet: total settlers and buildings, counts by settler type, and four buttons with the game's own names (Population, Occupations, Recruitment, Production statistics) that open those original pages |
| `B-07-stats` | Goods sheet: stock per good (icon + name + amount, own land only), buttons Inventory / Tools and Weapons / Priority of Buildings / Priority of Goods (original pages) |
| `B-08-menu` | Game menu: Resume, Save game, Pause, Quit game, UI mode switch (Mobile HUD > Classic Touch), Full-screen map on/off, HUD size -/+ |
| `B-09-save-peek` | "Save game" opens the original Save/Quit page in the restored panel ("panel peek"); a "Back to HUD" button top right |
| `B-10-building-card` | Tapped Guard Tower: card with the name, Details (original building page in the peek), Destroy (tap twice: "Sure?"), Close |
| `B-11-800x600-ingame`, `B-12-800x600-build` | Same HUD at 800x600 (registry `Resolution=1`): everything scales with the frame (factor = height/480 x `hud_scale`) |

Touch targets: bar buttons ~86x46 game px, cards ~146x56, card/menu buttons >= 36 px high (x2.25 on a 1080p phone).
Taps outside an open sheet close it and do not reach the map. HUD buttons capture their finger (W1 pointer capture), so
a press that slides off a button does nothing.

## How it works

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
- The minimap is drawn by the frame renderer, not the panel, so it stays visible. Its hit test in HandleInput needs
  `x < [0x730f24] + d`, so with panel width 0 the HUD handles minimap taps itself (formula from game-state-ui.md §1)
  and queues `gd::CenterOn` `0x51b560` (thiscall gd, `(tx, ty)`) on the logic thread. Writing the camera from the host
  instead left stale ring-buffer columns after a big jump.
- The panel is only redrawn when `[gd+0x2d27c]` (panel dirty) is set: after restoring the layout the HUD sets it,
  otherwise the panel area kept map pixels.
- The panel renderer does not use `[0x730f24]` (all panel art is at fixed positions), so it can draw on top of the
  full-screen map for the "panel peek" and icon sampling; panel clicks need the original layout (HandleInput tests
  `x < [0x730f24]`), so peek mode restores it.
- Runtime resolution switch (F1-F3) in HUD mode: the 800x600 frame after F2 showed a black band with a zigzag edge
  around x ~ 515..650 (screenshots `scratch/ui-hud/r9-*.png`); starting directly at 800x600 (registry
  `Resolution=1`) is correct. HYPOTHESIS: another buffer (fog of war / visibility) is sized at game start from the
  map view width. Open.
- Camera clamp: the clamp `0x51b4d0` is unchanged, so near the west map edge the void outside the (sheared) world is
  visible where the panel used to hide it. Cosmetic.

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
| Minimap tap | `gd::CenterOn 0x51b560` | VERIFIED (view jumped) |
| Save / Quit | page 7, sub-page 0 / 1 in the panel peek (then the original buttons) | VERIFIED (page shown) |
| Pause | key `P` (0x50) down/up into `m.input` | STATIC |
| Settlers / goods pages | page 3 + `[gd+0x2d2a0]` tab, page 2 + `[gd+0x2d290]` tab, in the peek | VERIFIED (Inventory) |
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

## Testing on desktop

```sh
mkdir -p scratch/ui-hud/data && echo '{"ui_mode":"MobileHud"}' > scratch/ui-hud/data/mods-settings.json
cd port
I="1200:tap 165 235;3200:tap 630 467;5200:tap 730 567;7200:tap 730 567;32000:shot ../scratch/ui-hud/a.png;33000:tap 47 455;34500:shot ../scratch/ui-hud/b.png;35000:tap 90 250;36500:shot ../scratch/ui-hud/c.png"
./target/release/s3run --headless --game-dir ../game-files/gog-s3-ultimate --data-dir ../scratch/ui-hud/data --max-seconds 38 --inject "$I"
```

Bar button centres at 640x480: x = 47, 137, 227, 317, 407, 497, 587 (y 455). "Back to HUD" = (575, 27). Random
maps differ per run, so unit/building positions must be read from a screenshot first. Note: switching the UI mode in
the HUD menu saves `mods-settings.json` in the data dir. `S3_HUD_DEBUG=1` logs the landscape buffer widths.

Unit tests (`cargo test -p s3-mods`): build table, slot rectangles, minimap tile math, hit rectangles.

## Open / next

- Runtime F1-F3 switch in HUD mode (black band), see above.
- Touch-context (W3) mod and the HUD both take taps; the HUD must stay on top (it is added after the framework's
  own mods; if another mod is added later it gets events first). Taps on HUD elements are consumed by the HUD.
- Settler type and remaining good names; tooltips for building cards (cost) — needs the panel's goods counters / cost
  tables (not RE'd).
- Building card: more actions (priority/stop = command 0x04, work area = 0x05) once verified.
- ~~Try on the Android emulator~~: done (ux-verify1). Note: map taps in the HUD are handled by touch-context
  (tap = select / move / place); that needed `touch_game::layout` to accept panel width 0. In multiplayer
  `[0x7e3408]` is 0, so `gd()` falls back to `[logic+0x3a0]` (ux-verify1).
- touch.rs' ✕ placement cancel is not drawn in this mode (the banner has Cancel), but its hit area top right is
  still active during placement.
