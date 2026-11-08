# hud-settlers (2026-11-08) — done

Goal: Mobile HUD settlers sheet + selection card: real names per settler type, game icons next to each row
(sampled at runtime), no "Type N".

## Done
- RE: settler types (docs/reverse-engineering/game-state-units.md "Settler types"), Population tab
  (game-state-ui.md "Settlers page tab 1").
- Code: `ui_hud_data::settler_info` / `pop_icon_rect` / `build_slot_of` / `war_machine_building` (+ test);
  ui_hud.rs harvest step SETTLERS_PAGE (6), `settler_icon`, `settler_total`, settlers sheet + selection card with
  icons, `log_population_check` (S3_HUD_DEBUG); ui_hud_game::dump_settler_types (S3_HUD_SETTLER_DUMP).
- Verified on desktop (2316x904, 2176x1812, three races), counts equal to the original tab. Docs + journal.

## Open
- Types 0x14 (worker, no building), 0x25 (tower figure), 0x2a unknown; professions only seen at runtime for 3/4/6/7.

## Verify
- `cargo test -p s3-mods`; s3run MobileHud recipe in docs/porting/ui-hud.md, Settlers `wtap 1486 855` (2316x904).
