# Settlers United (SU)

Community launcher/patch for S3 and S4 (settlers-united.com). Electron app installed at `C:\Program Files\Settlers United`.
Update feed: `https://files.settlers-united.com` (generic provider, `stable` channel).

## Components (local copy: `game-files/settlers-united-launcher/resources/`)

| Path | What |
| --- | --- |
| `app.asar` | Electron UI: `main.prod.js`, `dist/renderer.prod.js` (list with `bunx @electron/asar list`) |
| `bin/s3u.exe` (5.3 MB) | S3 starter/injector — HYPOTHESIS |
| `bin/s3.dll` (15.7 MB) | S3 hook layer (newer than the 1.47 MB `s3.dll` in the game dir). Game-dir `s3.dll` imports `d3d9`, `gdiplus`, `winhttp`, `dbghelp`, `IMM32`, MSVC 14 runtime → D3D9 renderer + ImGui overlay + screenshots + crash reporting (Sentry/crashpad) |
| `bin/s3.ai.dll` | AI replacement/extension — HYPOTHESIS |
| `bin/s3_assets.bin`, `bin/s3_artifacts/`, `bin/s3/` | SU assets |
| `bin/settlers-united.dll` | shared core (lobby/network?) |
| `bin/wireguard/` | VPN for multiplayer (DirectPlay over virtual LAN) |
| `bin/ffmpeg.exe`, NAudio*.dll, NetTopologySuite, Newtonsoft.Json | recording / .NET helpers |
| `bin/s4*` | Settlers 4 equivalents (not our concern) |

In the game dir SU added: `s3_united.exe` ([exe-variants](../game-files/exe-variants.md)), `s3.dll`, `d3dx9_43.dll`,
`mfc140.dll`, `ucrtbase.dll`, `crashpad_handler.exe`, `.sentry-native/`, `imgui.ini`, `s3Scripts/`, `s3ce.exe` + `res/*.po`,
and modified `GFX/Siedler3_25.f8007e01f.dat` (backup `.original`).

## End-of-game statistics (checked 2026-11-08, read as data)

- Game-dir `s3.dll` strings (`0x1121e0..0x112308`): "Points", "Settlers", "Buildings", "Mine", "Gold", "Mana",
  "Soldiers", "Battles", ImGui ids `#gamestats-open` / `#gamestats-container`, "View Extended Game Stats", "Stats",
  "Count", "Game Time"; `res/s3ce.*.po` also translates "food" and these keys. So SU adds a button on the
  statistics screen that opens an ImGui window with per-category charts (count against game time); HYPOTHESIS:
  sampled during the game from the game's statistics counters (we did not run SU).
- Launcher (`app.asar`): game state `EndStatistic`; statistics images saved to `Pictures/Settlers United/Settlers 3
  Statistics/` (timestamp `%Y-%m-%d_%H.%M.%S` in `s3.dll`) and listed in a launcher page ("There are no statistics
  available yet. Play a game.").
- Our own equivalent: [menu-layout.md "Statistics over time"](../porting/menu-layout.md). Not done: saving the
  statistics as images.

## Relevance to the port

- SU proves the original exe can be wrapped: rendering moved to D3D9 by hooking DirectDraw. We do the same idea at the API layer (HLE).
- SU's bug fixes are inside its DLLs (closed source). We must not ship them. Our port targets the **plain GOG S3.EXE** first.
- Open: list which known S3 bugs SU fixes so we can decide whether to re-implement any as in-memory patches. ([unknowns](../reverse-engineering/unknowns.md) #9)
