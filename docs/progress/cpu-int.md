# Progress: cpu-int (x86-32 integer interpreter, crate s3-cpu except fpu*.rs)

Goal: a complete, exact and fast integer interpreter. Doc: docs/porting/cpu.md.

## Done
- Decode cache rewritten (arena + L1 array, cache moved out of the Cpu during run, no Arc).
- Threaded dispatch (src/fast.rs) with fast handlers for hot 32-bit forms. Generic path in exec.rs.
- Flags computed in one step with a parity table.
- Gaps filled: jcxz (67 E3), CX loops, near_branch_target, BCD ops, salc, popfd mask (AC/ID), cpuid without
  MMX, privileged/far ops -> GP, ud0/ud1 -> #UD, 16-bit address wrap, 0x67 string ops refused.
- Tests: src/diff_tests.rs (native differential tests, x86_64) plus tests.rs additions; all pass.
- Benchmark: 88 -> ~170-185 MIPS.

## Next (if resumed)
- Run s3run headless against the game once the workspace builds (s3-win32 was mid-edit by another agent) and
  look for CPU Fault/Unimplemented exits.
- Optional speed-ups: lazy flags, block chaining, 8-bit fast forms.

## How to verify
CARGO_TARGET_DIR=~/Projects/settlers-3/port/target cargo test -p s3-cpu -j 4
CARGO_TARGET_DIR=... cargo test --release -p s3-cpu -j 4 -- --ignored --nocapture bench_mips
