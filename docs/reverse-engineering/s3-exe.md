# S3.EXE structure (GOG, sha256 00a925cf…)

## PE header

| Field | Value |
| --- | --- |
| Machine | i386, PE32, GUI subsystem 4.0 |
| ImageBase | `0x00400000`, **no relocation table** (must load at fixed base — easy for our loader, guest address space must keep 0x400000-0x7ED000 free) |
| EntryPoint | RVA `0x221BC6` |
| SizeOfImage | `0x3ED000` |
| Linker | 6.0 (MSVC 6 / Visual Studio 98) |
| TLS | none |
| Timestamp | 2000-03-31 17:09:52 |

| Section | RVA | VSize | RawSize | Flags |
| --- | --- | --- | --- | --- |
| `.text` | `0x1000` | `0x2537F6` (2.3 MB code) | `0x254000` | RX |
| `.rdata` | `0x255000` | `0x3CB36` | `0x3D000` | R |
| `.data` | `0x292000` | `0x158AC8` | `0x116000` | RW (≈270 KB BSS) |
| `.rsrc` | `0x3EB000` | `0x1840` | `0x2000` | R |

Code mentions source paths `E:\Siedler3\DirectX\Ddraw.cpp`, `Dplay.cpp`, `Dsound.cpp`; RTTI names
`CDirectDrawDevice`, `CSurface`, `CPalette`, `CSoundEffect`, `CGameThread`, `CGameNetSession` in namespace `DirectX`,
`CMCIException` in `Win32`, `Lobby::CLMAddPlayerToGroup`. Containers are Watcom-style `WC*` templates.
Debug/log strings are German ("Konnte keine Kreisposition finden"). Banner: "another finest directx game by volker und dirk".

## Static imports — 219 functions

Full list: [s3-exe-imports.md](s3-exe-imports.md). Counts: KERNEL32 130, USER32 41, GDI32 23, ADVAPI32 10 (registry),
`_INMM` 8 (winmm: `timeGetTime`, `timeSetEvent`, `mciSendCommandA/StringA` …), ole32 4 (`CoCreateInstance`, `CoCreateGuid` …),
RPCRT4 2, SHELL32 1. Threads: `CreateThread` + events/semaphores/`WaitForMultipleObjects`. `VirtualProtect` is imported (check why).

## Dynamically loaded (LoadLibraryA / GetProcAddress / CoCreateInstance)

Strings found: `ddraw.dll`, `dsound.dll` (`DirectSoundCreate`), `dinput.dll`, DirectPlay (via COM, `DPLAY:` / `DPERR_*` logs),
`wsock32.dll`, `wininet.dll`, `COMCTL32.dll`, `comdlg32.dll`, `ProtectCD.dll` (copy protection — check if still called in GOG build).
Video: MCI `open AVIVideo!%s alias %s` → Indeo 5 AVI via MCI; `GetCDVideoDirectory`. Music: MCI CD audio (GOG `_inmm.dll` redirects to ogg).

Port implication: the HLE surface is small. Needed families: Win32 kernel (files, threads, sync, time, memory),
a window + message loop, GDI subset (fonts/text? — check which GDI calls are used in game vs. setup paths),
registry (fake, preloaded from `regs.cmd` values), DirectDraw (8/16-bit surfaces, palette, blit, flip),
DirectSound (buffers, mixing), DirectInput (maybe unused — mouse comes via window messages; `GDIMouse` registry),
MCI (music/video), DirectPlay (multiplayer; later).

## Details

- DirectX creation paths and full method list: [directx-surface.md](directx-surface.md).
- `ProtectCD.dll` string has no code references (dead, static view).
- `VirtualProtect` is only used by a page-touch helper (`FUN_0061ab60`), not for self-modifying code.
- Address of the statically linked CRT and helpers: roughly `0x610000-0x630000` (HYPOTHESIS from function addresses).

## TODO

- ~~List the exact DirectDraw/DirectSound COM methods used~~, ~~resolution/pixel formats~~ → [directx-surface](directx-surface.md#static-details-for-the-hle-2026-11-05-static-re-pass-see-also-static-startupmd) (2026-11-05).
- ~~Main loop and game tick~~ → [static-startup](static-startup.md) (2026-11-05). Instruction census → [static-isa](static-isa.md).
- CRT range: `_except_handler3` `0x622230`, `__CxxFrameHandler` `0x61ec26`, `__CxxThrowException` `0x61f027`, `_ftol` `0x61fe98`, `_beginthreadex` `0x62279a`, `exit` `0x61f097` (confirms the CRT sits at ≈ `0x61e000-0x630000`).
