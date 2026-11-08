# Progress: hud-native (Mobile HUD: no original panel peek, 2026-11-08) — FINAL

Goal: UI mode B never shows the original left panel. Replace the panel peeks (building Details page 4, Save page 7,
goods tabs page 2, settlers tabs page 3) in `port/crates/s3-mods/src/ui_hud.rs` with native HUD sheets (window
layer), data from game memory, actions via game functions / `LogicAction`.

## Done
- RE: page 4 handler (0x52fe6b..0x5322xx), page 7 save (0x52e21a..), recruitment (0x52cbf7..) ->
  docs/reverse-engineering/game-state-ui.md section 8, game-state-units.md command table.
- Code: building sheet, save sheet, recruitment row in the settlers sheet, goods sheet without tabs; peek removed
  (ui_hud.rs, ui_hud_game.rs `LogicAction::{Request, WorkArea, Recruit, ReadSaveSlots, SaveGame}`, ui_hud_data.rs
  tables, zoom.rs no peek minimum height).
- Verified on desktop (s3run 2316x904): building sheet (tower + site), stop building, priority, max garrison,
  recruit, save slot 1 + list. Screenshots main repo scratch/hud-native/.
- cargo test -p s3-mods: 87 + 1 pass. build-apk.sh OK.

## Open
- Work area button STATIC (needs a finished woodcutter etc.).
- Not native: goods Tools/Weapons + priority pages, production statistics, soldier recruitment, special building
  pages (docs/porting/ui-hud.md "No original panel").
- Not tried on the phone.

## Verify
`cargo test -p s3-mods`; recipe in docs/porting/ui-hud.md "No original panel".
