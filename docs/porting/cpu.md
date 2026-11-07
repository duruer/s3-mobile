# x86-32 integer interpreter (`port/crates/s3-cpu`)

Owner of this doc: the cpu-int task. The x87 part is in [x87-fpu.md](x87-fpu.md). Crate overview and contracts
are in [architecture.md](architecture.md#cpu-s3-cpu). Which instructions S3.EXE uses: [static-isa.md](../reverse-engineering/static-isa.md).

## Files

| File | Content |
| --- | --- |
| `src/lib.rs` | `Cpu`, `CpuExit`, `FaultKind`, `Context`, the decode cache (`DecodeCache`) and the run loop |
| `src/fast.rs` | Threaded dispatch: picks a handler per decoded instruction; specialised handlers for hot forms |
| `src/exec.rs` | Generic semantics of every integer instruction (`step`), operand helpers, string ops, shifts |
| `src/flags.rs` | Eager EFLAGS helpers (`alu_add/alu_sub/alu_logic/set_szp/cond`), parity table |
| `src/fpu*.rs` | x87 (fpu agent) |
| `src/tests.rs` | Hand-assembled snippets (iced `code_asm`), `run_both` cross-checks fast vs generic path |
| `src/diff_tests.rs` | Differential tests against the real host CPU (x86_64 only) |
| `src/jit/` | Optional AArch64 JIT (perf-jit, 2026-11-07): `mod.rs` (state, trampoline, links, helpers), `compile.rs` (block translator), `a64.rs` (hand-written encoder), `execmem.rs` (W^X code memory). See [JIT](#jit-optional-aarch64-perf-jit-2026-11-07) |
| `src/jit_tests.rs` | JIT vs interpreter differential tests (aarch64 only) |

## Execution model

- **Decode cache**: basic blocks (at most 64 instructions, end at the first control-flow instruction or before the
  thunk page). Blocks live in an arena `Vec<Box<Block>>` (boxed, so addresses are stable and chaining can use
  pointers; `#[repr(C, align(64))]`, the fields read at every block entry share one cache line). A
  `HashMap<eip, index>` sits behind a direct-mapped 8192-entry L1 array `(eip, index)`. `Cpu::run` moves the cache
  out of the `Cpu` (`mem::take`) while it executes, so instructions are borrowed straight from the arena with no
  `Arc`/refcount per block. `invalidate_range` drops overlapping blocks, clears every chaining pointer and rebuilds
  the map (rare: S3.EXE has no self-modifying code). There is one cache per `Cpu`, i.e. per guest thread. Memory
  cost is about 72 bytes per decoded instruction (iced `Instruction` 40 + `Op` 32) plus one `Op` and ~128 bytes per
  block.
- **Threaded dispatch** (`fast.rs`, reworked 2026-11-05 by perf-cpu and 2026-11-07 by perf-interp, see
  [performance.md](performance.md)): at decode time each instruction gets a 32-byte `Op` record: handler
  `fn(&mut Cpu, &mut GuestMemory, &Op) -> bool`, next EIP, register numbers (8-bit: 0..3 = AL..BL, 4..7 = AH..BH),
  displacement, immediate (pre-extended) or branch target, EA form as a const generic (`[disp]`, `[base+disp]`,
  `[base+index*s+disp]`, `[index*s+disp]`; FS/GS and 16-bit addressing go generic). A `generic` op keeps the
  pointer to its iced `Instruction` (same block) in `disp`/`imm` (`Op::set_ins`). Every block's `ops` ends with a
  `block_end` sentinel. **Tail-call threading** (2026-11-07): each non-branch handler ends with
  `next(cpu, mem, op)` = call the handler of `op + 1`, which LLVM compiles to `ldr x, [op, #32]!; br x` (a sibling
  call; even without it the depth is bounded by 65), so a block runs as one chain of indirect branches with one
  prediction site per handler; control transfers (`call/ret/jmp/jcc`, always last) return `true` straight to the
  run loop, other blocks end at the sentinel. On aarch64-linux-android the hot handlers are frameless (e.g. `mov
  r32,[base+disp]` = 14 instructions including the dispatch); Windows ARM64 keeps a frame record (`stp x29,x30`).
  Specialised handlers: `mov` r,r / r,m / m,r / r,imm / m,imm for 8/16/32 bits, ALU `add/or/adc/sbb/and/sub/xor/cmp/
  test` in r,r / r,imm / r,m / m,r / m,imm for 8/16/32 bits, `movzx/movsx` (r32 from m8/m16/r8/r16), `lea`, `push`
  (r/imm/m), `pop r`, `call` (rel/r/m), `ret`, `ret imm16`, `jmp` (rel/r/m), all `jcc` (one handler per condition),
  `inc/dec r32`, `shl/shr/sar r32,imm8`, `nop`; since 2026-11-07 also `imul` (r32,r/m32 / r32,r/m32,imm / r/m32),
  `mul r/m32`, `cdq`, `cwde`, `shl/shr/sar` r8/r16 by imm8 and r8/r16/r32 by CL, `setcc r8`, `neg/not r32`,
  `inc/dec m32` (what the in-game profile still sent to the generic path, ~1.1 % of instructions). Everything else
  uses the generic `exec::step`.
- **Superinstructions** (2026-11-07): when a block ends with `cmp`/`test` (r,r / r,imm for 8/16/32 bits; m,imm for
  some sizes and `[disp]`/`[base+disp]`) or `dec r32` directly followed by a `jcc` with one of the 12 conditions
  `cond_fast` reads from the lazy state, the compare's `Op` gets a fused handler `cmp_jcc` (decode-time peephole,
  `fuse_cmp_jcc`). Its body is the compare handler followed by the `jcc` handler (branch target read from the
  `jcc`'s own `Op` behind it), so flags, faults and counts are those of the two instructions; it saves one dispatch
  and lets the compiler fold the condition into the fresh operands (`cmp r32,r32; je` = 18 aarch64 instructions).
  The pair still counts as 2 retired instructions (counts are per block).
  Rule: a fast handler must behave exactly like `step`. `Cpu::set_generic_only(true)` disables the fast handlers
  (debugging aid). The tests run every snippet through both paths.
- **Contract of a handler**: before a block runs, `cpu.eip` holds the address after its *last* instruction
  (`Block::end`): only the last instruction can branch and no other instruction reads EIP (the generic `exec::step`
  reads `cpu.eip` only for `call`/`ret`/`jmp`/`jcc`/`loop`, which end blocks). Branches overwrite it. A failing
  handler stores its exit in `Cpu::pending_exit` and its own `Op` address in `Cpu::fault_op` (cold out-of-line
  `av` / `generic_exit`, reached by a tail call so the fast paths need no stack frame) and returns `false`; the run
  loop (`block_exit`) computes the instruction index from `fault_op`; for a fault it rewinds EIP (and the fault's
  `eip`) to the faulting instruction, as Windows would report it, and does not count it; `Interrupt`/`Halt` count
  it and leave EIP after it. Fast handlers do not update `cur_ip`; the generic wrapper does (x87 uses it as the last
  instruction pointer).
- **Run loop**: per block: thunk/magic check, budget check, successor lookup, entry counter, one `special` test
  (hooked or native), then the first handler (which runs the whole block). Retired instructions are counted through
  the remaining budget only (`instructions = base + max - left`, stored before native calls and at exits). If the
  budget ends inside a block, its first `left` instructions run one by one through `exec::step` (`run_partial`,
  cold; same results as the fast handlers by their contract), so thread switches happen at exactly the same
  instruction counts. **Block chaining**: each block caches its last two successors (start EIP + block pointer;
  cleared by `invalidate_range`), skipping the L1 lookup; in game 98.8 % of block entries hit it
  (90 000 vms s3det: 528 M + 646 M hits, 14 M L1 lookups, 118 k map/decode lookups).
- **Native blocks** (`NativeBlock`, `Cpu::set_native_blocks`): host-registered native replacements of guest code
  regions, matched by `code_hash` of the code bytes, called at block entry with the remaining budget; see
  [performance.md](performance.md#native-fast-paths-s3-cpu-nativeblock-s3-mods-fastrs).
- **Flags**: **lazy** (2026-11-05). `add`/`sub`/`cmp` without carry-in, logic ops (`and/or/xor/test`) and `inc/dec` store
  `lk` (kind), `lsz`, `la`, `lb`, `lr` (and `lcf` = CF before inc/dec); `Cpu::eflags()` materialises, `set_eflags`
  replaces. Partial updates (`set_flag`, `set_szp`) materialise first. `adc/sbb` with carry-in and everything else
  stay eager. `jcc` reads conditions straight from the lazy state (`cond_fast`). The field `eflags` is gone: use
  `eflags()` / `set_eflags()` (or `Context`).

## Semantics notes (what is exact)

- All six status flags are exact for add/adc/sub/sbb/cmp/neg/inc/dec (CF preserved)/and/or/xor/test, shifts and
  rotates by any count (count masked to 5 bits; count 0 changes nothing; 8/16-bit rcl/rcr use count mod 9/17),
  shld/shrd, mul/imul (CF=OF), bt/bts/btr/btc (memory form with register offset addresses outside the dword,
  signed), bsf/bsr (ZF; destination unchanged for a zero source).
- `div`/`idiv`: #DE (`FaultKind::DivideError`) on zero divisor and on quotient overflow (including
  `INT_MIN / -1`). `aam 0` also raises #DE.
- String ops: `movs/stos/lods/scas/cmps` b/w/d with `rep`/`repe`/`repne`, DF honoured, fast paths for forward
  `rep movs` (memmove-equivalent when dst <= src or there is no overlap) and `rep stos`. A fault in the middle of a
  `rep` leaves ECX/ESI/EDI at the progress so far, with EIP at the instruction. `0x67` (SI/DI/CX) string ops are
  refused with `Unimplemented`; none exist in S3.EXE.
- `jcxz` (`67 E3`, tests CX; the blitters use it) and `jecxz`. `loop/loope/loopne` with a `0x67` prefix use CX.
- 16-bit addressing (`0x67` + `[bx+si]` etc.) wraps the effective address at 64 KiB.
- `pushfd/popfd`: popf can change CF PF AF ZF SF DF OF, AC (bit 18) and ID (bit 21), so the 486 and CPUID
  probes succeed. IF always reads 1. TF/IOPL/NT/RF/VM are not modelled.
- `cpuid`: leaf 0 = "GenuineIntel", max leaf 1. Leaf 1 = family 5 model 2 (Pentium), EDX = FPU|TSC|CX8,
  **no MMX** (bit 23 clear), so S3.EXE never selects its MMX copy routine (static-isa.md #8). This
  deliberately differs from the earlier "Pentium MMX" note in the architecture contract.
- `rdtsc`: host monotonic nanoseconds / 5 (a 200 MHz TSC).
- BCD (`daa/das/aaa/aas/aam/aad`) follow the Intel pseudo-code. `salc` is implemented.
- `int3` -> `CpuExit::Interrupt(3)`, `int n` -> `Interrupt(n)`, `into` -> `Interrupt(4)` if OF, `hlt` -> `Halt`.
  `ud0/ud1/ud2` and undecodable bytes -> `FaultKind::InvalidOpcode`.
  `in/out/ins/outs/iret/retf/lgdt/.../bound/arpl/lds/les/lfs/lgs/lss` -> `FaultKind::GeneralProtection`
  (only data decodes as these; see static-isa.md). Far `call`/`jmp` -> `GeneralProtection`.
- Not modelled: segment limits, alignment checks, 16-bit operand-size `call/ret` (IP truncation), `lock` atomicity
  (one guest thread runs at a time, so it is free).
- Memory: guest addresses below 0x10000 fault (`AccessViolation{addr, write}`). Everything else is plain host
  memory (the 4 GiB window plus a guard; s3-mem).

## x87 hook

Any mnemonic that the integer `match` does not handle goes to `fpu::exec(cpu, mem, ins) -> Option<R>`
(owned by the fpu task). `None` -> `FaultKind::Unimplemented` with the mnemonic and disassembly in `detail`.
The task contract named this hook `exec_fpu`. The existing `fpu::exec` signature was kept so that both agents'
code compiles.

## Performance

`cargo test --release -p s3-cpu -- --ignored --nocapture bench_mips`. The loop mixes loads, stores, ALU ops,
compare-and-branch, call/ret and push/pop.

| Version | MIPS (x86_64 desktop, 16 cores, other builds running) |
| --- | --- |
| Foundation (`HashMap<u32, Arc<Block>>`, mnemonic `match` only, flags via several `set_flag`) | 88 |
| + arena/L1 cache, one-step flags, parity table, explicit jcc arm | 105 |
| + threaded dispatch with fast handlers | 170-185 |
| + `Op` records, 8/16-bit forms, cold fault paths, lazy flags, block chaining (perf-cpu, 2026-11-05) | 280-290 |

Windows 11 ARM64 dev PC (Snapdragon X2 Elite, power throttling off, best of 5; perf-interp 2026-11-07):

| Version | MIPS |
| --- | --- |
| perf-cpu (commit `f0a50f7`) | 440 |
| + tail-call threaded dispatch inside blocks, lean run loop | 770-800 |
| + 32-byte `Op`, `special` flag | 900 |
| + boxed blocks, pointer chaining | 1120-1140 |
| + `cmp/test/dec` + `jcc` superinstructions | 1160 |

In game the interpreter does ~225 M instr/s on the desktop (more cache misses and generic instructions than the
benchmark). Measurements, profiling tools and next steps: [performance.md](performance.md). `perf` is not
installed on this PC; valgrind/callgrind is.

## Tests

`CARGO_TARGET_DIR=port/target cargo test -p s3-cpu -j 4` (on Windows ARM64 the two `fpu::golden` tests fail at
`f0a50f7` already: f2xm1 and the pure-function hash differ from the golden vectors, see
[determinism.md](determinism.md); open issue):
- `diff_tests.rs` (x86_64 hosts): add/adc/sub/sbb/cmp/and/or/xor/test (8/16/32), neg/not/inc/dec,
  shl/shr/sar/rol/ror/rcl/rcr by CL with counts 0..40, 63, 64, 255, 257 and by immediates, shld/shrd 16/32,
  imul 2-op, bsf/bsr, bt/bts/btr/btc, mul/imul/div/idiv 8/16/32 (including the #DE cases). Operands are about
  64 edge-case and pseudo-random values, with incoming flag states 0, CF and all flags set. Each case runs
  natively (inline asm, `popfq` -> op -> `pushfq`) and in the interpreter, through both the fast and the generic
  path. A deliberately broken parity table is caught, so the harness is sensitive.
- `fast.rs` `lazy_conditions_match_materialised_flags`: every lazy kind x size x condition x edge values, plus
  `cf()/af()/of()/flag()` against the materialised EFLAGS.
- `tests.rs`: control flow, imm8 sign extension, memory ALU forms, string ops (backward memmove, repe cmpsb,
  rep with ECX=0, a fault in the middle of a rep), `67 E3` jcxz, loops, BCD, pushfd/popfd probes, cpuid, int3/int n/
  ud2/invalid bytes/in, xlat, enter with nesting, xchg/cmpxchg/lock inc on memory, the FS-relative SEH prologue,
  0x66 16-bit operations, the 16-bit addressing wrap, and the MIPS benchmark (ignored).
- `tests.rs` (perf-interp, run on every host, unlike `diff_tests.rs`): `budget_stops_are_exact` (a loop with calls,
  imul, adc, cdq run with budgets 1/2/3/5/7/13 and in one go, fast and generic: each run retires exactly its budget,
  same final registers, flags, memory and count), `mid_block_exits_count_exactly` (faults/`int3`/`hlt` in the middle
  of a block and in the partial-block path: EIP and count), `imul_mul_cdq_forms_fast_vs_generic`,
  `shift_forms_fast_vs_generic`, `setcc_forms_fast_vs_generic` and `cmp_jcc_fusion_fast_vs_generic` (69 edge and
  pseudo-random values squared, two incoming flag states; every fused form x all 16 conditions, both outcomes).
  Mutations (wrong OF of `shr`, wrong imul overflow, fused branch never taken) are caught.

## JIT (optional, AArch64; perf-jit 2026-11-07)

**Switch.** Cargo feature `jit` (default on; compiled only for `target_arch = "aarch64"`; iOS builds must use
`--no-default-features`, Apple forbids JIT). At run time it is **off by default**: env `S3_JIT=1` or
`s3_cpu::set_jit_enabled(true)` (global), `Cpu::set_jit(Some(bool))` (per CPU, tests). `s3_cpu::jit_available()`,
`jit_enabled()`, `jit_stats()` (counters; `s3det` prints them when the JIT is on). Android: `<data dir>/jit.txt`
containing `1` ([android.md](android.md)). Debug: `S3_JIT_DUMP=1` appends
every compiled block (guest disassembly + `.inst` words) to `./s3jit.s` (assemble + `llvm-objdump -d` with the NDK
clang to read it), `S3_JIT_FBPROF=1` profiles fallback calls per form and exits to the run loop by reason (slow).

**What is compiled.** A block is compiled when the run loop has entered it 8 times (`jit::THRESHOLD`;
`Cpu::set_jit_threshold`) and it is not hooked. The translation covers exactly the decoded block (same
instructions, same `n`), so retired counts are unchanged by construction. One code arena of 16 MiB per decode
cache (per guest thread), allocated on the first compile; full arena, `invalidate_range` dropping anything,
`clear()` -> all JIT code is dropped (no JIT code runs at those points).

**Execution model** (all choices keep the interpreter's results bit-exact; `determinism.md`):
- Trampoline `enter(cpu, mem_base, code)` saves x19..x30, loads guest EAX..EDI into **x19..x26** (x27 = `Cpu`,
  x28 = guest memory base, x13 = remaining budget) and jumps into the block; every exit stores the registers and
  the budget back. The `Cpu` is `repr(C)`; the code uses field offsets (`offset_of!`).
- **Budget**: the run loop enters JIT code only when `n <= left` (like the fast path); every block re-checks
  `left >= n` on entry and otherwise exits with EIP = its start, so the run loop runs the partial block through
  `run_partial` exactly as before. Budget stops / thread switches happen at the same instruction counts.
- **Flags**: between blocks the flags are always in the `Cpu` in the interpreter's lazy format (`lk/lsz/la/lb/lr`
  + `lcf`, or `ef` with `lk = NONE`). Inside a block the compiler knows the last flag writer at compile time:
  `Lazy(kind, size)` with the operands in x9/x10/x11 (x12 = lcf for inc/dec), `Eager` (status bits in x9, from
  shifts/mul) or `Cpu` (unknown, e.g. at block start). Conditions (`jcc`, `setcc`) are evaluated with native
  compares of the (shifted) operands; from the `Cpu` state a helper (`jit_cond`) evaluates them; CF/AF for inc/dec
  and shifts are read inline from the `Cpu` state. The state is stored at block end, before helper calls and at
  side exits. For `LOGIC` the stored `la/lb` may differ from the interpreter's zeros; they are never read.
- **Faults**: every guest access first checks `addr < 0x10000`; on failure the code takes a **side exit before
  the instruction** (registers and flags as at its start) and the run loop continues the block from that `Op`
  with the interpreter handlers, which then fault (or not) exactly like the interpreter, including partial effects.
  #DE of `div/idiv` works the same way.
- **Coverage (native)**: everything with a fast handler except `adc/sbb` and 8/16-bit shifts by CL: `mov`
  (8/16/32, r/m/imm), `movzx/movsx`, `lea`, `push/pop`, `add/or/and/sub/xor/cmp/test` all forms and sizes,
  `inc/dec` r32/m32, `neg/not` r32, `shl/shr/sar` imm (8/16/32) and r32 by CL, `imul` (1/2/3 operands), `mul`,
  `cdq`, `cwde`, `setcc`, `jcc`, `jmp`/`call` (rel/r/m), `ret [imm]`; plus generic-path forms (they also store
  `cur_ip`): `loop`, `jecxz`, `67 E3` `jcxz`, `div/idiv` r/m32, `movzx r16,r/m8`, `inc/dec/neg` r8, `leave`,
  non-rep `movsb/movsd/stosb/stosd`, `mov` r32 <-> `fs:[..]`. Anything else (x87, `rep` strings, `adc/sbb`, ...)
  calls the interpreter handler for that single instruction (`jit_op` on a private `[op, block_end]` pair; registers
  and flags spilled/reloaded) and continues in JIT code; a failing fallback exits through `block_exit` as usual.
- **Linking**: a static successor that is compiled already gets a direct `b`; otherwise a per-EIP slot
  (`ldr`/`ldr`/`br`, the slot holds the exit stub until the target is compiled). Indirect targets (`ret`, `jmp/call`
  r/m, fallback branches) go through a 32768-entry direct-mapped `(eip, code)` table, a miss exits to the run loop.
  Thunk addresses and hooked blocks are never linked. Blocks with a **native fast path** get a stub that does
  what the run loop does at a block entry (budget 0 -> exit; else call the `NativeFn` with the budget, `instructions`
  set; declined -> block body, done -> continue at its EIP).
- **Code memory**: `VirtualAlloc` + `VirtualProtect(PAGE_EXECUTE_READ)` + `FlushInstructionCache` (Windows),
  `mmap` + `mprotect` + `dc cvau`/`ic ivau` (Linux/Android). W^X: only the pages being written are RW, then RX.
  Windows ARM64: the JIT frames have no unwind info, so a panic inside a helper called from JIT code aborts the
  process (helpers do not panic in normal operation).

**Tests** (`src/jit_tests.rs`, aarch64): `jit_matches_interpreter_on_random_programs` (1000 random programs: loops
over random mixes of all the forms above incl. 8/16-bit forms, memory forms with every EA kind, flag consumers,
forward branches, calls, pushes, faults (NULL accesses, #DE), string ops; each run on the interpreter and the JIT
with compile thresholds 0 and 2, in one go and in random budget slices of 1..400 instructions; after every slice
exit, registers, EIP, EFLAGS, observable lazy state, count and 1 MiB data + stack must be equal; mutations of a
condition code and of the `div` remainder are caught), `jit_respects_hooks_natives_and_invalidation` (hook hits,
native calls, self-modifying code + `invalidate_range`), `jit_native_stub_matches_interpreter` (a native fast path
that sometimes accepts, random slices; caught EIP not being set before the call), `a64::tests` (encodings checked
against clang, logical immediates).

**Found on the way**: block chaining used EIP 0 as "empty successor", so a guest jump to address 0 (NULL function
pointer) dereferenced a null block pointer and crashed the host instead of raising the guest fault. Empty is now
`u32::MAX` (never a block start).

## Hook point (2026-11-06, mods-w1)

`Cpu::set_hooks(Option<Arc<HookAddrs>>)`: decoded blocks end before any hooked address and blocks starting at one carry
`Block::hooked`; at block entry a hooked block exits with `CpuExit::Hook(eip)` unless `skip_hook_once(eip)` was called.
Cost: one byte test per executed block; `bench_mips` showed no measurable difference (details in [mods.md](mods.md)).
Test: `hook_exits_mid_block_and_skip_once`.
