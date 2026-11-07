# perf-landfull (progress notes)

Goal: native fast path for the dirty-triangle rasteriser of the landscape pass `0x54cbd0` (full landscape redraw
after a zoom-out size switch). Status: **done** (2026-11-08), see docs/porting/performance.md "perf-landfull".

## Done
- `port/crates/s3-mods/src/fast_land_rows.rs` (`land_rows`, row head `0x54e4bc` .. exit `0x54e6e8`), registered in
  fast.rs/lib.rs; `differential_land_rows` in fast_tests.rs (600 cases, 564 native, branch coverage counters).
- Measurements (pinch-out gaps JIT off/on, S3_PROFILE) and s3det 90 000 vms IDENTICAL (fast / S3_FAST=0 /
  S3_FAST_OFF=land_rows / JIT with and without fast paths), docs + journal.

## Next (not done)
- Per-triangle setup `0x54d015..0x54e4bc` (~200 instr per dirty triangle, branchy texture-pair choice) and the
  dirty case of the triangle loop head: now ~15 % of the pinch-out guest instructions; only if the phone still
  stutters. Measure on the phone.

## Verify
- `S3_EXE=... cargo test --release -p s3-mods differential_land_rows -- --nocapture`
- scripts in worktree `scratch/plf/` (pinch.ps1, gaps.py, det.sh, det_all.sh), outputs in
  `C:/Users/<user>/Projects/settlers-3/scratch/perf-landfull/`.
