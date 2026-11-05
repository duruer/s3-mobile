# Progress: re-units (W2 game-state RE part 1) — DONE 2026-11-05

Goal: addresses + layouts of selection, settler table, building table, map/tile + screen->map transform,
command functions (move/attack/select/build/destroy/priority) and the mouse click dispatch.

## Result
- Doc: docs/reverse-engineering/game-state-units.md (AGENTS.md map row added; integration-log.md inject line added).
- Code: port/crates/s3-desktop/src/debug_units.rs (read-only `xdump`/`xpeek`/`scan` inject commands, `@` pointer
  steps, `+` sums; unit test `split_units_commands`), wired into s3-desktop/src/main.rs with 4 small additive edits.
- Tests: `cargo test -p s3-desktop` 7 passed. Game reaches in-game with the new binary (run E: box select of 14
  soldiers + right-click move, screenshots scratch/units/e1.png, e2.png).

## Verified at runtime
- logic obj = app+0x90 (s3run: [0x700ffee0] = 0x10113d48), gd = [logic+0x3a0] (0x18790000)
- settler/building/tile tables, UI selection arrays, logic selection copy (after cmd 3), building click -> 0x7dfd6c,
  modes gd+0x2d284 (1 build, 4 building, 5 units), placement -> building entry at gd+0x2d2e4/e8, ScreenToMap result

## Open (see doc "Open questions")
- type names, move modes 0xd/0xe/0xf, commands 0x04/0x05/0x12..0x1c, construction-state bytes, map->screen formula

## How to resume / verify
- Regenerate disassembly: `objdump -d -M intel --no-show-raw-insn game-files/gog-s3-ultimate/S3.EXE > scratch/units/s3.asm`
- Run recipe: doc section "Tools" (in-game actions at >= 50 s on a loaded machine).
