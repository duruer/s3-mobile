# Progress: ui-hud (UI mode B "Mobile HUD", W4) — FINAL 2026-11-05

Goal: native large-button HUD (JSettlers-like) per docs/porting/mobile-vision.md. Doc: docs/porting/ui-hud.md.
Owns `port/crates/s3-mods/src/ui_hud.rs` (mod), `ui_hud_game.rs` (hooks + game state), `ui_hud_data.rs` (tables).
Wired in `s3-mods/src/lib.rs` (`ui_hud::install(m, applied.is_some())` before `hooks::commit`).

## Done
- Full-screen map (layout patch at 0x4c6d70 return, landscape buffer widened at 0x54bec0, panel 0x4b4d00 skipped,
  ScreenToMap 0x4ff170 corrected), cover mode fallback.
- Bottom bar, build sheets with icons sampled from the game's panel, placement (pokes + 0x4ff3f0), unit card + Move,
  building card + Destroy (cmd 0x06), settlers/goods sheets + original pages in a panel peek, minimap tap
  (0x51b560), game menu (save/quit via peek, pause, UI mode switch, full-screen toggle, HUD size).
- Goods per player (tile owner), goods ids partly mapped, goods icons sampled from the Inventory page.
- Screenshots scratch/ui-modes/B-01..B-12. Docs: ui-hud.md, AGENTS row, mods.md note, game-state-ui.md additions.
- Tests: cargo test -p s3-mods 43 pass; workspace: all pass except s3-dx `dplay_factories_and_stubs` (mp-dplay
  agent's in-progress DirectPlay work, not touched here). Default (Classic) mode still reaches in game
  (scratch/ui-hud/default-ingame.png).

## Open
- Runtime F1-F3 switch in HUD mode leaves a black band (start at 800x600 via registry works).
- Goods 1/2 and 17/32 order HYPOTHESIS; settler type names partial.
- Not tried on Android yet.

## Verify
mkdir -p scratch/ui-hud/data && echo '{"ui_mode":"MobileHud"}' > scratch/ui-hud/data/mods-settings.json, then the
recipe in docs/porting/ui-hud.md "Testing on desktop".
