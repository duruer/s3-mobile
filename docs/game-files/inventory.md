# Game files inventory

Local copies only (`game-files/`, gitignored). Copied 2026-11-05 with `rsync -rt` from the Windows partition.

## Sources

| Local dir | Original location (Windows C: = `/dev/nvme0n1p2`, label `Loki`) | Size |
| --- | --- | --- |
| `game-files/gog-s3-ultimate/` | `C:\GOG Games\Settlers 3 Ultimate` | 1.6 GB |
| `game-files/settlers-united-launcher/resources/` | `C:\Program Files\Settlers United\resources` (Electron app, rest is Chromium runtime) | 385 MB |
| not copied | `C:\Program Files (x86)\aLobby` (old community lobby: Java, OpenVPN, `S3AccessRights.exe`, `.sdb` shims) | 142 MB |

Mount for re-copy: `udisksctl mount -b /dev/nvme0n1p2 -o ro` → `/run/media/<user>/<disk>`.
The 2nd disk `sda1` (`Luna`) has other games but **no** Settlers 3.

GOG product: "The Settlers 3 - Ultimate Collection", gameId `1207659185`, buildId `52373880706327305`, English.
The install has **Settlers United patches applied** (files dated 2022-12-31 / 2025-12-22): see
[exe-variants.md](exe-variants.md), [../settlers-united/README.md](../settlers-united/README.md).

## Top-level layout (gog-s3-ultimate)

| Path | Size / count | What |
| --- | --- | --- |
| `S3.EXE` | 3,842,048 B | Main game (GOG). Also `S3_multi.EXE`, `s3_united.exe`, `s3_alobby.exe` (same size, patched) |
| `GFX/` | 234 MB, 93 files | Graphics. Each `siedler3_NN` exists twice: `.7c003e01f.dat` (RGB555 masks 7C00/03E0/001F) and `.f8007e01f.dat` (RGB565). `Siedler3_25.f8007e01f.dat.original` = backup made by SU |
| `SND/` | 12 MB | `Siedler3_00.dat`, `siedler3_01.dat` sound archives |
| `MUSIC/` | 37 MB | `Track02..14.ogg` — CD audio replaced by GOG; played through `_inmm.dll` (MCI/winmm wrapper) + libvorbis DLLs |
| `Map/` | 347 MB | `SINGLE/` (30), `MULTI/` (59), `TUTORIAL/T1.MAP`, `User/` (~95 community maps), `MultiBackups/` |
| `S3/`, `S3QOTA/`, `S3MCD/` | 371 / 259 / 48 MB | Campaign data for base game, Quest of the Amazons, Mission CD: `MIS_V`, `MIS_M`, `ADD` with `.MAP`, `.DAT`, `.WAV` (voice briefings) |
| `Save/` | 213 MB | User saves (`Multi/` ...) — user data, not game data |
| `S3Editor/` | 14 MB | Map editor (`S3Editor*.exe`, `S3Editor.gfx`, `.uif`) |
| `s3Scripts/global` | | SU Lua? scripts (HYPOTHESIS, not inspected) |
| `res/s3ce.*.po` | | translations for `s3ce.exe` (GOG/SU config tool?) |
| `S3CD1.DAT S3CD2.DAT S3GOLD*.DAT S3MCD1.DAT S3QOTA1.DAT S3.DAT` | 0-1 B | CD presence marker files |
| `DATA.DAT` | 27 KB | unknown |
| `_inmm.dll` | | GOG winmm proxy (CD audio → ogg) — **statically imported** by S3.EXE |
| `ir50_32.dll ir50_qc*.dll` | | Intel Indeo 5 video codec (for AVI intro/cutscenes). **No .AVI files found in the install** |
| `MSVCRT.DLL MSVCIRT.DLL BGLWINR.DLL NEWTOOLR.DLL PATCHW32.DLL` | | old runtime/tool DLLs (Editor / patcher) |
| `s3.dll` (1.47 MB), `d3dx9_43.dll`, `mfc140.dll`, `ucrtbase.dll`, `crashpad_handler.exe`, `.sentry-native/`, `imgui.ini` | | Settlers United injected layer (D3D9 renderer, ImGui overlay, Sentry) |
| `regs.cmd`, `directplay.cmd` | | GOG setup scripts: registry values, enable DirectPlay feature. See [../reverse-engineering/registry.md](../reverse-engineering/registry.md) |

## Hashes (sha256)

| File | sha256 |
| --- | --- |
| `S3.EXE` (GOG) | `00a925cfdcfc3349b5d0f3b8a4c312f01b813e363b4464f6f3d6de17804d950b` |
| `S3_multi.EXE` = `s3_united.exe` | `87b16b62ed7a0fc3ac2f9563043fff34140421378d49a225977c8076d644e570` |
| `s3_alobby.exe` | `16f1f57356ce7c739feed78c4b1081cb1969677bc818c4cb66ef3556af067d06` |
| `s3.dll` (game dir) | `5bb60a96ce49b9a6c7dbbec3e38738e155473a8d4570c952c0b0630ec51dfc73` |
| `SND/Siedler3_00.dat` | `f85baafa034b76a542765df5a5e0aaafb9d48811b24daad29d5da2af033760cf` |
| `SND/siedler3_01.dat` | `3d42eca4fc1c75cfc0e979b3405a8e9210d8911ab3dfa5363297d501fb9851e2` |
| SU `bin/s3.dll` (15.7 MB) | `bae85ed653dc539773000b3cb6922240c0b8b05bd4c836eb61abd0a26579f552` |
| SU `bin/s3u.exe` | `b2875c50e67205a14930b61e6fef5d275c2c4ce0de0ef9593e0d54342bdf55c0` |
| SU `bin/settlers-united.dll` | `ae3d8a3d8c057fca413b6c500cde6181ca00ee7367707e11c258c4b024f4f8bb` |

Port implication: the app must accept a **GOG install folder** (or a zip of it) from the user. Minimum needed files
are not yet known — track in [../reverse-engineering/unknowns.md](../reverse-engineering/unknowns.md).
