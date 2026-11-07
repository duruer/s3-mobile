# perf-jit (2026-11-07) — IN PROGRESS

Goal: optional arm64 JIT for hot guest blocks in `port/crates/s3-cpu` (bit-exact vs the interpreter, same retired
counts, same budget stops; default OFF, env `S3_JIT=1`; iOS builds keep the interpreter).
Branch: `worktree-agent-a3531f78a2f0d5bff` (worktree of settlers-3, base `e8042b2`).
Scratch: worktree `scratch/perf-jit/` (gitignored; `fast.ps1` = power-throttling-off launcher).

## Design (decided)
- Chained JIT: trampoline saves callee-saved regs, guest EAX..EDI in x19..x26, x27 = cpu, x28 = mem base,
  x13 = remaining budget, x9/x10/x11 = lazy flag operands (la/lb/lr), x12 = lcf.
- Each block entry checks `left >= n` (else exit to run loop -> run_partial), static successors linked through
  per-eip slots (indirect `br` via a slot table, slot = exit stub until the target is compiled), indirect
  targets through a direct-mapped (eip -> code) table.
- Memory ops check `addr < 0x10000` -> side exit at instruction i: the run loop continues the block with the
  interpreter's threaded handlers from op i (so faults are exactly the interpreter's).
- Lazy flags stored to Cpu (same lk/lsz/la/lb/lr/lcf format) at block exits.
- Unsupported instructions: helper call into the interpreter handler for that one instruction.

## Done
- (nothing committed yet)

## Next
- exec memory + arm64 encoder + compiler + run-loop integration + differential tests.
