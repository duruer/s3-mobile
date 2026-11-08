# Game state RE part 2: camera, screen layout, left panel, cursor modes, menus, resources, text (W2)

Target: GOG `S3.EXE` sha256 `00a925cfdcfc3349b5d0f3b8a4c312f01b813e363b4464f6f3d6de17804d950b` (all addresses for this
exe only). Workstream W2 of [mobile-vision.md](../porting/mobile-vision.md). Part 1 (units, buildings, selection,
commands, click dispatch, settler/building/pile tables): [game-state-units.md](game-state-units.md).

Status words: **VERIFIED** = seen at runtime in s3run (2026-11-05, re-ui task); **STATIC** = read from code, not
exercised; **HYPOTHESIS** = a guess. No decompiled code is reproduced here, only addresses, layouts and behaviour.

Names used below:

- `gd` = `CGameData` = `[logic+0x3a0]` (part 1). Also `*(0x7e3408)` (= `*(0x7e7b60)`). `0x18790000` in every s3run run
  so far (deterministic in our emulator; mods should take it from the ctor hook `0x4fc990` instead of hard-coding).
  **In a multiplayer game `*(0x7e3408)` stays 0** (VERIFIED 2026-11-05 ux-verify1, host and joiner); `[logic+0x3a0]` is
  valid there, so mods use it as the fallback (`touch_game::gd`).
- `logic` = `CLogicThread` (part 1). `HandleInput` = `0x524fa0` (thiscall logic), runs on the logic thread.
- `gfx` = the render-side screen object (`new 0x5f700` at `0x42c6b9`, ctor `0x42c820`, vtable `0x656aec`; `0x1011def8`
  in s3run). Owns the string table `[gfx+0x5f6b8]` and the text renderer `[gfx+0x5f6e0]`. Runs on the render thread.
- `snap` = the per-frame UI snapshot that the logic thread fills for the renderer (`[logic+0x3d0]`, function `0x51f110`).

## Summary for hook / HUD authors

| Need | How | Status |
| --- | --- | --- |
| Camera position | `[gd+0x2d25c]` x, `[gd+0x2d260]` y (world pixels, top-left of the map view). Write, then the clamp `0x51b4d0` (thiscall gd) | VERIFIED (poke moves the view) |
| Center camera on a tile | `0x51b560` thiscall gd, `(x, y)` stdcall (`ret 8`); clamps itself | STATIC (used by minimap click, hotkeys) |
| Scroll step | arrow keys: +-40 px (`0x525977..`) | VERIFIED (Up: y `0x5e7 -> 0x5bf`) |
| Screen layout per resolution | globals `0x730f08..0x730f40`, set by `0x4c6d70(resIndex)` (table below) | VERIFIED (all three) |
| Bigger map view ("zoom out") | switch to 800x600 / 1024x768 at runtime: F2 / F3 (F1 = 640x480), or registry `Resolution` 1 / 2; the port runs any size up to 3200x1200 ([resolution-zoom.md](../porting/resolution-zoom.md), section "Resolution internals" below) | VERIFIED |
| Panel page | `[gd+0x2d284]`: 1 build, 2 goods, 3 settlers, 4 building selected, 5 units selected, 7 Save/Quit menu, 8 messages, 9 paused | VERIFIED (1, 2, 3, 7, 9; 4/5 from part 1) |
| Build category / slot / type | `[gd+0x2d288]` cat 1..4, `[gd+0x2d28c]` list slot (0 = none), `[gd+0x2d2f0]` building type | VERIFIED |
| Start build placement for type T | write page 1, cat, slot, type T, then `[gd+0x2d27c] = 1`; the next map left-click places a site | VERIFIED (site count 6 -> 7) |
| Open the in-game menu | page 7, `[gd+0x2d298] = 0`, `[gd+0x2d27c] = 1` (ESC is not a hotkey) | VERIFIED |
| Redraw panel | `[gd+0x2d27c] = 1` ("panel dirty") | VERIFIED |
| Tooltip text id | `[logic+0x54]` (set by the hover pass of HandleInput) -> `snap+0x270` -> drawn by `0x4b3de1..` | STATIC + VERIFIED text ids |
| Text id -> string | `0x5faaa0` thiscall `[gfx+0x5f6b8]`, `(id, lang = -1)`, returns `char*`; id = `table<<16 | index` | VERIFIED (resolver `tools/re-ui/strings.py`) |
| Building names | text `0x80000 + type` (table 8) | VERIFIED |
| Goods in stock | sum of `amount` over the pile table (part 1) by good | VERIFIED (one map, panel = sum) |
| Panel renderer | `0x4b4d00` thiscall gfx, `(snap)`; called from the frame renderer at `0x4b2dc5` | STATIC |
| In-game message lines (tutorial text, chat, notices) | ring `logic+0x414` (5 x 0x1c, UTF-16); drawn by `0x4d82b0` + `0x4d8400` (skip both to hide); tutorial arrow = `[gd+0x2d304] = 1`; section 7 | VERIFIED |
| Safe place to change UI state | a hook at `HandleInput` entry `0x524fa0` (logic thread, before input is processed) | STATIC (s3run pokes between slices also worked) |

## 1. Camera and viewport

### Camera

| Field | Meaning | Status |
| --- | --- | --- |
| `gd+0x2d25c` | camera x, world pixels (left edge of the map view) | VERIFIED (poke moves the view) |
| `gd+0x2d260` | camera y, world pixels (top edge) | VERIFIED |
| `gd+0x2d254`, `gd+0x2d258` | second copy (equal to the above when idle; HYPOTHESIS: the value of the previous frame / scroll origin) | VERIFIED values |
| `gd+0x2d264` | cleared by arrow-key scrolling (HYPOTHESIS: "follow"/scroll-to target active) | STATIC |
| `gd+0x2d2d0` | world size `W` in tiles (384 on a 384 map) | VERIFIED |

- **Clamp** `0x51b4d0` (thiscall gd, no args): `x` into `[0, 24*W + 0x167 - mapW]`, `y` into `[0, 9*W + 0x176 - mapW]`
  where `mapW = [0x730f18]` (yes, the y limit also uses the view width in this build). Call it after writing the camera.
  **Patched 2026-11-07** (gog-base, `0x51b528` `mov edi,[0x730f18]` -> `[0x730f1c]`): with our wide custom
  resolutions (mapW ~1400-2500) the y limit collapsed; on the small tutorial map the camera could not go below
  the upper half (user report). Now `y` into `[0, 9*W + 0x176 - mapH]` (VERIFIED on the phone: tutorial pans to
  the south coast). The host clamp `touch_logic::clamp_camera` uses `map_h` the same way.
- **Center on tile** `0x51b560` (thiscall gd, `(tx, ty)`, `ret 8`): `x = (W + 2*tx + 24 - ty)*8 - mapW/2`,
  `y = 9*ty + 200 - mapH/2 - height(tx,ty)` with `height` = byte `gd+0x67448 + (tx*768+ty)*0x18` (tile +4) and
  `mapH = [0x730f1c]`; then calls the clamp. Callers: minimap click (`0x525438`), `0x4fef2d`, `0x51b6d4`, `0x526a90`,
  `0x52e1ef` (HYPOTHESIS: "go to event" from a message).
- Tile geometry implied by these formulas: a tile is 16 px wide (x step 8 per half tile), rows are 9 px high, rows are
  sheared (x shifts by 8 px per row). The renderer converts camera pixels to a first tile column with `/16` and row
  with `/18` in `0x51f9xx` (STATIC). Exact map -> screen formula for HUD markers: still open (see part 1).
- **Scrolling**: arrow keys (VK 0x25..0x28) change the camera by 40 px and clamp (`0x525977`, `0x5259b1`, `0x5259ea`,
  `0x525a0a`); right-drag scroll is in HandleInput (part 1, `[0x7dfd80]`). Edge scrolling: not looked at.
- Two-finger pan for touch: write `x/y` (`+0x2d25c/+0x2d260`) on the logic thread and call `0x51b4d0`, or keep
  `delta` in host and write absolute values each frame. Centering: call `0x51b560`.

### Screen layout per resolution (`0x4c6d70(resIndex)`, stdcall, `ret 4`)

Panel grid: the left panel is a grid of `cell x cell` pixel cells; `col = (x - [0x730f3c]) / cell`,
`row = (y - [0x730f40]) / cell`, 40 columns (0..0x27) x 106 rows (0..0x69). All panel buttons are defined in grid
units, so **the same (col,row) works at every resolution**.

| Global | Meaning | 640x480 | 800x600 | 1024x768 |
| --- | --- | --- | --- | --- |
| `0x730f08` | resolution index + 1 | 1 | 2 | 3 |
| `0x730f0c` / `0x730f10` | screen width / height | 640 / 480 | 800 / 600 | 1024 / 768 |
| `0x730f24` | panel width (x < this = panel or minimap) | 136 | 176 | 216 |
| `0x730f18` / `0x730f1c` | map view width / height (`= screen - panel`, full height) | 504 / 480 | 624 / 600 | 808 / 768 |
| `0x730f2c` / `0x730f30` | minimap origin x / y | 5 / 10 | 8 / 5 | 6 / 8 |
| `0x730f28` | minimap width (in pixels; the map is drawn as a sheared parallelogram) | 128 | 160 | 208 |
| `0x730f34` | minimap bottom y | 142 | 170 | 224 |
| `0x730f38` | panel grid cell size (px) | 3 | 4 | 5 |
| `0x730f3c` / `0x730f40` | panel grid origin x / y | 8 / 152 | 8 / 171 | 8 / 228 |
| `0x730f14` | 16 (HYPOTHESIS: tile width) | 16 | 16 | 16 |
| `0x730f20` | 437 / 540 / 697 (HYPOTHESIS: top of the bottom button bar region) | 437 | 540 | 697 |

Values VERIFIED with `peek 0x730f08` at 640x480, 800x600 (`Resolution=1`) and 1024x768 (F3). `[0x7dfd58]` is cleared
by the same function.

- **Minimap hit test** (HandleInput `0x52537b..`): `y < [0x730f34]` and
  `[0x730f2c] + d < x < [0x730f24] + d` with `d = ([0x730f34] - y)/2` (sheared). Tile:
  `ty = W*(y - [0x730f30]) / [0x730f28]`, `tx = W*((y - [0x730f30])/2 - [0x730f28]/2 - [0x730f2c] + x) / [0x730f28]`,
  then `0x51b560(tx, ty)` (STATIC).
- **Minimap image and drawing** (REA static + VERIFIED in s3run 2026-11-05, hud-polish): the minimap is a square
  RGB565 image of `mw x mw` pixels (`mw = [0x730f28]`) at `[snap+0x4c]` (`snap` = render snapshot, arg of the frame
  renderer `0x4b26c0` and of the panel renderer `0x4b4d00`); column `c` / row `r` = tile `(c*W/mw, r*W/mw)`. The
  frame renderer copies it into the back buffer `[gfx+0xec]` (pitch `[gfx+0x90]` bytes) sheared:
  `x = mx + (mw - r - 1)/2 + c`, `y = my + r`. Block `0x4b2b34..0x4b2ce5`: incremental copy in 64 interleaved chunks
  (per-chunk counters `0x7ad4f8[64]`, "dissolve"; full copy when `[snap+0x3c] == 1`). Block `0x4b366a..0x4b3cf3`:
  copies the minimap part at `x >= [0x730f24]` (over the map) every frame, draws the frame art (gfx resources
  `0x40001` at `(panelW, 0)`, `0x40003` at `(panelW, [0x730f34])`), then erases the previous camera rectangle
  (`gfx+0x5f6a4..b0`, from the image) and draws the new one in white (`0xffff`) and stores it (`gfx+0x5f694..a0`).
  Camera rectangle in image coordinates (`0x4b3a3c`): `k = (mw << 8) / [snap+0x2c]`,
  `x = ([snap+0x24] + 11)*k >> 8`, `y = ([snap+0x28] + 16)*k >> 8`, `w = [snap+0x30]*k >> 8`,
  `h = ([snap+0x34] - 40)*k >> 8` (signed, truncating). After `0x4b3cf3`: the drag-selection box (`0x7dfd34`,
  `0x7dfb98..`). Skipping both blocks (jump) hides the minimap without side effects (Mobile HUD, ui-hud.md).
- **Map rect**: `x >= [0x730f24]` (the minimap's sheared right edge overlaps the map at the top). The map is rendered
  under the panel frame; the view origin is the camera.
- **Other resolutions**: `0x4c6d70` only knows indexes 0..2 (other values leave the table unchanged) and the panel art
  exists in three variants (resource `0x40000`, variant `[0x730f08]-1`, `0x4b4dfb`). Registry `Resolution` (part of
  `HKLM\Software\BlueByte\Siedler3\1.0\General`, see [registry.md](registry.md)) selects 0/1/2 at start; values 1 and 2
  VERIFIED. A custom resolution (e.g. 1280x720) would need: our DirectDraw mode, a patched `0x4c6d70` case (map view
  sizes are used generically via `0x730f18/0x730f1c`) and reuse of variant 2 panel art. HYPOTHESIS, untested.
- **Runtime switch** (VERIFIED in s3run): F1/F2/F3 (VK 0x70..0x72, handlers `0x525a77`, `0x525b36`, `0x525bfb`) write
  the wanted index to `[0x7a7af0+0x70]` (setter `0x49ded0`) and signal the render thread; the game re-runs
  `SetDisplayMode` and the layout. Ignored when `[0x7dfd44] != 0` (HYPOTHESIS: multiplayer/replay) or already active.
  This is the cheapest "zoom out / more map" for mobile: switch to 1024x768 and let our presenter scale the frame.

## 2. The left panel

### Page state (all in `gd`, copied every frame to the render snapshot by `0x51f110` at `0x51f5ff..`)

| Field | snap | Meaning | Status |
| --- | --- | --- | --- |
| `+0x2d284` | `+0x60` | page: 0 none, 1 build, 2 goods, 3 settlers, 4 building selected, 5 units selected, 6 (TAB key; HYPOTHESIS: next unit group), 7 Save/Quit menu, 8 messages, 9 game paused, 0xb (`G` key, unknown) | VERIFIED 1,2,3,7,9; 4,5 part 1 |
| `+0x2d288` | `+0x64` | build category 1..4 (page 1) | VERIFIED |
| `+0x2d28c` | `+0x68` | build list slot 1..12, 0 = nothing selected (cleared after a site is placed and by right-click) | VERIFIED |
| `+0x2d290` | `+0x6c` | goods page tab 1..4 | VERIFIED (1) |
| `+0x2d294` | `+0x70` | goods page sub-selection (cleared when tab 2 is chosen) | STATIC |
| `+0x2d298` | `+0x74` | menu sub-page (page 7: 0 main, 1 quit confirmation, 2 save slot list) / building page action (page 4, part 1) | STATIC (0 VERIFIED) |
| `+0x2d29c`, `+0x2d2a4`, `+0x2d2a8` | `+0x78`, `+0x80`, `+0x84` | further page state (not analysed) | |
| `+0x2d2a0` | `+0x7c` | settlers page tab 1..4 | VERIFIED (1) |
| `+0x2d27c` | – | panel dirty: set to 1 after every change | VERIFIED (needed after a poke) |
| `+0x2d280` | – | set together with settlers tab 4 (HYPOTHESIS: statistics refresh) | STATIC |
| `+0x2d2f0` | – | building type to place | VERIFIED |
| `+0x2d320 + n*0x4b0` (n = 0..3) | `+0x88` bits | message categories (battle, general, resources, trade): a bottom button works only when its slot is non-zero; bit n of `snap+0x88` = "has messages", blinking with bit 5 of the step counter | STATIC |

Each click handler also plays the click sound (`0x53f560(0x3d)`, thiscall logic) and calls `0x542630` (thiscall logic;
HYPOTHESIS: refresh/notify). After choosing a building type it calls `0x4ff3f0` (thiscall gd; HYPOTHESIS: recompute the
placement ghost map). Poking without these calls still showed the placement ghosts and placed the site (VERIFIED), so
a HUD may poke + set the dirty flag; calling the same functions from a logic-thread hook is the cleaner option.

### Click handling

All panel clicks are handled inside `HandleInput` (`0x524fa0`), left-down branch at `0x526aaa`: if `x < [0x730f24]`
the grid cell is computed and a long chain of `(col range, row range)` tests follows (`0x526aaa..0x532fa2`). Page
specific blocks start with `cmp [gd+0x2d284], page`: build list `0x526f8a..0x528a37`, goods `0x528a38`, settlers
`0x52c237`, building selected (4) `0x52xxxx/0x53xxxx` (part 1), units (5) `0x52ec5a`, menu (7) `0x52e21a`,
messages (8) `0x52e1b0`, `0xb` `0x532f4f`. `tools/re-ui/panelmap.py` lists every state write with the preceding
column/row tests (heuristic, linear).

The **same code is also the tooltip query**: `HandleInput` counts calls without input in `[logic+0x74]`; on the 10th
idle call it sets `[logic+0x78] = 3` and copies the mouse to `[logic+0x7c]/[logic+0x80]`; with `[logic+0x78] != 0`
every button block stores its tooltip text id in `[logic+0x54]` instead of acting (STATIC). `[logic+0x78]`: 0 = an
input event is being handled, 2 = queue empty, 3 = tooltip pass (idle count == 10), 1 = idle count > 10. So a tooltip appears
when the pointer rests ~10 logic ticks on a button. For touch, a host "hover" (mouse move without button) shows the
same tooltip.

Grid map of the common buttons (all VERIFIED by tooltip text ids and clicks at 640x480; pixel = origin + grid*cell):

| Button | cols | rows | Action | Tooltip id |
| --- | --- | --- | --- | --- |
| Build tab | 2..13 | 1..8 | page 1, slot 0 | `0x8004a` "Buildings" |
| Goods tab | 15..26 | 1..8 | page 2 | `0x8004b` "Goods" |
| Settlers tab | 28..39 | 1..8 | page 3 | `0x8004c` "Settlers" |
| Category 1..4 (page 1) | 0..9, 10..19, 20..29, 30..39 | 11..18 | cat = 1..4, slot 0 | `0x8004d..0x80050` Resource / Food / Military / Misc. Buildings |
| Goods tabs 1..4 (page 2) | 0..9, 10..19, 20..29, 30..39 | 11..18 | `+0x2d290` = 1..4 | `0x80051` Inventory, `0x80052` Tools and Weapons, `0x80081` Priority of Buildings, `0x80082` Priority of Goods |
| Settlers tabs 1..4 (page 3) | same | 11..18 | `+0x2d2a0` = 1..4 | `0x8005a` Population, `0x8005b` Occupations, `0x8005c` Recruitment, `0x80155` Production statistics |
| Build list slot `2k+1` / `2k+2` (k = 0..5) | 1..19 / 21..39 | `22+12k .. 31+12k` | slot, type (table below) | `0x80000 + type` |
| Menu (computer icon) | 0..7 | 97..103 | page 7, sub-page 0 | `0x8008f` "Save/Quit" |
| Messages 1..4 | 8..15, 16..23, 24..31, 32..39 | 97..103 | page 8, `0x4fee90(n)` (thiscall gd) | `0x800ad` Battle, `0x800ae` General, `0x800af` Resources, `0x800b0` Trade Messages |
| Menu: "Save Game" (page 7, sub 0) | 3..37 | 22..29 | opens the save slot list (`0x50b670` enumerates `slot%2i.sav`; HYPOTHESIS: sub-page 2) | – |
| Menu: "Quit Game" (page 7, sub 0) | 3..37 | 87..91 | sub-page 1 (confirmation) | – |

Recipe from [integration-log.md](../porting/integration-log.md) (640x480 pixels): menu icon `21 452`, "Save Game"
`68 225`, slot 1 `40 244` (slots 21 px apart), "Quit Game" `68 420`, YES `38 420`.

### Build list: category / slot -> building type (STATIC table from `0x526f8a..0x528a37`, names from text table 8)

Race = `[gd+0x11943e0 + player*0x148]` (player record +0). Race names are a HYPOTHESIS from the race-specific drinks:
0 Romans (Winery), 1 Egyptians (Brewery), 2 Asians (Distillery, Rice Farm), 3 Amazons (Mead). A slot with no entry for
a race is empty for that race.

| Cat | Slot | Type (race) | Name |
| --- | --- | --- | --- |
| 1 Resource | 1 / 2 / 3 / 4 | 0x02 / 0x04 / 0x03 / 0x05 | Woodcutter's Hut / Sawmill / Stonecutter's Hut / Forester's Hut |
| 1 | 5 / 6 / 7 / 8 | 0x09 / 0x0b / 0x08 / 0x0a | Iron Ore Mine / Iron Smelter / Gold Mine / Gold Smelter |
| 1 | 9 / 10 | 0x07 / 0x0c | Coal Mine / Tool Smith |
| 1 | 11 | 0x27 (0), 0x25 (1), 0x1d (2), 0x38 (3) | Charcoal Maker's Hut / Gem Mine / Sulfur Mine / Sulfur Mine (Amazon type) |
| 1 | 12 | 0x28 (2), 0x25 (3) | Gunpowder Maker / Gem Mine |
| 2 Food | 1..4 | 0x19 / 0x18 / 0x17 / 0x11 | Fisherman's Hut / Grain Farm / Pig Farm / Grain Mill |
| 2 | 5..8 | 0x15 / 0x14 / 0x1e / 0x31 | Slaughterhouse / Bakery / Waterworks / Donkey Farm |
| 2 | 9 | 0x0e (0), 0x26 (1), 0x16 (2), 0x33 (3) | Winery / Brewery / Distillery / Beekeeper's Hut (text `0x8015f`) |
| 2 | 10 | 0x24 (2), 0x34 (3) | Rice Farm / Meadmaker's Hut (text `0x80160`) |
| 3 Military | 1..5 | 0x0f / 0x10 / 0x12 / 0x06 / 0x0d | Guard Tower / Large Guard Tower / Castle / Lookout Tower / Weapon Smith |
| 3 | 6 | 0x1f (0), 0x2f (1), 0x30 (2), 0x32 (3) | Catapult / Ballista / Cannon / Gong Hall (text `0x8015e`) |
| 3 | 7 / 8 / 9 | 0x13 / 0x20 / 0x23 | Barracks / Shipyard / Healer's Quarters |
| 4 Misc. | 1..3 | 0x1a / 0x1b / 0x1c | Small / Medium / Large Residence |
| 4 | 4 / 5 / 6 | 0x01 / 0x22 / 0x21 | Storage Area / Marketplace / Landing Dock |
| 4 | 7 | 0x2c (0), 0x2a (1), 0x2e (2), 0x36 (3) | Small Temple / Sphinx / Small Temple / Small Temple |
| 4 | 8 | 0x2b (0), 0x29 (1), 0x2d (2), 0x37 (3) | Large Temple / Pyramid / Large Temple / Large Temple |
| 4 | 9 | 0x35 (3) | Alchemist's Den (text `0x80161`) |

Text `0x80000 + type` gives the name for types < 0x33 (`0x80000` "Building Site", `0x80001` "Storage Area", ...
`0x80032` "Magic Eye"); the Amazon types 0x33..0x38 use the text ids shown. Text ids `0x80033..` are spell names.
VERIFIED at runtime: type 2 (Woodcutter's Hut) placed via poke; `0x8000f` = "Guard Tower" = type 15 (start building,
part 1).

### Programmatic tab switch / building selection (for the Mobile HUD)

VERIFIED sequence (s3run pokes, `scratch/re-ui/t1`): `[gd+0x2d284]=1, [gd+0x2d288]=cat, [gd+0x2d28c]=slot,
[gd+0x2d2f0]=type, [gd+0x2d27c]=1` -> the panel shows the build list with the slot highlighted and the map shows the
placement ghosts; the next left-click on the map places the construction site (building count `[gd+0x1499e60]` 6 -> 7)
and resets the slot to 0 (page and category stay). Alternatives:

- Send command 1 directly (part 1) after checking `AddBuilding(x,y,type,player,0)`; no UI state needed.
- **Hotkeys** (key events through our USER32 input; table `0x534cbc`/`0x534c00`, dispatch `0x5254a7`):
  Q/W/E/R = page 1 category 1..4 (VERIFIED: W -> page 1 cat 2), A/S/D/F = goods tabs (STATIC), Y(Z)/X/C/V = settlers
  tabs (STATIC), Space = page 8 messages, P = pause toggle (page 9 "Game Paused", VERIFIED; other hotkeys are ignored
  while paused), TAB = page 6 / selection cycling (`[0x7dfd68]`, HYPOTHESIS), G = page 0xb, arrows = scroll 40 px,
  Home (0x24) = `0x51b6b0` (HYPOTHESIS: center on castle), F1..F3 = resolution, F5/F6/F12, `+`/`-` (0x6b/0x6d),
  Enter (chat, `0x5254ae`), 1..9 (`0x542820`, HYPOTHESIS: unit groups or speed), T/B/O/I/U/J/K toggle globals
  `0x7deb54`, `0x7deb49`, `0x7dfdac`, `0x7dfda8` (debug/display options, HYPOTHESIS). Hotkeys are skipped while
  `[0x7dfda8] != 0` (`0x52548d`, chat input HYPOTHESIS).

### Where the panel is drawn (for UI mode B)

- Frame render (render thread) `0x4b2900..`: the text DIB / landscape / then `0x4b4d00` (call at `0x4b2dc5`) draws the
  panel: background art from gfx resource `0x40000` variant by resolution (`0x5faaf0` lookup at `0x4b4df6`), then a
  per-page block (`[snap+0x60]` checks at `0x4b53b5` (7), `0x4b5449` (2), ...), sprites via `0x5faca0`/`0x5fdab0`/
  `0x5f64c0` (HYPOTHESIS: get sprite / blit / blit transparent).
- Hiding the panel: skip `0x4b4d00` (hook + `return_from_function(0, 1)`) and crop the presented frame to
  `x >= [0x730f24]` in our presenter. The map is not rendered under the panel (HYPOTHESIS), so skipping the panel alone
  leaves stale pixels; cropping is the safe option. The minimap is part of the same top-left art (drawn in the frame
  render, `0x4b2b58..` uses `0x730f28/2c/30`).
- Input: with the panel hidden, HUD buttons must change state by pokes/commands/hotkeys (above); map clicks keep working
  with the original coordinates (map x starts at `[0x730f24]`).

## 3. Cursor / interaction mode

There is no separate cursor-mode variable; the mode is derived from the panel page and selections:

| Mode | Condition | Effect of left click on the map | Status |
| --- | --- | --- | --- |
| Normal | page 0/2/3/7/8 or page 1 with slot 0 | select (drag box / single) | VERIFIED |
| Build placement of type T | page 1, `[gd+0x2d28c] != 0`, `[gd+0x2d2f0] = T` | place a site (command 1), slot -> 0; right-click cancels (`[gd+0x2d28c]=0`) | VERIFIED |
| Building selected | page 4, `[0x7dfd6c]` = building id | building panel actions; right-click deselects | part 1 |
| Building target / work area | page 4, `[gd+0x2d298] = 3` | the next click sets the target tile (command 5) | STATIC (part 1) |
| Units selected | page 5, UI selection list non-empty | right-click (quick!) = move/attack (command 9, modifiers 0xd..0xf) | VERIFIED (part 1) |
| Paused | page 9 | input ignored | VERIFIED |
| Hover/tooltip pass | `[logic+0x78] != 0` | none; tooltip id set | STATIC |

Attack is not a mode: a move command onto an enemy target attacks (part 1). The mouse cursor image is chosen by our
cursor composer (see [user32-gdi32.md](../porting/user32-gdi32.md)); the game calls `SetCursor` with resources.

## 4. Dialogs and menus

- In game there are no Windows dialogs: Save/Quit is panel page 7 (`0x52e21a`), drawn by `0x4b4d00`.
  Sub-pages (`[gd+0x2d298]`): 0 = "Save Game" / "Quit Game" (VERIFIED, `scratch/re-ui/m1.png`), 1 = quit
  confirmation YES/NO, 2 = save slot list (8 slots, render loop at `0x51f1dd..`; STATIC). The save itself is a system
  command (0xf1..0xff family, part 1); a save writes `Save/slotNN.sav` without a name prompt.
- **Open the menu programmatically** (ESC is not in the hotkey table): page 7 + sub-page 0 + dirty (VERIFIED).
  Then "Save Game"/"Quit Game" are grid clicks (table above) or the integration-log pixel recipe.
- After "Quit Game -> YES" the game shows the statistics screen and then the 800x600 main menu (main menu, load list,
  options are full-screen 800x600 screens of the app's menu system, not the panel; not analysed here).
- Pause: P toggles page 9 (`0x52571b`).
- Message pages: page 8 with category `0x4fee90(n)`, n = 1..4.

## 5. Player resources and statistics (HUD)

- **Goods in stock**: the pile table (`gd+0x123885c`, 0xe bytes, count `[gd+0x1238858]`, part 1). Layout seen:
  `+0 u8 good`, `+1 u8 amount (<= 8)`, `+2/+3 u8` (HYPOTHESIS: reserved/incoming), `+4 u8 0xfe` (stored pile) or `0xfd`
  (HYPOTHESIS: free/transport pile), `+6 u16 x`, `+8 u16 y`, `+0xa/+0xc u16 next/prev pile of the same good`.
  The "Storage Area" inventory equals the sum of `amount` per good over the player's piles (VERIFIED on one map: planks
  8+8+8+1 = 25, stone 25, good 0xc 8+2 = 10, 0xd 8+7 = 15, ...). Which piles belong to which player: by tile ownership
  / economy sector (HYPOTHESIS; the panel probably sums piles of the local player's sectors). Good id -> name: text
  table not mapped yet (HYPOTHESIS: another table of the same string file; use `strings.py all:<t>`).
- **Settlers**: count settler table entries (part 1) by owner and type; the Population page shows the same split
  (carriers 32 at start = type 0 count, VERIFIED in part 1).
- **Economy sectors** `gd+0xe145e8` (0x2e0 bytes, count `[gd+0xe115dc]`): distribution/priority settings, not stock
  counts (VERIFIED: no inventory numbers there).
- Statistics pages (settlers tab 4 "Production statistics", end-of-game statistics): not analysed.

## 6. Text and tooltip rendering

- **Strings**: `0x5faaa0` thiscall `strtab = [gfx+0x5f6b8]`, `(id, lang)`; `lang = -1` -> `[strtab+0xc40]` (1 in our
  runs = English). `table = id >> 16`, header `[strtab+0x37c + table*4]` with `u16 count` at +8 and offsets at
  `+0xc + (count*lang + index)*4`, base `[strtab+0x264 + table*4]`. The tables live in memory-mapped text files.
  Table 8 = UI texts (building names `0x80000+type`, tooltips `0x8004a..`, messages). A hook here translates or
  replaces any UI text (localisation, larger HUD labels).
- **Panel text drawing**: `0x4a87d0` (cdecl, printf-style; args `(textObj=[gfx+0x5f6e0], out, 1, RECT*, flags, fmt,
  ...)`, uses `_vsnprintf` `0x621408`) renders into a 640x480 1-bpp DIB (`CreateDIBSection` at `0x4a7b42`) with
  `DrawTextA` (`0x4a8725`); the DIB is then composed onto the frame. Font: `CreateFontIndirectA` `0x4a8028`.
  Host-side alternative: our `DrawTextA` HLE (`s3-win32/src/gdi32.rs`) sees every string with its rectangle.
- **Tooltip**: `[logic+0x54]` (text id, `-1` = none) and the mouse position `[logic+0x7c]/[logic+0x80]` are copied to
  `snap+0x270/+0x27c/+0x280` by `0x51f110` and cleared; the renderer (`0x4b3de1..0x4b3f4b`) builds a box of
  180x20 px around the pointer (`x-90..x+90`, `y-15..y+5`, clamped to the screen), saves the background (`0x4d65e0`),
  draws the string with `0x4a87d0` (flags 9) and restores (`0x4d6740`). Hook `0x4b3de1` region or read `snap+0x270` to
  draw our own tooltip.
- Other text users: `0x41xxxx..0x42xxxx` (TextOutA/DrawTextA; main menu screens, HYPOTHESIS), `0x5a1970..` (TextOutA +
  `CreateFontA`; HYPOTHESIS: debug/statistics overlay).

## Tools (re-ui)

- `s3run --inject` debug commands (`port/crates/s3-desktop/src/debug.rs`): `dump/peek/poke/regs/watch/dumpall DIR
  [mapped]` (`mapped` also writes memory-mapped file views, needed for the string tables; output is game data, keep it
  in `scratch/`). Plus re-units' `xpeek/xdump/scan`.
- `tools/re-ui/run.sh MAXSEC "inject"` starts a random single-player map (in game after ~17-45 s).
- `tools/re-ui/panelmap.py ASM START END` (panel button table), `tools/re-ui/strings.py DUMPDIR GFXOBJ ID...|all:T`
  (text ids; find `GFXOBJ` with `scan 0x656aec`), `tools/re-ui/scan.py`, `diff.py`, `peek.py` (snapshot diffs),
  `findstock*.py` (counter search), `dis.sh ADDR [N]` (objdump listing `scratch/re-ui/s3.asm`, local only).

## Open questions

- Good ids -> names; which piles count for a player; Tools/Weapons and priority pages (commands 0x07..0x11, part 1).
- Pages 6 and 0xb, `gd+0x2d29c..0x2d2a8`, number keys `0x542820`.
- Map -> screen formula (HUD markers), edge scrolling.
- Whether skipping `0x4b4d00` leaves stale pixels (test with a mods hook).

## Additions from the Mobile HUD work (ui-hud, 2026-11-05)

Details and evidence: [ui-hud.md](../porting/ui-hud.md).

- Programmatic building selection also needs `gd::0x4ff3f0` (thiscall gd, no args; bumps the placement-marker
  generation `gd+0x2d300`) after the pokes, else no placement markers are drawn and map clicks do not place (VERIFIED;
  the earlier poke test had markers from a previous real click).
- The panel is only redrawn when `[gd+0x2d27c]` is set (VERIFIED). The panel renderer `0x4b4d00` does not read
  `[0x730f24]`; it draws at fixed positions.
- Landscape ring buffer: object `[gfx+0x5f6cc]` (vtable `0x65a520`), width `+0x50` = map view width + `[0x730f44]`
  (32 in s3run), set by `0x54bec0` from the display setup `0x42ef..`; blitter `0x54c9d0` wraps at that width. A wider
  map view therefore needs the layout change before the display setup.
- Full-screen map works with `[0x730f24] = 0`, `[0x730f18] = screen width` set at `0x4c6d70` return plus a
  `ScreenToMap` x correction (VERIFIED at 640x480 and 800x600 from start; runtime F1-F3 switch leaves a black band, open).
- Goods: the panel's Inventory numbers are separate counters, not a live sum of piles (VERIFIED by poking piles).
  Piles of the local player = piles on tiles owned by the player (tile +8). Good ids, see ui-hud.md.
- Command `0x06` (destroy building) VERIFIED through `Queue::Post 0x4309c0` from a HandleInput-entry hook.

## Resolution internals (res-zoom, 2026-11-05)

Details and how the port uses them: [resolution-zoom.md](../porting/resolution-zoom.md). All VERIFIED in s3run
unless marked STATIC.

- `[0x7dfd44]` = resolution request: F1-F3 handlers (`0x525a77`, `0x525b36`, `0x525bfb`) write 1..3, call the setter
  `0x49ded0` and `ReleaseSemaphore([[0x7a800c]+8], 1, 0)` (`0x525b5b`). Copied to `snap+0xa0` by the snapshot fill
  (`0x51f4f5`). `0x4c7490(snap)` (render thread, from `0x4b26c0`) switches for 1..3 (inline layout table, the 640 case
  uses minimap x 4 instead of 5), calls `gfx::0x42ce90(w, h, 16, 0, 0)`, sets `0xfa`; returns al=1 (frame skipped)
  for 1..3 / `0xfa`. The logic thread (`0x53db30`) on `0xfa`: waits on the semaphore `[0x7a800c]` (`0x53db80`,
  timeout `0x40d0e0(7)`), rebuilds the minimap image (`gd::0x4fb650` -> `gd+0x1223656`, `[0x730f28]^2` pixels),
  `gd+0x2d264 = 0`, `gd+0x2d254/58 = 10 000 000` (full landscape redraw), `gd+0x2d278 = gd+0x2d27c = 1`, clamp
  `0x51b4d0`, `[0x7dfd44] = 0`. While `[0x7dfd44] != 0` the simulation step is skipped (`0x53dd5c`). Without the
  semaphore token the logic thread blocks forever (VERIFIED by calling the switch without it).
- `0x42ce90` (thiscall gfx): index = 2 for w 1024, 1 for 800, else 0; if the current mode differs,
  `SendMessage(hwnd, 0x804, index)`; window proc `0x45cc9c` -> `0x405c80(index)` -> `IDirectDraw2::SetDisplayMode`
  with constants (calls at `0x405cc3` index 1, `0x405cf2` index 0/2). The display setup `0x42e430` (vtable, no direct
  callers) then runs again: map rect `gfx+0x5f6e8..+0x5f6f4` from the layout, new landscape surface via `0x54bec0(mapW
  + [0x730f44], mapH + [0x730f48], surf, rect)` (`[0x730f44]` = 32, `[0x730f48]` = 102; the object stores the rect
  at `+0x58/+0x5c` size and `+0x74..+0x80`), text DIB `0x4a78f0(screenW, screenH)` (1 bpp, `CreateDIBSection`).
- Initial layout: `0x4c6d40` (from the gfx ctor `0x42c990`) -> `0x4c6d70(registry index)`; game start sets the mode
  through `0x42ca70` -> `0x42ce90([0x730f0c], [0x730f10])`.
- **Render snapshot** `[logic+0x3d0]`: `new(0xc7720)` + ctor `0x523480` (lazy, in `0x51f110`); grid of 130 x 130
  tiles at `+0x500` (0x30 bytes each, index `row + col*130`, filled at `0x522410..`, out-of-map tiles `0x522f60`),
  view size `+0x30` cols = `(mapW+23)/16 + 10`, `+0x34` rows = `(mapH+255)/9 + 11`, camera `+4/+8`, previous camera
  `+0xc/+0x10`; behind the grid: 16 x 16-byte array `+0xc65c0`, strings `+0xc66c0..`, 20 x 0x1c `+0xc66d8`, 100 x
  0x24 `+0xc690c`, count `+0xc771c`. Rows are sheared: column index up to `cols + rows/2`; 1024x768 uses 126 of 130
  columns (with neighbour reads). In placement mode the fill also writes marker bytes into visible `gd` tiles
  (`0x5223b2`, tile `+8`).
- Fixed caps that do not limit bigger views: hit list 10000 entries (`0x4d8240`), sprite lists 4000 / 8000 entries
  in 250 row buckets (`0x4d7c90`, `0x4d7e10`).
- `0x5158f0` (thiscall gd): tile at the view centre from camera and `0x730f18/1c` (STATIC, callers not checked).
- `0x4d82b0`: bottom-left message lines, positioned from `[0x730f38]` and `[0x730f10]`; details in section 7.

## 7. In-game messages (re-messages, 2026-11-08)

Goal: draw the game's text messages at a constant on-screen size in a window-resolution overlay (the port changes the
render size for zoom, so everything drawn into the game frame grows and shrinks). VERIFIED = seen in s3run on
2026-11-08 (tutorial at 800x600; recipe and evidence at the end), STATIC = read from code only.

### What is drawn as text over the map

| Text | Source | Drawn by (render thread, inside the frame renderer `0x4b26c0`) | Clickable |
| --- | --- | --- | --- |
| **Message lines** bottom-left (tutorial text, chat, desync / CD / ladder notices, "game already saved", ...) | ring in `logic` (below) | text into the text DIB: `0x4d82b0` (call `0x4b2833`); icons + composite: `0x4d8400` (call `0x4b422f`) | only the tutorial "arrow" corner |
| **Chat input line** (multiplayer, Enter) | `logic+0x3f8` | the same two functions | no (keyboard) |
| Tooltip | `logic+0x54` -> `snap+0x270` | `0x4b3de1..0x4b3f4b` (section 6) | no |
| Game clock `hh:mm:ss` while `[0x7deb54]` is set (T key) | `[0x7dfd48]` (ms) | `0x4b3f95..0x4b412d`, rect `(W-80, H-20, W, H)`, font 1 | no |
| End banner "You've Lost." `0x80113` / "You've won." `0x80115` / "The game ends as a draw." `0x8015c` | `[0x730f64]` = 0 / 1 / 2 (`-1` = game running) | `0x4b412d..0x4b422c`, rect `(W-[0x730f20], 20, W, 47)` | no |
| Panel info text (`snap+0x274` text id, printf args `+0x284..+0x290`, from `logic+0x50..0x64`) | panel rect `gfx+0x5f654..0x5f660` (set at `0x4c73da`) | `0x4b2838..0x4b2b13` | panel |

All of them use the game's text renderer `[gfx+0x5f6e0]`: GDI `DrawTextA` into a 1-bpp DIB the size of the screen
(created by `0x4a78f0`). `0x4d65e0(x, y, w, h)` clears a rectangle of that DIB, `0x4d6740(x, y, w, h)` copies the set
pixels of a rectangle onto the frame (per-run blitter `0x4d6950`); both thiscall `gfx`, `ret 0x10` (STATIC). Fonts are
created per resolution in `0x4c6fd0..0x4c73b6` (`0x4a7df0`); font 0 has height `5 * cell` (STATIC).

### Message ring (logic thread)

All in `logic` (`CLogicThread`, `[0x700ffee0]` in s3run; part 1). VERIFIED by `xdump 0x700ffee0@0x414 0x8c` +
`tools/re-messages/slots.py`, matching the screenshots.

| Field | Meaning |
| --- | --- |
| `logic+0x414 + i*0x1c`, i = 0..19 | entries; only `i < [logic+0x644]` are used, and `[logic+0x644] = 5` (set once at `0x53cade`) |
| entry `+0x00` (16 B) | string object: `+4` pointer to **UTF-16** text, `+8` length in chars, `+0xc` capacity (reference count byte at `ptr-1`; the game's wide string class) |
| entry `+0x10` | timer in logic ticks; the line is shown while it is `!= 0`. 800 for normal lines, 1 000 000 for icon `>= 0xfa` (tutorial: "forever") |
| entry `+0x14` | icon: `< 0x14` = player colour mark (locally generated lines pass `word [0x7ace64 + player*2]`, = 1 for player 0), `0xfd` = info "i" (gfx `0x10088`), `0xfe` = second icon (gfx `0x10089`), `0xfc` = tutorial **arrow** (gfx `0x1008b`), `0xff` = none |
| entry `+0x18` | dirty byte (text must be redrawn into the DIB) |
| `logic+0x648` | next write index; it **decreases** (`(idx + n - 1) % n`): a new line goes one slot up until it wraps |
| `logic+0x3f8` (16 B) | chat input line (UTF-16; starts with the player name and ": ", prefix length `logic+0x40c`), `+0x400` length (max 0x50), `+0x408` chat open, `+0x410` dirty |

Functions (all thiscall `logic`, logic thread; STATIC unless marked):

- `0x51e440` **AddMessage(string by value (16 B), icon)**, `ret 0x14`: assigns the text to slot `[logic+0x648]`, timer
  800 (1 000 000 if icon `>= 0xfa`), icon, dirty; moves the index down; for icon `< 0x14` it also clears the timer of
  the next slot (one free line between chat lines). Plays sound `0x3d`. 40 call sites: chat echo `0x5256e5`,
  "[Server]: " `0x541051`, text ids `0x8012c` "Ladder games cannot be saved.", `0x8012d` "The game has already been
  saved within the last minute.", `0x80152` "This is the last tower", `0x80174` "You cannot steal goods from an ally.",
  hard-coded desync / CD / ladder / lobby strings (`0x53a2f6..0x53b7ec`, `0x53d8a5..0x53d99a`, `0x540801..0x541333`).
- `0x51e650` ClearMessages: all timers 0, all dirty, index = n-1 (top slot).
- `0x51e680` Tick: every timer `> 0` is decremented and marked dirty; called once per logic loop at `0x53e6d5`
  (VERIFIED rate: ~74 per second in s3run, so a normal line lives ~11 s).
- `0x51e2e0` chat editor (adds a key to `logic+0x3f8`). The Enter handler `0x5254ae` opens the chat **only in a
  network game** (`0x4318d0` on `0x7a7a18`; VERIFIED: Enter + typing in single player leaves the ring and the chat
  line empty).

**Tutorial text** (VERIFIED): the tutorial (`[0x7dfdb0] == 0x63`) is a state machine in `gd`: step `[gd+0x2d308]`,
wait counter `[gd+0x2d304]`. `0x53fbb0` (called every logic loop at `0x53e629` in the tutorial) checks the condition
of the current step; for "press the arrow" steps it is `0x515b00` (decrement `gd+0x2d304`, true at 0). On success
`0x53f950` looks the step up in the table `0x72faf0` (16-byte entries `{step, next step, icon, text id}`, first entry
`{0, 1, 0xfd, 0x70090}`), sets the next step, resets the wait counter to 100 000 (`0x515af0`), loads the text
(`0x5faaa0` on `[0x7a8010]`, table 7) and calls `0x540400(text, icon)`: **ClearMessages**, then split the text into
chunks of at most 75 chars at a space (no pixel measuring), at most 4 lines, each through `AddMessage` from the top
slot down. Icons: first chunk = the step icon, middle chunks `0xff`; if the step icon is `0xfd` the **last** chunk
gets `0xfc` (the arrow; a one-line text gets only the arrow, no "i"). Step icon `0xfe` = no arrow (the step waits for
a game action). At 800x600 the first line is clipped at the right edge ("...boot camp. Pl" / "the arrow button to
continue.") because 75 chars do not fit the 580 px line rectangle.

### Per-frame copy and drawing

The snapshot fill `0x51f110` (logic thread) copies every frame: chat line `logic+0x3f8 -> snap+0xc66c0` (`0x523a40`),
chat dirty `logic+0x410 -> snap+0xc66d4` (then clears it), `snap+0xc66d0` = chat icon choice (0/1; HYPOTHESIS: team vs
all), and the 20 entries `logic+0x414.. -> snap+0xc66d8 + i*0x1c` (`0x523330` copies string, timer, icon, dirty),
clearing each `logic` dirty byte. **In the port the snapshot tail is moved** by the res patch (`s3-mods/src/res.rs`
`snap_delta()`, `SNAP_TAIL_SITES`): use `snap + 0xc66d8 + snap_delta()` etc., or read `logic` directly.

Render thread (`ecx` = `gfx`, the only stack argument = `snap`; both functions `ret 4`):

1. `0x4d82b0` (from `0x4b2833`, early in the frame): if the chat line is dirty, clear its rectangle and draw it into
   the DIB; for each entry with the dirty byte set: clear its rectangle and, if the timer is `!= 0`, draw the text
   (`0x4a8a20`: UTF-16 -> ANSI, then `0x4a8090` -> `DrawTextA`; font 0, flags 0x40) clipped to the line rectangle.
2. `0x4d8400` (from `0x4b422f`, the last drawing before the flip): composite the chat rectangle (if dirty or non-empty)
   and its icon (`0x4000a` / `0x40009`); for each entry with dirty or timer `!= 0`: draw the icon sprite (timer `!= 0`)
   and composite the line rectangle from the DIB onto the frame.

Geometry (game pixels; `c = [0x730f38]` = 3 / 4 / 5 for heights < 600 / < 768 / >= 768, `H = [0x730f10]`; the
constants are the table `0x658ab8..0x658ad4` times `c`; not shifted by the panel width). VERIFIED at 800x600.

| Item | Rectangle |
| --- | --- |
| line slot i (0..4, 0 = bottom) | `x 50c .. 195c`, `y H - (21 + 6i)c .. H - (15 + 6i)c` (800x600: slot 0 = 200..780 x 516..540, slot 4 = y 420..444) |
| icon of slot i | anchor `x 48c` (`0xfc` arrow: `48c + 8`), `y H - (18 + 6i)c`; the sprites carry their own offset (at 800x600 the arrow of slot 4 is seen around (197, 487), of slot 2 around (197, 535)) |
| chat line | `x 50c .. 195c`, `y H - 10c .. H - 5c`; icon anchor `(48c, H - 7c)` |

### Clicks

- **Tutorial arrow** (VERIFIED): the map branch of HandleInput's left-down (`0x532fa2`, `x >= [0x730f24]`): if
  `[0x7dfdb0] == 0x63` and `x - [0x730f24] < 50` and `H - y < 210` (fixed game pixels, not scaled) then
  `[gd+0x2d304] = 1` and the click ends there; the next logic loops advance the step if it is an arrow step. The area
  is the whole bottom-left corner of the map (no test whether the arrow is shown). Clicks at (196, 570) and (196, 395)
  advanced, (300, 570) did not. **Poking `[gd+0x2d304] = 1` advances the same way** (VERIFIED, step 1 -> 2), so an
  overlay button can do that (or inject a game click at `([0x730f24] + 10, H - 10)`). HYPOTHESIS: with the Mobile HUD
  layout (`[0x730f24] = 0`) the area is `x < 50`, left of where the arrow is drawn: tapping the original arrow may not
  work there.
- **Message lines are not clickable.** "Go to the event" lives in the panel messages page (page 8): the buttons
  Messages 1..4 call `0x4fee90(n)` (thiscall gd), which stores the event position in `gd+0x2d314/0x2d318`; the "go to"
  button (cols 0x11..0x16, rows 0x54..0x59 on page 8) centres the camera with `0x51b560(x, y)` (`0x52e1ef`) and closes
  the page. STATIC.

### Suppressing the game's drawing (VERIFIED)

Patching `0x4d8400` to `ret 4` (s3run `poke 0x4d8400 c20400`, restore `83ec4c`) removes all message lines, icons and
the chat line from the frame with no stale pixels (the landscape is redrawn every frame); restoring brings them back at
once (`scratch/re-messages/r3`). The text still goes into the DIB through `0x4d82b0`; skipping that too keeps the DIB
clean for the other composites (tooltip, clock) and saves the GDI work. The logic side (ring, timers, tutorial) is not
affected.

### Implemented (2026-11-08): `s3-mods/src/messages.rs`

Hooks `0x4d82b0` (skip) and `0x4d8400` (read the 5 entries + chat line from the snapshot, skip); the `messages` mod
draws them in the window layer (14 px at a 600 px high window, bottom-left right of the HUD minimap / Classic panel,
slot positions kept), info icon as a blue "i", the tutorial arrow as a "Next >" button that writes
`[gd+0x2d304] = 1` (VERIFIED: tutorial step 1 -> 2 in s3run). Same on-screen size at 1536x600 and 616x240.

### Hook plan for the constant-size overlay

1. Mods hook at `0x4d82b0` entry: `return_from_function(0, 1)` (render thread; no DrawText into the DIB).
2. Mods hook at `0x4d8400` entry: `snap = [esp+4]`; copy the 5 used entries (`snap + 0xc66d8 + snap_delta() + i*0x1c`:
   text pointer / length at `+4` / `+8`, UTF-16; timer `+0x10`; icon `+0x14`) and the chat line (`+0xc66c0 +
   snap_delta()`, shown when non-empty) into the overlay state, then `return_from_function(0, 1)`. Reading the snapshot
   here gives exactly what the game would have drawn (the logic thread writes the ring concurrently).
3. Overlay (window resolution): draw slot i from the bottom up at a fixed size and keep the game's slot order
   (slot 4 = top); hide lines with timer 0. Icon `0xfd` = info, `0xfc` = arrow button, `0xfe` = second icon,
   `< 0x14` = player colour mark. The 75-char tutorial chunks can be re-joined (consecutive slots from the `0xfd`/`0xfe`
   line down to the `0xfc`/`0xff` line) and re-wrapped by pixel width.
4. Arrow tap: write `[gd+0x2d304] = 1` (from a logic-thread hook such as `HandleInput` entry `0x524fa0`, or from the
   host between slices) or inject a game left click at `([0x730f24] + 10, H - 10)`, only while `[0x7dfdb0] == 0x63`.
5. Tooltip, clock and end banner can stay in the frame or be hooked the same way later (table above).

### Recipe and evidence

`sh tools/re-messages/run.sh OUT SECS "inject"` starts the tutorial (main menu 165 35; in game after ~3 s at 800x600;
`START=scen` = random map). Example inject: `12000:xdump 0x700ffee0@0x414 0x8c OUT/s.bin;18000:click 196 570;21000:shot
OUT/c.png`, then `python -I tools/re-messages/slots.py OUT/s.bin`. `tools/re-messages/lst.py ADDR N` prints the local
objdump listing. s3run `poke` now also invalidates decoded code, so code patches take effect at once. Evidence (local,
not committed): `scratch/re-messages/r1..r6`.

Open: the chat line, its icons and the colour mark (icon `< 0x14`, drawn via `0x5fab50(0x40000)` / `0x5fac10(icon +
0x10000)`) are STATIC only (need a two-instance MP run); the exact sprite offsets of the icons; whether other missions
use `0x540400` (`[0x7dfdb0]` values other than 0x63).
