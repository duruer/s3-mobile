# Progress: cursor — FINAL (2026-11-05)

Goal: draw the Windows mouse cursor (GDIMouse=1) from S3.EXE .rsrc cursors, composited at present time into a
host copy of the frame for all frontends; respect ShowCursor counter.

## Done
- `port/crates/s3-runtime/src/cursor.rs`: `pick_group_entry`, `decode_cursor_resource`/`decode_dib_cursor`
  (1/4/8/24/32 bpp, AND/XOR, alpha), `builtin_arrow` (own pixel art), `load_cursor_resource`, `CursorState`
  (images, current, show_count, clip, disabled, `compose`, `refresh`), `draw_cursor`. 4 unit tests with synthetic
  cursors (mono AND/XOR incl. invert, 32 bpp alpha, group pick, hot spot, ShowCursor hide, edge clipping, refresh).
- `Machine::present` composes the cursor; `Machine::refresh_cursor` (service "cursor", added by user32 SetCursor)
  re-presents the last frame when only the cursor moved/changed.
- user32: `LoadCursorA`/`LoadImageA(IMAGE_CURSOR)` decode + register images (`register_cursor_image`),
  `SetCursor`/`ShowCursor`/`ClipCursor` mirror into CursorState.
- s3-desktop: minifb window hides the OS pointer.
- Verified headless: scratch/cursor/menu.png (cursor at injected 300,300 in idle menu), ingame.png (320,240, pixels
  match RT_CURSOR 3), ingame2.png (over build icon). Game still reaches in game. `cargo test --workspace` 153 passed.
- Docs: user32-gdi32.md "Cursor drawing", status.md, android.md, architecture.md, integration-log.md.

## Next (not done)
- Look at it on the Android emulator (cursor follows taps; with the mirrored right-drag map scroll the cursor
  shows the emulated position, not the finger).
- Icons (`LoadIconA`) are not decoded (not needed).
