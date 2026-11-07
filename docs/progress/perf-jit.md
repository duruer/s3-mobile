# perf-jit (2026-11-07) — IN PROGRESS

Goal: optional arm64 JIT for hot guest blocks in `port/crates/s3-cpu` (bit-exact vs the interpreter, same retired
counts, same budget stops; default OFF, env `S3_JIT=1`; iOS builds keep the interpreter).
Branch: `worktree-agent-a3531f78a2f0d5bff` (worktree of settlers-3, base `e8042b2`).
Scratch: worktree `scratch/perf-jit/` (gitignored): `fast.ps1` (power-throttling-off launcher, args from a file),
`det.sh <s3det> <outdir> [env]` (90000 vms inject run), `bench.sh <test exe> [runs] [env]`, `asm.sh <file.s>`
(NDK clang + llvm-objdump; disassemble `S3_JIT_DUMP=1` output `s3jit.s`), `base/` = baseline binaries (e8042b2).

## Design (implemented)
- `src/jit/{mod,compile,a64,execmem}.rs`, cargo feature `jit` (default on, compiled only on aarch64), runtime
  switch `S3_JIT=1` / `s3_cpu::set_jit_enabled` / `Cpu::set_jit(Some(bool))`.
- Trampoline saves x19..x30; guest EAX..EDI in x19..x26, x27 = Cpu, x28 = mem base, x13 = budget left;
  lazy flags in x9/x10/x11 (+x12 lcf) inside a block, interpreter format in Cpu between blocks.
- Block entry: `left >= n` else exit (run loop runs a partial block). Memory ops: `addr < 0x10000` -> side exit
  (run loop continues the block from op i with the interpreter handlers).
- Untranslated instructions: helper call into the interpreter handler (`jit_op`, op pair `[op, block_end]`).
- Links: direct `b` if the target is compiled at compile time, else per-EIP slot (indirect); indirect branches
  via a 32768-entry direct-mapped table. Native fast-path blocks get a stub that calls the native fn (like the
  run loop); hooked blocks are never compiled. Any invalidation / clear / full arena -> drop all JIT code.

## Done
- Commit 18ba153: first version + tests. Fixed pre-existing interpreter bug: block chaining treated EIP 0 as an
  empty successor slot (jump to address 0 crashed the host).
- Direct links, native stub, bigger jtab, stats (`s3det` prints `jit_stats()` when JIT on; `S3_JIT_FBPROF=1`
  fallback/exit profile; `S3_CPU_TIME=1` time in Cpu::run).

## Results so far (s3det 90000 vms, `EVERY=90000`, throttling off)
- Interpreter: Cpu::run 5.66 s (process 6.9-7.3 s). JIT: Cpu::run 2.25 s (process 3.7 s).
- Determinism: per-second hashes + insns=11886929367 IDENTICAL to baseline e8042b2.
- bench_mips: interpreter ~1200-1350, JIT ~6300 (one run 2300; noisy).

## Next
- Native: loop/jcxz, shifts r8/r16 + by CL, idiv/div, movzx r16, FS mov, movsd; x87 fild/fistp are the top
  fallbacks (7.6 M each) -> maybe a fast path in s3-mods.
- s3run fps, APK build, docs (cpu.md JIT section, performance.md, determinism.md, journal).
