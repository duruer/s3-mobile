# Progress: integration-r1 (integration runner, round 1)

Goal: build s3run, run headless against the GOG install, find how far the game gets, classify blockers,
write docs/porting/integration-log.md.

## Status: DONE (2026-11-05)
- Reached in-game (random map 384) at ~16 s; build menu + building placement work with injected input.
- Fixed: gdi32 DEFAULT_BITMAP for new DCs (SelectObject on memory DC returned 0 -> game crash), test added.
- Added `S3_SAMPLE=1` per-thread sampler to s3run.
- Results, recipes, blockers, performance: docs/porting/integration-log.md.
- Local screenshots: scratch/keep/r1-*.png (never commit).

## Verify
See "How to run" in docs/porting/integration-log.md.
