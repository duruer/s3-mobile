# Progress: res-zoom (custom resolutions + zoom)

Goal: arbitrary game resolutions (640..2560 x 480..1440, screen aspect e.g. 2229x904) patched in memory, and
pinch/menu zoom = internal render resolution in steps, map point under fingers fixed, deterministic for MP.
Owner files: port/crates/s3-mods/src/res*.rs, zoom*.rs, s3-dx ddraw mode list, docs/porting/resolution-zoom.md.

## Status
- 2026-11-05 start: reading docs.

## Next
- RE layout 0x4c6d70, surface allocs, buffers assuming 640/800/1024.

## 2026-11-05 15:00 checkpoint
- RE done (see docs/porting/resolution-zoom.md once written): key limit = render snapshot grid 130x130 tiles
  (`[logic+0x3d0]`, size 0xc7720, index row+col*130); fix = move tail fields (51 code sites) -> 256 columns;
  rows stay 130 -> MAX_H 780.
- Code: s3-mods/src/res.rs (patch + hooks 0x4c7490 switch, 0x42ce90, 0x405cc3/0x405cf2 SetDisplayMode,
  0x54bec0 landscape max, 0x4a78f0 text DIB max), zoom.rs (levels, pinch, ZoomMod), settings.res, lib.rs install,
  touch.rs pinch -> zoom::pinch, s3-dx SetDisplayMode up to 4096. Builds.
- Next: desktop run with S3_RES_TRACE=1 S3_RES_ASPECT=2229x904, screenshots, fps, pinch inject, mp-hash.

## 2026-11-05 ~16:00 checkpoint
- WORKS on desktop: HUD + Classic at 1480x600 etc., pinch zoom levels 888x360..1920x780 (S3_RES_ASPECT=2229x904).
- Found: logic ack of a switch waits on semaphore [0x7a800c] released by the F-key handler -> we ReleaseSemaphore.
- Fast path: display mode = largest zoom size for the aspect; zoom steps only change layout + ddraw "logical"
  presented size (s3-dx DDSDM_PORT_LOGICAL flag on IDirectDraw2::SetDisplayMode, present crop). Slow path (real
  mode set) only on first frame / aspect change. Variant change -> 0xfa ack (minimap rebuild).
- Next: menu->game re-entry check, mp-hash determinism, fps numbers, classic menu zoom buttons, tests, docs.

## 2026-11-05 ~16:45 checkpoint
- Fixed: mode set must run the display setup with layout = mode size (render surfaces), then layout = zoom size.
- Fixed: HUD black band after F1-F3 (res off too): landscape view rect cached by display setup -> synced every
  frame (`sync_view_rects`) + forced redraw only on frames whose snapshot matches the view (`snapshot_matches`).
- Classic menu: Zoom in / Zoom out buttons (ui_classic.rs, additive). MP 2-peer different zooms: IDENTICAL 1194 ticks.
- Save/load round trip at custom res OK. FPS desktop (loaded host, ~175 MIPS): 888x360 46, 1384x560 24,
  1776x720 20, 1920x780 16 fps.
- Next: regression runs, tests, docs/porting/resolution-zoom.md + other docs.

## 2026-11-05 ~17:15 FINAL
- Done: res.rs/zoom.rs + ddraw logical size + Classic zoom buttons + touch pinch; docs: porting/resolution-zoom.md
  (main), game-state-ui.md "Resolution internals", directx-hle.md, mods.md, touch.md, ui-hud.md, AGENTS.md link.
- Tests: cargo test -p s3-mods -p s3-dx OK (new: res::tests x3, zoom::tests x3, s3-dx ddraw_logical_size_crops_present).
- MP: two runs IDENTICAL (mp1 1194 ticks, mp2 921 ticks) with different zooms per peer.
- Open: phone fps per level (default level maybe too big for ~100 MIPS), HUD menu zoom buttons (hud-polish),
  heights > 780 (row stride), classic area below panel art.
- Verify recipe: see resolution-zoom.md "Verified" + `s3run --headless --display native --window 2229x904`.
