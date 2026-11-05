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
| `0x5fd080` / `0x5fd120` / `0x5fd0c0` | draw | 8-15 % | shadow/darken sprite variants -> **fast paths** for the pixel loops |
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
   handler goes to `Cpu::pending_exit`. The generic path reaches its `iced_x86::Instruction` through `Op::ins`.
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
Next steps if more is needed: superinstructions for frequent pairs (`cmp`+`jcc`, `mov`+`mov`), a
`Box<[Op]>` arena without the separate `Instruction` array (cache footprint), and finally an arm64 JIT
(Android only; iOS forbids JIT, keep the interpreter as the base).

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
- Host side: `valgrind --tool=callgrind --instr-atstart=no` on `s3det`, then `callgrind_control -i on` once the
  wanted virtual time is reached (watch the hash file); the run before that costs only ~5x.
- Deterministic benchmark: `s3det --vms 90000` wall time with the inject script of `tools/determinism-test.sh`.

## Not done / next

- Multi-core: all guest threads share one host core. Running the draw thread on its own host thread would give
  the logic thread (and audio) a core of their own, but the machine (HLE state, memory) is single-threaded today.
- ~~Measure on the phone~~: done (verify-fps1, table above). Phone profiling recipe: `adb shell su -c "simpleperf
  record -p PID -f 4000 --duration 8 -o /data/local/tmp/s3perf.data"`, pull, `simpleperf report --sort
  vaddr_in_file` (NDK `simpleperf/bin/linux/x86_64`) and symbolize the addresses with `llvm-symbolizer
  --obj=port/target/aarch64-linux-android/release/libs3android.so` (the .so in the APK is stripped).
- Next fast path: the `0x4b26c0` tile loop (HUD default 48-58 fps on the phone).
