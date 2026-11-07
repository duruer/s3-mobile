# Determinism (W5 prerequisite)

Settlers III multiplayer is **lockstep**: every peer runs the full simulation and only commands are exchanged.
All peers must compute bit-identical game state from identical inputs, on x86_64 desktops and arm64 phones.
This doc lists every source of nondeterminism in our emulator, what is fixed, how it is tested, and what is open.
Owner of the code parts: `port/crates/s3-cpu/src/fpu*.rs`; test tool `s3det`.

## Summary

| Source | Status |
| --- | --- |
| x87 basic arithmetic (`+ - * /`, `fsqrt`) | **Deterministic**: IEEE-754 binary64, correctly rounded on every host |
| x87 transcendentals (`fsin fcos fptan fsincos fpatan f2xm1 fyl2x fyl2xp1`) | **Fixed 2026-11-05**: pure-Rust fdlibm (`libm` crate, pinned), was host libm |
| `fprem`/`fprem1` | **Fixed**: exact integer long division (was host `fmod` via `%`), exact quotient bits |
| NaN results (sign/payload, two-NaN propagation) | **Fixed**: computed by our x87 rules, never by the host |
| f32 <-> f64 conversion of NaNs | **Fixed**: bit-level |
| `frndint`/`fist*` rounding | Deterministic (exact functions; NaN guarded) |
| FMA contraction | None: Rust never fuses `a*b+c`; verified 0 `fmadd/fmsub` in the aarch64 build of s3-cpu |
| Denormals / FTZ / DAZ / rounding mode of the host | Assumed IEEE default; checked at runtime by `fpu::math::host_float_env_check()` (s3det calls it; **frontends should too**, see open items) |
| 80-bit extended precision | Not modelled (f64 registers). Deterministic across our hosts; differs from real x87 only where the game relies on > 53-bit precision or the wide exponent range (see below) |
| `GetTickCount`/`timeGetTime`/waits/timers | Wall clock (`Host::now_ms`). Not part of lockstep state by design of the game (it runs on real PCs); deterministic in `s3det` (virtual clock) |
| `QueryPerformanceCounter` | Wall clock; only used by the blit benchmark at startup (picks a blitter, not simulation state). Virtual in `s3det` |
| `GetSystemTime`/`GetLocalTime`/time zone | Wall clock. **Seeds CRT `rand`** (`srand(time(NULL))`, see below). Fixed epoch in `s3det` |
| `rdtsc` | Wall clock (`exec.rs`), but **S3.EXE contains no `rdtsc`** (static census) |
| `CoCreateGuid`/`UuidCreate` | Host-time-seeded RNG (`rpcrt4::random_guid`). First divergence `s3det` found; deterministic in `s3det` |
| Thread scheduling | Round robin, instruction quanta: deterministic for a given clock + input history. In normal runs slices are wall-clock bounded, like on Windows; the game is built for preemptive scheduling, so simulation results must not depend on it (two-peer tests identical, multiplayer.md 6). The experimental multi-core mode (`S3_MT=1`, [multicore.md](multicore.md)) runs the draw thread truly in parallel; s3det never enables it; MP runs with MT peers: simulation tables identical at every tick |
| Uninitialised guest memory | Deterministic: the 4 GiB window is zero-filled by mmap, decommit/discard re-zeroes, heap reuse depends only on guest history |
| Directory listings (FindFirstFile) | Sorted case-insensitively in the VFS (`vfs.rs`), independent of the host file system |
| Host file timestamps | Not normalised (GetFileTime/FindFirstFile return host mtimes). Matter only if the game uses them in synced state (HYPOTHESIS: no; savegame list display only) |

## x87 model and why it is deterministic

Implementation: [x87-fpu.md](x87-fpu.md); this section is the determinism contract.

- **Registers are IEEE binary64 (`f64`).** Every operation produces exactly one IEEE result:
  `+ - * /` and `sqrt` are correctly rounded (round-to-nearest-even) on x86_64 (SSE2), aarch64, i686-SSE2 and wasm.
  Precision control PC=24 is applied by bit manipulation (`conv::apply_pc`); PC=53 and PC=64 both give f64.
  Rounding control RC is honoured by `fist*`, `frndint`, `fbstp`; arithmetic always rounds to nearest (the only RC
  changer in S3.EXE is the CRT `_ftol` at `0x61fe98`, which only does `fistp` while RC=truncate, see static-isa.md).
- **Relation to real hardware:** MSVC programs run with control word `0x027F` (PC=53, HYPOTHESIS for S3.EXE, the
  CRT sets it at startup). With PC=53 a real x87 rounds `+ - * / sqrt` to 53 bits like we do; remaining
  differences are (a) the wider exponent range of the 80-bit registers (intermediate overflow/underflow, double
  rounding of subnormal results), (b) values loaded from `m80` are rounded to 53 bits, (c) transcendentals
  (x87 uses its own microcode, < 1 ulp of the 64-bit result, and a 66-bit pi for argument reduction). This matters
  only for **PC crossplay** (W5 later): mobile + our desktop peers all run this same code and agree bit for bit.
- **Transcendentals** (`fpu_math.rs`): the `libm` crate (pure Rust port of musl/FreeBSD fdlibm, MIT), built with
  `default-features = false` so no arch-specific intrinsics are used, version **pinned with `=0.2.16`** in
  `s3-cpu/Cargo.toml`. fdlibm is not correctly rounded but is within 1 ulp, and it is a fixed sequence of IEEE
  basic operations, so its output depends only on the input bits.
  - `fsin/fcos/fptan/fsincos`: `libm::sin/cos/tan` (fsincos = fsin + fcos separately, so the results equal the
    single instructions). |x| >= 2^63: C2=1, operand unchanged (as hardware).
  - `fpatan`: `libm::atan2(y, x)`.
  - `f2xm1`: `expm1(p) + e*(1 + expm1(p))` where `p + e = x*ln2` exactly (Dekker product without FMA, plus the low
    part of ln 2) — within 1 ulp on the x87 domain [-1, 1] (tests: `exp2m1(0.5)`, `exp2m1(-0.25)`).
  - `fyl2x`: `y * libm::log2(x)`; `fyl2xp1`: `y * (libm::log1p(x) / ln2)` (two roundings, deterministic).
  - Accuracy compared to the old host-libm code: both are within ~1 ulp; results can differ in the last bit,
    which is invisible to the game (it only runs these in the CRT math functions; see "What feeds the simulation").
- **`fprem`/`fprem1`** (`math::fprem`): integer long division of the 53-bit significands, exact remainder for any
  exponent difference, quotient bits Q2/Q1/Q0 exact (the old code lost them for quotients >= 2^53). `fprem1`
  rounds the quotient to nearest even. Always the complete remainder (C2=0); real hardware reduces at most 63
  exponent bits per instruction and sets C2=1, which the CRT loops over — same final result.
- **NaNs**: x86 SSE creates `0xFFF8...` (negative quiet NaN = x87 "indefinite"), aarch64 creates the positive
  `0x7FF8...`, and when both operands are NaNs the host returns one depending on operand order, which LLVM may
  commute. We never let the host decide: `Fpu::finish` → `math::propagate_nan` (one NaN: that NaN quieted; two:
  the larger significand, tie → destination operand; SNaN raises IE), invalid operations → `INDEFINITE`
  (`0xFFF8_0000_0000_0000`, the x87 value). `fld m32`/`fst m32` convert NaNs bit by bit (`conv::f32_to_f64`,
  `conv::f64_to_f32`). `frndint` / `round_rc` return NaN inputs quieted without calling `floor/ceil/trunc/roundeven`
  (which are libm calls on baseline x86_64).
- **Float -> int** (`fist/fistp/fisttp`): round with RC (exact), range check, then `as` (exact for in-range
  values); out of range / NaN -> integer indefinite `0x8000...` + IE. Deterministic.
- **Host FP environment**: correct results need FTZ/DAZ off and round-to-nearest on the emulation thread. That is
  the default for Linux/Android/macOS processes on x86_64 and aarch64, but a library could change it on our
  thread. `fpu::math::host_float_env_check()` tests subnormal arithmetic, rounding and correctly-rounded
  division/sqrt at runtime.

### Golden vectors (`fpu_golden.rs`)

- `golden_x87_vectors`: 2048 pseudo-random operand pairs (several magnitude ranges + random bit patterns + a
  list of specials: ±0, ±inf, QNaN/SNaN, min/max subnormal, min normal, max) run through the **interpreter**
  (assembled loop, real decode/dispatch path) for 27 cases: `fadd fsub fmul fdiv fdivr`, PC=24 `fmul fdiv`,
  `fst m32`, `fsqrt fsin fcos fptan fsincos fpatan f2xm1 fyl2x fyl2xp1 fprem fprem1 fscale fxtract`,
  `frndint` in all 4 RC modes, `fcom`, `fxam`. Result bits and the status word of every item are hashed (FNV-1a)
  and compared with stored constants. 64-bit hosts only (needs the 4 GiB guest window).
- `golden_x87_int_conversions`: `fist m16/m32`, `fistp m64` in all 4 RC modes.
- `golden_pure_functions`: the numeric layer alone (math::*, conv::* incl. f80 conversion, ldexp, PC rounding),
  no guest memory, so it also runs on 32-bit hosts.
- Verified 2026-11-05: identical hashes on x86_64 (debug and release) and **i686** (`--target
  i686-unknown-linux-gnu`, pure layer). aarch64: the test binary cross-compiles
  (`--target aarch64-linux-android`, NDK `aarch64-linux-android26-clang` linker) and contains no FMA instructions,
  but has **not been run yet** (no qemu-aarch64 on this PC, no phone). To run it on a device:
  ```sh
  adb push port/target/aarch64-linux-android/debug/deps/s3_cpu-<hash> /data/local/tmp/s3cpu
  adb shell /data/local/tmp/s3cpu golden
  ```
- **If a hash changes**: on a new host only → real determinism bug, do not update the constant. After an
  intended change in fpu*.rs or a `libm` bump → update the constants in the same change and note it here: it
  breaks multiplayer compatibility between app versions (the lobby must compare a "sim version").

## What feeds the simulation (static RE, S3.EXE GOG sha256 `00a925cf…`)

- No `rdtsc`, no SSE (static-isa.md). x87 is mostly float constant initialisers, `_ftol` (44 callers) and a few
  CRT math functions; HYPOTHESIS (unchanged): the simulation itself is integer.
- **CRT `rand`** = `0x61f802` (MSVC LCG `x = x*0x343FD + 0x269EC3`, state per thread at `_getptd()+0x14`),
  **`srand`** = `0x61f7f5`. `srand` is called twice, both times with `time(NULL)` (`0x61f6a7`, built from
  `GetLocalTime`/`GetSystemTime`/`GetTimeZoneInformation`): in `0x431780` (app/object init called from
  `0x40cee0`) and in `CDrawThread` init `0x4932e0` (string "Menus::CDrawThread::Initialisier..."). Because the
  state is per thread, each `srand` only seeds the calling thread. `rand` has 16 direct callers (`0x40fb40`,
  `0x40ff70`, `0x410370`, `0x410acd`, `0x431ab0`, `0x47fea0`, `0x483760`, `0x4c7730`, `0x4d3050`, `0x548fb0`,
  `0x5492a0`, `0x5499b0`, `0x549bd0`, `0x5ff4f0`, `0x6042e0`, `0x60bca0`; `0x40fb40` is the serial/registry code).
  Open (W2): which of them run on the logic thread and whether the game uses its own seeded RNG for the
  simulation (no other common LCG multiplier — `0x41C64E6D`, `0x08088405` — occurs in `.text`). A lockstep game
  must seed the simulation RNG from the host's game-start message, so HYPOTHESIS: the logic RNG is separate and
  network-seeded; the time-seeded CRT `rand` is used for UI/effects/map preview.
- Timing: `timeGetTime` 31 sites (animation, scrolling, network timing), `GetTickCount` 2 sites, QPC only in the
  blit benchmark; logic thread period 64 ms (`0x53d761`). Logic progress in MP is gated by network turns, not by
  local time (HYPOTHESIS until W5 runs two peers).

## Determinism test: `s3det`

`port/crates/s3-desktop/src/bin/s3det.rs` (binary `s3det`, in the `s3-desktop` package) runs the game headless
with a **virtual clock**: `now = (guest instructions executed) / ipms + 1 ms per idle slice` (default ipms =
150 000, i.e. a 150 MIPS guest CPU, about the desktop interpreter speed, so a run takes roughly real time). The
clock only advances between slices of ipms/4 instructions; every guest-visible time is a function of the
instruction count. It overrides (registry re-registration after load, no change to the HLE crates):
`QueryPerformanceCounter` (virtual µs, strictly increasing), `GetSystemTime`/`GetLocalTime`/
`GetSystemTimeAsFileTime` (2001-01-01 00:00 UTC + virtual time), `GetTimeZoneInformation` (UTC),
`CoCreateGuid`/`UuidCreate` (fixed-seed xorshift). Inject times are virtual ms.

Every `--hash-every` virtual ms it prints `vms= insns= presents= mem= cpu= bytes=`: `mem` hashes all committed
guest-writable pages (not file mappings: GFX/maps are read-only data), `cpu` hashes every thread's registers,
EIP, x87 registers/status/control and instruction count (env `S3DET_HASH_FULL=1` adds EFLAGS, FS base and the
x87 tag word / last IP / last data pointer; perf-jit). `--dump-at V --dump f` writes the hashed pages raw;
`python3 -I tools/memdiff.py a b` lists differing byte ranges.

```sh
cd port && CARGO_TARGET_DIR=$PWD/target cargo build --release -j 4 -p s3-desktop --bin s3det
cd .. && tools/determinism-test.sh port/target/release/s3det 75000   # two parallel runs, diff of hashes
```

Results (2026-11-05, x86_64 desktop, two runs **in parallel**, fresh empty data dirs):

1. First attempt: instruction counts and CPU hashes identical for 45 000 vms, memory hash diverged at 6 000 vms.
   `memdiff` → one 16-byte range at `0x7a7ac8` = a GUID from `CoCreateGuid` (host-time-seeded). Fixed in s3det.
2. After the GUID override: **60 000 vms (menu → scenario → random map 384 → in game), all 61 hash lines
   identical**, final `insns=5896666465 mem=94407f5976cfd7ec`, identical final screenshots, 47 s wall each.
3. `tools/determinism-test.sh` (75 000 vms, adds build menu → place building → right click in game): see
   "Latest run" below.

This proves the emulator (CPU, x87, HLE, scheduler) is a deterministic function of (game files, clock, inputs).
It does **not** yet prove two *different hosts* agree (needs the same run on an arm64 device; the hashes printed
by `s3det` are directly comparable across hosts because guest memory layout is host-independent) and it does not
test lockstep MP itself (two peers with independent wall clocks) — that is W5's job.

### Latest run

2026-11-05, `tools/determinism-test.sh scratch/determinism/target/release/s3det 75000`, two runs in parallel:
**IDENTICAL**, 76 hash lines, final `insns=7699350378 presents=1953 mem=a68d8effad5b92f2 cpu=8e27020e7e8bfa7b`
(56 MB of writable guest memory hashed per line), 60.7 s wall each. The final frame shows the game with the
build menu open (the placement click at 420 300 did not create a site — choose a free spot when extending the
script).

### JIT (perf-jit, 2026-11-07)

The optional AArch64 JIT ([cpu.md](cpu.md#jit-optional-aarch64-perf-jit-2026-11-07)) must not change any of the
above: same registers, EFLAGS (as the lazy model materialises them), memory, EIP, retired counts and the same
budget stop points. Checked on the Windows 11 ARM64 PC with the 90 000 vms inject of `tools/determinism-test.sh`:

- `s3det` at the JIT commit, `S3_JIT=1` vs the baseline binary of `e8042b2` (interpreter): all 91 per-second
  hash lines **IDENTICAL**, `insns=11886929367 mem=6e141c4900044d20 cpu=028473e586e3d37e`.
- Same binary `S3_JIT=0` vs `S3_JIT=1` with **`S3DET_HASH_FULL=1`** (new: the CPU hash then also covers EFLAGS,
  FS base, x87 tag word, last IP and last data pointer): **IDENTICAL**, 91 lines, `cpu=e584fa62bd32339a`.
- `S3_FAST=0` (no native fast paths) with the JIT: same `insns`/hashes as the interpreter.

Every JIT change must repeat this (and `cargo test -p s3-cpu jit` on an aarch64 host).

## Open items / recommendations

- **Golden vectors fail on Windows 11 ARM64** (found by perf-interp 2026-11-07, rustc 1.99.1 / LLVM 23.1.1,
  `aarch64-pc-windows-msvc`, already at commit `f0a50f7`, not caused by the interpreter work):
  `golden_x87_vectors` differs only in **`f2xm1`**, and `golden_pure_functions` gives `0xa8d2b319571e5c63`
  (expected `0x7d0c9df6e90f4fca`). The compiled `math::exp2m1` contains no
  FMA (checked in the asm), so the suspect is `libm::expm1` or a toolchain change since the constants were made on
  x86_64; HYPOTHESIS, unverified. If it is aarch64-specific, phones and x86_64 desktops would desync in lockstep
  wherever the game runs `f2xm1` on sim data. Next step: run the golden test on x86_64 with the same toolchain and
  on the phone (`adb shell`), then bisect the pure-function hash per function.

- **Frontends**: call `s3_cpu::fpu::math::host_float_env_check()` (re-exported as
  `s3_runtime::cpu::fpu::math`) on the emulation thread at startup and refuse multiplayer if it fails.
- **Run the golden vectors and `s3det` on an arm64 phone** (or emulator-5610 with arm64 translation, by an agent
  allowed to use it) and compare with the x86_64 hashes above. s3det on Android needs a small harness (the
  binary is desktop-only today); simplest: build `s3det` for `aarch64-linux-android` and run it via `adb shell`
  with game files in `/data/local/tmp` (HYPOTHESIS: works since it has no window/audio dependencies besides `png`).
- **Mods (W1)**: `s3det` does not install `s3-mods` patches. Once hooks change game behaviour, install the same
  patch set in s3det so the test covers it; hooks must themselves be deterministic (no host time, no HashMap
  iteration order, no host floats in sim-visible results).
- **MP sim version**: include the s3-cpu golden hash (or a constant derived from it) in the lobby handshake so
  peers with different FPU semantics refuse to play together.
- `rpcrt4::random_guid` and `hosttime` stay host-random/host-time for normal play (unique session GUIDs are
  wanted); only s3det overrides them.
- `rdtsc` (`exec.rs`) still uses the host clock; harmless for S3.EXE (never executed).
- Crossplay with real Windows PCs (later): needs either proof that sim state never touches the x87 (likely, to be
  confirmed by W2 with a watch on the logic thread's x87 usage) or a full 80-bit soft-float x87.
