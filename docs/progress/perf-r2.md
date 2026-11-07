# perf-r2 (2026-11-07)

Goal (user): do perf items 1-7 of docs/porting/performance.md "Measurements 2026-11-07" (not 8 = default zoom).
Baseline: phone HUD zoom 10 (1944x760) 50-56 fps; desktop ARM64 headless 2316x904 zoom 10 33-35 fps (S3_FPS_CAP=0).

Order: 3 APerformanceHint (s3-android) -> 2 host presentation (s3-dx blit, copies, HUD layer) -> 1/4/5 in parallel
agents (worktrees) -> 6 JIT -> 7 multi-core.

Done:
- Item 3: s3-android/src/perf_hint.rs (APerformanceHint via dlsym, engine thread). Fold4: `dumpsys performance_hint`
  "HAL Support: false" -> createSession returns NULL -> no effect on this phone (kept for API 33+ devices with HAL).
- Item 2: s3-dx blit 16-bit row memcpy; s3-mods overlay `rounded` spans (pixel-identical test, 400 random cases);
  s3-runtime cursor keeps only the patch under the cursor; `Machine::present_owned` (no last_guest_frame copy),
  `present_cursor_owned` (overlay output not copied); s3-android presenter swaps buffers with the frame slot.
  Phone simpleperf: host presentation share ~17 % -> ~10 % of the engine thread. fps comparison noisy (random map).

- Items 1/4/5 done by worktree agents and merged into the (uncommitted) main checkout with git merge-file:
  perf-tile 05eab4b (frame_tiles, land_mark), perf-shadow 00cfb85 (shadow_zero, shadow_rows), perf-interp 7f38075
  (threaded dispatch, fused cmp/test/dec+jcc, new fast forms, 32-byte Op). Merged state: s3-mods 75 tests ok;
  s3det 90000 vms fast vs S3_FAST=0 IDENTICAL and identical to the pre-interp merge (insns=11886929367 with this
  checkout's other changes); wall 13.1 s -> 8.9 s.
- Known failing on Windows ARM64 before this work: s3-cpu fpu::golden (f2xm1 differs: arm64 desync risk, see
  determinism.md), s3-runtime vfs_case_insensitive_and_overlay (path separator).
- Windows Defender quarantines docs/reverse-engineering/wine-trace.md (Trojan:Python/RemoteShellExec.Z!MTB, false
  positive); restore with git restore; needs a user decision (exclusion).

- Committed on main (user approved): 73b7244, c2bc88b, 9e43283, d8e9903, e8042b2. Defender exclusion: user chose it
  but the UAC prompt was cancelled -> not added; wine-trace.md restored in e8042b2 (Defender keeps deleting it).
- Items 6 (perf-jit) and 7 (perf-mt) running as worktree agents from e8042b2.

Next: phone measurement (phone locked at 14:47), merge 6/7 results.
