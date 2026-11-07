# perf-tile: native fast path for the frame renderer tile loop 0x4b26c0 [done]

Goal: native fast path (s3-mods `fast_tile.rs`) for the hot tile loop of `0x4b26c0` (draw thread, 42 % of guest
instructions in game, HUD 2316x904). Contract: docs/porting/performance.md "Native fast paths".

## Done (branch `worktree-agent-a9494cd44a081baee`)
- `port/crates/s3-mods/src/fast_tile.rs`: `frame_tiles` (cell loop `0x4b2e44..0x4b330e` incl. row advance, sound
  statistics, empty object call inlined via `fast_obj::plan/apply`; hash checks of the row head and of the 24
  instruction ranges of `0x4d3050`'s empty case) and `land_mark` (`0x4b277e..0x4b27a7`).
- `fast_obj.rs` split into `plan` / `setter_ok` / `apply` (behaviour unchanged).
- `fast.rs`: registration, `extra_ranges` (hooks in the inlined code disable `frame_tiles`), env `S3_FAST_OFF`.
- Tests: `differential_frame_tiles` (400 cases, path coverage counters), `differential_land_mark`,
  `obj_empty_path_hashed`; all 68 s3-mods tests pass with the real exe.
- `tools/perf/code_hash.py --concat`.
- Results: profile `0x4b26c0` 41.9 % -> 6.2 %; HUD 2316x904 uncapped fps 28.9/35.0 -> 41.0/46.5; s3det 90000 vms
  identical vs `S3_FAST=0` and vs `S3_FAST_OFF=frame_tiles,land_mark` (insns=11817005518).
- Docs: performance.md (table, perf-tile paragraph, next), status.md, journal.md.

## Notes
- The worktree builds on Windows only with the uncommitted `s3-mem` Windows support copied from the main checkout
  (`crates/s3-mem/Cargo.toml`, `src/sys.rs`, `Cargo.lock`); NOT committed here (belongs to another agent's work).

## Next (not this task)
- Shadow sprite loops `0x5fd080` / `0x5fd120` (now 40 % of the profile), `0x5fc860`; phone re-measure.

## Verify
- `S3_EXE=.../S3.EXE cargo test --release -p s3-mods fast_tests -- --nocapture`
- s3det 90000 vms S3_FAST=0 vs default (tools/determinism-test.sh inject), identical hashes + insns
