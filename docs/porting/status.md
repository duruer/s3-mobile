# Port status (after the polish night, verify1, and the mobile UX + online workflow, 2026-11-06)

One-page summary of what works, with evidence. Details: [integration-log.md](integration-log.md) (desktop),
[android.md](android.md) (Android), [architecture.md](architecture.md) (design).

Screenshots named below are **local only** (`scratch/` is gitignored; they show game graphics and must never be
committed).

## What works

The **unmodified GOG `S3.EXE` + the user's data** runs in our own host: iced-x86 interpreter (`s3-cpu`) + PE loader
+ HLE of KERNEL32/USER32/GDI32/ADVAPI32/WINMM/DirectX. No Wine, no game code or data in the repo or the APK.

| Step | Desktop `s3run` (Linux, headless) | Android APK (emulator `s3_api35`, x86_64) |
| --- | --- | --- |
| Start -> main menu 800x600x16 (intro AVI skipped by a fake MCI video) | ~0.2 s, all 13 buttons, logo, "Version 1.60", GDI text in our font | < 1 s, same picture |
| Single Player: Scenario -> map popup OK -> Game Setup OK (no serial prompt) -> players screen | yes (injected clicks) | yes (taps) |
| Players OK -> loading -> in game, random map 384, 640x480x16 | in game at ~16-17 s | in game at ~20-25 s |
| Build menu, categories, building list, placement markers, construction site placed, settlers walk to it | yes | yes (by touch) |
| Map scrolling | arrow keys, right-drag | one-finger drag (mapped to mirrored right-drag), arrow keys |
| Music (MCI cdaudio -> `MUSIC/*.ogg`), effects (DirectSound mixer) | verified via `s3run --audio-wav` (music continuous, effects in game, no gaps/drift/clipping; [directx-hle.md](directx-hle.md)); real playback `--audio-play` behind feature `audio-cpal` | AAudio output (emulator boots with `-no-audio` by default, so not listened to yet) |
| Pause / resume (Home, reopen) | n/a | yes, guest clock stops while paused |
| Mouse cursor (game's own `.rsrc` cursors: arrow, hourglass) | yes | yes, follows taps (verify1) |
| Save (in-game computer menu, slot 1) -> Quit -> Game Statistics -> main menu -> Load Game -> game continues | yes (inject) | yes (taps, verify1) |
| Text input (Game Setup "Name of the Game", serial number prompt without `regs.cmd`) | hardware keys / `text` inject; keyboard requests logged (`text input: ...`) | soft keyboard via letterbox `ABC` button + adb text (verify1); since 2026-11-09 it opens by itself on the game's text fields (serial-keyboard) |
| Back / Esc | Esc in game: no effect (game behaviour) | Back hides the IME; in game harmless; in main menu = Exit Game, app closes |
| Real arm64 phone (Galaxy Z Fold4 cover screen, 2026-11-05 verify-fps1) | n/a | **works**: GLES presenter 2316x904 full width @120 Hz, in game Classic 1536x600 **60 fps** (cap), HUD 48-60 fps, full-screen map, FPS counter (details below) |

**End-to-end verification 1 (2026-11-05)**, whole flow on both frontends in one session each: desktop
`scratch/verify1/d1-menu.png` .. `d8-loaded.png` (+ `run3.log`; the WAV was analysed then deleted); Android (both-ABI release APK,
emulator-5610) `scratch/android-shots/verify1-01-menu.png` (menu, cursor), `-03-ime` (soft keyboard, typing),
`-06-players`, `-08-ingame`, `-09-ghosts` (build list, tooltip, cursor), `-10-placed` (construction site),
`-11-gamemenu`/`-12-savelist`/`-13-saved` ("Game Saved"), `-14-back`, `-15-resumed` (after Home + reopen),
`-16-scrolled` (drag scroll), `-17-quitq`, `-18-stats`, `-19-menu`, `-20-loadlist`, `-22-loadtap2` (loading,
hourglass cursor), `-23-loaded` (saved game continues).

Older evidence (local): desktop `scratch/keep/r1-menu.png`, `r1-setup.png`, `r1-players.png`, `r1-loading.png`,
`r1-ingame.png`, `r1-build-cat.png`, `r1-place-sel.png`, `r1-placed-a.png`; Android `scratch/android-shots/main-menu.png`,
`attempt1-1.png` (menu), `-2` (Game Setup), `-5` (in game), `-6`/`-7` (build menu, markers), `-9` (construction site),
`-16` (after resume).

**Mobile UX verification 1 (ux-verify1, 2026-11-05)**: touch-context (W3) + UI mode A "Classic Touch" + UI mode B
"Mobile HUD" + DirectPlay over our relay, checked together on desktop (`s3run --inject` touch gestures) and on the
Android emulator (adb taps; both-ABI release APK). Details and screenshot list: [integration-log.md](integration-log.md)
("Mobile UX verification 1").

| Check | Desktop | Android (emulator-5610) |
| --- | --- | --- |
| Tap = select own unit, tap ground = move (green flag, quick right click), **no long press anywhere** | mode A + B | mode A + B (`ux1-04..06`, `ux1-13..15`, `ux1-20..22`) |
| Build: panel taps (A, magnifier) / bottom sheet card (B) -> tap a marker = construction site | A + B | A + B (`ux1-08/09`, `ux1-17/18`) |
| Long-press drag box select, two-finger tap deselect, two-finger drag pan | A + B (inject `lpdrag`, `tap2`, `drag2`) | one-finger drag pan only (adb has no multi-touch; `input draganddrop` pans instead of a long press) |
| Menu -> Save -> Quit -> Load continues | A (floating Menu) + B (HUD menu -> panel peek) | earlier verify1 (original panel) |
| UI mode switch in game | A -> B (new "UI: Mobile HUD" button while the Menu is open) and B -> A (HUD menu) | both directions (`ux1-11/12`, `ux1-24/25`); B full screen after the next game start |
| Multiplayer over our relay | 2 instances, 3104 ticks (~3.5 min) with touch commands from both: hashes IDENTICAL | **Android joins a desktop host** through the relay at `ws://10.0.2.2:7788/` (`relay-url.txt`), both in the same game, taps move units on both sides, mode A and B (`ux1-45..57`) |

Which UI mode to use: side-by-side comparison with screenshots and a recommendation in [ui-modes.md](ui-modes.md).

Fixed during ux-verify1: touch-context was inactive in HUD mode (panel width 0) and in every multiplayer game
(`[0x7e3408]` is 0 there; now `[logic+0x3a0]`), the APK lacked the INTERNET permission, Android had no relay
setting, and mode A had no way to switch to mode B.

Tests: `cargo test --workspace --release -j 4` = 232 passed, 1 ignored, 0 failed (ux-verify1; re-run at wrap-up 2026-11-06, plus a headless run to in game in mode A: `scratch/wrapup-ux/ingame50.png`). Earlier: 159 passed, 1 ignored, 0 failed (2026-11-05 verify1; re-run at wrap-up 2026-11-06), including a
differential test of the integer ALU against the host CPU.

## Performance

| Where | Numbers |
| --- | --- |
| Interpreter micro-benchmark | ~280-290 M guest instructions/s (desktop, one core; was ~170-185 before perf-cpu 2026-11-05) |
| Desktop, in game after perf-cpu (2026-11-05) | 80-82 fps uncapped at 640x480 (was ~33), 53-55 fps at 800x600 with the 60 cap; details [performance.md](performance.md) |
| Desktop, loading | 130-165 M instr/s on one guest thread |
| Desktop, in game | ~48 presents/s (WaitForVerticalBlank paced), render thread 70-85 M + AI ~20 M instr/s, ~60 % of one core |
| Emulator x86_64, in game | ~30 presents/s at ~95 M instr/s, engine thread ~77-92 % of one core, RSS ~200 MB |
| APK | 15 MB (arm64-v8a + x86_64, stripped, uncompressed .so) |
| **Phone** (Samsung Galaxy Z Fold4 SM-F936B, Snapdragon 8+ Gen 1, cover screen 2316x904, verify-fps1 2026-11-05) | before this workflow ~26 fps at ~100 M instr/s. After perf-cpu only: Classic 1536x600 38-40 fps (250 MIPS). After the verify-fps1 fast paths (`snap_tiles`, `obj_empty`, `land_clean`): **Classic Touch default zoom (1536x600) 60/62/60 fps at the 60 cap**, zoom 1232x480 60 fps (~320 MIPS-equivalent, headroom), zoom 1848x720 42-46 fps; **Mobile HUD** default 48-58 fps (48 with much animated water on screen), 1232x480 60 fps, 1848x720 46-60 fps. Engine thread ~94-99 % of one core at ~2 GHz average clock. Table: [performance.md](performance.md#phone-measurements) |

The interpreter is single-threaded (cooperative green threads), so it uses one core; the presenter is a second
thread (~10-14 %).

## Install on a phone

Full steps: [android.md](android.md#install-for-users-phone). Short version:

1. Build or take `port/target/apk/settlers3port.apk` (`cd port && scripts/build-apk.sh`); install with
   `adb install -r -g settlers3port.apk` (or copy to the phone, allow unknown sources).
2. Start the app once (status screen "Game files missing").
3. Copy the contents of your own GOG install (folder with `S3.EXE`, `regs.cmd`, `GFX`, `SND`, `MUSIC`, `Map`, `S3`)
   to `Android/data/com.settlers3port.app/files/game/` (adb push or USB/MTP; ~0.6-1.2 GB; only the
   `GFX/*.f8007e01f.dat` set is needed).
4. Reopen the app. In menus a tap is a left click. In game the touch mods are active (UI mode A by default, see
   [ui-modes.md](ui-modes.md)): tap = select / move / place, drag = pan, long press then drag = box select,
   two-finger tap = cancel/deselect, pinch = resolution; Save / Quit via the floating **Menu** button (A) or the HUD
   menu (B). Back = Esc, soft keyboard by itself on game text fields or three-finger tap (the ESC/ABC/RMB letterbox buttons were removed 2026-11-07).
   Multiplayer on the same Wi-Fi: "Multi-player Game: LAN" -> "Local network / Wi-Fi" -> New Game on one device, Search for games + Join Game on the others (embedded relay + UDP discovery, multiplayer.md section 8). Internet: put a relay URL (e.g. `ws://host:7788/`) in `<data dir>/relay-url.txt`.

## Known issues

| Issue | Notes |
| --- | --- |
| Intro/mission videos skipped | MCI AVI faked (Indeo 5 decoding not implemented) |
| Esc / Back does not open an in-game menu | Esc does nothing in game on desktop either; use the panel's computer icon (Save / Quit) |
| Loading time varies on desktop | one verify1 run needed ~77 s from players OK to in game (usually 9-10 s; load average was low). HYPOTHESIS: host contention or map variance; not reproduced in 2 later runs |
| Three-finger tap (keyboard toggle) untested on a device | adb has no multi-touch. HYPOTHESIS: Motorola's three-finger screenshot gesture eats it (user report 2026-11-09). The keyboard now opens by itself on game text fields |
| Soft keyboard can cover a text field | landscape IME over the lower half; the serial field (y 276..300 of 600) may be hidden. Typing + Enter work. Next: game area above the IME |
| Multiplayer: LAN menu entry, no lobby UI | **LAN play out of the box (2026-11-08): phone host <-> Windows PC join over Wi-Fi verified, no desync** (embedded relay + UDP discovery, multiplayer.md section 8). DirectPlay HLE over our relay works (2 desktop peers, lockstep, no simulation desync in runs of up to ~6 min, [multiplayer.md](multiplayer.md)); Android joins a desktop host (ux-verify1). Relay URL: env `S3_RELAY_URL` (desktop) or one line in `<data dir>/relay-url.txt` (Android, read at engine start); no in-app setting yet. "Multi-player Game: Internet" (Blue Byte lobby, wsock32) stays unavailable; no x86_64 <-> arm64 peer tested; no desync hash on Android |
| HUD (mode B) after an in-game switch from mode A | covers the panel ("cover mode") until the next game start, then full screen (by design, [ui-hud.md](ui-hud.md)). After B -> A the minimap showed map pixels for a few seconds, then cleared (Android, cosmetic) |
| Box select needs a long press then drag | not testable with adb (no hold-then-move); verified on desktop. Two-finger gestures and pinch: desktop inject + unit tests only |
| x87 precision | 53-bit, round-to-nearest only; unmasked FPU exceptions not delivered (see [x87-fpu.md](x87-fpu.md)) |
| Desktop real playback only with `--features audio-cpal` (not listened to yet) | WAV sink `--audio-wav` always available |
| Game files must be copied via adb/MTP | Android 11+ file managers often cannot open `Android/data`; no folder picker yet |

## Next steps

Priority order:

1. ~~**Real arm64 phone test**~~: done 2026-11-05 (verify-fps1, numbers above). Open: battery / thermal over a long session; HUD default zoom 48-58 fps (the frame renderer's tile loop `0x4b26c0` got native fast paths 2026-11-07, perf-tile: desktop HUD 2316x904 ~29-35 -> ~41-47 fps; phone not re-measured yet).
2. ~~**Cursor**~~: done 2026-11-05 (S3.EXE `.rsrc` cursors composed at present time, all frontends; verified on desktop and Android). See [user32-gdi32.md](user32-gdi32.md#cursor-drawing).
3. ~~**Save/load** on Android, on-screen keyboard~~: both verified on the emulator 2026-11-05 (verify1).
4. **Audio polish**: listen on a device (music/effects balance: music default 0.6 vs effects at -20 dB set by the game), latency. WAV analysis done (desktop).
5. **SAF folder picker / launcher** (planned Tauri 2 + Svelte launcher, see [strategy.md](strategy.md)) so users
   can pick their GOG folder or zip without adb; validate files (hashes in [inventory](../game-files/inventory.md)).
6. **Performance**: profile the render thread (blitters), superblock chaining, maybe a cheap flag-laziness path; only
   consider option C (recompilation) if phones are too slow.
7. **In-game Esc menu**: find out why Esc does nothing in game (game option, or an HLE gap), so Back can open Save/Quit.
8. ~~**Touch UX**~~: context touch, two UI modes, pinch = resolution: done 2026-11-05 ([touch.md](touch.md),
   [ui-classic.md](ui-classic.md), [ui-hud.md](ui-hud.md)), verified on Android (ux-verify1). Next: real phone,
   in-app settings screen (mode, relay URL). Old letterbox buttons removed 2026-11-07.
9. **Videos**: Indeo 5 decoder (or keep skipping).
10. **Multiplayer**: LAN works out of the box (embedded relay + discovery, phone host <-> PC verified 2026-11-08); DirectPlay HLE over our relay works desktop <-> desktop and Android -> desktop (ux-verify1); next: relay URL setting + lobby UI on Android, public relay.
11. **iOS**: same Rust core (interpreter only, no JIT needed), a UIKit/Metal frontend implementing `Host`.

## Licensing (checked 2026-11-06, three times)

- Our code: GPL-3.0-or-later (`LICENSE` at the repo root, `license` in `port/Cargo.toml`).
- Bundled assets: only `port/crates/s3-win32/assets/fonts/LiberationSans-{Regular,Bold}.ttf`, SIL OFL 1.1, license
  text in `LICENSE-LiberationSans.txt` next to them (OFL fonts may be bundled with GPL software). `font8x8` (MIT)
  for the status screen. App icon is our own vector drawable.
- Rust dependencies (`cargo metadata`, re-checked 2026-11-06 incl. new `jni` 0.22 and optional `cpal` 0.15): all MIT / Apache-2.0 / BSD-3-Clause / Zlib / 0BSD / Unlicense (or dual with
  those); no GPL-2.0-only (no unicorn). Re-checked at the mobile UX wrap-up: 160 packages incl. `libm` 0.2.16,
  `fontdue`, `tokio`, `tokio-tungstenite`, `serde_json` all permissive (`r-efi` is MIT OR Apache-2.0 OR LGPL-2.1+).
  Mod overlays reuse the bundled Liberation Sans; HUD icons are sampled from the running game into memory only,
  never stored.
- `git ls-files`: no game bytes (no .exe/.dat/.map/.ogg/.wav/.avi, no images, no decompiled code dumps; docs only
  contain addresses, names and short findings). `game-files/`, `scratch/`, `port/target/`, `*.apk`, `*.keystore` are
  gitignored. Unit tests use hand-written x86 snippets only.
