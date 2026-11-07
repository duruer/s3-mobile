# Performance (fps, interpreter speed, native fast paths)

Owner: perf-cpu (workflow `wf_a446e364-b12`, 2026-11-05). Goal: >= 60 fps on the phone (target 60-120).
Related: [cpu.md](cpu.md) (interpreter internals), [display.md](display.md) (present path, FPS counter),
[determinism.md](determinism.md) (why every change below must be bit-exact).

## Summary (desktop, x86_64, one core, 640x480 in game, random map 384)

| Version | In-game fps (uncapped, `S3_FPS_CAP=0`) | Interpreter in game | `bench_mips` | `s3det` 90 000 vms wall |
| --- | --- | --- | --- | --- |
| Before (vblank grid pacing, old interpreter) | 28-30 (paced), ~33 uncapped | ~100-140 M instr/s | ~178 MIPS | 114.9 s |
| + frame limiter instead of 60 Hz grid | 36-47 at cap 60 (CPU bound) | same | same | same |
| + interpreter rework (pre-decoded ops, lazy flags, chaining) | 66-70 | ~225 M instr/s | ~280-290 MIPS | 66.3 s |
| + native fast paths (4 render loops) | **80-82** | ~255 M "guest-equivalent" instr/s | same | 55.9 s |

Default settings with the custom resolution mod (800x600 in game) run at 53-55 fps at the 60 cap
(`scratch/perf/def4.png`). Numbers are noisy (other agents build on the same PC, random map per s3run run); the
`s3det` wall time over a fixed deterministic workload is the most reliable comparison.

**Phone: measured 2026-11-05 (verify-fps1), see [Phone measurements](#phone-measurements)**: perf-cpu alone gave
~2.5x guest MIPS (100 -> 250) but the phone's default wide size 1536x600 needs ~6.5 M guest instructions per frame
(~10 000 snapshot tiles), so it ran 38-40 fps; three more fast paths over the per-tile loops (below) brought Classic
Touch to the 60 cap.

### Phone measurements

Samsung Galaxy Z Fold4 (SM-F936B, Snapdragon 8+ Gen 1, Android 14), cover screen 2316x904 @120 Hz, display mode
Native wide, cap 60, random map 384, readings of the detailed FPS counter (3 samples 3 s apart; "MIPS" counts a
native fast path as the guest instructions it stands for). Screenshots `scratch/vfps/phone/` (local only).

| Build | UI mode | Render size (zoom level) | fps | MIPS |
| --- | --- | --- | --- | --- |
| before the workflow | HUD | 800x600-ish | ~26 | ~100 |
| perf-cpu | HUD | 1536x600 (6, default) | 36 | 250 |
| perf-cpu | Classic | 1536x600 (6) / ~1440x560 (5) / 1336x520 (4) | 38-40 / 40 / 44 | 250 |
| perf-cpu | Classic, Fit 4:3 | ~693x520 | 60 (cap) | 200 |
| + `snap_tiles` + `obj_empty` (v1) | Classic | 1536x600 (6) | 54-56 | 353-365 |
| + `land_clean` + `obj_empty` v2 | **Classic** | **1536x600 (6, default)** | **60 / 62 / 60** | 412-419 |
| same | Classic | 1232x480 (3) | 60 / 60 / 60 | ~320 (headroom) |
| same | Classic | 1848x720 (9) | 42 / 46 / 42 | 392-420 |
| same | HUD | 1536x600 (6, default) | 58 / 56 / 48 (48: lots of animated water) | 376-449 |
| same | HUD | 1232x480 (3) | 60 / 60 / 60 | ~322 |
| same | HUD | 1848x720 (9) | 58 / 60 / 46 | 438-509 |

Notes: the engine thread is 94-99 % of one core; simpleperf (`su`, 8 s, 4 kHz) shows it at ~2 GHz average clock
(the governor does not hold the prime core's 3.19 GHz; pinning the thread to cpu7 with `taskset` gave only ~5 %),
92 % of the process cycles in `s3-engine` (interpreter `run_with` + handlers; `to_framebuffer_into` 3.7 %, libc
5 %), 6.6 % in `s3-present`. Water and other animated landscape make triangles dirty every frame (`land_clean`
declines them), which is why the fps depends on the map view.

Desktop at the phone aspect (`s3run --headless --display native --window 2316x904 --fps full`, `S3_FPS_CAP=0`):
Classic 35 fps (perf-cpu, under the profiler) -> 43-45 (`snap_tiles`) -> **70-72** (all), HUD 62-64.
`s3det` 90 000 vms wall time 55.9 s -> 41.4 s.

## What limits fps

1. **Pacing**: the draw thread (`CDrawThread`) renders as fast as it can and calls
   `IDirectDraw2::WaitForVerticalBlank(DDWAITVB_BLOCKBEGIN)` once per frame (registry `WaitVBlank`=1). Our HLE used a
   60 Hz grid, so a frame needing 17 ms waited for the next boundary -> 30 fps (real vsync does the same).
   The game itself has no other fps cap: the logic thread is a separate 64 ms timer (`CLogicThread`), the draw
   thread only renders snapshots of it. Fixed: `WaitForVerticalBlank` is now a **frame limiter** (below).
2. **CPU**: after that the draw thread is CPU bound: ~2.5-2.9 M guest instructions per frame at 640x480 plus the
   logic thread (~20-50 M instr/s in game; during random-map generation 100% of the CPU for 10-60 s).
   All guest threads share one host core (cooperative green threads).

### Guest profile (in game, `S3_PROFILE`, functions from REA)

| Function | Thread | Share | What |
| --- | --- | --- | --- |
| `0x51f110` | logic | 18-20 % | builds the render snapshot grid (visible tiles, see resolution-zoom.md) |
| `0x4b26c0` | draw | 16-19 % | frame renderer (landscape/object pass over the snapshot tiles) |
| `0x4d3050` | draw | 12-15 % | per-tile object drawing (huge function, ~1200 lines of pseudocode) |
| `0x54cbd0` | draw | 10-12 % | landscape pass |
| `0x5fc350` | draw | 10-13 % | 16-bit RLE sprite blitter -> **fast path** for its opaque row loop |
| `0x5fd080` / `0x5fd120` / `0x5fd0c0` | draw | 8-15 % | shadow/darken sprite variants -> **fast paths** for the pixel loops, since perf-shadow the whole row loop of `0x5fd120` |
| `0x5fc160` + `0x5f48e0` | draw | 0-21 % | shadow sprite drawn as zero runs (map dependent) -> **fast path** for its row loop (perf-shadow) |
| `0x5fc860` | draw | ~5 % | 8-bit indexed sprite through a palette -> **fast path** |
| `0x5fc540` | draw | ~2 % | 4-instruction setter called ~1 M times/s |

The three big draw functions and `0x51f110` are game logic over tile structures, not pixel loops; they only get
faster through the interpreter.

## Frame limiter (s3-dx `ddraw.rs`)

`WaitForVerticalBlank` releases `1000 / cap` ms after its previous release, or at once if the frame was already
late (no catch-up bursts). Cap default **60**; env `S3_FPS_CAP=N` (0 = no wait); API
`s3_dx::ddraw::set_fps_cap(fps)` / `fps_cap()` for frontends and the display settings (refresh-rate choice,
docs/porting/display.md). Rendering only: the logic thread's timer is unaffected, so the simulation speed does
not change with the fps. Above 60 fps the game draws identical logic states more often (it has no interpolation
except camera scrolling, which uses `timeGetTime`), so 60 is the sensible default; 90/120 only make scrolling
smoother and cost battery.

## Interpreter changes (s3-cpu, details in [cpu.md](cpu.md))

1. **Pre-decoded `Op` records** (`fast.rs`): handler pointer + next EIP, register numbers, displacement, immediate
   or branch target, and the effective-address form as a const generic (`[disp]`, `[base+disp]`,
   `[base+index*s+disp]`, `[index*s+disp]`). Handlers take `(cpu, mem, op)` and return `bool`; the exit of a failing
   handler goes to `Cpu::pending_exit`. The generic path reaches its `iced_x86::Instruction` through the `Op`
   (`Op::ins` then, `Op::set_ins` since perf-interp).
2. **Fault paths out of line** (`#[cold]`): the fast paths of the handlers no longer set up a stack frame.
3. **8/16-bit fast forms**: `mov` r/m/imm, ALU (`add/or/adc/sbb/and/sub/xor/cmp/test`) for 8/16/32-bit operands —
   the blitters are full of `mov cl,[eax]`, `test bl,bl`, `mov [eax],dx`.
4. **Lazy flags** (`flags.rs`, `Cpu::lk`): add/sub/cmp/logic/inc/dec store kind, operands and result; EFLAGS are
   computed only when read (`Cpu::eflags()`), `jcc` evaluates its condition straight from the lazy state
   (`cond_fast`), CF/AF/OF have cheap accessors. Exactness: unit test `lazy_conditions_match_materialised_flags`
   (all kinds x sizes x conditions x edge values) plus the host-CPU differential tests.
5. **Run loop**: no per-instruction EIP check or `cur_ip` store (only the last instruction of a block can branch),
   instruction count added per block, **block chaining** (each block caches its last two successors).
6. `[profile.release.package.s3-cpu] codegen-units = 1`.

Remaining per-instruction cost (callgrind, benchmark): ~11 host instructions of dispatch + the handler body.

### perf-interp (2026-11-07): threaded code, pointer chaining, superinstructions

Measured first (in game, HUD 2316x904, `S3_PROFILE` + a pair profile): blocks average **6.5 instructions**; the
generic path executes ~1.1 % of the instructions (`imul m32` 0.54 %, `shr r16,imm` 0.19 %, `imul r,r` 0.10 %,
`cdq` 0.05 %, ...) but each costs several times a fast handler; instruction pairs are diffuse (`mov r,m; mov r,m`
4.3 %, `push; push` 4.1 %, everything else < 2 %), so pair superinstructions in general buy little, while the
per-instruction loop (11 host instructions + call/return + frame) and the per-block loop (~50 host instructions with
stack spills, because `exec::step` was inlined into the run loop) were the real cost. Changes, in order:

7. **Tail-call threaded dispatch inside a block**: each handler ends by calling the next `Op`'s handler (`fast::next`,
   compiled to `ldr x8, [x2, #32]!; br x8`), a `block_end` sentinel ends the chain, control transfers return to the
   loop directly. No per-instruction EIP store (EIP is set once to `Block::end`; no non-branch instruction reads
   it). Fault exits record their `Op` (`Cpu::fault_op`) through cold tail-called functions, so hot handlers are
   frameless on Android. A block the budget ends inside runs instruction by instruction through `exec::step`
   (`run_partial`), keeping budget stops exact. bench 440 -> ~780 MIPS.
8. **Lean run loop**: decode/lookup slow path and the partial-block path out of line (the run loop no longer
   spills), unchecked arena access, instruction count derived from the remaining budget, one `special` byte test
   (hooked | native) per block.
9. **Fast forms for the generic leftovers**: `imul` (2/3/1-operand), `mul`, `cdq`, `cwde`, 8/16-bit shifts by imm and
   all shifts by CL, `setcc r8`, `neg/not r32`, `inc/dec m32` (now ~0.03 % generic).
10. **32-byte `Op`** (the generic path's `Instruction` pointer lives in the unused `disp`/`imm`), Block hot fields in
    one cache line (`repr(C, align(64))`). bench ~900 MIPS.
11. **Boxed blocks + pointer chaining**: successors are cached as block pointers (no index multiply / arena reload).
    In game 98.8 % of block entries hit the 2-entry successor cache. bench ~1130 MIPS.
12. **Superinstructions** `cmp/test (r,r | r,imm | m,imm) / dec r32` + the block-ending `jcc` (12 lazy conditions):
    fused at decode time, body = both handlers (~7.8 % of in-game instructions are such pairs). ~3 % in game.

**Results** (Windows 11 ARM64 dev PC, Snapdragon X2 Elite, power throttling off, see below):

| | perf-cpu (`f0a50f7`) | perf-interp |
| --- | --- | --- |
| `bench_mips` (best of 5) | 440 MIPS | **1160 MIPS** |
| `s3det` 90 000 vms: time in `Cpu::run` | 16.6-18.8 s | **9.7-9.8 s** |
| `s3det` 90 000 vms: process CPU time | 20.4-23.2 s | **13.3-13.4 s** |
| `s3run` HUD 2316x904 uncapped, samples 50-80 s (random map, noisy) | 93-102 fps, draw thread 770-780 M instr/s | **126-190 fps** (3 runs), 1040-1230 M instr/s |

Determinism: `s3det` 90 000 vms old vs new CPU: **IDENTICAL** per-second hashes and `insns=11817005518`, also with a
stronger CPU hash that adds EFLAGS, FS base, the x87 last-IP and tag word (local s3det change for the check). The
native fast paths' differential tests (`s3-mods` `fast_tests`, real exe) pass.

**Benchmarking on the Windows ARM64 PC**: processes started from an agent shell run under Windows power throttling
(EcoQoS): 2-4x slower and very noisy (`bench_mips` 125-200 instead of 440 for the same binary, once 710 by chance).
Measure with throttling off: start the process, `SetProcessInformation(ProcessPowerThrottling,
{Version 1, ControlMask EXECUTION_SPEED|IGNORE_TIMER_RESOLUTION, StateMask 0})` on its handle and High priority (a
small PowerShell P/Invoke wrapper; the agent kept it in its scratchpad), and compare process CPU time or
`Cpu::run` time, not wall time, with alternating runs.

Notes for a later arm64 JIT (done since, see "perf-jit" below; kept for the reasoning): the per-block structure (exact retired counts per block, budget stop via
the generic path, native blocks entered only at block starts, hooks at block starts) carries over unchanged; a JIT
block can reuse the `Op` decode (EA forms, register numbers) and must keep the lazy-flag state layout
(`lk/la/lb/lr/lsz/lcf`) or materialise exactly like `flags.rs`; the `cmp_jcc` fusion shows the gain of folding
conditions into fresh operands. Next interpreter ideas: a return-address cache for `ret` (most of the 1.2 % L1
lookups), fusing `mov` + `mov` chains, passing the guest memory base in a register instead of `&GuestMemory`.
iOS forbids JIT, keep the interpreter as the base.

### perf-jit (2026-11-07): optional AArch64 JIT

Design and coverage: [cpu.md](cpu.md#jit-optional-aarch64-perf-jit-2026-11-07). Off by default (`S3_JIT=1`), cargo
feature `jit` (aarch64 only; iOS builds leave it out). In short: one compiled block per decoded block (same `n`),
guest registers in host registers, lazy flags in host registers inside a block and in the interpreter's format
between blocks, NULL-guard checks with side exits into the interpreter, per-instruction fallback to the
interpreter handler for the rest (x87, `rep` strings, `adc/sbb`), direct block links, an indirect-branch table and
native-fast-path stubs, so JIT code returns to the run loop only for thunks, budget ends and hooks.

Measured (Windows 11 ARM64 PC, throttling off; this PC was ~20 % slower in the afternoon than in the morning, so
only same-session pairs are compared):

| | interpreter | JIT |
| --- | --- | --- |
| `bench_mips` (same session, 3 runs) | 1074-1176 MIPS | **3976-4777 MIPS** (one earlier run 6364) |
| `s3det` 90 000 vms, time in `Cpu::run` (`S3_CPU_TIME=1`, alternating runs, native fast paths on) | 6.87 / 6.93 / 9.56 s | **2.84 / 2.84 / 2.76 s** |
| same, process CPU time | 8.84-11.9 s | **4.72 s** |
| `s3det` with `S3_FAST=0` (pure guest code), time in `Cpu::run` (morning) | 13.68 s | **3.13 s** (4.4x) |
| `s3run` HUD 2316x904 zoom 10 uncapped, samples 50-80 s, random map | 356 / 248 fps, 708 / 720 ms/s in the CPU | **443 / 328 fps**, 580 / 541 ms/s |

`s3run` is now limited by host-side work (presentation, HLE): the guest CPU uses ~55-60 % of the engine thread
with the JIT. Per presented frame the CPU time fell from ~2.0-2.9 ms to ~1.3-1.65 ms.

JIT statistics for the 90 000 vms run (`s3det` prints them with the JIT on): 14 375 compiled blocks, 7.6 M JIT
entries (exits: 4.7 M budget ends at a compiled block, 2.6 M hooked/special, 0.15 M thunks, 0.05 M uncompiled), no
side exits and no failed fallbacks, 20.2 M fallback calls: x87 `fild m64` / `fistp m64` 7.7 M each and `fxch`
3.8 M, all from one 16-byte copy loop at `0x5c3e40` (`fild [esi]; fild [esi+8]; fxch; fistp [edi]; fistp
[edi+8]; add esi,16; add edi,16; loop`), then `rep movsd` 0.78 M. Natively translating that loop (or a native
fast path for it in s3-mods: the f64 round trip per qword has to be reproduced exactly, our x87 keeps f64, so
values above 2^53 are rounded; HYPOTHESIS: the game copies pixel/struct data with it, which then is not a bit copy
under emulation either) is the next cheap step.

Before/after the native fast paths: with the JIT, `S3_FAST=0` costs 3.13 s vs 2.25 s with fast paths in
`Cpu::run` (same session), so the fast paths still pay off; switching single paths off (`S3_FAST_OFF`) changed
the time by less than the run-to-run noise (±0.3 s), so no single path is clearly slower than JIT code.

## Native fast paths (s3-cpu `NativeBlock`, s3-mods `fast*.rs`)

A **native block** replaces a guest code region *inside the CPU* (no hook exit to the host, so it pays off even for
loops entered millions of times per second):

- s3-cpu: `NativeFn = fn(&mut Cpu, &mut GuestMemory, budget) -> Option<u64>`; `NativeBlock { name, len, hash, f }`;
  `Cpu::set_native_blocks`. When a block is decoded at a registered address and `code_hash` (FNV-1a 64) of the
  next `len` bytes matches, the block gets the native function. At block entry the CPU calls it with the remaining
  instruction budget; `None` = decline (the original code runs), `Some(k)` = it did exactly what `k` guest
  instructions would have done.
- s3-runtime: `Machine::set_native_blocks` (also given to threads created later).
- s3-mods: `fast.rs` registers the GOG blocks (`gog_blocks()`) after the hooks are committed (a region containing a
  hook is skipped); env **`S3_FAST=0`** disables all. Our sources contain only addresses, lengths and hashes, never
  game bytes (`tools/perf/code_hash.py S3.EXE start end` prints them).

**Contract** (what keeps lockstep MP and `s3det` hashes intact): registers, EFLAGS (as the original's last flag
writer leaves them, via `Cpu::alu_sub` etc.), EIP, every memory byte and the retired-instruction count must equal
the original code's at the point where the native code stops. Stop only at a point the original passes through
(loop head or exit), run whole iterations within the budget (so thread switches happen at the same instruction
counts as without fast paths), and decline whenever the original could fault (NULL guard), not terminate normally,
or when its writes overlap what it reads.

| Name | Region | Original loop | Instructions |
| --- | --- | --- | --- |
| `sprite_rows` | `0x5fc4c0..0x5fc536` | opaque RLE rows of the 16-bit sprite blitter `0x5fc350` (whole rows: run headers, dword copies, odd pixel, row advance, `cmp edi,[esp+0x98]`) | 15/run + 6/dword + 2 (+4 odd) + 4, + 9/row |
| `palette_row` | `0x5fc93d..0x5fc952` | 8-bit -> 16-bit through a palette (`0x5fc860`) | 8/pixel |
| `darken` | `0x5fd0ea..0x5fd0ff` | `[eax] = ([eax] >> 1) & [0x7e8dec]` per dword | 8/dword |
| `dither` | `0x5fd096..0x5fd0ae` | checkerboard shadow in `0x5fd080` | 8 or 9/pixel |
| `snap_tiles` | `0x522219..0x52303e` | logic thread, render snapshot fill (`0x51f110`): copy one map tile into its snapshot cell per iteration (`fast_snap.rs`). Common path only: tile inside the map, `[gd+0x2d284] != 1`, not the special row `[gd+0x2d2e4]`, byte +0xb != `[gd+0x2d300]`, light byte with equal 3-bit fields and bit 7 clear, no object words, no flag 0x20; else stops at the loop head before that tile | 133/tile |
| `obj_empty` | `0x4d3050..0x4d5a4f` (+ setter `0x5fc540`, hash-checked at each call) | draw thread, per-tile object drawer called by `0x4b26c0` for every tile: the whole call (prologue, four setter calls, type-compare chain, epilogue, `ret 0x18`) for object type 0 when nothing is drawn (`fast_obj.rs`) | 110-116/call |
| `land_clean` | `0x54ce53..0x54e712` | draw thread, landscape pass `0x54cbd0`: triangle loop for clean triangles (no dirty corner bit, flags `[ebp-0x68]/[ebp-0xe4]/[ebp-0xe8]` != 1) - corner heights, min y into `[ebp-0x34]`, phase flip (`fast_land.rs`) | 60-66/triangle |
| `frame_tiles` | `0x4b2e44..0x4b330e` (+ row head `0x4b2e30..0x4b2e44` and the 24 instruction ranges of `0x4d3050`'s empty case, hash-checked at entry) | draw thread, frame renderer `0x4b26c0`: the cell loop over the snapshot (x/40 sector, ambient-sound statistics `[ebx+0x5f588/0x5f5cc]`, 4x4 water-decoration grid test, `+0x508` kind checks, the object call `0x4d3050` inlined for the `obj_empty` case via `fast_obj::plan/apply`) and the row advance (`fast_tile.rs`). Stops at the loop head before a cell with weighted water/shore sound (`0x4b2f0e`), decoration (`0x4b3046`), object type != 0 (incl. 0xfe), `+0x50a`/`+0x51e` != 0, `+0x508` kind 0x800/0x1000/0x1800 or a non-empty `0x4d3050` call | ~200/cell (incl. the call) |
| `land_mark` | `0x4b277e..0x4b27a7` | draw thread, pre-pass of `0x4b26c0` over one snapshot column: landscape byte `+0x502` to 0x1c / back to 0x10 by bit 2 of `+0x509` for cells with `(+0x505 & 0x3f)*4 <= 0x48` (`fast_tile.rs`) | 8-13/cell |

**Verification**
- Differential unit test `cargo test -p s3-mods fast_tests` (needs the local S3.EXE; skipped otherwise): maps the
  exe's sections into guest memory, 120 random cases per fast path (random RLE sprites, pitches incl. odd ones,
  palettes, masks, register values, small budgets for partial stops), runs the native function, then the
  interpreter for exactly the returned instruction count, and compares registers, EFLAGS, EIP and 5 MiB of memory
  byte for byte. It caught one bug (EBX after a budget stop in `dither`).
- Whole game: `s3det` 90 000 virtual ms (menu, random map, in game, build menu, place, cancel) with `S3_FAST=0` vs
  default: identical per-second memory/CPU hashes and instruction counts (`scratch/perf/det5`).
- The interpreter rework itself: `s3det` old CPU vs new CPU on the same code: identical for 90 000 vms
  (`insns=11897551127`, `scratch/perf/det3`).

verify-fps1 (2026-11-05) added `snap_tiles`, `obj_empty`, `land_clean`: each has a differential test against the real
exe (`differential_snap_tiles`, `differential_obj_empty`, `differential_land_clean`: random cells / frames /
budgets, compare registers, EFLAGS, EIP and memory); `s3det` 90 000 vms with all fast paths vs `S3_FAST=0`:
IDENTICAL (`insns=11817005518`, `scratch/vfps/det`); live two-instance MP with zoom pinches: IDENTICAL
(`scratch/vfps/mp`). Profile after them (HUD, 2316x904): `0x4b26c0` frame renderer tile loop **46 %** (~98
instructions per tile, branchy: ambient-sound statistics, tree/decoration checks, three call sites of `0x4d3050`),
`0x5fd080`/`0x5fd120` shadow sprites 17 %, `0x4d3050` (non-empty tiles) 7 %, `0x5fc860` 4.5 %. The frame renderer
loop is the next target for the HUD default to hold 60.

perf-shadow (2026-11-07) added the run/row loops of the two shadow sprite functions (`fast_shadow.rs`), each
including the per-run call through a function pointer (callee hash-checked, emulated with every stack write of the
call and its pushes, so memory below ESP is identical too):

| Name | Region (entries) | Original loop | Instructions |
| --- | --- | --- | --- |
| `shadow_zero` | `0x5fc2cd..0x5fc346` (row head `0x5fc2cd`, run head `0x5fc2d4`) | second row loop of `0x5fc160`: per run `(count, skip\|last)`, `call [0x7e8c74]` = `0x5f48e0` zeroes `count>>1` dwords + 1 word if `count&1`, source cursor skips the pixel bytes | 38/run + 5/dword + 3 (odd), 9/row end, 1/row head |
| `shadow_rows` | `0x5fd24f..0x5fd301` (`0x5fd24f`, row head `0x5fd256`, run head `0x5fd25d`) | row loop of `0x5fd120`: per run `call [0x7e8e20]` = `0x5fd080` (checkerboard, phase `[esp+0x90]`, row phase ESI) or `0x5fd0c0` (darken with masks `[0x7e8dd8]` word / `[0x7e8dec]` dword); row end: ESI = (ESI+1) C-mod 32 | 25/run + callee (`0x5fd080`: 11 empty, else 18 + 8/pixel + 1/cleared pixel; `0x5fd0c0`: 4 empty, else 17 (+6 head word, +6 tail word, +2+8/dword)), 15 or 18/row end, 1/row head |

Each native function dispatches on the entry EIP (a hash covers entry..exit; the row-head instruction that the run
head entry's hash does not cover is hashed separately before it runs), stops at a run head, row head or the exit,
and stops before a run that could fault, would not terminate (odd distance, wrap), writes into the stack frame or the
darken dword mask, or whose function pointer is not the known callee. The first loop of `0x5fc160` (`[0x7e8c70]`)
and the masked (fog) loop `0x5fc41a..0x5fc4af` of `0x5fc350` were not executed in the in-game profile and are not
covered. Tests: `differential_shadow_zero` (300 cases, 288 native), `differential_shadow_rows` (400 cases, dither
198 / darken 176 native; random/negative phases, odd destinations, wrong function pointers, budgets 1..800); a
deliberately wrong cost or pushed EBX fails them.

Profile (HUD, 2316x904, random map, from 60 s; native blocks count with their decoded length): before
`0x4b26c0` 45 %, `0x5fc160` 11.9 % + its callee `0x5f48e0` (attributed to `0x5f4830`) 9.0 %, `0x5fd120` 3.9 % +
`0x5fd080`/`0x5fd0c0` 3.7 %; after `0x4b26c0` 63 %, `0x5fc160` 1.5 %, `0x5fd120` 0.8 % (callees no longer
listed). The frame renderer `0x4b26c0` is now by far the main cost.

Ideas for more fast paths: the masked (fog) sprite loop `0x5fc41a..0x5fc4af` (when it shows up in a profile),
`0x5fc540` (could be inlined at its call sites as a superinstruction).
perf-tile (2026-11-07) added `frame_tiles` and `land_mark` (`fast_tile.rs`) for that loop. How `frame_tiles` covers
code outside its registered region: the row head `0x4b2e30..0x4b2e44` and the instructions of `0x4d3050`'s empty
case (24 ranges, 413 bytes, traced; test `obj_empty_path_hashed` checks that every executed instruction lies in
them) are FNV-hashed before the first change of each entry (`tools/perf/code_hash.py --concat`); `fast.rs`
`extra_ranges` disables it when a hook lies in that code (the hook must run). The inlined call reuses
`fast_obj::plan` (pure checks) and `fast_obj::apply` (stack writes, registers, `ret 0x18`), so both stay in step.
About 2 % of the cells end a native run (most: object type 0xfe, decoration grid, border cells with `+0x51e`);
the interpreter runs that cell and the next jump to the loop head enters the native path again.
Results (desktop ARM64, HUD, `--window 2316x904`, `S3_FPS_CAP=0`, random map, noisy shared PC):
- guest profile (`S3_PROFILE`, 60-80 s): `0x4b26c0` **41.9 % -> 6.2 %**; now on top: `0x5fd080` / `0x5fd120`
  shadow sprites 20 % each, `0x5fc860` 10 %, `0x51f110` 3 %, `0x4d3050` (non-empty) 3 %;
- fps (mean of the samples 60-80 s, two alternating runs each): before 28.9 / 35.0, after **41.0 / 46.5**
  (under the profiler 26-34 -> 47-56);
- `s3det` 90 000 vms: all fast paths vs `S3_FAST=0` and vs `S3_FAST_OFF=frame_tiles,land_mark`: IDENTICAL
  (`insns=11817005518`, `mem=4ca80f23bf398314`), wall time (two runs in parallel) 64 s -> 47 s.
Env **`S3_FAST_OFF=name,name`** leaves single fast paths out (bisecting a divergence, measuring one path).

Ideas for more fast paths: the masked (fog) sprite loop `0x5fc41a..0x5fc4af`, the run loop of `0x5fd120`
(calls through `[0x7e8e20]`), `0x5fc540` (could be inlined at its call sites as a superinstruction).

## Profiling tools

- `S3_SAMPLE=1 s3run ...`: per second `fps:N cpu:Mms` (presents, host ms inside the interpreter) and per-thread
  millions of instructions.
- `S3_PROFILE=<file> [S3_PROFILE_FROM_MS=<ms>] s3run ...`: at exit writes per-thread decoded-block entry counts
  (`<file>`), the 60 hottest blocks per thread disassembled with generic-path instructions marked `*`
  (`<file>.disasm`) and the executed instruction forms weighted by count (`<file>.forms`).
  `python3 -I tools/perf_profile.py <file> scratch/perf/funcs.txt` aggregates by function (function starts
  exported from REA `list_procedures`; keep that list in `scratch/`, it is derived from the exe).
  Native blocks are counted with their decoded length per entry (approximate).
  `DecodeCache::profile_pairs()` (perf-interp) returns adjacent instruction-form pairs weighted by block entries plus
  the total block entries (`#blocks`, for the average block length); s3run does not write it yet (a local
  `.pairs` output was used for the measurements above).
- Host side: `valgrind --tool=callgrind --instr-atstart=no` on `s3det`, then `callgrind_control -i on` once the
  wanted virtual time is reached (watch the hash file); the run before that costs only ~5x.
- Deterministic benchmark: `s3det --vms 90000` wall time with the inject script of `tools/determinism-test.sh`.

## Measurements 2026-11-07 (perf review)

Phone (Fold4 cover, HUD, Native wide, **zoom level 10 = 1944x760**, cap 60): **50-56 fps**. simpleperf 8 s
(`su`, 4 kHz): `s3-engine` 86 % libs3android + 5 % libc, `s3-present` ~6 %. Inside s3-engine (share of the thread):

| Host symbol | % | Note |
| --- | --- | --- |
| `Cpu::run` (dispatch + inlined handlers) | 28.7 | interpreter core |
| `fast::mov_rm<4,1>`, `mov_mr<4,1>`, `push_r`, `generic`, ... | ~35 | handlers (`generic` = iced fallback 2.8 %) |
| `__memmove_aarch64_simd` | 5.5 | frame copies: guest surface -> `FrameBuffer` -> overlay copy -> `Shared` -> presenter local -> GL |
| `s3_dx::blit::to_framebuffer_into` | 5.2 | per-pixel `u16::from_le_bytes` loop for RGB565 (could be one memcpy per row) |
| `s3_mods::overlay::Canvas::rounded` + `fill_rect` | 5.7 | HUD bar/cards redrawn in software every frame |
| fast paths (`obj_empty`, `sprite_rows`, `snap_tiles`, `land_clean`, `darken`) | ~5.6 | native code |

So ~17 % of the engine thread is host-side presentation work, not guest code.

Desktop (Windows 11 ARM64 laptop, headless, 2316x904, HUD, `S3_FPS_CAP=0`): zoom 10: 33-35 fps, zoom 6: 40-44 fps,
`S3_FAST=0`: 13 fps (guest ~140 M instr/s: this ARM64 host's interpreter is slower than the old x86_64 desktop).
Guest profile (zoom 10, in game): `0x4b26c0` frame renderer tile loop **42.5 %**, shadow sprites `0x5fd120` 7.8 % +
`0x5fd080` 7.7 %, `0x4d3050` 5.8 %, `0x5fc860` 4.5 %, `0x5fc160` 4.2 %, `0x5f4830`/`0x5f5400` 2.8 % each; logic
thread `t5` ~20 % of the guest instructions.

Ranked ideas (estimated gain on the phone at zoom 10):
1. Fast path for the `0x4b26c0` tile loop (42 % of guest work): +20-35 %. Medium/large (branchy; differential test
   like the others).
2. Host presentation work (~17 %): RGB565 row memcpy in `to_framebuffer_into`; fewer frame copies (hand buffers
   over instead of copy_from_slice twice); HUD drawn into a cached RGBA layer that is re-rendered only when it
   changes and composited by the GPU as a second texture: +10-15 %. Small/medium, no determinism risk.
3. CPU clock: the engine thread runs at ~2 GHz avg of the prime core's 3.19 GHz. `APerformanceHint` (NDK, API 33+)
   session for the engine thread with a 16.6 ms target may let the governor boost (HYPOTHESIS, untested;
   `taskset` alone gave ~5 %). Small effort, possibly large gain.
4. Shadow sprites `0x5fd120` / `0x5fd080` outer loops (15 %): +5-10 %.
5. Interpreter: superinstructions (`cmp/test+jcc`, `mov+mov`), smaller `Op`; +10-20 %, medium.
6. arm64 JIT (Android only, iOS forbids JIT): 2-4x guest speed, large effort. **Done (perf-jit, optional,
   `S3_JIT=1`)**: 4.4x on pure guest code, 2.4x of `Cpu::run` time with fast paths, see "perf-jit" above.
7. Draw and logic threads on separate host cores: large refactor (single-threaded machine).
8. Lower default zoom (6 instead of 10 = 1536x600): immediate ~+25 %, but a UX choice.

### Done in perf-r2 (2026-11-07)

- **Item 3, CPU hint** (`s3-android/src/perf_hint.rs`): `APerformanceHint` session for the engine thread (dlsym,
  API 33+), reports engine-thread CPU time per presented frame against `1000 / cap` ms. On the Fold4 it does
  nothing: `adb shell dumpsys performance_hint` says `HintSessionPreferredRate: -1`, `HAL Support: false`, and
  `createSession` returns NULL (logged "perf hint: session 0x0"). Kept for devices with the HAL (Pixel etc.).
- **Item 2, host presentation**: RGB565 rows copied with one `memcpy` (`s3_dx::blit::to_framebuffer_into`);
  `Canvas::rounded` computes the distance field only in corner rows / edge columns and fills the rest as spans
  (pixel-identical, test `rounded_spans_match_per_pixel_reference`), opaque spans are plain fills; the cursor keeps
  only the pixels under it instead of a second full frame; `Machine::present_owned` takes the frame from ddraw by
  value (no "last guest frame" copy) and `present_cursor_owned` hands the overlay output to the cursor state without
  a copy; the Android presenter swaps buffers with the frame slot. Per frame: 1 conversion + 6 full copies -> 1
  memcpy + 2 copies (overlay working copy, host slot). Phone simpleperf: host presentation work ~17 % -> ~10 % of
  the engine thread.

### Phone after perf-r2 items 1-5 (2026-11-07, commit e8042b2)

Fold4 cover, HUD, Native wide, zoom 10 (1944x760), cap 60: **60 fps in all 5 samples** (before: 48-56). Engine
thread at **70 %** of one core (`top -H`; before 94-99 % while missing 60), presenter 13 %. So roughly 40 % headroom
at the cap (~85 fps worth of work), enough for higher zoom levels or a 90 Hz cap.

## Not done / next

- Multi-core: all guest threads share one host core. Running the draw thread on its own host thread would give
  the logic thread (and audio) a core of their own, but the machine (HLE state, memory) is single-threaded today.
- ~~Measure on the phone~~: done (verify-fps1, table above). Phone profiling recipe: `adb shell su -c "simpleperf
  record -p PID -f 4000 --duration 8 -o /data/local/tmp/s3perf.data"`, pull, `simpleperf report --sort
  vaddr_in_file` (NDK `simpleperf/bin/linux/x86_64`) and symbolize the addresses with `llvm-symbolizer
  --obj=port/target/aarch64-linux-android/release/libs3android.so` (the .so in the APK is stripped).
- ~~Next fast path: the `0x4b26c0` tile loop~~: done (perf-tile). Next: the shadow sprite run loops `0x5fd080` /
  `0x5fd120` (40 % of the profile after perf-tile) and `0x5fc860`; measure the HUD default on the phone again.
