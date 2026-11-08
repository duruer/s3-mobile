# Game state RE part 1: units, buildings, map, selection, commands, click dispatch (W2)

Target: GOG `S3.EXE` sha256 `00a925cfdcfc3349b5d0f3b8a4c312f01b813e363b4464f6f3d6de17804d950b` (all addresses below are
for this exe only). Workstream W2 of [mobile-vision.md](../porting/mobile-vision.md). Companion doc (camera, panel,
cursor modes, dialogs, resources): `game-state-ui.md` (re-ui task).

Method: static analysis (objdump of S3.EXE into `scratch/units/s3.asm`, local only; REA/Ghidra for spot checks) plus
runtime checks with `s3run` memory dumps (`xdump`/`xpeek`/`scan` inject commands, see [Tools](#tools)). Status words:
**VERIFIED** = seen at runtime in s3run; **STATIC** = read from code, not exercised at runtime; **HYPOTHESIS** = a guess.

No decompiled code is reproduced here; only addresses, layouts and behaviour.

## Summary for hook / HUD authors

| What | Where | Status |
| --- | --- | --- |
| Logic thread object (`CLogicThread`, vtable `0x65a14c`, size 0x678) | heap; `app+0x90`; app object is a stack local of WinMain on thread t0. In our emulator: logic = `0x10113d48`, `[0x700ffee0]` holds it | VERIFIED (address is deterministic in s3run but do not hard-code it; hook the ctor `0x53c5e0` or read `app+0x90`) |
| Game data (`CGameData`, vtable `0x658c18`, size `0x153a128`, ctor `0x4fc990`) | `gd = [logic+0x3a0]` (s3run: `0x18790000`) | VERIFIED |
| Settler table | `gd+0x12a5e60`, 0x40 bytes/entry, index 1..`[gd+0x12a5e5c]` (maxusedsettler) | VERIFIED |
| Building table | `gd+0x1499e64`, 0x3a bytes/entry, index 1..`[gd+0x1499e60]` (maxusedbuilding) | VERIFIED |
| Map tiles | `gd+0x67444 + (x*768 + y)*0x18`, world size `[gd+0x2d2d0]` (384 for a 384 map) | VERIFIED |
| Player records | `gd+0x11943e0 + p*0x148` | VERIFIED (+0 race, +0xc/+0x10 start position) |
| Local player index | `[gd+0x2d2d4]` | VERIFIED (0 in single player) |
| UI selection (local) | slot `[0x7dfd68]` (-1 = none), count `word [0x7ad620 + slot*2]`, ids `word [0x7ad634 + slot*200 + i*2]` (max 100), dirty `dword [0x7ad5f8 + slot*4]` | VERIFIED |
| Selected building | `[0x7dfd6c]` = building id (0 = none) | VERIFIED |
| Logic copy of selection (synced by command 3) | count `dword [gd+0x11940bc + (p*10+slot)*4]`, ids `word [gd+0x118a47c + (p*10+slot)*200 + i*2]` | VERIFIED |
| On-screen hit list (what is under a screen point) | `0x7ade08`, count `[0x7dfd5c]`, double-buffer index `[0x7dfd64]` (0/1) | VERIFIED |
| Screen -> map | `gd::ScreenToMap` `0x4ff170`, thiscall(gd), `(int sx, int sy)`, ret 8, result in `[gd+0x2d2e4]` (x), `[gd+0x2d2e8]` (y) | VERIFIED (result), STATIC (algorithm) |
| Camera | `[gd+0x2d254]` x pixels, `[gd+0x2d258]` y pixels (copies at `+0x2d25c/+0x2d260`) | VERIFIED values; details in game-state-ui.md |
| Input handler (click -> action) | `CLogicThread::HandleInput` `0x524fa0` (logic thread) | STATIC + VERIFIED effects |
| Player commands | 12/8-byte requests `{0x21, cmd, len, 0, payload...}` posted with `0x4309c0` to the queue `[logic+0x3c4]`; executed by the switch at `0x535a9d` | STATIC + VERIFIED effects |

## Object model

- `app` (main window/application object, vtable `0x6553fc`, ctor `0x4043d0`) is a local of the WinMain-level function
  `0x45a0xx` (call at `0x45a183`); its `Run` loop is `0x404860` on thread t0. `app+0x90` = logic thread object, created at
  `0x4058cc..0x4058fe` (`new 0x678`, ctor `0x53c5e0`).
- `CLogicThread` (`0x53c5e0` ctor; second vptr at +4 = `0x65a140`; `Init/Run` virtual `0x53c9e0` creates `CGameData`
  with `new 0x153a128` at `0x53cb31` and stores it at `+0x3a0`). Relevant members:
  - `+0x3a0` gd; `+0x3c0` input event queue (read by HandleInput); `+0x3c4` outgoing request queue (player commands).
  - `+0x44/+0x48` last mouse x/y (screen, from events); `+0x78` input state (0..3, HYPOTHESIS: idle/drag/hold);
    `+0x220` non-zero blocks map input (HYPOTHESIS: modal dialog/menu open).
- The logic thread in s3run is the one whose EIP is mostly in `0x4dxxxx..0x54xxxx` (t4 in a normal single-player run).

How to find the pointers at runtime (for the W1 mod framework): hook the `CGameData` ctor `0x4fc990` (ecx = gd) or the
store `0x53cb63` (`mov [ebp+0x3a0], eax`, ebp = logic, eax = gd). Both are executed once per game start / load.

## Settler table (`gd+0x12a5e60`, 0x40 bytes)

Index 0 is unused (`owner = 0xff`). Iterate `1..=[gd+0x12a5e5c]`; entries with `owner == 0xff` are free.

| Off | Type | Meaning | Status |
| --- | --- | --- | --- |
| +0x00 | u8 | owner (player 0..7, 0xff = free) | VERIFIED |
| +0x01 | u8 | type ("art"): 0 = carrier (32 per player at start), 5/8/15 = the three start soldier kinds (sword/bow/third kind; panel showed 4/2/2), others 10, 11, 37, 39 seen | VERIFIED values, names HYPOTHESIS |
| +0x02 | u8 | 1 seen | ? |
| +0x03 | u8 | 0x64/0x14 seen | ? |
| +0x04 | u16 | x (tile) | VERIFIED |
| +0x06 | u16 | y (tile) | VERIFIED |
| +0x08 | u16 | 0..5, direction (HYPOTHESIS) | |
| +0x0c | u16 | animation step / counter (changes every dump) | HYPOTHESIS |
| +0x0e | u16 | job / state ("job" in the `SnotThere` log, `0xab`, `0xac`, `0xad`, `0xb0`, `0x17` seen) | STATIC |
| +0x10, +0x12 | u16 | neighbouring settler ids (HYPOTHESIS: links of a formation/list) | |
| +0x15 | u8 | cleared on job change (`0x50dbdb`) | STATIC |
| +0x22 | u16 | cleared on job change | STATIC |
| +0x24, +0x26 | u16 | commanded target x, y (set by the move command; spread around the clicked tile) | VERIFIED |
| +0x28, +0x2a | u16 | current destination x, y (next waypoint) | VERIFIED |
| +0x3c | u8/u16 | 0xff tested at `0x502599`; counter in soldiers | ? |

Settlers inside a building have a position but are not on the tile (tile `+0` = 0).

## Building table (`gd+0x1499e64`, 0x3a bytes)

| Off | Type | Meaning | Status |
| --- | --- | --- | --- |
| +0x00 | u8 | owner (0xff = free; entry 0 unused) | VERIFIED |
| +0x02, +0x04 | u16 | x, y (tile of the building origin; = the clicked placement tile) | VERIFIED |
| +0x06 | u8 | building type. 15 = Guard Tower (start building of random maps); 2 = first entry of the first build category ("Woodcutter"? HYPOTHESIS); 3, 4 seen for AI sites | VERIFIED (15, 2) |
| +0x07 | u8 | race of the owner (= player record +0) | VERIFIED |
| +0x08 | u16 | 2 / 0x3c on finished towers (HYPOTHESIS: occupants/strength) | |
| +0x0a, +0x0c | u16 | door / flag tile x, y | VERIFIED (next to x,y) |
| +0x10, +0x12 | u16 | settler ids working on a construction site (builder/digger) | VERIFIED (sites only) |
| +0x18 | u8 | bit 0 = working / building active (toggled by command 4 "Stop/Resume Working/Building"), bits 1..4 = construction priority (commands 0xf5/0xf6); game-state-ui.md section 8 | VERIFIED (site) |
| +0x18..+0x21 | u8 | construction progress bytes on sites (`01 03 01 05 04 00 00 04 00 01` on a fresh site, `01 00 01 ...` on finished) | HYPOTHESIS: build phase ("Baubauphase") |
| +0x21 | u8 | non-zero = construction site (building page tooltips; VERIFIED 0 on a finished tower, non-zero on a fresh site); also excludes the building from some command checks (`0x535b51`, `0x537307`) | VERIFIED |
| +0x30, +0x32 | u16 | x, y again (or a sign/target position) | VERIFIED values |
| +0x38 | u16 | 0xfa (250) on finished buildings | ? |

Tile `+0x0a` bit `0x20` marks a building tile; tile `+0x02` holds the building id (VERIFIED for the tower's origin).
`gd::DestroyBuildingAt(x, y)` (`0x4d97b0`, thiscall gd, ret 8) searches the building that covers a tile through the
building shape tables (`0x6aa174`, indexed by race and type) and calls `gd::CrushBuilding(id)` `0x4d9250`
(thiscall gd, ret 4, logs `Start crushing nr. %u art %u`). STATIC.

`gd::AddBuilding` `0x4d99e0`, thiscall gd, `(x, y, type, player, mode)`, ret 0x14: mode 0 = only test whether the site
is valid (used by the click handler before sending the command), mode 1 = place a construction site (used by the
command executor). Returns non-zero on success. STATIC + VERIFIED effect (placement created building 7 at the tile
`[gd+0x2d2e4/e8]`).

## Map tiles (`gd+0x67444`, 0x18 bytes, 768 tiles per x column)

`tile(x, y) = gd + 0x67444 + (x*768 + y) * 0x18`, valid for `0 <= x, y < [gd+0x2d2d0]`. Note the layout: **x is the
major index**. VERIFIED: 57 of 58 settlers have their id at `tile(x,y)+0` (the 58th was inside the tower).

| Off | Type | Meaning | Status |
| --- | --- | --- | --- |
| +0x00 | u16 | settler id standing on the tile (0 = none) | VERIFIED |
| +0x02 | u16 | building id (with flag below) | VERIFIED |
| +0x04 | u8 | terrain height (ScreenToMap uses it) | STATIC + plausible values 0..50 |
| +0x06 | u8 | terrain type ("worldart" in a log; bits 0xf0 tested by the move command) | STATIC |
| +0x07 | u8 | economy sector ("ecosec" in a log) | STATIC |
| +0x08 | u8 | owner of the land (0xff = nobody) | VERIFIED values (mostly 0xff, 0 near the player) |
| +0x0a | u8 | flags; 0x20 = building on tile | VERIFIED |
| +0x10 | u8 | sector (0 = not walkable/water, 1 = land seen) | STATIC (`Siedler steht auf sector 0` log) |
| +0x14 | u16 | map object (tree, stone, ...; "object" in a log) | STATIC |

## Selection

Local (UI) selection, written by the left-button-up handler, VERIFIED with a box drag over 8 and 14 soldiers:

- `[0x7dfd68]` current group slot (0 after a normal selection, -1 when a building is selected or nothing).
- `word [0x7ad620 + slot*2]` count; `word [0x7ad634 + slot*200 + i*2]` settler ids (max 100, check at `0x533b89`).
- `dword [0x7ad5f8 + slot*4]` dirty flag: 1 after the selection changed; the next right-click first sends
  command 3 (selection sync) and clears it.
- `[0x7dfd6c]` selected building id (single click on a building, kind 100 in the hit list). Clicking our tower set it to 1,
  `[0x7dfd68] = -1` and `[gd+0x2d284] = 4` (building panel). Selecting soldiers set `[gd+0x2d284] = 5`.
- Drag state: `[0x7dfd34]` drag active, start `[0x7dfb9c]` x / `[0x7dfb98]` y, current `[0x7dfd38]` x / `[0x7dfd3c]` y
  (VERIFIED: 200,270 -> 560,400).

Selection rectangle logic (left button up, `0x5339a1..0x534100`, STATIC, effect VERIFIED):

1. Normalise the rectangle. If width < 5 and height < 5 it is a **single click**: the rectangle becomes the point
   ±10 px horizontally and -5..+32 px vertically (units are hit at their feet).
2. Without Shift the old selection is replaced; with Shift (`flags & 1`) ids are appended to the current slot.
3. A box first finds the "best" class present (soldier kinds 1..9 and 40 first, then 50/51, 11, 21, 20, 10; HYPOTHESIS:
   soldiers before ships before specialists) and then selects only entries of that class.
4. For every hit-list entry inside the rectangle: append its id, call `CLogicThread::0x5428e0(id, slot)`
   (HYPOTHESIS: marks the settler selected / plays the voice), and remember a building (kind 100) in `[0x7dfd6c]`.
5. With Ctrl or Alt (`flags & 4` / `& 2`) on a single unit: select units of the same type around it (loop over
   the neighbourhood table `gd+0x110a588` and tile `+0` owners, `0x533f21..`).

Logic-side copy (what commands act on; VERIFIED: count 0 -> 14 after the right-click that sent command 3):
`dword [gd+0x11940bc + (player*10 + slot)*4]` count, `word [gd+0x118a47c + (player*10 + slot)*200 + i*2]` ids.

## What is under a screen point (hit list)

The renderer fills a list of **selectable things currently drawn**: base `0x7ade08`, entry stride 20 bytes (two 10-byte
copies; the reader uses `0x7ade08 + ([0x7dfd64] + 2*i) * 10`), count `[0x7dfd5c]` (`[0x7dfd60]` = count of the other
buffer). Entry (10 bytes): `u16 kind, i16 screen_x, i16 screen_y, u16 id, u16 0`. VERIFIED values:

- kind 100 = building (several entries per building, one per hot spot of its footprint), id = building id.
- kind 1 = swordsman, 2 = bowman, 3 = third soldier kind (ids 41..54 matched the selected settlers). Carriers are not in
  the list (not selectable).
- Screen coordinates are 640x480 game-screen pixels at the unit's feet.

So a touch frontend can find the unit/building under a finger without calling game code: nearest entry to the tap,
within the same ±10 / -5..+32 box the game uses. Ground tiles: `gd::ScreenToMap`.

## Screen -> map transform

`gd::ScreenToMap` `0x4ff170`, thiscall(gd), args `(int sx, int sy)` in 640x480 screen pixels, ret 8, no return value:
writes the tile to `[gd+0x2d2e4]` (x) and `[gd+0x2d2e8]` (y), clamped to `0..ws-1`. Returns without writing when
`sx < [0x730f24]` (left of the map view, i.e. in the panel). VERIFIED: right-click at (500,150) -> (127,231), and
soldiers' targets (+0x24/+0x26) were around that tile.

Algorithm (STATIC, our description): add the camera (`[gd+0x2d254]`, `[gd+0x2d258]`) and fixed offsets (x: -0x118,
-0x20 per resolution step `[0x730f08]` 1..3; y: -0xc8); x is quantised to 8-pixel half-columns and y to 18-pixel rows;
odd half-columns shift by 9 px. This gives a first tile on the screen column; the function then walks down the column
(`x+1, y+2` per 18 px) comparing the tile heights (`tile+4`) to find the tile whose raised position matches the pixel.
So heights matter; calling the game function (W1 `call_guest`) is the safest way. Map -> screen (for drawing HUD
markers) is the inverse: about `x_px = 16*x - 8*y`, `y_px = 9*y - height` plus camera (HYPOTHESIS, not verified).

`gd::0x4ff0a0(sx, sy)` (thiscall gd, ret 8) is the variant used on mouse move and for building placement; in build
mode it fills the same `[gd+0x2d2e4/e8]` with the snapped construction tile (VERIFIED: hover at (470,110) gave
(0xb3, 0xe2), the site was then created exactly there).

## Input path: from a mouse button to a game action

1. Window messages (thread t0). The WndProc object's vtable is `0x6572b0`; the message switch `0x45c860` calls:
   `WM_LBUTTONDOWN/DBLCLK -> +0x38 0x45b630`, `WM_LBUTTONUP -> +0x3c 0x45b850`, `WM_RBUTTONDOWN/DBLCLK -> +0x40
   0x45ba50`, `WM_RBUTTONUP -> +0x44 0x45bc70`, `WM_MOUSEMOVE -> +0x48 0x45be80`, keys -> `0x45b370` (also keeps the
   modifier bits). DirectInput mouse data (`0x45ce10 -> 0x45cea0`) only produces **relative move** events; DI buttons
   are ignored.
2. Each handler converts to an **input event** (20 bytes = 5 dwords) and posts it with `Queue::Post` `0x4309c0`
   (thiscall queue, `(buf, ndwords, flag)`, ret 0xc). Event layout: `+0 u8 type`, `+1 u8 modifiers`, `+8 i16 x`,
   `+0xa i16 y` (screen). Types: `0x10` left down, `0x11` left double click, `0x12` left up, `0x13` right down,
   `0x14` right double click, `0x15` right up, `0x16` mouse move (window message), `0x17` relative DI move;
   `0x01` key (`+4` = VK, e.g. ESC 0x1b), `0x03`, `0x20` other. Modifiers (`obj+0x14`): Shift = 1, Alt = 2, Ctrl = 4.
3. **Logic thread** `CLogicThread::HandleInput` `0x524fa0` pops events from `[logic+0x3c0]` (`0x430980`) and does all
   click logic (64 KB function, mostly if-chains):
   - `0x10/0x11` (left down): panel/minimap areas first (geometry globals `0x730f0c..0x730f38`, see game-state-ui.md);
     map area: build mode places a building (below), otherwise starts the selection drag.
   - `0x12` (left up, `0x5339a1`): selection rectangle / single-click selection (above).
   - `0x13` (right down, `0x534aa5`): stores the current game step `[gd+0x2d2d8]` in `[0x7dfd70]`; if
     `[0x7dfd80] == 1` it handles right-drag scrolling (HYPOTHESIS).
   - `0x15` (right up, `0x53431f`): **the right-click is ignored if the button was held for 15 or more game steps**
     (`[gd+0x2d2d8] - [0x7dfd70] >= 15`, check at `0x53437d`; with `[0x7dfd80] == 1` the limit is 50). This is why a
     long-press right-click does nothing in the original game (relevant for touch: a synthetic right-click must be a
     quick down+up, or the check can be patched). Then: in build mode (`[gd+0x2d284] == 1`) it cancels the placement
     (`[gd+0x2d28c] = 0`); in building mode (`== 4`) it deselects the building (`[0x7dfd6c] = 0`, mode 0). If units
     are selected: plays an acknowledgement (sound 0x39/0x3a/0x3b for <=5 / <=20 / more units, `0x53f560`), computes the
     target (minimap if the point is inside it, else `gd::ScreenToMap`), sends command 3 if the selection is dirty,
     then the move command (9 / 0xd / 0xe / 0xf by modifier, below).
   - `0x16` (move, `0x5348fa`): stores the mouse in `logic+0x44/0x48`, calls `gd::0x4ff0a0`, updates the drag end.
4. Commands are **not executed directly**: HandleInput builds a request and posts it to `[logic+0x3c4]`
   (`Queue::Post(buf, 3 or 2, 1)`), but only if `[0x730f64] != 0` (it is `0xffffffff` in single player, VERIFIED).
   The network layer (lockstep, `NET:` logs) returns it at the execution step; the executor at `0x535a5b` (in the
   logic message handler) switches on the request's command byte (`0x535a9d`, table `0x53b898`/`0x53b934`). Single
   player uses the same path, so **commands are the natural injection point for HUD actions and for multiplayer**.

## Player commands (requests)

Common header: `+0 u8 0x21` (request class), `+1 u8 cmd`, `+2 u8 length in bytes` (0x0c or 0x08), `+3 u8 0`.
Multi-byte numbers in the payload are **big-endian** (high byte first). Executor addresses are the case targets of the
switch at `0x535a9d`.

| cmd | Payload (from +4) | Built at | Executor | Meaning | Status |
| --- | --- | --- | --- | --- | --- |
| 0x01 | x(2), y(2), type(2), player(1) | `0x533065` (left click in build mode after `AddBuilding(...,0)` succeeded, sound 0x2e) | `0x535aa4` -> `gd::AddBuilding(x,y,type,player,1)` | place construction site | VERIFIED |
| 0x03 | player(1), slot(1), count(2), ids (2 each, high byte first) ... ; byte +2 = `((count+1)/2 + 2) * 4` | `0x534541` (right-click, if dirty) and `0x52ece7`, `0x52f90d`.. | `0x535cb0` | copy the UI selection to the logic copy | VERIFIED (logic copy filled) |
| 0x09 | player(1), slot(1), x(2), y(2) | `0x53487e` (right-click, no modifier) | `0x536045` (mode 0) | move selected units to tile; on an enemy building/unit this is the attack order (HYPOTHESIS: target classification inside the executor) | VERIFIED (units walked to the tile) |
| 0x0d | same | `0x5345fc` (Shift + right-click) | `0x536045` (mode 1) | variant (HYPOTHESIS: add waypoint / patrol) | STATIC |
| 0x0e | same | `0x534787` (Alt + right-click; a special case with a thief (settler type 0x17) on land of an allied player shows a text message `0x80174` instead) | `0x536045` (mode 2) | variant (HYPOTHESIS: patrol or steal) | STATIC |
| 0x0f | same | `0x534814` (Ctrl + right-click) | `0x536045` (mode 3) | variant (HYPOTHESIS: forced march, no fighting) | STATIC |
| 0x06 | building id(2), player(1); length 8 | `0x52fee7` (destroy button in the building panel, mode 4, `[gd+0x2d298] == 1`) | `0x53729c` -> `gd::CrushBuilding` `0x4d9250` | destroy building | STATIC |
| 0x04 | building id(2), new flag(1) = bit 0 of building +0x18 toggled, player(1); length 8 | `0x531d65` (building panel, sounds 0x35/0x36) | `0x536fbf` | Stop / Resume Working (Stop / Resume Building on sites) | VERIFIED (site, 2026-11-08) |
| 0x0c | building id(2), player(1), action(1): 1 max garrison, 2/3/4 add sword/bow/spear, 5 min garrison, 6/7/8 remove | `0x531499..0x531ab6` (towers / castle) | | garrison wishes (game-state-ui.md section 8) | VERIFIED (1) |
| 0x10 | kind(1), player(1), x(2), y(2), count(1), 0; length 12 | `0x52cbf7..0x52d5ff` (settlers page Recruitment) | | 1 geologist, 2 thief, 3 pioneer, 4 pioneer -> carrier, at the view-centre tile | VERIFIED (1, 3) |
| 0xf5 / 0xf6 | building id(2), owner(1); length 8 | `0x531eb3` / `0x531f2c` (building page) | | construction site highest priority on / off | VERIFIED |
| 0xfa | player(1); length 8 | `0x52e510` (multiplayer "Save Game") | | network save | STATIC |
| 0x05 | x(2), y(2), ..., building id | `0x53330e` (building panel, `[gd+0x2d298] = 3`) | `0x5370b4` | set a building's work area / target to a tile (HYPOTHESIS) | STATIC |
| 0x07, 0x08, 0x0a, 0x0b, 0x11 | economy sector / goods index / value | panel code `0x528c65..0x52e040` | `0x5375c7`, `0x53767d`, `0x53919d`, `0x5395b5`, `0x537728` (0x10 = recruitment, below; executor `0x539f30` HYPOTHESIS) | economy settings in the economy sector table `gd+0xe145e8` (0x2e0 bytes each): goods distribution / transport priorities / tool and weapon production (HYPOTHESIS for which is which) | STATIC |
| 0x12, 0x14, 0x15, 0x17 | player, building id, tile x/y, panel action | `0x533439`, `0x5335c9`, `0x5338f4`, `0x533729`, `0x530b43` | `0x5377cf`, `0x538e45`, `0x53888e`, `0x538f19` | building-panel actions with a target tile (0x14/0x17 only for building types 0x20/0x21: HYPOTHESIS harbour/shipyard) | STATIC |
| 0x13, 0x16, 0x18, 0x19, 0x1a, 0x1b, 0x1c | 8-byte requests | `0x52d68b`, `0x52f5ff`, `0x5300bc`.. | see table | unknown | STATIC |
| 0xf1..0xff | | `0x52e510` (0xfa), `0x531eb3` (0xf5), `0x531f2c` (0xf6) | `0x53a1af..0x53b34e` | system: save/load (`slot%2i.sav`), quit, game status, multiplayer save | STATIC |

Executor callee hints (STATIC): 0x02 -> `0x51b4d0`, 0x11 -> `0x51bc20`, 0x12 -> `0x5236b0`, 0x13 -> `0x4dda10`,
0x14/0x17 -> `0x4dbb90`/`0x4db710`, 0x16 -> `0x50ce80`, 0x18 -> `0x508450`, 0x19 -> `0x5194e0`, 0x1c -> `0x4dfb40`.

How a mod issues a command (HYPOTHESIS until W1 tries it): build the bytes as above and call `Queue::Post`
`0x4309c0` with `ecx = [logic+0x3c4]`, args `(ptr, ndwords = len/4, 1)` on the logic thread (where HandleInput runs).
For a move, first write the UI selection and send command 3, exactly like the right-click path. Alternative without
calling game code: post synthetic **input events** (type 0x10..0x16, 20 bytes) to `[logic+0x3c0]` and let HandleInput
do the work (this is what our window-message injection already does indirectly).

## Other game-state words seen in `gd+0x2d250..0x2d304`

| Address | Meaning | Status |
| --- | --- | --- |
| `gd+0x2d254/+0x2d258` | camera pixel x/y (`0xc04/0x7d4` in one run) | VERIFIED values |
| `gd+0x2d27c` | "refresh panel" flag (set to 1 after many UI actions) | HYPOTHESIS |
| `gd+0x2d284` | panel page: 0 none, 1 build, 2 goods, 3 settlers, 4 building selected, 5 units selected, 7 menu, 8 messages, 9 paused (game-state-ui.md) | VERIFIED |
| `gd+0x2d288/0x2d28c` | build category (1..4) / build list slot (0 = none; right-click clears it). Corrected by re-ui, see game-state-ui.md | VERIFIED |
| `gd+0x2d298` | building panel page/action | STATIC |
| `gd+0x2d2d0` | world size | VERIFIED |
| `gd+0x2d2d4` | local player | VERIFIED |
| `gd+0x2d2d8` | game step counter (used for the right-click time limit) | VERIFIED (increases) |
| `gd+0x2d2e4/0x2d2e8` | last computed target tile (ScreenToMap / placement) | VERIFIED |
| `gd+0x2d2f0` | building type selected for placement (2 after choosing the first building) | VERIFIED |

Global `[0x7dfdb0]` = mission/campaign id (0x42 and 0x63 have special cases). Header-level desync check (`0x53a2xx`)
compares checksums of the building, settler, pile (`gd+0x123885c`, 0xe bytes, count `gd+0x1238858`), trade
(`gd+0x10f45e8`, 0x5a bytes, count `gd+0x110a578`), economy (`gd+0xe145e8`, 0x2e0 bytes, count `gd+0xe115dc`), player
(0x148 bytes) and world tables; the dump helper `0x4ff6d0` writes them to `d:\*.dmp` (debug only).

## Consequences for touch (W3) and the HUD (W4)

- Right-click on touch: send right down + up within the same few frames (or move-command directly). A long press
  cannot be a right-click in the original logic (`0x53437d`).
- Tap on a unit/building: hit list at `0x7ade08`; tap on ground with units selected: command 9 to
  `ScreenToMap(tap)`; box select: synthetic left down/move/up, or write the UI selection + dirty flag.
- Build menu in the HUD: set `[gd+0x2d2f0]` type and mode (`[gd+0x2d284] = 1`, re-ui), or send command 1 directly
  after checking `AddBuilding(x,y,type,player,0)`.
- Selected-unit card: counts per kind from the hit list / settler types; building card: `[0x7dfd6c]` + building table.

## Tools

`s3run --inject` read-only commands added for this work (`port/crates/s3-desktop/src/debug_units.rs`):

- `t:xpeek EXPR LEN` / `t:xdump EXPR LEN FILE`: `EXPR` = address with `@OFF` pointer steps and `+` sums, e.g.
  `GD=0x700ffee0@0x3a0` then `$GD@0x2d280` (game state words), `$GD@0x12a5e5c` (settler count + table),
  `$GD@0x67444+0x4800` (tile column x=1).
- `t:scan VALUE [MAX]`: find dwords in all committed memory (e.g. a vtable to find heap objects).
- re-ui's `dump/peek/poke/regs` (`debug.rs`) for fixed addresses.

Recipe (in game after ~35-50 s on a loaded machine; check with a `shot`): see the runs in
`docs/progress/re-units.md`. Example box select + move:
`51000:move 180 260;51100:down 180 260;51300:move 300 330;51500:move 500 420;51700:move 620 470;51900:up 620 470;54000:rclick 520 140`.
Random maps differ per run (positions, races), so read positions from the dumps or a screenshot first.

## Open questions

- Names of settler types and building types (only 15 = Guard Tower, 2 = first building of the first category known).
- Exact meaning of move modes 0xd/0xe/0xf and of commands 0x04, 0x05, 0x12..0x1c.
- Construction state bytes in the building entry.
- Map -> screen formula (needed for drawing HUD markers on units).
