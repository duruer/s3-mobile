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
- Touch inject commands (2026-11-06, raw pointer events through the s3-mods gesture recognizer, like Android):
  `tap x y`, `tap2 x y`, `drag x1 y1 x2 y2 [ms]`, `longpress x y [ms]`, `pinch cx cy scale [ms]`; `S3_MODS_DEBUG=1`
  draws fingers and gesture names. See [mods.md](mods.md#desktop-testing-s3run---inject-touch-commands).
- Useful in-game coordinates (640x480): build menu (house icon) `25 167`; first building category `25 200`;
  first building in the list `37 237`.
- Multiplayer (2026-11-05, mp-dplay): `port/scripts/mp-local-test.sh` drives two headless instances + a local relay
  through the LAN menu into one game; `--mp-hash FILE` (per-simulation-tick state hash), env `S3_RELAY_URL`,
  `S3_NET_NAME`, `S3_DPLAY_TRACE=1`, `S3_GAME_LOG=1` (game's own debug log), `S3_EXE_ARGS` (guest command line,
  e.g. `-netmon`). See [multiplayer.md](multiplayer.md) section 6.
- More inject commands (2026-11-05): `t:shot file.png` writes the current frame at time t (several per run, no need
  for `--screenshot-every-present`); `t:text abc` presses one key per character 120 ms apart (letters, digits, space,
  `-`; capitals with Shift). Careful: in game, letters are hotkeys (typing opened the build menu).
- Read-only memory inspection for RE (re-units, 2026-11-05): `t:xpeek EXPR LEN`, `t:xdump EXPR LEN FILE` (EXPR with
  `@` pointer steps, e.g. game data `0x700ffee0@0x3a0@0x2d280`), `t:scan VALUE [MAX]`; see
  [game-state-units.md](../reverse-engineering/game-state-units.md#tools).
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
- 2026-11-05 touch-context (W3): `s3run --inject` gained `dtap`, `tap3`, `drag2`, `fling`, `lpdrag`; in-game touch recipe and results in [touch.md](touch.md).

## 2026-11-05 — mobile UX verification 1 (ux-verify1)

Scope: touch-context (W3), UI mode A "Classic Touch", UI mode B "Mobile HUD" and DirectPlay over our relay (W5),
together, on desktop and on the Android emulator. Progress note: `docs/progress/ux-verify1.md`. Screenshots and logs
(local only): `scratch/ux-verify1/{A,A2,B,B2,B3,B4,mp,mp5,mp-android,mp-android2,final}/`,
`scratch/android-shots/ux1-*.png`.

**Tests**: `cargo build --workspace --release` and `cargo test --workspace --release -j 4`: 232 passed, 1 ignored,
0 failed (the `s3-dx` DirectPlay test reported failing by the UI agents passes now).

**Desktop, mode A** (`A/`, default settings, 640x480 in game): menu by taps -> in game; `lpdrag` box select (10
selected, chip "10 selected") -> tap ground = move (flag, units walk) -> `tap2` deselect (count 10 -> 0) -> `drag2`
pan (camera +288/+192 incl. glide) -> panel taps build tab / category / Woodcutter's Hut (magnifier, "panel click"
log) -> tap a marker = site (slot 1 -> 0, poles visible) -> Menu -> Save Game -> slot 1 ("Game Saved") -> Close ->
Menu -> Quit Game -> YES -> statistics -> main menu -> Load Game -> entry -> saved game continues with the site.
`A2/`: Menu -> new **"UI: Mobile HUD"** button -> HUD in cover mode, Basic sheet works.

**Desktop, mode B** (`B*/`, `{"ui_mode":"MobileHud"}`): first run (`B/`) showed taps on the map acting as plain left
clicks (tap on the ground with units selected deselected them, `tap2`/`drag2` did nothing): **bug 1** below. After
the fix (`B2/`): box select (card "12 selected"), tap = move (flag), `tap2` = deselect (12 -> 0), `drag2` pan,
HUD menu -> "UI: Mobile HUD > Classic Touch" -> original panel back mid-game. `B4/`: Basic -> Woodcutter's Hut ->
banner -> taps on a grid; the second tap hit a marker and placed the site (slot 1 -> 0, boards 25 -> 24). (`B2`/`B3`
taps missed: water / castle; pick markers from a screenshot.) `B/`: HUD menu -> Save game (panel peek) -> Save Game
-> slot 1 -> Back to HUD -> Quit game -> YES -> statistics -> Load Game -> continues in HUD mode.

**Desktop multiplayer** (`mp/`, `HOST_EXTRA`/`JOIN_EXTRA` with touch gestures, 240 s): both in game, both built a
site, `RESULT: IDENTICAL (no desync)` over 3104 ticks, 30 executed messages each in the same order. **Bug 2**
showed up here (`mp2/`: `touch: tap (300,150) not in game -> fallback`); after the fix (`mp5/`) taps decide
`Command` (move) on host and joiner, units walk, hashes identical, 4 player commands.

**Android** (both-ABI release APK, emulator-5610, adb `input tap`; coordinates: menu `((267+x)*1.8, y*1.8)`, in game
`((214+x)*2.25, y*2.25)`):

| Flow | Shots |
| --- | --- |
| Mode A: tap soldier -> "1 selected", tap ground -> flag, soldier walks | `ux1-04`, `-05`, `-06` |
| Mode A: one-finger drag pans (`input draganddrop` = pan, not a box: no hold) | `ux1-07` |
| Mode A: panel taps -> placement (chip, ✕ Cancel) -> tap marker -> site ("+1" on the button) | `ux1-08`, `-09` |
| Mode A Menu -> "UI: Mobile HUD" -> HUD (cover mode), tap select / move, Basic -> Woodcutter -> site | `ux1-11`..`-18` |
| App restart with `MobileHud` saved -> full-screen HUD; tap select / move (flag), drag pan, HUD menu, -> Classic | `ux1-19`..`-26` |
| MP: relay on the PC `127.0.0.1:7788`, `relay-url.txt` = `ws://10.0.2.2:7788/`; LAN -> "Internet (Settlers 3 Port)" -> Search -> "Player's game" -> Join -> both players listed -> Ready; desktop host starts | `ux1-29`..`-31`, `-45`, `-46` |
| MP in game (Android = blue player, desktop = red); Android: tap select + tap move (flag, walks); desktop host taps 14 soldiers to move at the same time | `ux1-47`..`-50`, `mp-android2/host-220.png` |
| MP, Android switched to HUD mid-game: unit card, Move flag, walks | `ux1-55`..`-57` |

Recipe (desktop host for an Android joiner; the host waits ~150 s in the waiting room for the phone):

```sh
port/target/release/s3-relay --listen 127.0.0.1:7788 &
adb -s emulator-5610 push relay-url.txt /sdcard/Android/data/com.settlers3port.app/files/data/   # ws://10.0.2.2:7788/
HOST="1500:click 165 275;5000:click 200 131;6500:click 730 567;9000:click 181 180;10000:click 165 255;11000:click 630 467;12500:click 730 567;150000:click 723 114;153000:click 730 567"
S3_RELAY_URL=ws://127.0.0.1:7788/ port/target/release/s3run --headless --game-dir game-files/gog-s3-ultimate --data-dir scratch/h --max-seconds 330 --inject "$HOST"
# phone: LAN (165,275) -> network (200,131) -> Search (175,240) -> game (200,330) -> Join (610,567) -> Ready (723,134)
```

**Fixes made** (smallest change each):

1. `s3-mods/src/touch_game.rs` `layout()`: accepted only `panel_w > 0`, but mode B sets the panel width to 0, so
   touch-context thought no game was running and every gesture fell back to plain mouse mapping. Now `panel_w >= 0`
   (+ unit test `full_screen_hud_layout_is_all_map`).
2. **Multiplayer**: the global `[0x7e3408]` (`CGameData*`) stays **0** in a network game (VERIFIED on host and joiner
   with `peek`), while `[logic+0x3a0]` is valid. `touch_game::gd()` now falls back to the `[logic+0x3a0]` recorded by
   the `HandleInput` hooks (`note_logic`, touch.rs and ui_hud_game.rs); `ui_hud_game::gd()` uses the same fallback.
   Without it, touch play in MP could select (left click) but never move.
3. Android: manifest had no `INTERNET` permission; added. `s3-android/src/frontend.rs` reads the relay URL from
   `<data dir>/relay-url.txt` (sets `S3_RELAY_URL` before the engine starts).
4. Mode A had no way into mode B: while the Menu (page 7) is open, a "UI: Mobile HUD" button sits above "Close"
   (`ui_classic.rs` `FloatBtn::HudMode`; saves the setting, closes the menu).
5. Cosmetic: touch.rs no longer draws its ✕ placement cancel in mode B (the HUD banner has Cancel; the ✕ covered the
   resource strip). The invisible ✕ hit area top right still cancels placement in mode B.
6. `S3_TOUCH_TRACE=1` also prints each tap decision (`touch: tap (x,y) mode .. -> ..`, or why it fell back).

Not verified: pinch / two-finger gestures and long-press box select on Android (adb cannot do them; desktop + unit
tests only); real phone; attack on an enemy by tap; Android as MP host; desync hash on Android.
