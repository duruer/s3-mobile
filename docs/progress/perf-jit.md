# perf-jit (2026-11-07) — working subset DONE (default off), follow-ups listed below

Goal: optional arm64 JIT for hot guest blocks in `port/crates/s3-cpu` (bit-exact vs the interpreter, same retired
counts, same budget stops; default OFF, env `S3_JIT=1`; iOS builds keep the interpreter).
Branch: `worktree-agent-a3531f78a2f0d5bff` (worktree of settlers-3, base `e8042b2`).
Scratch: worktree `scratch/perf-jit/` (gitignored): `fast.ps1` (power-throttling-off launcher, args from a file),
`det.sh <s3det> <outdir> [env]` (90000 vms inject run, `EVERY=` hash interval), `cmp.sh` (alternating JIT off/on),
`bench.sh <test exe> [runs] [env]`, `fps.sh <s3run> <outdir> [env]` (HUD 2316x904 uncapped, mean fps 50-80 s),
`asm.sh <file.s>` (NDK clang + llvm-objdump; disassemble `S3_JIT_DUMP=1` output `s3jit.s`), `dump.sh`,
`base/` = baseline binaries (e8042b2), `jit3/` = latest.

## Done (commits on the branch)
- 18ba153 JIT v1 + differential tests; fix: chaining treated EIP 0 as empty successor (host crash).
- e64d2e7 direct links, native fast-path stubs, 32768-entry indirect table, stats / profiling env vars.
- 43f1cc2 native loop/jcxz/div/idiv/r8-r16 shifts/shift by CL/leave/movs/stos/FS mov; `S3DET_HASH_FULL=1`.
- docs commit: cpu.md "JIT", performance.md "perf-jit", determinism.md "JIT", android.md (`jit.txt`), journal,
  AGENTS.md map; s3-android reads `<data dir>/jit.txt`.

## Verification
- `cargo test --release -p s3-cpu`: all pass except the known `fpu::golden` (f2xm1) failures.
- `jit_tests`: random programs (1000 seeds x 3 modes), hooks/natives/invalidation, native stub.
- s3det 90000 vms: JIT vs baseline IDENTICAL (91 lines, insns=11886929367); with S3DET_HASH_FULL=1 JIT on/off
  IDENTICAL.
- APK `build-apk.sh --abi arm64-v8a` builds (not installed).

## Results (same-session pairs)
- bench_mips 1074-1176 -> 3976-4777 MIPS. s3det Cpu::run 6.9 -> 2.8 s (fast paths on), 13.7 -> 3.1 s (S3_FAST=0).
- s3run HUD 2316x904: ~1.3x fps (356 -> 443, 248 -> 328 on different random maps); now host-side bound.

## Next / open
- x87 copy loop `0x5c3e40` (fild/fistp m64 + fxch = 19 M of 20 M fallback calls): native translation or s3-mods
  fast path reproducing the f64 round trip.
- `rep movs/stos` natively (0.8 M), adc/sbb, 8/16-bit shifts by CL.
- Return-address prediction (shadow stack) for `ret`; fault-check elimination for repeated `[reg+disp]`.
- Phone test: push `jit.txt` = `1`, compare fps + run s3det-equivalent determinism on device.
