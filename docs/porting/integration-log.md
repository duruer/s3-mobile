# Integration log (s3run against the real GOG S3.EXE)

Each round: build `s3run`, run it headless against the local GOG install, record how far the game gets, blockers
(by area: cpu-int, cpu-fpu, kernel32-runtime-pe, user-gdi-misc, directx, engine-desktop) and performance.
Screenshots stay local (`scratch/keep/`, gitignored): they show game graphics and must never be committed.

## How to run

```sh
cd port
CARGO_TARGET_DIR=$PWD/target cargo build --release -j 4 -p s3-desktop
G="--headless --game-dir ../game-files/gog-s3-ultimate --data-dir ../scratch/data"
# main menu -> "Single Player: Scenario" -> map popup OK (630 467) -> Game Setup OK (730 567) -> players OK (730 567)
# -> random map 384 starts. One click per OK is enough (three OK screens; the two last OKs share a position).
I="1000:move 165 235;1200:click 165 235;3000:move 630 467;3200:click 630 467;5000:move 730 567;5200:click 730 567;7000:move 730 567;7200:click 730 567"
./target/release/s3run $G --max-seconds 40 --inject "$I" --screenshot ../scratch/keep/x.png
```

- `--screenshot-every-present <dir>` writes only the **first 300** presents (~100-900 KB each; delete the dir
  afterwards). `--screenshot <png>` writes the last frame at exit.
- `S3_SAMPLE=1` (added in r1): once per second prints, per guest thread, the millions of instructions executed in that
  second plus EIP and state (`[sample 17.1s] t4:73M@0xfff0020d/Blocked ...`). Cheap; use it to see which thread is busy.
- `S3_TRACE=calls S3_TRACE_LIMIT=100000000` traces every HLE call (pipe through `grep -v` of hot calls:
  `CriticalSection|TlsGetValue|LastError|HeapSize|IntersectRect|...`, and of `guest t5` AI debug strings).
- Inject coordinates are in the **current** display mode: 800x600 in menus, 640x480 in game.
- Useful in-game coordinates (640x480): build menu (house icon) `25 167`; first building category `25 200`;
  first building in the list `37 237`.
- More inject commands (2026-11-05): `t:shot file.png` writes the current frame at time t (several per run, no need
  for `--screenshot-every-present`); `t:text abc` presses one key per character 120 ms apart (letters, digits, space,
  `-`; capitals with Shift). Careful: in game, letters are hotkeys (typing opened the build menu).
- **Save/load round trip** (in game at ~45 s when other jobs run; it can take 16-45 s): computer icon `21 452` ->
  "Save Game" `68 225` -> slot 1 line `40 244` saves at once ("Game Saved", no name prompt; slots 2-8 are 21 px
  apart) -> computer icon `21 452` -> "Quit Game" `68 420` -> YES `38 420` -> Game Statistics (click anywhere,
  `400 300`) -> main menu (800x600) -> "Load Game" `165 355` -> first entry `300 126` -> saved game continues:
  ```
  I="$I;45000:click 21 452;48000:click 68 225;51000:click 40 244;53000:shot s1.png;54000:click 21 452;56000:click 68 420;58000:click 38 420;63000:click 400 300;66000:shot s2.png;67000:click 165 355;69500:shot s3.png;70000:click 300 126;95000:shot s4.png"
  ```
  Verified 2026-11-05 (see kernel32.md "Save / load").

## 2026-11-05 — round 1

**Reached: in game, playable with injected mouse input.** Unmodified GOG S3.EXE + data, interpreter + HLE.

| Step | Time after start | Result |
| --- | --- | --- |
| Startup, window, registry (`regs.cmd` imported, 48 values), WSAStartup fails cleanly, MCI cdaudio (14 ogg tracks) | 0 s | as in [wine-trace.md](../reverse-engineering/wine-trace.md) |
| Intro AVI | ~0.1 s | MCI `AVIVideo` faked: play notifies the end at once (no video) |
| **Main menu** 800x600x16 | ~0.2 s | correct: all 13 buttons, logo, "Version 1.60", GDI text rendered with our font |
| Click "Single Player: Scenario" | | **Game Setup** dialog (Random / Single Player Map / Multi-player Map / User, World size 384). No serial prompt (registry from `regs.cmd` is enough) |
| OK | | players screen (Player / Computer Opponent, "Status: Waiting for Players") |
| OK | ~8 s | loading: 3 threads created (`0x622805` = `_beginthreadex`), 640x480x16, Gfx/Snd mapped (141 files), loading picture with a fade (column-dissolve artefacts in single frames are the game's own fade, not a blit bug — HYPOTHESIS) |
| **In game** | ~16-17 s | 640x480 view: castle, settlers, trees, river, minimap frame (minimap black at start, same under wine), AI thread prints `Center - player: 1 ...`, `COMP_FoodForMines` like under wine, music `MCI cdaudio play tracks 7..8` |
| Build menu click | | first try: **crash** -> fixed (below). After fix: categories open, building list shows, selecting a building shows placement markers + ghost buildings, clicking the map **places a construction site** (stakes + flag; settlers walk to it) |

Order of calls matches the wine trace (registry, CDAUDIO window, DDraw probe, 2 threads at menu, 3 at game start,
SetDisplayMode 800x600 -> 640x480, `WaitForVerticalBlank` pacing, Blt of the 536x582 world buffer). Guest C++
exceptions (`RaiseException 0xE06D7363`) are caught normally (20-40 per run, wine: 36).

### Fixed in r1

- **user-gdi-misc**: `SelectObject(CreateCompatibleDC(0), bitmap)` returned 0 because new DCs had `bitmap = 0`.
  S3.EXE (`0x4a8123` CreateCompatibleDC, `0x4a8233` SelectObject, then `FormatMessageA` + `throw`) treats 0 as an
  error; the second throw escaped -> `UnhandledExceptionFilter` -> CRT `MessageBoxA "abnormal program termination"`
  -> `ExitProcess(3)`. Fix: DCs start with stock `DEFAULT_BITMAP` (`gdi32.rs`), test
  `select_bitmap_into_memory_dc_returns_previous`.
- **engine-desktop**: `S3_SAMPLE=1` per-thread sampler in `s3run`.

### Open issues / blockers (none stop the game)

| Area | Problem | Evidence |
| --- | --- | --- |
| user-gdi-misc | (FIXED 2026-11-05, cursor task: drawn at present time, see user32-gdi32.md#cursor-drawing) No mouse cursor is drawn. Registry `GDIMouse`=1, so the game uses the Windows cursor (`LoadCursorA` 44x, `SetCursor` 68x per run); `SetCursor` only stores the handle. Frontends (Android!) must draw the cursor image from `.rsrc` (`RT_GROUP_CURSOR`/`RT_CURSOR`) at the mouse position, or the user cannot see where a tap lands/what mode is active | `user32.rs set_cursor`; screenshots show no cursor |
| engine-desktop | ~~Desktop host has no audio sink~~ FIXED 2026-11-05 (audio-verify): `--audio-wav <file>` (+ `--audio-play` with feature `audio-cpal`), see [directx-hle.md](directx-hle.md) | `s3-desktop/src/audio.rs` |
| engine-desktop | `--screenshot-every-present` keeps only the first 300 frames (the in-game part is cut off when loading takes many presents). An "every Nth" or "last N" option would help | `MAX_PRESENT_SHOTS` |
| kernel32-runtime-pe | ~~`GetProcAddress(kernel32, IsProcessorFeaturePresent)` and `(user32, GetLastActivePopup)` not declared~~ fixed 2026-11 (input-ok-bug) | |
| directx | Intro AVI not played (faked); acceptable |  |

### Performance (desktop, release, one host core, other agents running)

- Menu: idle (threads blocked in `GetMessageA`/waits); ~10 M instructions total in 10 s.
- Loading (8-16 s): one thread at a time, ~130-165 M guest instructions/s.
- In game: ~48 presents/s (`WaitForVerticalBlank` paced); render thread ~70-85 M instr/s + AI ~20 M instr/s
  ≈ 90-110 M instr/s, **~60 % of one host core**. Android arm64 should be OK if it reaches >= 1/2 of that.
- Two early runs looked slower (loading picture still shown at 36 s); 7 later sampled runs all reached the game at
  16-17 s. HYPOTHESIS: those runs ran in parallel with other heavy jobs. Re-check on Android.

## 2026-11-05 — end-to-end verification 1 (verify1)

- `cargo build --workspace --release` and `cargo test --workspace -j 4`: 159 passed, 1 ignored, 0 failed. No breakage
  from the parallel polish tasks (cursor, save-load, input-ok-bug, audio-verify, android-ux).
- Desktop, one run (`scratch/verify1/run3.log`, `S3_SAMPLE=1`, `--audio-wav`): menu -> Single Player -> in game at ~17 s
  -> build menu, Woodcutter's Hut, ghost sites -> save slot 1 ("Game Saved") -> Quit -> statistics -> main menu ->
  Load Game list -> saved game continues. Cursor visible in all shots (`scratch/verify1/d1..d8*.png`). WAV: silent
  in the menu/loading, in game RMS ~1000-2600 (peak up to 12k), timing summary: no drift, max gap 43 ms.
  Note: the recipe's placement click `320 250` hit the castle on that random map (no site); the place depends on the
  map, so pick a ghost site from a screenshot.
- One earlier desktop run needed ~77 s from players OK to in game (normally 9-10 s), with a low load average; the
  injected clicks then all hit the loading screen. Not reproduced in the next 2 runs. HYPOTHESIS: host contention.
  Use `S3_SAMPLE=1` and check that `cdaudio play tracks 7..8` (game start) is logged before the in-game clicks.
- Android: the whole flow works by taps, see [android.md](android.md) (verification 1). No code changes were needed.
