# Journal

Newest first. One entry per session: what was done, what was learned, links to docs updated.

## 2026-11-07 — perf-r2 on the phone

After items 1-5 (e8042b2): Fold4 HUD zoom 10 holds the 60 fps cap (was 48-56), engine thread 70 % of a core.
Items 6 (arm64 JIT) and 7 (multi-core) running. → [performance](porting/performance.md)

## 2026-11-07 — perf-shadow: native row loops of the shadow sprite functions

- New `port/crates/s3-mods/src/fast_shadow.rs` (see [performance](porting/performance.md), "perf-shadow"):
  `shadow_zero` = second row loop of `0x5fc160` with its per-run callee `[0x7e8c74]` = `0x5f48e0` (zero runs),
  `shadow_rows` = row loop of `0x5fd120` with callee `[0x7e8e20]` = `0x5fd080` (checkerboard) or `0x5fd0c0`
  (darken). Whole runs/rows including the calls (stack slots of call + pushes reproduced), callees hash-checked.
- Finding: in game (HUD 2316x904) `0x5fc160` + `0x5f48e0` were 21 % of guest instructions on some maps (the profile
  attributed `0x5f48e0` to `0x5f4830`); its first loop (`[0x7e8c70]`) and the fog loop `0x5fc41a` of `0x5fc350`
  did not run at all. After: `0x5fc160` 1.5 %, `0x5fd120` 0.8 %, frame renderer `0x4b26c0` 63 %.
- Desktop benchmark (S3_FPS_CAP=0, 60-80 s, alternating runs, random maps): base ~35 / ~31 fps, new ~40 / ~38 fps.
- Differential tests `differential_shadow_zero` / `differential_shadow_rows` pass; s3det 90 000 vms fast vs
  `S3_FAST=0`: IDENTICAL, default settings (`insns=11817005518`) and HUD settings (`insns=12256208149`)
  (`scratch/perf-shadow/det-plain`, `det-hud`).

## 2026-11-07 — performance review

Phone HUD zoom 10 (1944x760): 50-56 fps. simpleperf: ~17 % of the engine thread is host presentation work (frame
copies, RGB565 conversion loop, software HUD drawing), rest interpreter; guest profile led by the `0x4b26c0` tile
loop (42 %). Ranked list of options in [performance](porting/performance.md#measurements-2026-11-07-perf-review).

## 2026-11-07 — full-screen menu layouts

- Menus are fixed 800x600; on the 2316x904 phone plain crop keeps only 312 rows. New `s3_mods::menu_layout`:
  per-screen pieces re-arranged on a wide canvas, detected by 4x4 probe colours (popups -> Fit), touch mapped back
  through the pieces, GPU quads with a `u_uv` sub-rect. 10 screens. Plain colour backgrounds because the game draws
  the cursor into the frame (tiles repeated it). `s3run --layout-preview/--layout-probe`. Phone: main menu full
  screen, moved Load Game button works. → [menu-layout](porting/menu-layout.md)
- User bugs: (1) black sides on the tutorial loading screen -> `loading` layout (640x480). (2) tutorial map could
  not be panned below the upper half: the game camera clamp `0x51b4d0` uses the view width for the y limit; with
  wide resolutions that collapses on small maps. gog-base patches `0x51b528` to read the view height; host clamp
  too. Both verified on the phone. → [game-state-ui](reverse-engineering/game-state-ui.md)
- User bug: a new game started with the castle in the upper-left part of the screen (Mobile HUD). The game centres
  the castle for the narrow start view (640x480); the res switch to the wide size kept the camera corner, and the
  HUD removing the panel shifted the view again. Fix: `res::switch_hook` anchors the view centre when no anchor is
  given; `ui_hud_game::apply_layout` shifts the camera by half the panel width. Verified desktop + phone (castle
  centred after Scenario -> map picker -> setup -> players, all through the menu layouts).
- PC crashed mid-session; files were intact. Phone adb port changes after reconnect (now :42279).

## 2026-11-07 — real game launcher icon, build-apk.sh on Windows

- Launcher icon is now the game's own crest from the user's `S3.EXE` (`RT_ICON` id 11, 32x32 4 bpp; id 10 is 16x16),
  extracted at build time by new `tools/s3_icon.py` (pure stdlib, PE or .ICO input) into `port/target`, never committed.
  `S3GOLD.ICO` = gold-edition 32x32 8 bpp variant; `gog.ico` = generic GOG CD icon. → [android](porting/android.md)
- `build-apk.sh` ported to Windows Git Bash. A new debug keystore was generated in `port/target` (the WSL one is
  not on this machine), so an APK built here cannot update an install signed on the old machine (uninstall first:
  this deletes the pushed game files under `Android/data`).
- Wireless adb: this PC had to pair first (`adb pair <ip>:<pair-port> <code>`; before that `adb connect` just says
  "failed to connect"). Then `adb connect 192.168.1.52:40781` worked.
- Reinstall without re-pushing 1.1 GB: with root, `mv` `files/game` + `files/data` to `/data/media/0/s3port-backup`
  (same fs, instant), uninstall, install, launch once, `mv` back, then `chown -R <new app uid>:ext_data_rw` and
  `chcon -R` to the new dir's context (incl. its MLS categories, e.g. `s0:c95,c257,c512,c768`). Game starts fine;
  launcher shows the crest icon (Samsung puts it on a white tile). → [android](porting/android.md)
- Legal: the crest is Ubisoft artwork, so an APK with it must not be published. Default icon is now our own adaptive
  vector "III" (white serif, red outline, navy; style like the Settlers United icon without its "IV"). Game crest only
  with `build-apk.sh --game-icon`. Installed on the phone, shows correctly. XML comments must not contain `--`
  (aapt2: "not well-formed").
- Removed the ESC / ABC / RMB letterbox buttons (`s3-android/src/overlay.rs` deleted, presenter/UI thread/Shared
  cleaned). Back = Esc, three-finger tap = keyboard remain. Verified on the phone (main menu, no buttons).
## 2026-11-07 — perf-tile: frame renderer cell loop native

- `0x4b26c0` (draw thread) was 41.9 % of guest instructions in game (HUD 2316x904). Its cell loop
  `0x4b2e44..0x4b330e`, per cell: sector `q = x/40` (magic multiply); ambient-sound statistics for sectors 0..15
  (`[ebx+0x5f588/0x5f5cc + 4*(land>>4)]` += `8` or `15-q` / `min(q,8)`; bytes `+0x503` 0xf8..0xfd add weighted sums
  at `0x4b2f0e`); a 4x4 water-decoration grid (`0x4b3046`: land < 0x10 at the cell and 3 neighbours,
  `(snap[0x24]+i) & 3 == 0`, `(snap[0x28]+r) & 3 == 0`); then `0x4d3050` from one of three sites: `L >= 0x49` with
  type `+0x503` (or 0xfe), else by the `+0x508` kind (0x1800 = typed object, 0x1000 / 0x800 = more drawers). Rows
  advance y += 9, x0 +/- 8. A pre-pass `0x4b277e` turns landscape 0x10 <-> 0x1c by bit 2 of `+0x509`.
- New native fast paths `frame_tiles` + `land_mark` (`port/crates/s3-mods/src/fast_tile.rs`), the empty object call
  inlined via `fast_obj::plan/apply`. Profile share 41.9 % -> 6.2 %; desktop HUD fps ~29-35 -> ~41-47; s3det 90 000
  vms identical to `S3_FAST=0`. New env `S3_FAST_OFF`. Details: [performance](porting/performance.md).
## 2026-11-07 — perf-interp: faster interpreter (s3-cpu)

- Measured first (in game, HUD 2316x904): blocks average 6.5 instructions, generic path ~1.1 % of instructions,
  instruction pairs diffuse (top pair 4.3 %), successor cache hit rate 98.8 %. The cost was the per-instruction
  call loop and a run loop that spilled registers (inlined `exec::step`).
- s3-cpu: tail-call threaded dispatch inside blocks (`fast::next`, `block_end` sentinel, no per-op EIP store,
  budget stops inside a block via `exec::step`), lean run loop, fast forms for imul/mul/cdq/cwde, 8/16-bit and CL
  shifts, setcc, neg/not, inc/dec m32, 32-byte `Op`, boxed blocks with pointer chaining, `cmp/test/dec + jcc`
  superinstructions. New tests: exact budget stops, mid-block exit counts, fast-vs-generic form tests that run on
  ARM hosts too.
- Windows ARM64 dev PC (power throttling off): bench_mips 440 -> 1160 MIPS; s3det 90 000 vms `Cpu::run` 17-19 s ->
  9.8 s (CPU time 20-23 s -> 13.3 s); s3run HUD 2316x904 uncapped 93-102 -> 126-190 fps (random maps).
  s3det old vs new: IDENTICAL hashes and `insns=11817005518` (also with EFLAGS/FS/x87 last-IP in the CPU hash).
- Learned: agent-started processes on this PC run under Windows power throttling (EcoQoS), 2-4x slower and noisy;
  benchmark with throttling disabled (performance.md). The `fpu::golden` tests fail on Windows ARM64 already at
  `f0a50f7` (f2xm1) — open determinism issue (determinism.md). `s3-runtime` `vfs_case_insensitive_and_overlay` also
  fails on Windows at `f0a50f7`.
- Docs: [cpu.md](porting/cpu.md), [performance.md](porting/performance.md), [determinism.md](porting/determinism.md).

## 2026-11-05 — fps, zoom, full screen, HUD (workflow)

User feedback (workflow wf_a446e364-b12): minimum 60 fps (was ~26 on the phone, ~30 emulator, ~48 desktop), an FPS
counter setting, full-screen display like Settlers United, the game's own left panel hidden in the Mobile HUD, a
full-screen map on minimap tap, in-game zoom in both UI modes. Result on the real phone (Galaxy Z Fold4 cover screen):
Classic Touch 60 fps at the default zoom (cap), Mobile HUD 48-58 fps, full width 2316x904 @120 Hz, pinch zoom,
full-screen map. Docs: [performance](porting/performance.md), [display](porting/display.md),
[resolution-zoom](porting/resolution-zoom.md), [ui-hud](porting/ui-hud.md), [status](porting/status.md).

### perf-cpu [done]
- S3 has no fps cap besides one `WaitForVerticalBlank` per frame; the logic thread is a separate 64 ms timer. Our
  60 Hz vblank grid turned >16.7 ms frames into 30 fps -> replaced by a frame limiter (`ddraw.rs`, default cap 60,
  `S3_FPS_CAP`, `set_fps_cap`). Then the draw thread is CPU bound at ~2.5-2.9 M guest instructions per frame (640x480).
- Guest profile in game (`S3_PROFILE` + REA): `0x51f110` snapshot builder ~19 %, `0x4b26c0` ~17 %, `0x4d3050` ~13 %,
  `0x54cbd0` ~11 %, blitters `0x5fc350/0x5fd080/0x5fd120/0x5fc860` ~25 %. Map generation (`0x5081a0/0x4da440`,
  logic thread) takes 10-60 s.
- s3-cpu: pre-decoded `Op` records (EA forms as const generics), 8/16-bit fast forms, cold fault paths, lazy flags
  (`Cpu::lk`, `eflags()/set_eflags()`), `cond_fast` jcc, block chaining, codegen-units=1. bench_mips 178 -> ~285;
  callgrind 12.07G -> 8.69G Ir. s3det 90 000 vms old vs new identical (`insns=11897551127`, `mem=63839f7d8d3d7fb6`),
  114.9 s -> 66.3 s.
- Native fast paths: s3-cpu `NativeBlock` (code-hash matched, budget-exact, exact instruction counts),
  `Machine::set_native_blocks`; s3-mods `fast.rs`/`fast_sprite.rs`/`fast_blit.rs` for `0x5fc4c0` (opaque sprite rows),
  `0x5fc93d` (palette row), `0x5fd0ea` (darken), `0x5fd096` (dither); `S3_FAST=0` disables. Differential `fast_tests`
  (real exe, 120 random cases each) caught an EBX bug at budget stops in dither (fixed). s3det fast on/off identical,
  64.4 s -> 55.9 s.
- Desktop in game uncapped 33 -> 81 fps at 640x480; 800x600 default 53-55 fps at cap 60.
- Tools: `S3_SAMPLE fps:/cpu:`, `S3_PROFILE` (+`_FROM_MS`, `.disasm`, `.forms`), `tools/perf_profile.py`,
  `tools/perf/code_hash.py`. Docs: performance.md (new), cpu.md, directx-hle.md, status.md.

### display [done]
- New `s3_mods::display`: modes Fit / Fill / Stretch / NativeWide (default), filters Nearest / Linear / SharpBilinear
  (default), FPS counter Off / Fps / Detailed; saved in `mods-settings.json` under `display`. The `Viewport` is the only
  screen<->game transform (Machine helpers `viewport`, `visible_game_rect`, `set_display`, `cycle_mode/filter/fps`, ...);
  CPU scaler `scale_to_xrgb` for desktop/fallback. New `fps.rs` (`FpsMod`, real presents + guest instructions per
  500 ms, installed last so it draws over the UI modes).
- Android: render thread `s3-present` (`present.rs`, EGL/GLES2 via own FFI `gl.rs`): RGB565 texture + one quad, sharp
  bilinear in the fragment shader, swap interval 1, only new frames drawn; high refresh via
  `ANativeWindow_setFrameRate[WithChangeStrategy]` (dlsym); JNI `display_prefs` picks the highest-refresh mode and
  cutout mode ALWAYS. Engine present only copies + signals; window loss waits for EGL surface destroy; EGL failure
  falls back to the CPU letterbox. s3-mods is now a non-optional dependency of s3-android.
- s3run: `--display`, `--filter`, `--fps`, `--window WxH` (resizable, mouse mapped through the Viewport; headless
  publishes a simulated viewport). Classic UI: Screen and FPS menu toggles; floating buttons and ✕ anchored to the
  visible rect.
- Tests: s3-mods 55, s3-android 25, s3-desktop 8 pass; arm64 APK builds (NEEDED libEGL, libGLESv2); headless in game
  in both modes at `--window 2229x904 --display fill --fps`. Docs: display.md (new), android.md, mods.md, ui-classic.md.

### res-zoom [done]
- RE: visible tiles are copied into a render snapshot (`[logic+0x3d0]`, new(0xc7720)): 130x130 grid at +0x500,
  0x30 bytes/tile, index row+col*130. Sheared rows need cols+rows/2 columns (1024x768 used 126 of 130). Patch moves
  the fields behind the grid (+0xc65c0..0xc7720) at 51 code sites plus the allocation size -> 256 columns; the row
  stride stays, so height <= 780.
- F1-F3: `[0x7dfd44]` = request; key handler releases semaphore `[0x7a800c]` (0x525b5b), the logic thread's ack waits
  on it (0x53db80). Mode set: 0x42ce90 posts 0x804, 0x405c80 calls SetDisplayMode, display setup 0x42e430 reruns and
  caches the view rect in the landscape object (+0x58/+0x5c/+0x74..+0x80) -> cause of the HUD black band after F1-F3;
  fixed by re-syncing every frame and redrawing on a snapshot of the new size.
- Code: s3-mods `res.rs`, `zoom.rs`; s3-dx SetDisplayMode up to 4096 + flag `DDSDM_PORT_LOGICAL` (crop the presented
  frame without recreating surfaces); settings `res` section; pinch calls zoom; Classic menu Zoom in/out.
- Verified on desktop: HUD at phone aspect 888x360..1920x780, Classic 4:3 800..1040 wide, save/load round trip, zoom
  off + HUD + F2 without black band, two-peer MP with different zooms IDENTICAL (1194 and 921 ticks). Desktop fps:
  888x360 46, 1480x600 ~24, 1920x780 16 (before verify-fps1's fast paths).
- Docs: resolution-zoom.md (new), game-state-ui.md "Resolution internals", directx-hle.md, mods.md, touch.md,
  ui-hud.md; tool `tools/res-zoom/snap_sites.py`.

### hud-polish [done]
- RE (frame renderer 0x4b26c0): the minimap is a square RGB565 image mw x mw (mw = `[0x730f28]`) at `[snap+0x4c]`
  (snap = argument of 0x4b26c0 and of the panel renderer 0x4b4d00); column/row = tile * mw / W; drawn sheared
  x = mx + (mw-r-1)/2 + c, y = my + r. Block 0x4b2b34..0x4b2ce5 copies it incrementally in 64 chunks (counters
  0x7ad4f8); block 0x4b366a..0x4b3cf3 copies the part over the map, draws the frame art (0x40001/0x40003) and the white
  camera rectangle (gfx+0x5f694/0x5f6a4). Camera rect (0x4b3a3c): k = (mw<<8)/[snap+0x2c]; x = ([s+0x24]+11)k>>8;
  y = ([s+0x28]+16)k>>8; w = [s+0x30]k>>8; h = ([s+0x34]-40)k>>8.
- HUD full-screen layout jumps over both blocks (render thread only, no lockstep impact) -> VERIFIED: the game's
  minimap, frame and camera rect are gone.
- New `ui_hud_minimap.rs`: bottom-left widget from the guest image; drag pans (gd::CenterOn 0x51b560), tap opens a
  full-screen map, tap there jumps the camera and closes. `Mod::on_key` (additive); Esc/Back closes sheets. Settings
  page (display mode/filter/FPS, zoom placeholder via F1-F3, HUD size, full map, UI mode); HUD laid out inside
  `display::visible_game_rect`; resource strip moved top left. Original layout recorded at the 0x4c6d70 return (custom
  resolutions work); landscape widening cap 1024 -> 4096. Tests s3-mods 55+1.

### verify-fps1 [done]
- Desktop: `cargo test --workspace --release` 257 passed, 1 ignored. HUD: minimap tap -> full map, tap jumps camera,
  settings page, pinch zoom to 1040x780; save/load OK (`scratch/vfps/sl`); two-instance MP with pinches IDENTICAL
  (`scratch/vfps/mp`).
- Phone display: GLES surface 2316x904, `setFrameRate(120)` = 0, mode 904x2316@120; Native wide = 1536x600 at default
  zoom. With perf-cpu only: HUD 36 fps, Classic 38-40 fps at 250 MIPS (CPU bound).
- New fast paths (s3-mods, each with a differential test vs the real exe): `fast_snap` (`snap_tiles`,
  `0x522219..0x52303e`, 133 insns/tile), `fast_obj` (`obj_empty`, whole `0x4d3050` call for empty tiles, 110-116
  insns, setter `0x5fc540` hash-checked), `fast_land` (`land_clean`, `0x54ce53` loop for clean triangles). s3det all
  fast vs `S3_FAST=0` IDENTICAL (`insns=11817005518`), 55.9 s -> 41.4 s; desktop 2316x904 uncapped 35 -> 71 fps.
- Phone after: Classic default 60/62/60 fps (cap), 1232x480 60, 1848x720 42-46; HUD default 48-58, 1232x480 60,
  1848x720 46-60. simpleperf: engine thread ~2 GHz avg, 92 % of cycles in s3-engine, 6.6 % presenter; pinning to cpu7
  ~5 % only.
- Fix: FPS counter in the HUD draws at the top centre (top right covered the full-map "Close"). Phone: full map
  opens, Close visible, Back closes, minimap drag pans.
- Remaining hotspot in HUD mode: the `0x4b26c0` tile loop, 46 % of instructions.

### wrap-up [done]
- Docs consistency: ui-hud.md (FPS counter position, Settings zoom +/- still the F1-F3 placeholder); AGENTS.md map
  already lists display, performance, resolution-zoom. Open: HUD Settings Zoom -/+ should call `zoom::zoom_in/out`;
  HUD default zoom below 60 fps on the phone; battery/thermal over a long session not measured.

## 2026-11-06 — mobile UX + online (workflow)

Result: the game is now touch-native and online-capable without modifying S3.EXE: in-memory hooks keyed by the
GOG sha256 `00a925cf…`, context-aware taps (no long-press right click), two switchable UI modes (A "Classic Touch",
B "Mobile HUD"; comparison and recommendation in [ui-modes](porting/ui-modes.md)) and DirectPlay 6 over our own
WebSocket relay (desktop <-> desktop lockstep without desync; Android joins a desktop host). Vision and workstream
status: [mobile-vision](porting/mobile-vision.md). Summary: [status](porting/status.md).

### wrap-up [done]
- New [ui-modes.md](porting/ui-modes.md) (A vs B side by side, screenshot paths under `scratch/`, pros/cons,
  recommendation: A default, B as in-game switch, decide after a real-phone test); AGENTS.md map row; status.md
  (title, phone controls for the touch mods, relay URL, licensing re-check); mobile-vision.md workstream status table.
- `cargo test --workspace --release -j 4`: 232 passed, 1 ignored, 0 failed. Headless run reaches in game in mode A
  (`scratch/wrapup-ux/ingame50.png`). Legal: no game bytes tracked or staged; 160 deps permissive (`r-efi` has an
  MIT option); overlays use the bundled Liberation Sans (OFL); HUD icons are sampled from the running game into
  memory only, never stored.

### hooks (W1 mod/hook framework) [done]
- s3-cpu: `CpuExit::Hook` when EIP reaches a hooked address; blocks split before hooked addresses (one flag test per
  executed block); `skip_hook_once` resumes the original code.
- s3-runtime `ext.rs`: `MachineExt` (hook dispatch, input filter, overlay compose; frame order game -> overlay ->
  cursor); `HostEvent::Pointer` for raw touch (ignored by the game).
- New crate s3-mods: skip/jump/on-return hooks; guest calls (stdcall, cdecl, thiscall, fastcall, virtual); `defer`
  to main-thread safe points; `guest_struct!` views; patch sets keyed by exe sha256 (own SHA-256), GOG set
  `gog-base` (safe points 0x60d9e0, 0x60d740); fontdue overlay with Liberation Sans; gesture recognizer (tap, double
  tap, long press, pan, pinch, two/three-finger tap); touch-to-mouse fallback (same as the old Android mapping);
  settings JSON with `ui_mode`; debug overlay `S3_MODS_DEBUG=1`. s3-engine installs it after loading the exe; s3run
  `--inject` gained `tap`, `tap2`, `drag`, `longpress`, `pinch`.
- Verified: workspace tests green; arm64 `cargo check` of s3-android; headless menu -> game by taps only, drag
  scrolls, tap opens the build menu. Overhead: micro-benchmark unchanged; in game hooks on/off in parallel 97.2 vs
  97.0 and 95.0 vs 94.9 M instr/s (< 1 %; sequential runs vary up to 10 % with host load).
- Finding: main-thread safe points are rare in game (~40-120 per 40 s; the main thread sleeps in GetMessageA), so
  deferred work can wait seconds; game work is therefore done in the logic-thread HandleInput hook instead.

### re-units (W2 part 1) [done]
- Logic thread object at app+0x90, game data gd = [logic+0x3a0]. Settlers gd+0x12a5e60 (0x40 B), buildings
  gd+0x1499e64 (0x3a B), tiles gd+0x67444 + (x*768+y)*0x18. UI selection 0x7dfd68/0x7ad620/0x7ad634/0x7ad5f8,
  selected building 0x7dfd6c, on-screen hit list 0x7ade08. gd::ScreenToMap 0x4ff170 writes gd+0x2d2e4/e8.
- Input: window messages -> 20-byte input events -> CLogicThread::HandleInput 0x524fa0. Left-up = box/single
  select, right-up = move. A right click held >= 15 game steps is ignored (0x53437d): why long-press right click
  cannot work.
- Commands {0x21, cmd, len, ...} posted to logic+0x3c4, executor switch 0x535a9d: 1 place, 3 sync selection,
  9/d/e/f move, 6 destroy, 4 building flag, 7/8/a/b/10/11 economy. Verified box select + move, placement, tower
  panel. s3run inject `xdump`/`xpeek`/`scan` (debug_units.rs). Doc [game-state-units](reverse-engineering/game-state-units.md).

### re-ui (W2 part 2) [done]
- Camera gd+0x2d25c/0x2d260 (world px), clamp 0x51b4d0, center-on-tile 0x51b560. Layout setter 0x4c6d70 sets
  0x730f08..0x730f40 for the 3 resolutions; panel grid cell 3/4/5 px, origin (8, 152/171/228). F1-F3 switch at
  runtime via 0x49ded0 -> [0x7a7af0+0x70]; registry Resolution 1/2 (all VERIFIED).
- Panel page gd+0x2d284 (1 build, 2 goods, 3 settlers, 4 building, 5 units, 7 menu, 8 messages, 9 paused), sub-tabs
  +0x288/+0x290/+0x2a0/+0x298, slot +0x28c, type +0x2d2f0, dirty +0x27c; selecting a building by poke places a
  site. Build list table via `tools/re-ui/panelmap.py`; names from the string table 0x5faaa0 (text 0x80000+type,
  `tools/re-ui/strings.py`). Hotkeys Q/W/E/R categories, P pause. Tooltip path HandleInput -> logic+0x54 ->
  snap+0x270 -> draw 0x4b3de1; panel renderer 0x4b4d00. Stock = sum of piles (one map). `dumpall mapped`.
  Corrected part-1 rows for 0x2d284/88/8c. Doc [game-state-ui](reverse-engineering/game-state-ui.md).

### determinism (W5 prerequisite) [done]
- Audit: x87 fsin/fcos/fptan/fsincos/fpatan/f2xm1/fyl2x/fyl2xp1 used host libm, fprem host `%`, NaNs from the host
  (x86 and ARM differ). Fix: `s3-cpu/src/fpu_math.rs` with pinned pure-Rust `libm =0.2.16`; exact f2xm1 (x*ln2,
  <= 1 ulp); fprem/fprem1 by exact integer division with exact quotient bits; x87 NaN rules;
  `host_float_env_check()`. `fpu_conv.rs`: bit-exact f32<->f64 NaN conversion. No FMA (0 fmadd/fmsub in the
  aarch64 build). Golden vectors (`fpu_golden.rs`) identical on x86_64 debug/release and i686; arm64 built, not run.
- REA: no rdtsc; CRT rand 0x61f802 (per-thread state `_getptd()+0x14`), srand 0x61f7f5 with time(NULL) in 0x431780
  and CDrawThread init 0x4932e0; 16 rand callers; no common LCG constants in code.
- Tool `s3det`: virtual clock from instruction count (150k/ms); fixed QPC, system/local time (2001), UTC time zone,
  CoCreateGuid/UuidCreate seeds; memory/CPU hashes per second, `--dump-at`; `tools/memdiff.py`,
  `tools/determinism-test.sh`. First divergence at 6 s = GUID at 0x7a7ac8 from CoCreateGuid; after fixing, two
  parallel 75 s runs IDENTICAL (`insns=7699350378 mem=a68d8effad5b92f2`), reproduced after a rebuild.
  Doc [determinism](porting/determinism.md).

### mp-design (W5) [done]
- Static RE of DirectPlay 6 use: CDirectPlay wrapper 0x5bb000-0x5c2100 (IDirectPlay4A at this+4, Lobby2A at
  this+8; vtable offsets confirmed by call sites). Lobby probe compares DPERR_NOTLOBBIED 0x8877042E at 0x5bb730;
  lobby-launch path exists. HRESULT name table 0x7a03b4. Open desc at 0x46ff80 (flags MIGRATEHOST|KEEPALIVE, app
  GUID {0E9F8400-D9A5-11CF-8F4D-00804880B189}, dwUser3 0xA0). EnumSessions async every 250 ms up to 20 s;
  CreatePlayer passes an event handle (HLE must SetEvent); setup sends guaranteed, lockstep bundles sent twice by the
  host; Receive with DPRECEIVE_ALL; DPSYS switch 0x5bcde1; setup message switch 0x470e25; desync check 0x53b59a;
  `-netmon` writes trace.lis. The Blue Byte lobby uses wsock32, not DirectPlay.
- New crates s3-net (wire/protocol/link/client/dplay; resumable link with per-direction seq/ack) and s3-relay
  (tokio + tokio-tungstenite hub, CLI). Doc [multiplayer](porting/multiplayer.md).

### touch-context (W3) [done]
- `touch-context` mod (GOG only): hook on HandleInput 0x524fa0; gd via [0x7e3408] (0 in menus). Hit list entry =
  u16 kind, i16 x, i16 y, u16 id at 0x7ade08 + (buf+2i)*10 (VERIFIED).
- Tap = select (enlarged hit box) / quick right click = move/attack / place; ✕ and two-finger tap cancel placement
  (slot 0 + dirty) or deselect (count 0 + dirty + page 0); camera writes gd+0x2d25c/60 with the host clamp
  (VERIFIED), inertia over an 80 ms window (fixed a 3x overestimate for two-finger flings); long press then drag =
  box select (VERIFIED 10-11 soldiers); pinch = F1/F2/F3 step (VERIFIED 640<->800; a second switch ~3.5 s later
  was ignored, HYPOTHESIS display re-init cool-down). Overlay: ripple, world-anchored flag, selection rings, press
  ring, box, ✕. touch installs before the UI modes so they get touches first.
- Android: TouchMapper replaced by raw HostEvent::Pointer; RMB/keyboard via Shared atomics. s3run inject `dtap`,
  `tap3`, `drag2`, `fling`, `lpdrag`; `S3_TOUCH_TRACE=1`. Doc [touch](porting/touch.md).

### ui-classic (UI mode A) [done]
- `ui_classic.rs` + `ui_classic_panel.rs`: magnifier bubble with the game's own button names, snap to button
  centres, floating Menu (page 7 via the HandleInput hook + click sound 0x53f560(0x3d)), context chip. Strings read
  directly from [gfx+0x5f6b8] (gfx from ctor hook 0x42c820). Page-1 grid model matches the game (Woodcutter picked
  and placed). In-game test: HandleInput within 1.5 s + gd vtable 0x658c18 + layout == frame size.
- Not verified: tooltip id at snap+0x270 stayed empty (HYPOTHESIS: cleared before read / needs idle ticks).
  Doc [ui-classic](porting/ui-classic.md), screenshots `scratch/ui-modes/A-*.png`.

### ui-hud (UI mode B, W4) [done]
- `ui_hud.rs`, `ui_hud_game.rs`, `ui_hud_data.rs`; active when `ui_mode == MobileHud`, switchable at runtime.
- Full-screen map: [0x730f24] = 0, [0x730f18] = screen width at the return of 0x4c6d70; the landscape ring buffer
  ([gfx+0x5f6cc], created by 0x54bec0, blitter 0x54c9d0 wraps at its width) must be widened, otherwise a zigzag
  column appears; ScreenToMap 0x4ff170 needs sx += original panel width; panel 0x4b4d00 skipped (VERIFIED).
- Placement by poke needs gd::0x4ff3f0 (marker generation); panel redraw only with [gd+0x2d27c]; destroy = command
  0x06 via Queue::Post 0x4309c0; minimap via gd::CenterOn 0x51b560 from the logic hook (host camera writes left
  stale columns) — all VERIFIED. Panel Inventory numbers are separate counters, not the pile sum (VERIFIED). Player
  goods = piles on own tiles (tile +8); without the filter 2x vs AI. Good ids 4 Bread, 5 Coal, 7 Iron ore, 8 Fish,
  12-16 tools, 24 Meat; 1/2, 17/32, 3 HYPOTHESIS. Icons copied from the presented frame while the original panel
  shows each page (memory only).
- Open: F1-F3 at runtime in HUD mode leaves a black band (HYPOTHESIS: second buffer sized at game start); starting
  at 800x600 works. Doc [ui-hud](porting/ui-hud.md), screenshots `scratch/ui-modes/B-*.png`.

### mp-dplay (W5) [done]
- `s3-dx/src/dplay_net.rs`: DirectPlay 6 HLE over the relay (one connection "Internet (Settlers 3 Port)",
  `S3_RELAY_URL` / `S3_NET_NAME`, sessions sync/async, Open CREATE/JOIN, players, Send/Receive with system messages
  and buffer retry, service `dplay-pump` signals player events). Test `dplay_two_machines_through_relay`; builds for
  aarch64-linux-android.
- Menu: "Multi-player Game: LAN" (165,275) is the DirectPlay path; "Internet" needs the Blue Byte lobby. Full flow
  with 2 headless s3run + local relay: create, search, join, ready, same map, lockstep. dwUser1/2 =
  3c44fe57/3d439f21 = version id ("Siedler3 Version 1.60").
- Lockstep RE: bundle = CRC32 (0x46dbd0) + sub-messages; receive loop 0x46adaa; 0x11 ExecuteTo (0x46d370), 0x20
  ack (0x46d610); gd+0x2d2d8 frame counter, simulation tick at 0x53e5a1 runs when word [0x7ace62] == 0; gd+0x2d2dc
  = simulation RNG state. Game log function 0x428720.
- Tools: `s3run --mp-hash FILE` / `--mp-hash-every N` (per simulation tick, not frame), `S3_MP_DUMP_STEP`,
  `S3_MP_HDR`, `S3_GAME_LOG`, `S3_MP_NETDBG`, `S3_EXE_ARGS`; `tools/mp_hash_compare.py`, `tools/mp_dump_diff.py`,
  `port/scripts/mp-local-test.sh`.
- Results: long1 (3979 ticks), long2 (5489), long3 (5052), a1 (391): no simulation desync (settlers, buildings,
  piles, trade, economy, players identical; commands at the same ticks). Tile bytes +9/+b/+c/+d (HYPOTHESIS fog /
  markers) and +0x0f on 3 tiles (HYPOTHESIS local marker) differ and are masked. "Stuck at Initiating
  player2player communication" = slow random start-position search, not a network bug.

### verify-1 (ux-verify1) [done]
- Tests 232 passed / 0 failed / 1 ignored. Desktop A: long-press box select, tap move, two-finger deselect/pan,
  panel build + tap place, save/quit/load. Desktop B: fixed `touch_game::layout()` to accept panel width 0 (+ test);
  then move, deselect, pan, placement (boards 25 -> 24), HUD save/quit/load. Added A -> B switch button ("UI: Mobile
  HUD" while the Menu is open).
- BUG fixed: in multiplayer [0x7e3408] stays 0 on both peers, so taps were plain left clicks; `touch_game::gd()` and
  `ui_hud_game::gd()` fall back to [logic+0x3a0] from the HandleInput hook. Desktop MP after the fix: 3104 ticks
  IDENTICAL, units move on both peers.
- Android: added INTERNET permission and `<data>/relay-url.txt` (-> `S3_RELAY_URL`). Emulator joined a desktop host
  via `ws://10.0.2.2:7788`; tap select/move in A and B on both sides. Single player both modes by taps; mode switch
  both ways. touch.rs no longer draws its ✕ in HUD mode (hit area still active). Cosmetic: stale minimap pixels for
  a few seconds after HUD -> Classic. Not checkable via adb: long press + drag, multi-touch, pinch (covered by
  desktop inject and unit tests). Progress note [ux-verify1](progress/ux-verify1.md).

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
