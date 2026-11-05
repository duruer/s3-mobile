# Journal

Newest first. One entry per session: what was done, what was learned, links to docs updated.

## 2026-11-06 — polish night (workflow)

Result: cursor, save/load, soft keyboard and Android UX polish on top of the working port; the whole flow (menu ->
in game -> build -> save -> quit -> load) verified on desktop and on the emulator. Summary: [status](porting/status.md).

### wrap-up [done]
- AGENTS.md map rows updated (cursor, 1/4 bpp DIBs, Android UX, audio verification, inject recipes); every doc is
  listed. android.md: the panel's computer menu no longer "draws empty" (fixed by save-load). status.md updated
  (title, phone controls, next steps renumbered, licensing re-check).
- Legal: `git ls-files` has no game bytes; new deps `jni` 0.22 (MIT/Apache-2.0) and optional `cpal` 0.15
  (Apache-2.0) are GPL-compatible; no new bundled assets. `cargo test --workspace -j 4`: 159 passed, 1 ignored.

### cursor [done]
- The mouse cursor is now drawn. New `s3_runtime::cursor` reads S3.EXE cursors (RT_GROUP_CURSOR -> RT_CURSOR):
  mono AND/XOR and colour cursors up to 32 bpp with alpha, plus our own code-drawn arrow for IDC_* cursors.
  CursorState is fed by user32 LoadCursorA/LoadImageA/SetCursor/ShowCursor/ClipCursor. `Machine::present` draws the
  cursor on a host copy of the frame for all frontends. A "cursor" service (`Machine::refresh_cursor`) re-sends the
  last frame when only the cursor changed, because the game does not redraw when the mouse moves.
- Found by decompiling: the game's cursor routine FUN_0045c3a0 sets IDC_ARROW, then game cursor 115 (normal) or
  139 (busy); id -1 hides it with ShowCursor(FALSE). S3.EXE has 9 cursor groups, all 32x32 1 bpp.
- Checked headless (menu, loading, in game): the drawn in-game cursor matches RT_CURSOR 3 pixel for pixel. The
  minifb window hides the system pointer. Doc: [user32-gdi32](porting/user32-gdi32.md#cursor-drawing).

### save-load [done]
- Save/load round trip verified on desktop headless: start random 384; in game computer icon (21,452), "Save Game"
  (68,225), slot 1 line (40,244) saves at once to the overlay `Save/slot 1.sav` (~4.6 MB, header type 0x0b, no
  name prompt); Quit Game (68,420), YES (38,420), statistics (click anywhere), main menu, Load Game (165,355),
  entry (300,126): the saved game continues.
- Fixed (1) gdi32 had no 1/4 bpp DIB support: S3 draws all in-game panel text and tooltips with DrawTextA into a
  640x480x1 top-down DIB section used as a mask, so the Save/Quit menu and tooltips were invisible.
- Fixed (2) CreateThread treated the stack argument as the full size; Windows treats it as the commit and keeps
  the PE reserve. S3 passes 0x1000, so threads got 64 KiB and the save-slot reader (FUN_004a5c80, 256 KiB of
  locals via _chkstk) ran below its stack; its SEH frames failed frame_valid and the Win32::CFileNotFound* for a
  missing "slot N.sav" (thrown by FUN_00619610, caught in FUN_004a5600) became an "unhandled exception".
- VFS needed no change. Load Game lists FindFirstFileA("C:\S3\Save\*.sav"); Save\Multi\*.mps from the game dir
  stays visible (multiplayer only). The 19 startup MoveFileA calls in Map\Multi fail with ERROR_ALREADY_EXISTS on
  our GOG copy (ERROR_FILE_NOT_FOUND under wine); harmless.
- New: `S3_FILES=1` file-op log; s3run inject `shot <png>` and `text <str>` (in game, letters are hotkeys). Tests
  save_slots_overlay_and_game_dir, create_thread_stack_is_commit_not_reserve, draw_text_into_monochrome_dib.
  Docs: [kernel32](porting/kernel32.md), [user32-gdi32](porting/user32-gdi32.md), [integration-log](porting/integration-log.md).

### input-ok-bug [done]
- "The first OK on the players screen is ignored" (desktop and Android) was a miscount, not an emulation bug.
  Single Player: Scenario has three OK screens: map-selection popup over Game Setup (OK 630,467), Game Setup (OK
  730,567) and the players screen (OK at the same 730,567). The click that looked ignored was the Game Setup OK
  (handler 0x488300 builds the game parameters; CoCreateGuid in 0x431ab0). The players OK runs 0x4955b0
  (message 10) -> 0x496c90 -> PostThreadMessageA(t0, 0x801), which starts loading.
- How S3 reads menu clicks: main thread t0 runs a GetMessageA loop; its window procedure handles mouse-move
  (0x45bec5), button-down (0x45b698) and button-up (0x45b89e), calls ClientToScreen and wakes GUI thread t2 with
  SetEvent (from 0x430b64). After each message it calls GetDeviceData with rgdod=NULL, so in menus the DirectInput
  buffer is only flushed. On button-up the UI button (0x41ab10) fires the listener at [this+0x38] (via 0x41a9b0),
  then the one at [this-0x10]. Verified headless: one click per OK, 1 s apart, reaches in game.
- Added kernel32 IsProcessorFeaturePresent (TRUE only for 2 = compare-exchange-double and 8 = RDTSC, matching our
  CPUID) and user32 GetLastActivePopup (returns hwnd). New test s3-win32 tap_gives_one_press_per_tap. Docs fixed:
  status, android, integration-log, kernel32, user32-gdi32.

### audio-verify [done]
- s3run audio sink (`port/crates/s3-desktop/src/audio.rs`): `--audio-wav <file>` writes a bounded WAV
  (`--audio-wav-seconds`, default 300; header rewritten every second) and prints a timing summary at exit
  (lead/drift/max gap). `--audio-play` plays through cpal 0.15/ALSA behind the cargo feature `audio-cpal` (default
  off). New `tools/wav_stats.py` (python3 -I: RMS per window, clipping, silence, zero gaps).
- Bug: winmm's music_service called Host::queue_audio itself while the s3-dx DirectSound mixer also did (two
  producers). Fixed: music goes into AudioStreams[MUSIC_STREAM_ID] (LEAD_MS 300, cleared on stop/close/new play).
  New env `S3_MUSIC_VOLUME`. dsound.rs: host-queue guard MAX_HOST_QUEUE_MS=250 (drift_cut_frames counter), debug
  log and voices_started counter per Play. 3 new s3-dx tests (gapless stream passthrough, 22k->44.1k resampling +
  pan, queue guard).
- Measured in game: lead flat at 60 ms, drift 0 over 40 s, no gaps; music track 7 at -31/-28 dBFS, no clipping;
  effects -35..-55 dBFS (11 mono 22050 Hz buffers, SetVolume -2000, SetPan, one-shot Play, no streaming or
  GetCurrentPosition); menu silent, as under wine. Docs: [directx-hle](porting/directx-hle.md), user32-gdi32,
  architecture, integration-log, status.

### android-ux [partial]
- New `uithread.rs` runs JNI jobs on the Java main thread with no Java code of our own: a Unix socket pair is
  registered on `AndroidApp::java_main_looper()` via `add_fd_with_callback` (jni 0.22). Used for immersive
  fullscreen (WindowInsetsController hide systemBars with swipe behaviour; `setSystemUiVisibility(0x1706)`
  fallback), showing/hiding the keyboard (`IMM.showSoftInput(getCurrentFocus(), 0)`, `hideSoftInputFromWindow`,
  visibility from `getRootWindowInsets().isVisible(ime)`) and `Activity.finish`.
- New `overlay.rs` draws ESC/ABC/RMB buttons in the letterbox bars; touches starting on a button never reach the
  game. touch.rs: three-finger tap requests the keyboard; two-finger right click fires when a finger lifts;
  right-click-next mode. keys.rs/android.rs send Key plus Char from KeyCharacterMap; once the host sends any Char,
  USER32 stops making chars itself (`host_chars`). frontend.rs/screen.rs: first-run screen plus a wait for
  REQUIRED_PARTS; `guest_exited` closes the app.
- Findings: on a NativeActivity the soft keyboard (Gboard) works (typed characters arrive as plain key events
  through the fallback input connection). Esc in the main menu exits S3 (ExitProcess), on desktop too. Esc in game
  does nothing (desktop too); the in-game menu is the panel's "computer" icon (its "draws empty" state was fixed by
  save-load). Open: Back cannot open the in-game menu; three-finger tap untested (adb has no multi-touch).
- Verified on emulator-5610 (scratch/android-shots/ux-*-s.png). Doc: [android](porting/android.md).

### verify-1 [done]
- Workspace build + tests OK (159 passed, 1 ignored). Desktop s3run headless, one run: menu -> SP -> in game
  ~17 s -> build list/ghosts -> save slot 1 -> quit -> stats -> menu -> Load Game -> saved game continues. Cursor
  visible; WAV silent in menu/loading, RMS ~1000-2600 in game, no drift, max gap 43 ms. One run took ~77 s to load
  (normally ~10 s, low load average), not reproduced in 2 runs (HYPOTHESIS: host contention).
- Android release APK (arm64-v8a + x86_64) on emulator-5610, all by adb taps: menu, SP, soft keyboard via ABC
  (adb text, Backspace, soft-key tap typed into the Game Setup name), in game ~25 s, Woodcutter's Hut site placed,
  save slot 1 "Game Saved", Back harmless in game, Home+reopen resumes, drag scroll, Quit/YES/statistics/menu, Load
  Game, saved game continues. Cursor on every screen (arrow, hourglass while loading). ~102 M instr/s, ~30
  presents/s, RSS 225 MB. Finding: the Load Game screen is 800x600. No code changes.
  Docs: status, android, integration-log.

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
