# Android app (s3-android)

Status (2026-11-05, android-integration-1): **the original S3.EXE is playable on the emulator** (AVD `s3_api35`,
x86_64, release APK with arm64-v8a + x86_64). Same point as desktop s3run:

| Step | Result on the emulator |
| --- | --- |
| Launch -> main menu 800x600x16 | < 1 s, all buttons, logo, "Version 1.60" (intro AVI is skipped, no error dialog any more) |
| Tap "Single Player: Scenario" | Game Setup with the map-selection popup (Random / World size) on top |
| Tap the popup's OK | Game Setup |
| Tap Game Setup OK | players screen |
| Tap players OK (same position as Game Setup OK) | loading (3 threads, 640x480x16), in game after about 20-25 s |
| In game | castle, settlers, AI thread messages, music `MCI cdaudio play tracks 7..8` |
| Tap build menu, category, building, then a ghost site on the map | construction site placed (stakes + flag) |
| One-finger drag on the map | map scrolls with the finger (see Controls) |
| Home, then reopen | pause/resume, game continues where it was |

Screenshots (local only, not in git): `scratch/android-shots/attempt1-1.png` (menu), `-2` (Game Setup), `-5` (in game),
`-6`/`-7` (build menu, placement markers), `-9` (construction site), `-16` (after resume).

Performance (x86_64 emulator, 4 vCPU, release): menu idle; loading ~125 M guest instructions/s; in game about
**30 presents/s at ~95 M instructions/s**, engine thread ~77-92 % of one core, `android_main` ~11 %. RSS ~200 MB.
Desktop showed ~48 presents/s at a similar instruction rate on another random map, so frames cost more here
(HYPOTHESIS: busier map; not profiled). The game stays responsive. A real arm64 phone was not tested yet.

**End-to-end verification 1 (2026-11-05)**: release APK with both ABIs on emulator-5610, the whole flow by
`adb shell input`: menu -> Single Player -> popup OK -> soft keyboard (`ABC` button) typing into the game name ->
Game Setup OK -> players OK -> in game (~25 s) -> build menu, Woodcutter's Hut, tap a ghost site -> construction
site -> computer icon -> Save Game -> slot 1 ("Game Saved") -> Back (harmless) -> Home + reopen (game continues) ->
drag scroll -> Quit Game -> YES -> Game Statistics (tap) -> main menu -> Load Game -> entry -> saved game loads and
continues. The cursor shows on every screen. In game ~102 M instr/s, ~30 presents/s, process ~96 % CPU, RSS 225 MB.
Screenshots `scratch/android-shots/verify1-*.png` (list in [status.md](status.md)).

Tap coordinates used for testing (emulator screen 2400x1080, landscape): menu 800x600 is drawn at scale 1.8 with x
offset 267 buffer px, so `screen = ((267 + gx) * 1.8, gy * 1.8)`: Single Player: Scenario `778 423`, map popup OK
`1615 841`, Game Setup OK and players OK (same spot) `1795 1021`. In game 640x480: scale 2.25, offset 214: build menu `538 376`.
Both mappings put the 4:3 frame at the same screen x (480), so a screenshot does not tell which mode is active:
**Load Game is 800x600** (entry `300 126` in 800x600 = screen `1020 227`; using the 640x480 formula misses).
More in-game taps (640x480 game coords): first category `25 200`, first building `37 237`, computer icon `21 452`,
Save Game `68 225`, slot 1 `40 244`, Quit Game `68 420`, YES `38 420`; Game Statistics: tap anywhere.

Code: `port/crates/s3-android` (crate `s3-android`, library `libs3android.so`), `port/android/` (manifest, icon),
`port/scripts/build-apk.sh`, `port/scripts/run-emulator.sh`.

## App facts

| Item | Value |
| --- | --- |
| Package / label | `com.settlers3port.app` / "Settlers 3 Port" |
| Activity | `android.app.NativeActivity` (`hasCode=false`, no Java/Kotlin, no Gradle), `lib_name` = `s3android` |
| SDK | minSdk 26 (AAudio), targetSdk 35, landscape (`sensorLandscape`), fullscreen theme, keep-screen-on flag |
| ABIs | `arm64-v8a` (phones) and `x86_64` (emulator). The `.so` files are stripped and stored uncompressed (`extractNativeLibs=false`), aligned for 16 KiB pages |
| Game dir | `/sdcard/Android/data/com.settlers3port.app/files/game` (= `getExternalFilesDir()/game`). The user copies their own install there. **The APK contains no game files** |
| Data dir | `/sdcard/Android/data/com.settlers3port.app/files/data` (registry JSON, overlay of written files, saves). `jit.txt` there containing `1` switches the optional arm64 JIT on at the next engine start (perf-jit, [cpu.md](cpu.md#jit-optional-aarch64-perf-jit-2026-11-07); logged as `jit: true`) |
| Log tag | `s3port` (`adb logcat -s s3port`) |
| Launcher icon | **Default: our own adaptive icon** (`res/drawable/ic_launcher.xml` = background `ic_launcher_bg` navy radial gradient, foreground `ic_launcher_fg` roman numeral "III" in white serif letters with a red outline, `ic_launcher_mono` for themed icons). Style inspired by the Settlers United launcher icon ("III" over "IV"), drawn from scratch as vector paths, no game or SU pixels, no "Settlers" wordmark. Safe for published APKs. **`--game-icon` (personal builds only)**: `build-apk.sh` extracts the 32x32 4 bpp `RT_ICON` (id 11, Settlers III crest) from the user's local `game-files/gog-s3-ultimate/S3.EXE` (override `S3_ICON_EXE`) with `tools/s3_icon.py` into `port/target/apk/icon-res/drawable-*dpi` and links it as an aapt2 overlay. That APK contains Ubisoft artwork: never publish it |
| Dev switches | Files in the data dir read at engine start: `jit.txt` (`1` = arm64 JIT on), `fps-cap.txt` (frame cap, `0` = unlimited; default 60) |
| Signing | debug keystore `port/target/debug.keystore` (generated by build-apk.sh, password `android`, gitignored) |

Uninstalling the app deletes `files/` including the pushed game copy (normal Android behaviour for app-specific dirs).
On the rooted test phone the copy can be kept across a reinstall (e.g. new signing key): `su` `mv` `files/game` and
`files/data` to `/data/media/0/s3port-backup`, uninstall + install + launch once, `mv` them back, then
`chown -R <new uid>:ext_data_rw` and `chcon -R <context of the new files dir>` (`ls -laZ`; the MLS categories change
with the uid). Done 2026-11-07.

## Build

```sh
cd port
scripts/build-apk.sh                 # release, arm64-v8a + x86_64 -> target/apk/settlers3port.apk
scripts/build-apk.sh --abi x86_64    # emulator only (faster)
scripts/build-apk.sh --shell         # without the `engine` cargo feature: status screen/input/audio only
```

`--shell` exists because the other crates are edited in parallel and are sometimes briefly broken; the shell
build (`cargo build -p s3-android --no-default-features`) depends only on `s3-runtime`. Sizes: release
`libs3android.so` about 4.6 MB per ABI after `--strip-debug`; the APK with both ABIs is about 9.3 MB.

Steps in the script: `cargo ndk -t arm64-v8a -t x86_64 --platform 26 build --release -p s3-android`
(`CARGO_TARGET_DIR=port/target`, `-j 4`), `aapt2 compile/link` with `platforms/android-35/android.jar`, add
`lib/<abi>/libs3android.so` (stripped with the NDK's `llvm-strip --strip-debug`, `zip -0`), `zipalign -P 16 4`,
`apksigner sign` + `verify`. Runs on Linux and on Windows Git Bash (2026-11-07: SDK `%LOCALAPPDATA%\Android\Sdk`, NDK prebuilt `windows-x86_64`, `.exe`/`.bat` tool suffixes, `python` instead of `python3`, libs added with Python `zipfile` stored since there is no `zip`). Tools: build-tools 36.0.0, NDK 27.2.12479018, cargo-ndk 4.1 in `~/.cargo/bin`.
The script refuses to build with less than 2 GB free disk.

Host unit tests (letterbox math, MotionEvent -> pointer events, audio ring/resampler, key map + chars, status and first-run screens, Host impl):
`CARGO_TARGET_DIR=port/target cargo test -p s3-android -j 4` (on Windows add `--lib`: the cdylib cannot link the GLES/EGL symbols for the host).

## Emulator (our AVD only)

`scripts/run-emulator.sh` manages **only** AVD `s3_api35` on console port 5610 (`adb -s emulator-5610`).
Never use other AVDs or `emulator-5600` (other projects).

```sh
scripts/run-emulator.sh start            # create AVD if missing, boot headless (-no-window, swiftshader, -no-audio)
scripts/run-emulator.sh install launch   # install target/apk/settlers3port.apk, (re)start the app
scripts/run-emulator.sh push             # copy game files (see below)
scripts/run-emulator.sh logcat           # dump the app log (logcat 30 = follow for 30 s)
scripts/run-emulator.sh shot menu        # -> scratch/android-shots/menu.png
scripts/run-emulator.sh tap 1200 540     # tap in screen pixels (the emulator screen is 2400x1080 in landscape)
scripts/run-emulator.sh stop             # force-stop the app, emulator keeps running
scripts/run-emulator.sh kill-emulator    # shut down s3_api35
```

AVD: `system-images;android-35;google_apis;x86_64`, device `pixel_6` (1080x2400), `hw.ramSize=3072`,
`disk.dataPartition.size=6G`, 4 cores. `S3_EMU_AUDIO=1 scripts/run-emulator.sh start` boots with host audio
(off by default so a night run makes no noise). Cold boot takes about 45 s. The emulator log is written to
`scratch/emulator-s3_api35.log`.

## Game files on the device

`run-emulator.sh push` copies from `GAME_DIR` (default `game-files/gog-s3-ultimate`) only what the game reads
(per [wine-trace.md](../reverse-engineering/wine-trace.md)):

- `S3.EXE`, `regs.cmd` (the GOG registry defaults; the registry HLE imports it from the game dir on first
  start, logcat: `registry: applied 48 values from .../regs.cmd`), the small top-level `*.DAT` marker files (`DATA.DAT`, `S3CD1.DAT`, `S3GOLD1.DAT`, ...);
- `GFX/*.f8007e01f.dat` only: the RGB565 set (118 MB). The port always reports a 565 primary surface, so the
  555 set (`*.7c003e01f.dat`) is never opened;
- `SND`, `MUSIC` (the Ogg CD tracks), `S3` (intro AVI, missions), `Map/SINGLE`, `Map/TUTORIAL`;
- without `--minimal`: also `S3QOTA`, `S3MCD`, `Map/MULTI`, `Map/User` (about 1.2 GB in total; `--minimal` is
  about 0.6 GB).

Not copied: Windows DLLs (our HLE replaces them), `Save/` (the user's saves), manuals, editor, launcher files.
The script checks free space on the device first. On a real phone use the same target directory, either
`adb push` or a file manager with access to `Android/data` (newer Android versions restrict this; adb always
works).

**First-run screen** (android-ux, verified on the emulator): while any of `S3.EXE`, `GFX`, `SND`, `Map`
(`frontend::REQUIRED_PARTS`, case-insensitive) is missing, the app shows a plain "Game files needed" screen
(`screen::first_run_screen`) with the folder as the user sees it (`Android/data/com.settlers3port.app/files/game/`,
highlighted), the full path, what to copy, which parts are still missing, the `adb push` command and the hint that
Android 11+ file managers cannot open `Android/data`. It checks again every 2 s; once everything is there it waits
3 s more (a push may still be running) and starts by itself (verified: moved the game dir away and back while the
app ran). Screenshot: `scratch/android-shots/ux-firstrun-s.png`.

## Runtime design

**2026-11-05 (display agent): presentation moved to a third thread**, the EGL/GLES render thread
(`present.rs`, `gl.rs`): GPU scaling with display modes Fit / Fill / Stretch / Native wide, sharp-bilinear filter,
high refresh rate request, cutout area used, engine never waits for the display. The activity thread now only
handles lifecycle and input (touch mapping through `s3_mods::display::Viewport`). Details:
[display.md](display.md). The description below is the older two-thread design; the frame/status parts now go to
the render thread.

Two threads (`crates/s3-android/src`):

- **Activity thread** (`android.rs`, `android_main`): polls android-activity events, owns the `ANativeWindow`
  and the AAudio stream, turns input into `HostEvent`s, draws.
- **Engine thread** (`frontend.rs`, `run_engine`, 16 MiB stack): waits for `S3.EXE`, builds `s3_engine::Engine`
  with `AndroidHost`, calls `run_for_ms(16)` in a loop. It sleeps while paused. A panic or a machine stop is
  caught; the stop reason and `Engine::summary()` are shown on screen and written to logcat.

They share `frontend::Shared`: the newest frame (`present` copies into a reused buffer and wakes the activity
loop through `AndroidAppWaker`), the input event queue, the status (phase/title/body), pause and quit flags, and
the audio ring.

**Presentation** (`layout.rs`): the window buffer geometry is set to the game resolution widened (or heightened)
to the window's aspect ratio, format `R5G6B5`. The frame is copied unscaled into the middle with black bars and
the compositor scales the buffer to the screen. Example: 800x600 on a 2400x1080 screen gives a 1334x600 buffer
with the frame at x offset 267. The geometry is recomputed on window init/resize and on game mode changes
(640x480 in game, 800x600 in menus). To learn the real window size, the geometry is first reset to 0x0.

**Status screen** (`screen.rs`): colour bars, grid and corner markers (red top-left, green top-right, blue
bottom-left, white bottom-right) plus text in the 8x8 `font8x8` font (MIT). It is shown before the first
frame: "Starting", "Game files missing" with the copy instructions, the engine stop reason. If the engine stops
after it has drawn frames, the reason is drawn as a panel over the last frame. The app never shows a black screen.

**Lifecycle**: `Pause` -> `HostEvent::Pause`, the engine thread stops running slices, AAudio is paused,
rendering stops. `Resume` -> `HostEvent::Resume`, AAudio restarts. The guest clock (`Host::now_ms`) does not
advance while paused, so game timers do not jump. `TerminateWindow` drops the window (the engine keeps its
frames). `Destroy` sets quit and joins the engine thread.

**Audio** (`audio.rs`): `Host::queue_audio(samples, rate, channels)` is converted to 44100 Hz stereo i16 (linear
interpolation, mono duplicated) and pushed into a lock-free SPSC ring (1 s). The AAudio data callback
(shared mode, `PCM_I16`, 2 channels, 44100 Hz) pops from it and plays silence on underrun. Overflow drops new
samples. `audio_queued_frames` reports the ring fill in the caller's rate for mixer pacing. An AAudio error
callback (device change) reopens the stream. If AAudio cannot open, the app runs silent.

## Foldables, resizing (2026-11-08)

The activity is `android:resizeableActivity="true"`. With `false`, unfolding a Galaxy Z Fold4 while the game ran
kept the app in **size compatibility mode**: still at the cover screen's size, scaled and letterboxed on the inner
screen (`dumpsys activity activities`: `mSizeCompatScale=0.93955094 mSizeCompatBounds=Rect(0, 482 - 2176, 1331)`).
Now the window follows the display: `MainEvent::WindowResized` / `ConfigChanged` (handled without a restart,
`configChanges` lists screenSize etc.) -> `Presenter::set_window` -> new EGL surface size -> viewport, window
layer and zoom aspect follow (`zoom::aspect` from the published window size). Verified on the Fold4: inner screen
2176x1812 full screen in game (120 fps); `wm size 1300x2176` while in game -> the frame re-laid out for 2176x1300
at once, `wm size reset` -> back. Split screen / pop-up windows are allowed now too. Not tested: an actual
fold/unfold while in game (needs the person; same code path), and whether Samsung's "continue apps on cover
screen" setting keeps the app running when folding.
The full-screen menu layouts were made for wide screens; on the near-square inner screen they are placed small
in the middle (works, could use layouts for ~1.2:1).

## Controls

Since 2026-11-05 the activity thread only reports fingers: every MotionEvent becomes raw `HostEvent::Pointer`
events (`touch.rs`: Down/PointerDown -> Down, Move -> Move for every pointer with a changed position,
PointerUp/Up -> Up, Cancel -> Cancel for all; game pixels via the letterbox). Gesture recognition and what a gesture
does run in `s3-mods` on the engine thread, identical to the desktop `s3run --inject` touch commands:
**see [touch.md](touch.md)** (in game: tap = select / move / attack / place, ✕ or two-finger tap = cancel, drag = camera
with inertia, long-press-drag = box select, pinch = resolution zoom; no long-press right click). Menus: tap = left
click, drag = left drag, two-finger tap = right click, three-finger tap = soft keyboard. Not yet tried on the
emulator/phone (multi-touch needs a real device or emulator console gestures): unit-tested + desktop-verified.

Former mapping (`TouchMapper`, removed): tap = left click, long press = right click, map drag = mirrored right drag.

The 60 ms gap between button down and up is there because the game also polls the button state
(DirectInput buffer, GetAsyncKeyState). A real mouse (ChromeOS, DeX, emulator with mouse) is passed through
directly: move, left/right/middle buttons.

**Letterbox buttons: removed 2026-11-07** (user request). There used to be ESC / ABC / RMB buttons in the black bars
(`overlay.rs`, android-ux). Their functions are covered without them: Back = Esc, three-finger tap = soft keyboard
(`Shared::keyboard_req`), two-finger tap = cancel; menus via the UI modes (Classic: floating Menu, HUD: menu sheet).
`s3_mods::set_right_next` ("next tap = right click") still exists in s3-mods but nothing on Android arms it now.

Keys (`keys.rs`): Back -> `VK_ESCAPE` (the IME takes Back first while it is open), hardware
keyboard letters, digits, arrows, Enter, Space, Tab, Backspace, Delete, Shift/Ctrl/Alt, F1-F12, PgUp/PgDn,
Home/End/Insert, comma, period. Volume keys stay with the system.

Text: for every key-down the character is taken from Android's `KeyCharacterMap` (key code + meta state, via
android-activity `device_key_character_map`) and sent as `HostEvent::Char` after the `Key` event
(`keys::char_for_key`): printable characters, plus Enter 0x0D, Backspace 0x08, Esc 0x1B, Tab 0x09 like Windows'
TranslateMessage. Once the host sends any Char, USER32 stops synthesizing WM_CHAR itself (`host_chars`), so all
chars come from the host. Keys with a character but no VK mapping get a plausible VK (`keys::vk_for_char`) or
only the Char.

**Soft keyboard**: `ABC` button or three-finger tap. On the UI thread: `InputMethodManager.showSoftInput(
activity.getCurrentFocus(), 0)` (the NativeActivity content view, like `ANativeActivity_showSoftInput`) /
`hideSoftInputFromWindow`; visibility from `getRootWindowInsets().isVisible(WindowInsets.Type.ime())`. The IME
has no real InputConnection (NativeContentView), so Android's fallback connection turns typed characters into
plain key events, which arrive through the normal input queue. Verified on the emulator (Gboard): Game Setup
"Name of the Game" field: 4x soft Backspace deleted "game", soft `h` `i` typed, `adb shell input text "Bob1"`
(Shift meta for the capital) gave "Player's hiBob1" (`scratch/android-shots/ux-ime-s.png`, `ux-typed-s.png`).
The game window is not resized for the IME (it covers the lower half; the field in Game Setup stays visible).
Note: with the emulator's `hw.keyboard=yes` the soft keyboard still showed.

**Back / Esc in the game itself** (same on desktop s3run, so game behaviour, not Android): in the **main menu**
Esc = "Exit Game": the guest calls ExitProcess. The app then closes itself (`Shared::guest_exited` ->
`Activity.finish()`, then `process::exit(0)` after `android_main` returns, so the next launch starts clean) —
like Back at the root of any Android app (verified; relaunch reaches the menu again). **In game Esc does not
open a menu** in the port (the manual lists "Esc: Exit game"; desktop s3run with `key 0x1B` in game shows no
dialog either). The in-game menu (save / end game) is the "computer" icon at the bottom of the left panel
(640x480 game ~(19,453), screen ~(525,1015)); it works since the save-load fix (gdi32 1 bpp DIB sections: the panel
text is drawn as a mask; before that the panel looked empty), see [user32-gdi32.md](user32-gdi32.md). So Back does
not open the in-game menu; Back in game is harmless (the game keeps running); use the computer icon for Save / Quit.

**Immersive fullscreen**: status and navigation bars are hidden (`WindowInsetsController.hide(systemBars())`,
behaviour "show transient bars by swipe"; fallback for API < 30: `setSystemUiVisibility(0x1706)` sticky
immersive). Re-applied on window init and on every focus gain. Android shows its one-time "Viewing full screen"
hint on the first start. `FLAG_KEEP_SCREEN_ON` stays set while the activity is in front.

**UI-thread calls without Java code** (`uithread.rs`): View/IME calls must run on the Java main thread, but
`android_main` runs on its own thread. A Unix socket pair's read end is registered on the Java main looper
(`AndroidApp::java_main_looper().add_fd_with_callback`); `UiThread::run` queues a closure and writes one byte, the
looper then calls us on the UI thread, where the closure gets a JNI env (jni 0.22, the version android-activity
uses) and the activity object. Java exceptions are cleared and logged.

Arrow keys (hardware keyboard, `adb shell input keyevent DPAD_RIGHT`) also scroll the map (verified).

Not done yet: pinch zoom; IME-aware layout (moving the game above the keyboard).

## Multiplayer (relay)

The APK has the `INTERNET` permission. The relay URL is read once at engine start from
`/sdcard/Android/data/com.settlers3port.app/files/data/relay-url.txt` (one line, e.g. `ws://10.0.2.2:7788/` for a
relay on the PC that runs the emulator); default `ws://127.0.0.1:7777/`. Then the game's own "Multi-player Game: LAN"
menu lists "Internet (Settlers 3 Port)". Verified: the emulator joined a desktop-hosted game and played it by taps in
both UI modes (ux-verify1, [integration-log.md](integration-log.md)). No in-app setting yet.

## Install for users (phone)

1. Install the APK (`adb install -r -g settlers3port.apk`, or copy it to the phone and open it; allow unknown sources).
2. Start the app once (it creates its folder and shows "Game files missing").
3. Copy the **contents** of your GOG install folder (the folder that contains `S3.EXE`, `GFX`, `SND`, `MUSIC`, `Map`,
   `S3`, ...) to `Android/data/com.settlers3port.app/files/game/` on the phone's internal storage. With a PC:
   `port/scripts/run-emulator.sh push` logic, or simply `adb push <GOG folder>/. /sdcard/Android/data/com.settlers3port.app/files/game/`.
   Android 11+ file managers often cannot open `Android/data`; adb or a PC USB connection (MTP) usually can.
   Minimum: `S3.EXE`, `regs.cmd`, top-level `*.DAT`, `GFX/*.f8007e01f.dat`, `SND`, `MUSIC`, `S3`, `Map`.
4. Open the app again (or wait: it checks every 2 s). Saves and settings go to `.../files/data/`.

Uninstalling the app deletes both folders.

## Known issues (2026-11-05)

| Issue | Notes |
| --- | --- |
| ~~No mouse cursor is drawn~~ | fixed 2026-11-05: the game's cursor is composed into the frame at the emulated mouse position (follows taps/drags), see [user32-gdi32.md](user32-gdi32.md#cursor-drawing). Verified on the emulator 2026-11-05 (verify1: arrow follows taps, hourglass while loading) |
| Back / ESC do not open an in-game menu | Esc does nothing in game (desktop too, game behaviour or unknown option). Use the panel's "computer" icon (Save / Quit), which works since 2026-11-05 |
| Three-finger tap not tried on a device | adb input has no multi-touch; unit-tested only |
| Intro and mission videos are skipped | MCI AVI faked |
| Long press needs < ~3 % screen movement within 500 ms | a slow drag turns into a right click |
| Not tested on a real arm64 phone | the arm64 library is built and in the APK |
