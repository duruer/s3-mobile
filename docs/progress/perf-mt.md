# perf-mt: use more than one host core — DONE (prototype, no reliable gain)

Goal: design + feasibility of multi-core execution, then a prototype behind `S3_MT=1` (default off, default mode
bit-identical), with determinism evidence and fps numbers. Write-up: [multicore.md](../porting/multicore.md).

## Done (branch worktree-agent-ad1bc5b9109dfdd7a)
- RE of the draw/logic handshake: triple-buffered render snapshot (InterlockedExchange slot `0x700ffeb0` + events).
- `S3_HOST_PROF=1` host profiler, per-thread `Cpu::run` time in the summary, `S3_TIMELINE`, `S3_MT_DEBUG`.
- Prototype A: `s3-runtime/src/mt.rs` (big lock, draw worker, lock-free budget exits, wake epoch, Windows sleep-poll,
  deferred present with 600 µs delay), `GuestMemory::alias_view`, `Machine::idle`, s3run `S3_MT=1`.
- Test `s3-runtime` `mt_pipeline_matches_single_thread`.
- `s3det --write-trace FROM-TO --write-trace-out FILE` (+ `Machine::run_observer`).
- `mp-local-test.sh` `HOST_MT`/`JOIN_MT`/`PYTHON`; `tools/perf/fast.ps1`, `tools/perf/fps-bench.ps1`.
- Evidence: s3det 90000 vms identical to e8042b2 (`insns=11886929367 mem=6e141c4900044d20`), also with write trace;
  write trace: draw thread writes 0 bytes of `gd`; MP: 6 runs with MT peers, simulation tables identical every tick
  (one MT/MT run had the known world-tile-only difference, not reproduced in 3 runs with dumps).
- fps: MT -40..+17 % vs default, noisy -> no gain to ship (reasons in multicore.md).

## Open / next
- Option B: overlay display list rasterised outside the machine (presenter thread), then retry A.
- Catch the mp3 world-tile difference with `S3_MP_DUMP_STEP` (same signature as the default-mode run long3).
- Android: MT not wired (env only in s3run).

## Verify
- `scratch/perf-mt/det.ps1`-style: `s3det --vms 90000` with the tools/determinism-test.sh inject, compare hash lines.
- `cargo test -p s3-runtime mt_pipeline`; `tools/perf/fps-bench.ps1 -Bin ... -Out ... [-Mt 1]`.
