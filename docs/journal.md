# Journal

Newest first. One entry per session: what was done, what was learned, links to docs updated.

## 2026-11-05/06 — overnight Android port (workflow)

Result: the unmodified GOG S3.EXE runs on desktop and in the Android APK (emulator) from the main menu into a
playable game. Summary and next steps: [status](porting/status.md).

### wrap-up [done]
- Docs checked against code: stale notes marked (architecture first-integration result, directx-hle "not yet run",
  x87-fpu cpuid MMX note); strategy = option B implemented. New [status](porting/status.md). Added GPL-3.0 `LICENSE`.
- Legal check: git has no game bytes or decompiled dumps; only bundled asset is Liberation Sans (OFL 1.1, license file
  present); all Rust deps permissive. `cargo test --workspace`: 136 passed, 1 ignored.

### android [done]
- android-integration-1: release APK (arm64 + x86_64) runs S3.EXE on emulator s3_api35: menu -> Game Setup ->
  players (tap OK twice) -> in game; construction site placed by touch; pause/resume works.
- One-finger drag on the map = right button held with the path mirrored and halved, so the map follows the finger
  (S3 scrolls in the right-drag direction, accelerated). Arrow keys also scroll.
- Emulator in game: ~30 fps at ~95 M instr/s, about one core, RSS ~200 MB. Docs: [android](porting/android.md).

### integration-final [in game]
- Round 1: S3.EXE in s3run: main menu at ~0.2 s, Game Setup without serial prompt, in game on random map 384 at
  ~16-17 s; build menu and placing a construction site work with injected clicks. Call order matches the wine trace;
  guest C++ exceptions caught normally (20-43 per run, wine 36).
- Fixed gdi32: new memory DCs start with stock DEFAULT_BITMAP (SelectObject returned 0 -> C++ throw -> abort when
  opening the build menu). Added `S3_SAMPLE=1` per-thread sampler.
- Desktop in game: ~48 presents/s, ~100 M instr/s (render 70-85 M, AI ~20 M), ~60 % of one core; loading 130-165 M.
- Open (not fatal): no cursor drawing (GDIMouse=1), no desktop audio sink, two missing GetProcAddress names.
  Doc: [integration-log](porting/integration-log.md).

### android-shell [done]
- s3-android: engine thread, RGB565 letterboxed into ANativeWindow, touch -> mouse (tap left, long press / two-finger
  tap right, drag left held), AAudio, pause/resume with stopped guest clock, status screen instead of black.
- APK without Gradle (`scripts/build-apk.sh`, ~9.6 MB, 16 KiB-aligned stripped .so, debug keystore in port/target).
  AVD s3_api35 (API 35 x86_64, pixel_6) on port 5610, `scripts/run-emulator.sh`.
- Main menu on Android in < 1 s. Device needs regs.cmd (registry imports 48 values) and only the RGB565 GFX set
  (`*.f8007e01f.dat`, 118 MB).

### directx [done]
- s3-dx: DirectDraw (guest-memory surfaces, RGB565, present on primary writes/Flip, WaitForVerticalBlank = 60 Hz
  pacer, GetDC via s3_runtime::gdi), DirectSound software mixer (44.1 kHz stereo, clock-paced), DirectInput 3
  (SysMouse/SysKeyboard, event notification), DirectPlay stubs. 13 tests. Doc: [directx-hle](porting/directx-hle.md).

### user-gdi-misc [done]
- USER32, GDI32 (fontdue + Liberation Sans, OFL), ADVAPI32 registry (defaults, then regs.cmd, then
  data_dir/registry.json), WINMM/_INMM (timer thread, MCI cdaudio -> MUSIC/TrackNN.ogg via lewton, fake videos),
  stubs for ole32/rpcrt4/shell32/wsock32/wininet/comctl32/comdlg32. Doc: [user32-gdi32](porting/user32-gdi32.md).
- A failing MCI AVI open shows "Error during video play (266)"; a fake video that finishes at once skips the intro.
  WSAStartup failing (WSASYSNOTREADY) does not disturb startup.

### kernel32 [done]
- All 130 static KERNEL32 imports + waitable timers (via GetProcAddress) in s3-win32/src/kernel32*.rs; generic
  `Machine::poll_wait`, PulseEvent, critical sections with direct hand-off, VFS whiteouts. Doc: [kernel32](porting/kernel32.md).
- GFX .dat views are mmap'ed into the guest (~230 MB cost no RAM until touched). Menu threads arm 500 ms periodic
  waitable timers. 13 tests.

### cpu-int / cpu-fpu [done]
- Integer interpreter complete: 88 -> ~170-185 MIPS (Arc-free decode cache, fast lookup, parity table, hot-form
  handlers in fast.rs). cpuid = Pentium family 5 **without MMX**. 67-prefixed jcxz, CX loops, BCD, salc, AC/ID probes,
  GP/UD faults. `diff_tests.rs` checks ALU/shift/mul/div/bit ops against the host CPU. Doc: [cpu](porting/cpu.md).
- x87: 8 f64 registers, condition codes, RC on integer stores/frndint (CRT `_ftol` 0x61fe98), exact 80-bit conversion,
  BCD, fnstenv/fnsave layouts, fcomi, fprem quotient bits; MMX movq/movd/emms. Limits: 53-bit precision, arithmetic
  always round-to-nearest, unmasked exceptions not delivered. Doc: [x87-fpu](porting/x87-fpu.md).

### static-re [done]
- ISA census (tools/isa-census): i386 + x87 only, no SSE/cmov/rdtsc, no self-modifying code; cpuid/MMX only in a
  guarded probe -> report no MMX. [static-isa](reverse-engineering/static-isa.md)
- Startup: entry 0x621bc6 -> CRT -> WinMain 0x45a120 (refuses year > 2037) -> CThread::Execute 0x60e2d0; WndProc
  0x610040. CLogicThread 64 ms timer; draw thread paced by WaitForVerticalBlank + Blt. C++ EH everywhere (CxxThrow
  x149) -> real SEH + RtlUnwind needed. [static-startup](reverse-engineering/static-startup.md)
- Resolution 0/1/2 = 640x480/800x600/1024x768 at 16 bpp; GFX suffix from pixel masks; filename case differs ->
  case-insensitive VFS. DInput SysMouse only; DirectPlay/Lobby GUIDs decoded. unknowns #1/#7/#10-#14 updated.

### wine-trace [done]
- Wine 11 WoW64 in Xvfb reaches intro, menu (800x600x16) and in game (640x480x16); a wine virtual desktop is needed.
  [wine-trace](reverse-engineering/wine-trace.md), tool `tools/wine_relay_summary.py`.
- unknowns #1 resolved: serial = HKLM ...\General\SingleNumber + HKCR\CLSID\{0e9f8400-...}\Seed.
- DDraw exclusive fullscreen RGB565, primary without back buffer; DSound 22050/16; music via _INMM ogg -> waveOut;
  intro = Indeo 5 AVI via MCI; data files memory-mapped; C++ exceptions are normal flow; timeSetEvent never called.

### architect [done]
- Rust workspace `port/` with 9 crates (s3-mem, s3-cpu, s3-runtime, s3-pe, s3-win32, s3-dx, s3-engine,
  s3-desktop/s3run, s3-android); runtime: thunk registry with UNIMPLEMENTED auto-stubs, cooperative green threads,
  nested call_guest, guest-side SEH + RtlUnwind, handles, COM factories, case-insensitive VFS with write overlay.
- s3-pe loads S3.EXE (entry 0x621BC6, 219 imports). First headless run stopped at InitializeCriticalSection after
  67 instructions (since fixed). Contract: [architecture](porting/architecture.md); strategy decided (option B).

## 2026-11-05 (c) — tooling inventory

- Documented RE tools (`~/Projects/reverse-engineering`, `/opt/ghidra`) and the Android SDK/NDK/AVDs → [tooling](tooling.md).
- REA in this session is our own stdio server, so open/close is safe. In the shared multi-agent setup it is not (see tooling).

## 2026-11-05 (b) — static RE with REA/Ghidra

- Opened S3.EXE in REA (Ghidra 12.1.2 auto-analysis, ephemeral DB, nothing saved).
- XOR at 0xFE72 = decoder for a registry-stored blob (HKLM value + HKCR key) → [exe-variants](game-files/exe-variants.md), [unknowns](reverse-engineering/unknowns.md) #1.
- `ProtectCD.dll`: no code xrefs → dead string (static view).
- DirectX surface mapped: IDirectDraw2 via CoCreateInstance, DirectSound via DirectSoundCreate, DirectInput 3.0, DirectPlay 6; full method list from the game's own error strings → [directx-surface](reverse-engineering/directx-surface.md).
- VirtualProtect only in a page-touch helper, no self-modifying code seen → good for an interpreter/JIT.

## 2026-11-05 — project bootstrap

- Created `~/Projects/settlers-3` with umbrella `AGENTS.md` (index only), `docs/`, `tools/`, gitignored `game-files/`.
- Found the GOG install on Windows C: (`/dev/nvme0n1p2`, `Loki`) at `C:\GOG Games\Settlers 3 Ultimate`; Settlers United at
  `C:\Program Files\Settlers United`. Not on the `Luna` disk. Copied both (game 1.6 GB, SU resources 385 MB) → [inventory](game-files/inventory.md).
- `S3.EXE` vs `s3_united.exe`: 7-byte diff (NOPed XOR decoder at 0xFE72; `SingleNumber`→`SerialNumber`) → [exe-variants](game-files/exe-variants.md).
- `S3.EXE`: MSVC 6, base 0x400000, no relocs, 219 static imports; DirectX/winsock/MCI loaded dynamically → [s3-exe](reverse-engineering/s3-exe.md).
- Registry defaults from `regs.cmd` → [registry](reverse-engineering/registry.md).
- Installed rizin, wine, wine-mono, winetricks, python-pefile (via kdesu). Added `tools/pe_imports.py`.
- Draft strategy: own PE loader + x86 core + Win32/DirectX HLE (blockheads-pc pattern) → [strategy](porting/strategy.md).
- Next: run under Wine with relay trace (strategy "Next steps").
