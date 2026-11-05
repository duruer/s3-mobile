# Progress: hud-polish (Mobile HUD fixes, 2026-11-05) — FINAL

Goal: (1) hide game's left panel + its minimap completely in HUD mode; (2) own HUD minimap widget (camera rect, drag
pans, tap opens full-screen map overlay, tap there jumps + closes); (3) HUD menu entries for FPS counter / display
mode / zoom settings. Owns s3-mods/src/ui_hud*.rs (+ ui_hud_minimap.rs).

## Done
- RE (REA, frame renderer 0x4b26c0): minimap = square RGB565 image mw x mw at [snap+0x4c]; blocks
  0x4b2b34->0x4b2ce5 and 0x4b366a->0x4b3cf3 draw minimap/frame art/camera rect; camera rect formula at 0x4b3a3c.
- ui_hud_game.rs: skip hooks for both blocks while full layout active; snap recorded in panel hook; original
  layout recorded at 0x4c6d70 return (custom res); landscape cap 1024 -> 4096.
- ui_hud_minimap.rs (new): read image, MapGeom (shear), draw, tests.
- ui_hud.rs: widget bottom-left (drag pans via CenterOn, tap opens Sheet::Map), full map overlay (tap = jump +
  close, Close button, Back/Esc via new Mod::on_key in lib.rs), Settings sheet (display mode/filter/FPS via
  crate::display, zoom -/+ placeholder = F1-F3, HUD size, full map, UI mode), layout inside visible_game_rect,
  resource strip moved top-left (FPS counter top-right).
- cargo test -p s3-mods: 55 pass.

- Desktop VERIFIED (s3run inject, scratch/hud-polish/*.png): game minimap gone, widget drawn, drag pans,
  tap opens full map, tap in map jumps + closes, Esc (key 0x1b) closes, settings sheet toggles FPS counter (saved),
  cover mode (full map off) shows the game's minimap again, Classic mode unchanged and in game.
- Docs: ui-hud.md (new sections), game-state-ui.md (minimap RE), mods.md (on_key).

- Final re-test on the current tree (incl. other agents' s3-cpu changes): workspace check OK, cargo test -p s3-mods
  55 + 1 pass, HUD in game, drag/tap/full-map jump OK (scratch/hud-polish/final-*.png, montage3.png).

## Open / next
- Zoom -/+ in Settings is a placeholder (F1-F3 via touch_logic::pinch_zoom_key); swap `ui_hud::zoom_step` /
  `zoom_label` to res-zoom's API when it exists (no res*/zoom* files yet at finish).
- Not tried on Android/phone (Verify phase): check the widget size on 2229x904 and Back closing the map.
- Full map shows only what the game's minimap shows (unexplored = black); no extra building markers.

## Verify
docs/porting/ui-hud.md "Testing on desktop".
