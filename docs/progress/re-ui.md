# re-ui (W2 part 2: camera, panel, cursor mode, dialogs, resources, text hooks) — DONE 2026-11-05

Goal: document game-state UI internals of GOG S3.EXE. Result: docs/reverse-engineering/game-state-ui.md (complete;
open questions listed at its end). AGENTS.md index row added; part-1 doc rows for gd+0x2d284/0x2d288/0x2d28c corrected.

## Code touched
- port/crates/s3-desktop/src/debug.rs (read-only debug inject cmds dump/peek/poke/regs/watch/dumpall; new:
  `dumpall DIR mapped`) + test split_dumpall_mapped_flag. main.rs wiring unchanged from earlier step (small).
- tools/re-ui/: run.sh, dis.sh, panelmap.py, strings.py, findstock*.py, scan.py, diff.py, peek.py, rtti.py.

## Verify
cd port; CARGO_TARGET_DIR=$PWD/target cargo test --release -j 4 -p s3-desktop   (8 passed)
sh tools/re-ui/run.sh 50 "45000:shot ../scratch/re-ui/final.png"   -> in game (verified 2026-11-05).
Evidence (local): scratch/re-ui/t1 (poke build select + placement), t2/t3 (goods/settlers pages), t4 (F1/F3 resolution),
t5 (P pause), res2s.png (Resolution=2), m1.png (menu page).

## Next (for someone else)
- Good id names, pile ownership, map->screen formula, pages 6/0xb, test hiding the panel via a mods hook on 0x4b4d00.
