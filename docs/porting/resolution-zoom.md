# Custom resolutions and zoom (res-zoom)

Status: implemented 2026-11-05 (agent `res-zoom`), desktop-verified (`s3run`, screenshots read), MP two-peer hash
IDENTICAL with different zoom levels per peer. Not yet tried on the phone (Verify phase).

Goal (user feedback 2026-11-05): use the whole phone screen like Settlers United (no 4:3 letterbox) and zoom in the
game (pinch, both UI modes, independent of the HUD). The original GOG S3.EXE only knows 640x480, 800x600 and
1024x768 (F1-F3). The port now runs the game at **any size from 480x360 up to 2560x780** (width a multiple of 8) and
uses the render size as the zoom: a smaller size shows less map with bigger pixels, the frontend scales the frame
to the screen.

Code: `port/crates/s3-mods/src/res.rs` (game patch + switch), `zoom.rs` (levels, pinch, settings), ddraw port
extension in `port/crates/s3-dx/src/ddraw.rs` (`DDSDM_PORT_LOGICAL`), Classic menu buttons in `ui_classic.rs`,
pinch hook in `touch.rs`. RE details: [game-state-ui.md](../reverse-engineering/game-state-ui.md) section
"Resolution internals". Display modes / `Viewport`: [display.md](display.md).

## What the player gets

| | |
| --- | --- |
| Zoom levels | 12 render heights: 360, 400, 440, 480, 520, 560, 600 (default), 640, 680, 720, 760, 780. Width = height x window aspect (display mode "Native wide", "Fill", "Stretch"; 4:3 in "Fit"). Phone cover screen 2229x904: 888x360 ... 1480x600 (default) ... 1776x720 ... 1920x780. 4:3: 640x480, 800x600 = the game's own sizes |
| Pinch | both UI modes, continuous: fingers apart = zoom in (one level per factor 1.12 of finger distance), the map point under the fingers stays in place. One switch at a time (the next one when the game finished the previous one, at least 120 ms apart); the final level is saved at the end of the pinch |
| Menu | Classic Touch: Menu -> "Zoom in (n/12)" / "Zoom out (n/12)". Mobile HUD: API for the HUD menu (`zoom::zoom_in/zoom_out/label`), wiring is hud-polish's |
| Persisted | `mods-settings.json` `"res": {"enabled": true, "zoom_level": 6}`; `enabled: false` = the game's own sizes (F1-F3 work again) |
| Mode A minimum | 480 high (the original panel art needs it); levels 0-2 are skipped in Classic Touch |

Panel in UI mode A: the panel art, minimap and panel button grid of the largest original layout whose height fits
("variant": < 600 -> 640x480 art, < 768 -> 800x600 art, else 1024x768 art); the map view takes the rest of the
width. Below the panel art (heights between the variants) the area stays black.

## Why the limits (the render snapshot grid)

The logic thread copies the visible tiles into a **render snapshot** (`[logic+0x3d0]`, allocated once by `0x51f110`
with `new(0xc7720)`, ctor `0x523480`): a grid of 130 rows x 130 columns at `snap+0x500`, 0x30 bytes per tile, index
`row + col*130` (`0x52240f`, `0x522f66`). The view needs `cols = (mapW+23)/16 + 10` and `rows = (mapH+255)/9 + 11`
(`0x51f990..0x51fab4`, stored at `snap+0x30/+0x34`), and because map rows are sheared the column index runs up to
`cols + rows/2` (+3 for neighbour reads). 1024x768 already needs 126 of the 130 columns; a wide phone frame would
write far past the grid into the fields behind it. The row stride (130) is a constant in many instructions
(displacements like `+0x1860` = one column, `+0x30` = one row), so it stays: **height <= 780** (`rows + 3 <= 129`).
The columns are cheap to extend: only the fields **behind** the grid (`snap+0xc65c0..0xc7720`: message lines, chat,
tooltip/object lists) are addressed with absolute offsets, at 51 code sites (snapshot dtor `0x405a50`, frame renderer
`0x4b3314..0x4b34a2`, panel `0x4c245e`, message lines `0x4d82fc..0x4d85b2`, fill `0x51f88c..0x51f98a`, ctor
`0x5234aa..0x523502`, unwind funclets `0x62f43c/0x62f44b`) plus the allocation size `push 0xc7720` (`0x51f16b`).
`res::patch_snapshot` adds `(256-130)*0x1860` to each (checked against the original bytes first): the grid gets
**256 columns** (2560x780 needs 237). Site list generator: `tools/res-zoom/snap_sites.py S3.EXE scratch/re-ui/s3.asm`
(objdump listing + exe bytes, reads the exe as data). The patch is applied with every GOG patch set (also when the
zoom is off) so all peers have the same heap layout.

Other fixed-size things checked: the on-screen hit list caps at 10000 entries (`0x4d8240`), the sprite lists at
4000/8000 entries with 250 row buckets (`0x4d7c90`, `0x4d7e10`): fine up to 780 rows of pixels. The frame renderer's
ambient-sound statistics only look at the left 640 px (`local_1c/0x28 < 16`, `< 0x259`), harmless. The text DIB
(`0x4a78f0`, 1 bpp) and the landscape ring buffer (`0x54bec0`) are made 2560x780 (+ margins `[0x730f44]` = 32,
`[0x730f48]` = 102) by hooks, so no size ever overflows them.

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
- Heights above 780 need the row stride (130) changed in all grid accesses (~45 instruction sites incl. neighbour
  displacements); not attempted.
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
