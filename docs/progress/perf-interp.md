# perf-interp (2026-11-07) — DONE

Goal: speed up the s3-cpu interpreter with bit-exact results and identical retired-instruction counts (lockstep MP).
Owner: port/crates/s3-cpu only (+ docs). Write-up: docs/porting/performance.md "perf-interp", docs/porting/cpu.md.

## Done (branch worktree-agent-a8bc815ea88c16159)
1. Tail-call threaded dispatch inside blocks (`fast::next`, `block_end` sentinel, no per-op EIP store, branch
   handlers return directly, budget-truncated blocks via `exec::step` in `Cpu::run_partial`), lean run loop.
2. Fast forms: imul/mul/cdq/cwde, 8/16-bit + CL shifts, setcc r8, neg/not r32, inc/dec m32.
3. 32-byte `Op` (generic insn pointer in disp/imm), `Block::special`, Block `repr(C, align(64))`.
4. Boxed blocks + pointer chaining.
5. `cmp/test/dec + jcc` superinstructions (`fast::cmp_jcc`, `fuse_cmp_jcc`).
6. `DecodeCache::profile_pairs()`.
7. Tests: budget_stops_are_exact, mid_block_exits_count_exactly, *_forms_fast_vs_generic, cmp_jcc_fusion_fast_vs_generic.

## Results (Windows ARM64 PC, throttling off)
- bench_mips 440 -> 1160 MIPS; s3det 90000 vms Cpu::run 16.6-18.8 s -> 9.7-9.8 s; s3run HUD 2316x904 93-102 -> 126-190 fps.
- s3det old (f0a50f7 s3-cpu) vs new: IDENTICAL (insns=11817005518), also with EFLAGS/FS/x87 last-IP+tags hashed.
- s3-mods tests (incl. fast_tests vs real exe) pass.

## Open
- fpu::golden tests fail on Windows ARM64 at f0a50f7 already (f2xm1) -> determinism.md open item.
- s3-runtime vfs_case_insensitive_and_overlay fails on Windows at f0a50f7 already.
- Used for measuring but not committed (reverted): a copy of the main checkout's uncommitted Windows s3-mem backend
  (needed to build this worktree on Windows), s3-desktop `.pairs` output, s3det "s in Cpu::run" print and a stronger
  CPU hash. The benchmark scripts lived in the agent scratchpad (fast.ps1 = power-throttling-off launcher).
- Ideas: return-address cache for `ret`, mov+mov fusion, memory base in a register; arm64 JIT later (notes in
  performance.md).
