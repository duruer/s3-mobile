# Progress: hud-gaps (issue #2, Mobile HUD missing features)

Goal (user, 2026-11-19): do both, release together (one push at the end):

1. One-tap switch HUD -> Classic Touch (Classic -> HUD is already one tap).
2. Native HUD features: specialist commands (geologist "find minerals", thief, pioneer) on the unit card;
   goods tab "Tools and Weapons" (toolsmith / weaponsmith production settings).

Rule: commit locally per step, push only once at the end (each `feat:` push = a release).

## Done (2026-11-19)

- RE, code, s3run verification, docs (ui-hud.md "Missing features", game-state-ui.md section 8,
  game-state-units.md), journal, README. Committed and pushed as one `feat:` commit.

## Open

- Hold Position / Take ammunition / Take land / Steal goods: STATIC only (same request path as the verified
  geologist order). Priority tabs (0x0a/0x0b/0x11), ships (0x16), soldier recruitment: not native yet.

## Verify

- `cargo test -p s3-mods`; s3run inject recipes in docs/porting/ui-hud.md.

## Log

- 2026-11-19: RE done (page 5 buttons per kind, goods tab 2 commands 0x07/0x08, sector via logic+0x88/8c/90).
  Code: ui_hud_game.rs (SelectionOrder, ToolOrder/ToolShare, ToolsState in snapshot), ui_hud.rs (Sheet::Tools,
  selection card row 3 = kind button + "Classic panel", menu "Classic panel", Stats + toolsmith links).
  VERIFIED in s3run: Tools sheet shows game values (weapons 16/12/8), + / bar change them. Scythe/Fishing rod names
  were swapped in GOODS (fixed). Next: verify geologist "Find resources" + soldiers "Hold position", docs, commit.
