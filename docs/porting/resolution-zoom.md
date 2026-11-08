# Custom resolutions and zoom (res-zoom)

Status: implemented 2026-11-05 (agent `res-zoom`), desktop-verified (`s3run`, screenshots read), MP two-peer hash
IDENTICAL with different zoom levels per peer. Not yet tried on the phone (Verify phase).
**overlay-window (2026-11-08)**: mod UI moved to a window-resolution layer (constant size at every zoom), so the
minimum went down to **200** px high (levels 200 and 240 added; `res::MIN_H` 200); UI mode A shows its panel at a
constant scale with a matching render width ([display.md](display.md) "Classic panel at a constant size").
**res-tall (2026-11-08)**: maximum raised from 2560x780 to **3200x1200** (row stride of the snapshot grid patched, sprite
lists enlarged), see "Taller views (res-tall)" below.

Goal (user feedback 2026-11-05): use the whole phone screen like Settlers United (no 4:3 letterbox) and zoom in the
game (pinch, both UI modes, independent of the HUD). The original GOG S3.EXE only knows 640x480, 800x600 and
1024x768 (F1-F3). The port now runs the game at **any size from 480x360 up to 3200x1200** (width a multiple of 8) and
uses the render size as the zoom: a smaller size shows less map with bigger pixels, the frontend scales the frame
to the screen.

Code: `port/crates/s3-mods/src/res.rs` (game patch + switch), `zoom.rs` (levels, pinch, settings), ddraw port
extension in `port/crates/s3-dx/src/ddraw.rs` (`DDSDM_PORT_LOGICAL`), Classic menu buttons in `ui_classic.rs`,
pinch hook in `touch.rs`. RE details: [game-state-ui.md](../reverse-engineering/game-state-ui.md) section
"Resolution internals". Display modes / `Viewport`: [display.md](display.md).

## What the player gets

| | |
| --- | --- |
| Zoom levels | 18 render heights: 360, 400, 440, 480, 520, 560, 600 (default), 640, 680, 720, 760, 780, 840, 900, 960, 1040, 1120, 1200 (the last six since res-tall; phone 2316x904: up to 3072x1200). Width = height x window aspect (display mode "Native wide", "Fill", "Stretch"; 4:3 in "Fit"). Phone cover screen 2229x904: 888x360 ... 1480x600 (default) ... 1776x720 ... 1920x780. 4:3: 640x480, 800x600 = the game's own sizes |
| Pinch | both UI modes, continuous: fingers apart = zoom in (one level per factor 1.12 of finger distance), the map point under the fingers stays in place. One switch at a time (the next one when the game finished the previous one, at least 120 ms apart); the final level is saved at the end of the pinch |
| Menu | Classic Touch: Menu -> "Zoom in (n/12)" / "Zoom out (n/12)". Mobile HUD: API for the HUD menu (`zoom::zoom_in/zoom_out/label`), wiring is hud-polish's |
| Persisted | `mods-settings.json` `"res": {"enabled": true, "zoom_level": 6}`; `enabled: false` = the game's own sizes (F1-F3 work again) |
| Mode A minimum | 200 with the panel split (`NativeWide`): below 480 the frame stays 480 high (the panel art needs it) and the map part is cropped, `zoom::classic_crop` / `PanelSplit::crop_h` (2026-11-08; same map pixels as a smaller render, and instant). Without the split (Fit etc.) 480, levels 0-6 skipped |

Panel at custom sizes: the panel art, minimap and panel button grid of the largest original layout whose height
fits ("variant": < 600 -> 640x480 art, < 768 -> 800x600 art, else 1024x768 art); the map view takes the rest of the
width. Below the panel art (heights between the variants) the area stays black. **UI mode A uses the 640x480
variant at every size** (`res::CLASSIC_VARIANT`, since 2026-11-08): switching the art at 600 / 768 made the panel
jump on screen while pinching ([display.md](display.md) "Classic panel at a constant size").

## Why the limits (the render snapshot grid)

The logic thread copies the visible tiles into a **render snapshot** (`[logic+0x3d0]`, allocated once by `0x51f110`
with `new(0xc7720)`, ctor `0x523480`): a grid of 130 rows x 130 columns at `snap+0x500`, 0x30 bytes per tile, index
`row + col*130` (`0x52240f`, `0x522f66`). The view needs `cols = (mapW+23)/16 + 10` and `rows = (mapH+255)/9 + 11`
(`0x51f990..0x51fab4`, stored at `snap+0x30/+0x34`), and because map rows are sheared the column index runs up to
`cols + rows/2` (+3 for neighbour reads). 1024x768 already needs 126 of the 130 columns; a wide phone frame would
write far past the grid into the fields behind it. The row stride (130) is a constant in many instructions
(displacements like `+0x1860` = one column, `+0x30` = one row), so res-zoom kept it: **height <= 780**
(`rows + 3 <= 129`). res-tall changed it to 258 (below).
The columns are cheap to extend: only the fields **behind** the grid (`snap+0xc65c0..0xc7720`: message lines, chat,
tooltip/object lists) are addressed with absolute offsets, at 51 code sites (snapshot dtor `0x405a50`, frame renderer
`0x4b3314..0x4b34a2`, panel `0x4c245e`, message lines `0x4d82fc..0x4d85b2`, fill `0x51f88c..0x51f98a`, ctor
`0x5234aa..0x523502`, unwind funclets `0x62f43c/0x62f44b`) plus the allocation size `push 0xc7720` (`0x51f16b`).
`res::patch_snapshot` adds `(256-130)*0x1860` to each (checked against the original bytes first): the grid gets
**256 columns** (2560x780 needs 237). Site list generator: `tools/res-zoom/snap_sites.py S3.EXE scratch/re-ui/s3.asm`
(objdump listing + exe bytes, reads the exe as data). The patch is applied with every GOG patch set (also when the
zoom is off) so all peers have the same heap layout.

Other fixed-size things checked: the on-screen hit list caps at 10000 entries (`0x4d8240`), the sprite lists at
4000/8000 entries with 250 row buckets (`0x4d7c90`, `0x4d7e10`; CORRECTED by res-tall: the bucket is `2*row` of the
snapshot, so 250 buckets only reach row 124 - at 780 the objects of the last row were already dropped; now 360
buckets). The frame renderer's
ambient-sound statistics only look at the left 640 px (`local_1c/0x28 < 16`, `< 0x259`), harmless. The text DIB
(`0x4a78f0`, 1 bpp) and the landscape ring buffer (`0x54bec0`) are made `MAX_W x MAX_H` (+ margins `[0x730f44]` = 32,
`[0x730f48]` = 102) by hooks, so no size ever overflows them.

## Taller views (res-tall, 2026-11-08)

Max size now **3200x1200** (`res::MAX_W/MAX_H`; zoom levels up to 1200 high). All in `res::code_patches()`, applied
by `res::patch_snapshot` with every GOG patch set (all-or-nothing check of the original bytes first):

| What | Change | Sites |
| --- | --- | --- |
| Snapshot grid row stride | 130 -> **258** (`res::SNAP_ROWS`). The index `row + col*130` is always compiled as `mov r,col; shl r,6; add r,col; lea x,[row+r*2]`; `shl r,7` makes it `col*258` with the same instruction length | 10 `shl` imm8 6 -> 7 (`STRIDE_SHL_SITES`): `0x4a99c5` (object drawer `0x4a9960`), `0x4b2e7a`, `0x4b2fa3` (frame renderer), `0x4c8603` (settler drawer `0x4c85a0`), `0x4d309a` (`0x4d3050`), `0x4d5a95` (`0x4d5a50`), `0x52240f`, `0x522f66` (fill), `0x54ce80`, `0x54cef8` (rasteriser) |
| Column steps / neighbours | `n*0x1860` -> `n*0x3060` in displacements and immediates; `3*130`, `130` -> `3*258`, `258` | 30 `STRIDE_DISP_SITES` (renderer pre-pass `add eax,0x1860` `0x4b279f`, renderer neighbours `+0x4eb2`/`+0x4e22` `0x4b2ff6`/`0x4b3003`, the 6-direction neighbour heights of `0x4c85a0` (`-0x1360`, `-0x1390`, `+0x1d60`, `+0x1d90`, three copies), 15 next-column corner reads of the rasteriser `0x54ce95..0x54d347`), 2 `STRIDE_INDEX_SITES` (`0x4b275b`, `0x4b27af`) |
| Grid columns | 256 -> **320** (`SNAP_COLS`; 3200x1200 needs 300) | tail move as before; 4 tail sites the generator had missed: `0x51f1f8` `mov eax,0xc636c` (= tail `0xc65c0` minus the logic-object offset `0x254` it is added to later) and the fill's unwind funclets `0x642cdc/0x642ceb/0x642d02` (above the generator's `0x630000` limit) |
| Sprite lists | bucket = `2*row` (+-2) of the snapshot row, the game has 250 buckets (rows >= 125 were not drawn). The six bucket arrays (heads / counts of lists 1-3 at `gfx+0x13a8c, 0x13e74, 0x4ad60, 0x4b148, 0x5edb4, 0x5f19c`) move behind the `gfx` object, **360 buckets** (`SPRITE_BUCKETS`, needs `2*rows+4`) | 62 array references, bounds `cmp ...,0xfa` (6 inserts `0x4d7c90..0x4d8184`, init `0x4b2d6c`, draw loops `0x4b34d3`, `0x4b3565`), first free node `0xfa`/`0xfb` (`0x4b2da7/b1/bb`: nodes 0..249 are the bucket tails), allocation `push 0x5f700` (`0x42c6b9`) + 6*360*4 |

How the sites were found: a temporary memory-access trace in the interpreter (effective address of every load/store
inside the three snapshot buffers, per instruction; branch commit `0956d16`, removed again) listed the 210
instructions that touch the grid in a HUD game (7 functions), then each function was reviewed completely in the
objdump listing (`tools/res-tall/x130.py` finds the `x130` idiom, `stride_scan.py` candidate constants, `sites.py` the
byte offsets). There are **three** snapshots (the logic fills one while the renderer draws another), all from the one
allocation site. Fixed-size things re-checked for big views: hit list 10000 (`0x7ade12`, full = objects not clickable),
sprite node pools 4000/8000/4000 (full = sprites dropped; `S3_RES_TRACE` prints the fill level every 300 frames:
3072x1200 HUD start scene 728/751/627 nodes, 22 hits), the snapshot's per-player arrays (`snap+0x294..0x2e4`, not
per row), text DIB and landscape buffer (hooks, follow `MAX_*`). The native fast paths (`fast_land/obj/snap/tile.rs`)
use `res::SNAP_ROWS` and hash the *patched* code (`fast_tests::fast_path_hashes_match_patched_code`; the
differential tests load the patched code via `res::apply_code_patches`).

FPS (desktop Windows ARM64, HUD, window 2316x904, `S3_RES` forced, `S3_FPS_CAP=0`, EcoQoS off via
`tools/perf/fast.ps1`, native fast paths on, no JIT, random-map start scene, median of samples 62-85 s;
`tools/res-tall/fps_sizes.ps1`):

| Render size | Pixels | fps |
| --- | --- | --- |
| 1536x600 (default level on the phone) | 0.92 M | 440 |
| 2000x780 (old max) | 1.56 M | 345 |
| 2304x900 | 2.07 M | 274 |
| 2664x1040 | 2.77 M | 235 |
| 3072x1200 (new max) | 3.69 M | 176 |

Cost of the patch at the same size (the display mode is now the 3072x1200 top level instead of 2000x780): within
the run-to-run noise of the random map - 1536x600 two runs each: res-tall 440 / 409, main before it 478 / 332;
2000x780 345 vs 358. The start scene is light (small explored island); a busy late game costs more per frame. HYPOTHESIS (not measured): on
the phone the top levels will be well below 60 fps (verify-fps1 measured 42-60 fps at 1848x720 there).

Memory: each snapshot grows from 1.6 MB to 4.0 MB (x3), the display mode is the largest zoom size of the aspect
(phone 3072x1200 surfaces) and the landscape buffer 3232x1302: `s3det` guest memory 64.9 MB -> 81.3 MB.

Verified (desktop, `scratch/res-tall/`, local only): HUD 3072x1200 (`S3_RES`) full landscape and objects to the
bottom rows, right and left screen edge (`c_down/c_right/c_left.png`); Classic 4:3 pinch 800x600 -> 1600x1200 and
back (`mp1/host-60.png`). `s3det` 90 000 vms: two runs IDENTICAL and `S3_FAST=0` IDENTICAL to the fast-path run
(91 hash lines, `final insns=12330027314`); the hashes differ from before the patch from the first second on (the
`mem` hash covers the patched code bytes, and the heap is laid out differently) - same for every peer of this
version, but peers of different port versions cannot play together. MP two peers (`mp-local-test.sh`, Artokia,
both pinching through 640x480 ... 1600x1200 at different times): **IDENTICAL**, 1226 ticks.

## How a size change works

The game's own F1-F3 switch (RE): the key handler (`0x525a77..`, logic thread) writes the wanted index to
`[0x7dfd44]`, stores it in the settings (`0x49ded0`) and **releases one token of the semaphore `[0x7a800c]`**
(`0x525b5b`). The frame renderer (`0x4b26c0`, render thread) calls `0x4c7490(snap)`: for `snap+0xa0` (= the copy of
`[0x7dfd44]`) in 1..3 it writes the layout table, calls `0x42ce90(w, h, 16, 0, 0)` (posts `0x804(index)` to the
window; `0x405c80` calls `IDirectDraw2::SetDisplayMode` with 640/800/1024 constants; the display setup `0x42e430` runs
again and re-creates surfaces, landscape object, text DIB from the layout globals), sets `[0x7dfd44] = 0xfa` and skips
frames while it is non-zero. The logic thread (`0x53db30`) sees `0xfa`, **waits on the semaphore** (timeout 300 s),
rebuilds the minimap image (`gd::0x4fb650`, `gd+0x1223656`), sets the previous camera `gd+0x2d254/58` to
10 000 000 (= full landscape redraw), clamps the camera, marks the panel dirty and clears `[0x7dfd44]`. While
`[0x7dfd44] != 0` the simulation step is skipped (`0x53dd5c`), exactly as in the original game.

Our switch (`res::switch_hook`, hook at `0x4c7490` entry, render thread, once per in-game frame):

1. **Mode set** (first in-game frame, or the window aspect changed): the display mode becomes the **largest zoom size
   for the aspect** (e.g. 1920x780 on the phone). The layout globals are written for that size *before*
   `0x42ce90` (the display setup sizes its render surfaces from them; with the zoom size there, later bigger steps were
   clipped to it), the `SetDisplayMode` call sites `0x405cc3` / `0x405cf2` get the custom size (pending-mode set by
   the `0x42ce90` entry hook for non-standard sizes), then the layout is set to the zoom size, the cached view
   rectangles are updated and the presented size is set (step 2). Semaphore + `0xfa` like F1-F3.
2. **Zoom step** (fast path, same mode): only the layout globals (`write_layout`, `0x730f08..0x730f40` +
   `[0x7dfd58]`), the cached view rectangles (`gfx+0x5f6e8`, landscape object `+0x58/+0x5c/+0x74..+0x80`) and the
   **presented ("logical") size** change: `IDirectDraw2::SetDisplayMode(w, h, 16, 0, DDSDM_PORT_LOGICAL)` - a flag of
   our ddraw HLE that crops the presented primary surface to `w x h` without touching surfaces (s3-dx
   `set_logical`; a real mode change clears it). The camera is moved so the pinch point keeps its world position
   (`zoom::anchor_camera`) and clamped like `0x51b4d0`; the panel is marked dirty. When the variant changes (panel
   art / minimap size) the logic acknowledgement is used (semaphore + `0xfa`) because the minimap image must be
   rebuilt. Then frames are skipped until the logic thread delivered a snapshot of the new size (`snap+0x30/+0x34`),
   and the next frames force a full landscape redraw (`snap+0xc/+0x10` = 10 000 000).
3. Every frame: the cached view rectangles are compared with the layout globals and re-synced (+ redraw on a
   matching snapshot). This also fixes the **black band with a zigzag edge after F1-F3 in HUD mode** (the HUD changes
   panel/map width after the display setup; the landscape was drawn only as wide as the cached rectangle, and the
   redraw happened on a snapshot of the old width). Verified with the zoom off: HUD + F2 now shows a complete map.

`S3_RES_TRACE=1` logs every step (`res: switch 1480x600 -> 1576x640 mode 1920x780 (fast, ...)`, view-rect syncs,
display-setup sizes, logic ticks while a switch is pending). Desktop testing without a window:
`s3run --headless --display native --window 2229x904` (the window aspect drives the size) or `S3_RES_ASPECT=WxH`;
`S3_RES=WxH` forces one size.

## Determinism (multiplayer)

Resolution is local view state: the original game lets every MP peer pick 640/800/1024 and the patch only changes
render-side memory (snapshot, layout, surfaces). Places where the view touches `gd`: the snapshot fill writes
placement-marker bytes into visible tiles (`0x5223b2`, tile `+8`, only in build mode; masked in the MP hash already)
and the minimap image `gd+0x1223656` (UI only). The ack path pauses the local step like F1-F3.

Test (2026-11-05, `scratch/res-zoom/mp1/`, local only): `PORT=7795 HOST_EXTRA="45000:pinch ...;55000:...;..."
JOIN_EXTRA="50000:pinch ...;70000:..." port/scripts/mp-local-test.sh scratch/res-zoom/mp1 110` - host went
800x600 -> 856x640 -> 1040x780 -> ... -> 1040x780, joiner 800x600 -> 744x560 -> 640x480 -> 696x520 -> 904x680 (fast
steps incl. variant changes with the ack path): **RESULT: IDENTICAL (no desync)**, 1194 ticks, executed messages in
the same order and steps.

## Performance (desktop, loaded host ~175 guest MIPS, HUD mode, `--fps full`)

| Render size | Pixels | fps |
| --- | --- | --- |
| 888x360 | 0.32 M | 46 |
| 1384x560 | 0.78 M | 24 |
| 1776x720 | 1.28 M | 20 |
| 1920x780 | 1.50 M | 16 |

The frame cost grows about linearly with the pixel count (landscape blit, sprites, present conversion are per pixel);
the game's 800x600 (0.48 M) sits between the first two rows. On the phone (~100 M instructions/s measured) expect
roughly 0.55x these numbers until the perf-cpu fast paths land: the default 1480x600 is about 13 fps there, 888x360
about 25 fps. So on the phone the default level should probably be lower (e.g. level 3 = 1184x480); left at 6 until
measured in the Verify phase.

**Measured on the phone (verify-fps1, 2026-11-05)**: the window is now the full 2316x904 (cutout area used), so the
levels are 1536x600 (6, default), 1232x480 (3), 1848x720 (9). With the new fast paths Classic Touch holds 60 fps at
the default, HUD 48-60; 1848x720 42-60 depending on the view. Default kept at 6. Table:
[performance.md](performance.md#phone-measurements). A zoom step itself costs one skipped frame (fast path); a mode set (game start, aspect
change) about 1-2 s on the desktop (display re-setup).

## Verified (desktop, screenshots in `scratch/res-zoom/`, local only)

- HUD 2229x904 aspect: 1480x600 at start (`c1`), pinch 888x360 / 1920x780 / 1184x480 (`o_all.png`), all with full
  landscape, HUD bars and minimap; castle stays under the pinch point.
- Classic 4:3: 800x600 -> 856x640 -> 1040x780 (`l1`, `l2`, panel variant 2/3), Classic 1480x600 (`f0`).
- Classic menu Zoom buttons (`m1.png`: "Zoom out (7/12)" after one tap).
- Save at 640x480 logical / 1040x780 mode -> quit -> Load Game -> game continues -> pinch to 904x680 (`s_all.png`).
- Res off + HUD + F2: no black band (`n3.png`, before the fix `n1.png`).
- MP two peers (above).

## Open / next

- Phone verification (fps per level, pinch feel, fold/unfold = aspect change -> mode set).
- HUD menu zoom buttons (hud-polish, API ready). The HUD restores the panel with the original map width of the
  variant (`ui_hud_game::original_layout`); `res::classic_layout(m)` gives the right one for custom widths (the
  per-frame `fix_map_width` in res.rs corrects it one frame later meanwhile).
- ~~Heights above 780 need the row stride (130) changed~~ done by res-tall (up to 3200x1200).
- Classic mode: paint the area below the panel art (heights between the variants).
- EnumDisplayModes still lists only the three original sizes (the game does not need the custom ones listed).

## Camera on a size switch (2026-11-07)

`res::switch_hook` keeps a screen point over the same world point (`zoom::anchor_camera`): the pinch anchor, or
**the view centre when there is none** (game start, F1-F3). Before, the start view (castle centred by the game for
640x480) kept its top-left corner, so on a 1944x760 view the castle sat in the upper-left part. The Mobile HUD
removing / restoring the panel (`ui_hud_game::apply_layout`) moves the camera by half the panel width for the same
reason. Verified on the phone.

## Cheaper zoom steps (2026-11-07)

Pinch stuttered: after every size switch `res::switch_hook` forced a full landscape redraw for **3** frames
(`snap+0xc/+0x10` = 10 000 000); a full redraw runs the `0x54cbd0` triangle rasteriser (`0x54d382`) over the
whole view (100-280 ms per frame). Now `redraw_frames_after`: 1 frame when the view grows, 0 when it only shrinks
(zoom in; the landscape buffer already covers the smaller view). Desktop HUD 2316x904 uncapped, fps in the pinch
seconds: in 181/158 -> 311/436, out 75/181 -> 290/273. Screenshots mid/end of in and out pinches: no black bands.
(A presentation-only zoom during the pinch was tried first and reverted: the user wants the real zoom per step.)

Pinch follows the fingers continuously (2026-11-07): `zoom::pinch` asks for the render height `h_start / scale`
(8 px steps, `PinchState` overrides the level in `target_size`), one switch at a time (`res::switch_pending`, at
least 33 ms apart, so the step size adapts to how fast the game finishes a switch); the end snaps to the nearest
level and saves it. Before: 40 px level steps, at least 120 ms apart, levels skipped. Desktop JIT, 2316x904, cap 60:
no frame over 79 ms during in/out pinches, 51-61 fps.
perf-landfull (2026-11-08): the remaining full redraw after a view-growing step is now cheap: native fast path
`land_rows` for the rasteriser's row/pixel loop (performance.md, "perf-landfull"). Frame after a zoom-out step on
the desktop: JIT off 37-117 ms -> 17-42 ms, `S3_JIT=1` 14-44 ms -> 14-17 ms (holds the 60 cap).

## More zoom in, FPS counter size (2026-11-08)

- `res::MIN_H` 360 -> 280, `MIN_W` 480 -> 320; `zoom::LEVEL_HEIGHTS` gained 280 and 320 (14 levels; default level
  index 6 -> 8 = still 600 px; Classic minimum index 3 -> 5 = still 480). Tested in game with `S3_RES`: 816x320,
  720x280, 616x240, 512x200 all run (60 fps, no crash, landscape fine), but the Mobile HUD is drawn into the frame:
  below ~280 its icons vanish (top resource icons at 240, bottom bar icons at 200), so 280 is the limit until the
  HUD is drawn at window resolution (multicore.md option B). Zoom out above 780: see the res-tall work.
- FPS counter: font and padding scale with the frame height (11 px at 600), so it keeps its on-screen size at
  every zoom level, like the HUD. (Superseded by overlay-window: drawn in the window layer.)

## Zoom in to 200 (overlay-window, 2026-11-08)

The mods' UI is now drawn at window resolution ([mods.md](mods.md) "Window layer"), so the HUD no longer limits
the zoom: `res::MIN_H` 280 -> 200 (`MIN_W` stays 320), `LEVEL_HEIGHTS` gained 200 and 240 (22 levels; default index
8 -> 10 = still 600; Classic minimum 5 -> 7 = still 480). NOTE: saved `res.zoom_level` indices of older settings files
now mean a level 2 steps closer (the exact `zoom_height` of a pinch is unaffected). Verified headless 2316x904 with
`zoom_level` 0: HUD at 512x200 full size and sharp, icons harvested (the harvest raises the render size to 480 for a
moment), a building placed through the sheet (`scratch/overlay-window/vis/hudmin/`).
