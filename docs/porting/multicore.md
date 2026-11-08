# Multi-core execution (perf-mt)

Status 2026-11-07 (perf-mt): design + RE + measurements done; option A (true parallel guest execution of the draw
thread) is implemented as an **experimental prototype behind `S3_MT=1` (default off)**. It is correct (unit test,
lockstep MP runs with MT peers identical, default mode bit-identical) but gives **no reliable fps gain** on the dev
PC; the reasons and the way forward are below. Related: [architecture.md](architecture.md) (single-threaded
`Machine`), [performance.md](performance.md), [determinism.md](determinism.md), [multiplayer.md](multiplayer.md).

## Starting point (measured)

All guest threads are cooperative green threads inside one `Machine` on one host thread. In game (HUD, 2316x904
window, zoom 10 = 1944x760, `S3_FPS_CAP=0`, Windows ARM64 dev PC, throttling off, commit `e8042b2`, random map):

| What | Per second | Per frame (348 fps) |
| --- | --- | --- |
| draw thread `CDrawThread` (t4) guest instructions | ~2 800 M ("guest equivalent", fast paths included) | ~8 M |
| logic thread `CLogicThread` (t5) | ~780 M | |
| time in `Cpu::run` (all threads) | 707 ms | 2.0 ms (draw ~1.56, logic ~0.44) |
| host time in HLE handlers (`S3_HOST_PROF=1`) | ~250 ms | ~0.78 ms |
| - overlay copy + HUD compose (`apply_overlay_to`) | | 0.48 ms |
| - cursor + `Host::present` | | 0.06 ms |
| - 4 plain `Blt`s landscape ring buffer -> back buffer | | 0.20 ms |
| - primary surface -> `FrameBuffer` | | 0.05 ms |
| everything else (services, waits, critical sections, hooks) | ~60 ms | ~0.15 ms |

So the draw thread's own path (its guest code + the HLE work in its thunks) is ~2.3 ms of the ~2.9 ms frame; the
logic thread is ~0.45 ms (15 %). That bounds what moving the draw thread to its own core can give: **at most ~+18 %**
(frame = max(draw path, logic) instead of their sum), before any synchronisation cost.

## How the game synchronises draw and logic (RE, VERIFIED with `S3_TRACE=thunks` / `S3_TIMELINE`)

The render snapshot (`[logic+0x3d0]`, 0xc7720 bytes, see [resolution-zoom.md](resolution-zoom.md)) is
**triple-buffered** (three buffers rotate, e.g. `0x1a08f920`, `0x1ab07660`, `0x1a980000` in one run):

- Logic thread: the tick loop (`0x53e540..0x53e6f5`) calls the snapshot fill `0x51f110` up to five times per
  pass (between the simulation sub-steps `0x4e7810`, `0x4e05e0`, `0x4e9670` and after the AI). The fill first asks
  `0x51f0b0` whether the draw thread wants a new snapshot (vtable call on `[logic+0x3d8]`, a waitable); only then it
  fills its private buffer `[logic+0x3d0]` (allocated lazily with `new(0xc7720)`) and publishes it in `0x51f0e0`:
  `[logic+0x3d0] = InterlockedExchange([logic+0x3cc] /*shared slot*/, [logic+0x3d0])`, then
  `SetEvent([logic+0x3d4])` ("snapshot ready", event handle 0x45c in our runs). The shared slot lives on the main
  thread's stack (`0x700ffeb0`: the app object is a WinMain local).
- Logic idle wait: `WaitForMultipleObjects(2, [timer 64 ms, "want next" event], FALSE, t)` at `0x4631e1` (generic
  waitable-bag loop `0x463100`).
- Draw thread (`0x42c710`, one frame): `0x42c780` waits for "snapshot ready" (`WaitForSingleObject(0x45c, 500)`
  from `0x60c090`), takes the newest snapshot with `[draw+0x5c] = InterlockedExchange([draw+0x58], [draw+0x5c])`,
  signals "want next" (`SetEvent(0x458)` via `0x42cf80`), renders `0x4b26c0(snap)` and presents
  (`WaitForVerticalBlank`, `Blt`/`BltFast` to the primary). Paused branch (`[draw+0x68]`): exchange, `SetEvent`,
  `Sleep(60)`.
- Timeline of one frame (MT run, µs): `t4 WaitForVerticalBlank 42`, `t4 BltFast(primary) 795`,
  `t4 WaitForSingleObject -> 0 898`, `t4 InterlockedExchange 985`, `t4 SetEvent 1007`, `t5 WaitForMultipleObjects
  -> 1 1072`, `t4 BltFast x2 1371/1401` (landscape), `t5 InterlockedExchange 1692`, `t5 SetEvent 1715`, `t5
  WaitForMultipleObjects BLOCK 1892`, `t4 WaitForVerticalBlank 3298`.

So the game is already a **two-stage pipeline**: the logic thread fills snapshot N+1 while the draw thread renders
snapshot N, and both handovers are HLE calls (`InterlockedExchange` + auto-reset events), i.e. synchronisation
points we control.

## Options

### A. Draw thread on its own host thread (true parallel guest execution) - prototype, see below

The presenting guest thread runs its `Cpu` on a second host thread; the main host thread keeps the cooperative
scheduler for all other threads. Guest memory is shared; everything else in `Machine` is guarded by one big HLE
lock, released only while a CPU executes guest code. Expected gain: up to ~+18 % (Amdahl, above); effort: medium
(done). Risks: guest data races are those the game has on a multi-core PC; handovers are HLE calls under the lock
(mutex = acquire/release, so x86-TSO vs arm64 ordering does not matter there); `lock inc/dec` (6 sites, all on the
CRT locale counter `[0x7eaab4]`, `0x627f24..0x62c164`) run non-atomically in the interpreter (a lost update only
matters for a later `setlocale`, startup only); mod hooks on the draw thread (resolution switch `0x4c7490`,
`0x4b4d00`, `0x4b366a`, `0x4b2b34`) race with the logic thread as they already do at quantum boundaries.

### B. Pipeline host-side work (guest scheduling unchanged)

Move overlay compose + cursor + `Host::present` (0.54 ms/frame) out of the draw thread's present thunk onto a
presenter host thread. Gain: up to ~0.55 of ~2.9 ms (+15-20 %) **in the default single-thread mode too**, no
determinism risk (frames are output only). Obstacle: the overlay (`overlay_compose(&mut Machine, ..)`, every HUD /
Classic widget) reads game state through `Machine` while drawing, so the overlay must first be split into "record
a display list under the machine" (cheap) and "rasterise the list" (the 0.48 ms) - `s3_mods::overlay::Canvas` has
12 drawing calls plus 3 direct pixel accesses (`ui_classic.rs` magnifier read, `ui_hud.rs` icon sampling read,
`ui_hud_minimap.rs` pixel writes). The host also needs a `Send` present sink (desktop: the `Shared` frame slot;
Android: the GL presenter's slot). Medium-large effort in s3-mods (owned by the UI agents).
**Done 2026-11-08 (overlay-window)** for the overlay part: mods record a display list in window pixels
(`s3_mods::layer`), the frontend rasterizes only changed rectangles (Android: render thread, 2nd texture). The frame
copy + compose left the engine thread (see [mods.md](mods.md) "Window layer", measurements in
[performance.md](performance.md)); the cursor compose + `Host::present` copy are still on it.

### C. Others

- C1: deferred present in the MT mode (done, part of the prototype): the draw worker only queues the frame; the main
  host thread composes it. Moves the work off the draw thread but the main thread then holds the big lock while
  composing, so the draw thread waits for the lock instead (measured below). Needs B's display list to pay off.
- C2: split the 4 landscape blits (0.2 ms) over helper threads: identical bytes, deterministic, <= ~5 %.
- C3: audio mixing on its own thread: the mixer service is ~0.5 % of the time.
- C4: arm64 JIT (other agent): orthogonal; makes the guest part smaller, so the host part (B) matters more.

## Determinism argument

Lockstep MP needs every peer's **simulation** (logic-thread state: settlers, buildings, piles, economy, RNG
`gd+0x2d2dc`, world tiles) to be a function of the game setup and the network command stream only.

1. The normal (non-`s3det`) mode already interleaves threads by host timing (wall-clock `run_for_ms` slices, host-time
   waits/timers, the frame limiter), and two-peer runs are identical: the simulation does not depend on the draw
   thread's progress. MT makes the interleaving finer (true concurrency); it only adds a new kind of dependency if
   the logic thread reads memory the draw thread writes, or if HLE state the draw thread changes feeds the
   simulation other than through commands.
2. What the draw thread writes (write-set trace, below): its stack/TEB, render-side heap blocks, a redraw-flag array
   and blitter function pointers in `.data`, the hit-list double-buffer index, the shared snapshot slot (through
   `InterlockedExchange`). **Nothing in the game-data block `gd`** (simulation).
3. The one draw -> logic data path is the on-screen hit list (`[0x7dfd64]` = buffer index flipped by the renderer
   at `0x4b3f5e`, list at `0x7ade0a`, read by the click code `0x533b3a..`): clicks become player commands, which
   lockstep executes at agreed ticks on every peer - exactly as with the original game on two PCs.
4. `s3det` never enables MT (only `s3run` does, on `S3_MT=1`), so the deterministic single-thread scheduler stays the
   reference; with MT off the scheduler code path is the old one (checked: identical s3det hashes and instruction
   counts).

Conditions for the parallel mode: (a) the draw thread never writes simulation memory, (b) logic reads of render-side
memory do not feed simulation state, (c) HLE state the draw thread changes reaches the simulation only via the
command queue. (a) is checked by the write trace, (a)-(c) end to end by MP runs with MT peers against default peers.

## Prototype (`S3_MT=1`, s3run only)

Code: `port/crates/s3-runtime/src/mt.rs` (+ hooks in `machine.rs`), `s3_mem::GuestMemory::alias_view`, s3run
`main.rs` (`S3_MT=1` calls `Machine::enable_mt`).

- **Who goes to the worker**: the first non-main guest thread that presents 30 frames in a row (`mt_note_present`;
  in a game the menu draw thread and later `CDrawThread`; one worker at a time, a new one after the thread exits).
  Its `Cpu` is moved to the worker (a placeholder with mirrored counters stays in `Machine::threads`,
  `Thread::offloaded`).
- **Big lock** (`BigLock`): held by the main host thread all the time (frontend code included) except (1) while it
  runs guest code of a *runnable* top-level thread (`run_cpu_unlocked`: the CPU is moved out of the thread table,
  `Thread::running_unlocked`, executed on a second alias view, then moved back) and (2) while idling
  (`Machine::idle`, used by `Engine::run_for_ms` and `call_guest` waits). The worker takes it for every thunk, hook,
  fault and every 16th budget exit (budget exits need no HLE state: dropping the lock for each of them cost ~40
  lock trips per frame).
- **Wakeups** (`Wake`): an epoch counter bumped by every non-blocking thunk dispatch; waiters spin-yield 300 µs
  (`S3_MT_SPIN_US`), then sleep until a bump or the timeout. On Windows the condvar timeout follows the 15.6 ms system
  tick (measured: the draw thread's 1 ms waits took 15 ms), so Windows polls with 100 µs `thread::sleep` steps
  (high-resolution timer).
- **Deferred present**: on the worker, `present_owned` only queues the frame (`mt_defer_present`, a newer frame
  replaces an unpresented one); the main thread composes and presents it in `run_slice` (`mt_flush_present`) at least
  600 µs later (`S3_MT_PRESENT_DELAY_US`), when the draw thread is usually in its long lock-free render stretch.
- Diagnostics: the run summary prints per-thread `Cpu::run` time and an `mt:` line (lock waits, idle, worker waits,
  lock hold time, frames presented / dropped); `S3_MT_DEBUG=1` logs worker waits > 1.5 ms; `S3_TIMELINE=<host ms>`
  prints µs timestamps + args of every completed HLE call for 200 ms; `S3_HOST_PROF=1` the host-time profile.
- Test: `cargo test -p s3-runtime mt_pipeline` (two guest threads in the game's wait/present/signal pattern; ST and MT
  produce the same counts and guest results, the presenting thread really runs on the worker).
- Not wired into the Android frontend (no gain to ship).

### Measurements (desktop ARM64, throttling off, HUD 2316x904 zoom 10, `S3_FPS_CAP=0`)

| Run | Default mode (ST) fps | MT fps |
| --- | --- | --- |
| random map, samples 50-80 s (`tools/perf/fps-bench.ps1 -Random -Secs 80`, last 30 s), several builds of the same ST path | 307, 310, 339, 348, 363, 368 | 205, 223, 275, 278, 306, 331, 343, 369, 380, 408 |
| fixed map 384-2-Artokia, samples 30-70 s (`tools/perf/fps-bench.ps1`), big lock only | 210, 246 | 217, 185 |
| same + deferred present | 199, 231 | 190, 182 |
| same + present delay 600 µs (final) | 240 | 193, 245 |

MT is between -40 % and +17 % and much noisier (seconds with 30-60 fps). Where the time goes (summary `mt:` lines,
~70 s in game): the worker holds the big lock 0.75 ms per frame with presents on the worker (overlay compose under
the lock, so the logic thread's HLE calls wait), 0.3 ms per frame with the deferred present, but then waits ~0.5
ms per frame for the lock the main thread holds while composing; plus 0.1-0.5 ms per frame of handover latency
(the draw thread waits for "snapshot ready", the logic thread for "want next") and the renderer's guest code runs
~10-25 % slower on the worker (the snapshot it reads was just written by another core: cross-core cache traffic;
HYPOTHESIS, not measured with counters). With only ~15 % of the frame on the logic thread there is no room for these
costs.

**Conclusion**: true parallel guest execution is feasible and safe for the simulation (evidence below), but the
partition the game offers (draw vs logic) is too unbalanced for a gain while all HLE work shares one lock. To get
real gains, first do B (overlay rasterisation outside the machine / on a presenter thread: helps the default mode
by itself), then A again with the overlay out of the lock; finer-grained HLE locking (critical sections,
`Interlocked*`, events without the big lock) would remove most remaining lock waits.

## Evidence

- **Default mode unchanged**: `s3det --vms 90000` (inject of `tools/determinism-test.sh`) before (commit `e8042b2`)
  and after all changes: 91 identical hash lines, `insns=11886929367 mem=6e141c4900044d20 cpu=028473e586e3d37e`;
  also identical with `--write-trace` active.
- **Write-set trace** (`s3det --write-trace 60000-85000 --write-trace-out FILE`, new; `port/crates/s3-desktop/src/
  write_trace.rs`): one sampled scheduling turn per thread every 40 virtual ms (625 samples per thread, in game,
  build menu open), all writable pages compared before/after the turn (including the HLE calls of that turn):
  - logic (slot 5): 3410 bytes, incl. 2850 in the game-data block `gd` (`0x18b50000+0x1540000`), `.data` globals,
    heap, its stack.
  - draw (slot 4): 8633 bytes: render block `0x18880000+0x18d000` (7799), heap `0x10100000` block (375, 0x1c-byte
    records), `.data` redraw-flag array `0x7ad4f8[]` (set to 1 by the logic thread at `0x4fb54a`, read and cleared by
    the renderer `0x4b2b6a..0x4b2c40`), blitter function pointers `0x7e8c70/0x7e8c74` (draw code only),
    `0x7dfd64` (hit-list buffer index), its stack and TEB, 3 bytes of the snapshot slot `0x700ffeb0`.
  - bytes written by both: only the snapshot slot `0x700ffeb0` (via `InterlockedExchange`). **0 bytes of `gd`**
    written by the draw thread. (Sampling covers part of each frame; the hit list itself was not hit by a sample.)
- **Lockstep MP with MT peers** (`port/scripts/mp-local-test.sh` with the new `HOST_MT`/`JOIN_MT`, Artokia, 150 s,
  select + move clicks by both players, `--mp-hash` every tick):

  | Run | Host | Join | Ticks | Player commands | Result |
  | --- | --- | --- | --- | --- | --- |
  | mp1 | MT | ST | 1803 | 0 | IDENTICAL |
  | mp2 | MT | ST | 1497 | 8 | IDENTICAL |
  | mp3 | MT | MT | 1816 | 4 | simulation tables identical at every tick; world tiles differ from tick 576 (20 of 29 world hashes) |
  | mp4 / mp6 / mp7 | MT | MT | 1653 / 1818 / 1827 | 8 / 8 / 4 | IDENTICAL |
  | mp5 (control) | ST | ST | 1486 | 6 | IDENTICAL |

  mp3 has the signature of the world-tile difference already seen in default-mode runs (multiplayer.md 6, run long3:
  settler/building/pile/economy/player tables identical while a few tile bytes differ); it did not reproduce in three
  more MT/MT runs with raw dumps (`S3_MP_DUMP_STEP`). Open: catch it with dumps and check the byte offsets.
- Scratch (local only): `scratch/perf-mt/` (fps runs, `wtrace/report.txt`, `mp1..mp7`).
