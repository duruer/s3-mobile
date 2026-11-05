# Progress: ui-classic (UI mode A "Classic Touch") — FINAL 2026-11-05

Goal: mode A per docs/porting/mobile-vision.md. Doc: docs/porting/ui-classic.md (AGENTS row added, mods.md pointer).

## Done
- `port/crates/s3-mods/src/ui_classic.rs` (mod + hooks HandleInput 0x524fa0 / gfx ctor 0x42c820) and
  `ui_classic_panel.rs` (pure panel model, 5 tests). lib.rs: `pub mod ui_classic;` + `ui_classic::install(m, applied.is_some())`.
- Features: panel magnifier bubble with game string names, snap to button centres, floating Menu/Close (page 7),
  context chip. Cancel button left to touch.rs (✕ top right), Menu moved bottom right.
- Verified headless (recipe in ui-classic.md): reached in game, all features work; screenshots scratch/ui-modes/A-*.png.
- `cargo test -p s3-mods` ok; workspace ok except s3-dx `dplay_factories_and_stubs` (concurrent mp-dplay work, not touched here).

## Next (open)
- names for pages 4/5/8 (game tooltip id not picked up), touch.rs press ring under Menu, map save-slot/quit-confirm
  sub-pages, Android emulator run.
