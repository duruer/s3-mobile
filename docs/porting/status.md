# Port status (after the overnight run, 2026-11-06)

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
| Single Player: Scenario -> Game Setup (no serial prompt) -> players screen | yes (injected clicks) | yes (taps) |
| Players OK -> loading -> in game, random map 384, 640x480x16 | in game at ~16-17 s | in game at ~20-25 s |
| Build menu, categories, building list, placement markers, construction site placed, settlers walk to it | yes | yes (by touch) |
| Map scrolling | arrow keys, right-drag | one-finger drag (mapped to mirrored right-drag), arrow keys |
| Music (MCI cdaudio -> `MUSIC/*.ogg`), effects (DirectSound mixer) | produced, but no desktop audio sink | AAudio output (emulator boots with `-no-audio` by default, so not listened to yet) |
| Pause / resume (Home, reopen) | n/a | yes, guest clock stops while paused |
| Real arm64 phone | n/a | **not tested yet** (arm64-v8a library is in the APK) |

Evidence (local): desktop `scratch/keep/r1-menu.png`, `r1-setup.png`, `r1-players.png`, `r1-loading.png`,
`r1-ingame.png`, `r1-build-cat.png`, `r1-place-sel.png`, `r1-placed-a.png`; Android `scratch/android-shots/main-menu.png`,
`attempt1-1.png` (menu), `-2` (Game Setup), `-5` (in game), `-6`/`-7` (build menu, markers), `-9` (construction site),
`-16` (after resume).

Tests: `cargo test --workspace -j 4` = 136 passed, 1 ignored, 0 failed (2026-11-06 ~03:00), including a
differential test of the integer ALU against the host CPU.

## Performance

| Where | Numbers |
| --- | --- |
| Interpreter micro-benchmark | ~170-185 M guest instructions/s (desktop, one core) |
| Desktop, loading | 130-165 M instr/s on one guest thread |
| Desktop, in game | ~48 presents/s (WaitForVerticalBlank paced), render thread 70-85 M + AI ~20 M instr/s, ~60 % of one core |
| Emulator x86_64, in game | ~30 presents/s at ~95 M instr/s, engine thread ~77-92 % of one core, RSS ~200 MB |
| APK | 9.6 MB (arm64-v8a + x86_64, stripped, uncompressed .so) |

HYPOTHESIS: a current arm64 phone core is in the same class as the emulator's x86_64 core for this interpreter,
so expect 25-50 fps; unmeasured. The interpreter is single-threaded (cooperative green threads), so it uses one core.

## Install on a phone

Full steps: [android.md](android.md#install-for-users-phone). Short version:

1. Build or take `port/target/apk/settlers3port.apk` (`cd port && scripts/build-apk.sh`); install with
   `adb install -r -g settlers3port.apk` (or copy to the phone, allow unknown sources).
2. Start the app once (status screen "Game files missing").
3. Copy the contents of your own GOG install (folder with `S3.EXE`, `regs.cmd`, `GFX`, `SND`, `MUSIC`, `Map`, `S3`)
   to `Android/data/com.settlers3port.app/files/game/` (adb push or USB/MTP; ~0.6-1.2 GB; only the
   `GFX/*.f8007e01f.dat` set is needed).
4. Reopen the app. Tap = left click, long press / two-finger tap = right click, drag on the map = scroll, Back = Esc.

## Known issues

| Issue | Notes |
| --- | --- |
| No mouse cursor drawn | game uses the Windows cursor (`GDIMouse`=1); `SetCursor` only stores the handle. Needs cursor resources from `.rsrc` drawn by the frontend |
| Intro/mission videos skipped | MCI AVI faked (Indeo 5 decoding not implemented) |
| First OK tap on the players screen is ignored | tap again; same on desktop |
| No on-screen keyboard | save names / player name need a hardware keyboard |
| Save/load not verified | VFS write overlay in `files/data` exists, but no save/load round trip was run |
| Multiplayer unavailable | DirectPlay objects report "nothing available"; WSAStartup fails on purpose |
| x87 precision | 53-bit, round-to-nearest only; unmasked FPU exceptions not delivered (see [x87-fpu.md](x87-fpu.md)) |
| Desktop has no audio sink | sound only on Android |
| Game files must be copied via adb/MTP | Android 11+ file managers often cannot open `Android/data`; no folder picker yet |
| `GetProcAddress` misses `IsProcessorFeaturePresent`, `GetLastActivePopup` | CRT copes with NULL |

## Next steps

Priority order:

1. **Real arm64 phone test**: install, copy files, measure fps / instr/s / battery / thermal.
2. **Cursor**: draw `RT_GROUP_CURSOR` images from S3.EXE `.rsrc` at the mouse position (frontend overlay).
3. **Save/load** round trip (in-game save menu, `Save/` in the overlay), plus an on-screen keyboard (JNI
   `showSoftInput`) for save names.
4. **Audio polish**: listen on a device, check music/effects balance, latency, resampling quality, desktop sink (cpal).
5. **SAF folder picker / launcher** (planned Tauri 2 + Svelte launcher, see [strategy.md](strategy.md)) so users
   can pick their GOG folder or zip without adb; validate files (hashes in [inventory](../game-files/inventory.md)).
6. **Performance**: profile the render thread (blitters), superblock chaining, maybe a cheap flag-laziness path; only
   consider option C (recompilation) if phones are too slow.
7. **Touch UX**: pinch to change zoom/resolution, immersive mode, larger hit areas, configurable long-press time.
8. **Videos**: Indeo 5 decoder (or keep skipping).
9. **Multiplayer**: DirectPlay over our own TCP/UDP relay (HLE of IDirectPlay4A sessions), LAN first.
10. **iOS**: same Rust core (interpreter only, no JIT needed), a UIKit/Metal frontend implementing `Host`.

## Licensing (checked 2026-11-06)

- Our code: GPL-3.0-or-later (`LICENSE` at the repo root, `license` in `port/Cargo.toml`).
- Bundled assets: only `port/crates/s3-win32/assets/fonts/LiberationSans-{Regular,Bold}.ttf`, SIL OFL 1.1, license
  text in `LICENSE-LiberationSans.txt` next to them (OFL fonts may be bundled with GPL software). `font8x8` (MIT)
  for the status screen. App icon is our own vector drawable.
- Rust dependencies (`cargo metadata`): all MIT / Apache-2.0 / BSD-3-Clause / Zlib / 0BSD / Unlicense (or dual with
  those); no GPL-2.0-only (no unicorn).
- `git ls-files`: no game bytes (no .exe/.dat/.map/.ogg/.wav/.avi, no images, no decompiled code dumps; docs only
  contain addresses, names and short findings). `game-files/`, `scratch/`, `port/target/`, `*.apk`, `*.keystore` are
  gitignored. Unit tests use hand-written x86 snippets only.
