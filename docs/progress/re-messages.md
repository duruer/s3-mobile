# re-messages (RE in-game text messages: tutorial text, event messages, chat, tooltips) — DONE 2026-11-08

Goal: find how/where in-game messages are built, stored per frame and drawn, for a later constant-size overlay
(suppress game drawing, redraw strings, forward clicks). Result: docs/reverse-engineering/game-state-ui.md section 7
"In-game messages" (+ summary row, journal entry). Scratch: C:\Users\<user>\Projects\settlers-3\scratch\re-messages
(local only: s3run build in target/, runs r1..r6, game data).

## Code touched
- port/crates/s3-desktop/src/debug.rs: `poke` also calls `Machine::invalidate_code` (code patches take effect).
- tools/re-messages/: run.sh (tutorial / START=scen runner), slots.py (decode ring dumps), lst.py (listing viewer).

## Verify
cd port; CARGO_TARGET_DIR=<scratch>/target cargo build --release -p s3-desktop
sh tools/re-messages/run.sh r3 20 "10000:shot r3/a.png;12000:poke 0x4d8400 c20400;14000:shot r3/b.png"
  -> a.png has the tutorial text, b.png none.

## Open (for someone else)
- MP: chat line / chat icons / colour mark only STATIC; arrow click area in HUD layout ([0x730f24] = 0) HYPOTHESIS.
