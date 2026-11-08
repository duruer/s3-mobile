# panel-text (crisp Classic panel texts in the window layer)

Goal: panel texts / tooltips drawn by the game with DrawTextA into the 1-bpp text DIB look blurry when the panel is
enlarged (Classic Touch). Redraw them in the mods window layer and keep the low-res version out of the frame.

## Done (2026-11-08)
- RE (game-state-ui.md section 6 "Text DIB composite").
- `s3_win32::user32::draw_text_layout` (DrawTextA layout, pub); `s3-mods` depends on `s3-win32`.
- `port/crates/s3-mods/src/panel_text.rs` (hooks + mod), magnifier inset texts (`ui_classic.rs`).
- Verified desktop (screenshots local in `scratch/pt/d1..d5`, `d0`/`d4` = `S3_PANEL_TEXT=0`).
- Docs: display.md, ui-classic.md, journal.

## Open
- Not checked on Android / in the Mobile HUD panel peek (should work: drawn through the same viewport).
- Map labels (`0x4bffe3..`, `0x4c2841..`, `0x4c8bad..`, `0x4c9058..`) are per-frame texts: not seen in tests.

## Verify
`sh scratch/pt/run.sh scratch/pt/dN 70 "60000:wtap 37 851;62500:wshot scratch/pt/dN/a.png"` (copy a Classic
`mods-settings.json` into `scratch/pt/dN` first).
