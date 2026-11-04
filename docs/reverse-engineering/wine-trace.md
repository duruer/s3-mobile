# Wine reference run of GOG S3.EXE (2026-11-05)

What the original game does at runtime, observed under wine. Use it as the spec for the HLE layer.
All facts here are OBSERVED in the trace unless marked HYPOTHESIS. Raw logs were deleted (too big); the
summaries are in `scratch/wine-*` (gitignored, local only).

## Setup (how to repeat)

| Item | Value |
| --- | --- |
| wine | 11.21, `WINEARCH=win64` prefix, 32-bit game via WoW64. No mono/gecko (`WINEDLLOVERRIDES="mscoree,mshtml="` at `wineboot -i`). Prefix ≈ 380 MB |
| Prefix | `scratch/wineprefix`; `C:\S3` is a **symlink** to `game-files/gog-s3-ultimate` (no copy) |
| Display | `Xvfb :77 -screen 0 1024x768x16`, run with `DISPLAY=:77` and `WAYLAND_DISPLAY` unset |
| Virtual desktop | **Required.** `HKCU\Software\Wine\Explorer` `Desktop=Default`, `...\Explorer\Desktops` `Default=1024x768` (`scratch/wine-desktop.reg`). Without it Xvfb cannot change mode and the game shows *"DirectDraw Error DDERR_UNSUPPORTED occured at CDirectDraw::DDSetDisplayMode"* and stops |
| Registry | `regs.cmd` converted to `scratch/wine-s3.reg` (keys under `HKLM\SOFTWARE\Wow6432Node\...` = 32-bit view), imported with `wine regedit /S` |
| Launch | `scratch/wine-run.sh "<WINEDEBUG>" <log>` (cd `C:\S3`, `wine S3.EXE`, log capped with `head -c 300M`) |
| Relay filter | `+relay` on everything is ~300 MB in 45 s. `RelayFromInclude` did **not** filter in wine 11 WoW64. What worked: `HKCU\Software\Wine\Debug\RelayInclude` = the game's static imports minus hot ones (Tls*, Heap*, Enter/LeaveCriticalSection, Interlocked*, Get/SetLastError), then keep only calls whose `ret=` is inside S3.EXE (`0x400000-0x800000`) or `_INMM.dll`. Script: `tools/wine_relay_summary.py <log> [start] [end] [out] [regex]` |
| Input | `xdotool` on display :77 (Escape skips the intro; clicks by absolute coords in the 800x600 menu) |

## Result: it runs

| Step | What happened | Screenshot (local) |
| --- | --- | --- |
| No virtual desktop | MessageBox `DDERR_UNSUPPORTED ... CDirectDraw::DDSetDisplayMode` | `scratch/wine-shot2.png` |
| Start (+3 s) | Intro AVI plays (Blue Byte / Roman temple, Indeo 5 via MCI "Wine MCI-AVI player" window, 640x480 area at top-left) | `scratch/wine-shot4.png` |
| Escape | **Main menu**, 800x600x16: Tutorial, Campaign, Mission CD Campaign, Amazon Campaign, Campaign: Normal, Single Player: Scenario, Multi-player LAN / Internet, Load Game, Online Help, Tips & Tricks, Credits, Exit Game. "Gold Edition" logo, "Version 1.60" | `scratch/wine-shot5.png` |
| Single Player: Scenario (1st run) | **"Please enter your Settlers III serial number"** dialog (see serial section). Typing the GOG key `0497-2584-2194-4356-2336` + OK was accepted | `scratch/wine-r6.png` |
| Game Setup | Map dialog (Random / Single Player Map / Multi-player Map / User), team settings | `scratch/wine-r8.png`, `wine-d4.png` |
| OK, OK | Random map 384 and "Beginner I" both start; briefing screen (English + German text), then the **in-game view at 640x480x16** with settlers, castle, minimap, menus | `scratch/wine-r11.png`, `wine-d6.png`, `wine-d7.png` |

Startup to main menu is ~3 s plus the intro length. The game was never stopped by a CD check.

## Serial number / XOR decoder keys (resolves unknowns #1)

| Key | Value | Access |
| --- | --- | --- |
| `HKLM\Software\BlueByte\Siedler3\1.0\General` | `SingleNumber` (REG_SZ) | read at startup (size query, then data); written after the serial dialog: 0x61 bytes (96 chars + NUL) |
| `HKCR\CLSID\{0e9f8400-d9a5-11cf-8f4d00804880b189}` | `Seed` (REG_DWORD) | key opened at startup (twice); created by the serial dialog with `Seed` = random DWORD (we got `0x6a0f`) |

Note the GUID is **malformed on purpose** (no dash between `8f4d` and `00804880b189`), so it looks like a COM class key.

- Fresh GOG registry (no HKCR key): `RegOpenKeyExA(HKCR, "CLSID\{0e9f8400-...}")` returns 2 (not found) → the menu works,
  but **"Single Player"/campaign asks for the serial**. After entering it the game writes `SingleNumber` **encrypted**
  (the encoder is not NOPed in GOG, only the decoder at file offset `0xFE72`) and `Seed`.
- Plain-text GOG `SingleNumber` **plus** any `HKCR\...\Seed` value: no prompt (tested in run 2).
- **Port rule:** our fake registry must contain both `HKLM\...\General\SingleNumber` = the `regs.cmd` text and
  `HKCR\CLSID\{0e9f8400-d9a5-11cf-8f4d00804880b189}\Seed` = any DWORD. If the game writes `SingleNumber` again
  (encrypted), the GOG exe cannot read it back (HYPOTHESIS: then it would ask again), so do not persist that write;
  keep the plain value.

## Registry values read (in order)

All under `HKLM\Software\BlueByte\Siedler3\1.0\General` (note the spelling `BlueByte`; Windows is case-insensitive,
our fake registry must be too), opened with `KEY_ALL_ACCESS` (0xF003F). Each value is read with the usual two-call
pattern (size query with NULL buffer, then data).

1. `SingleNumber`
2. `HKCR\CLSID\{0e9f8400-...}` (open only)
3. `Playername`, `ClanFlag`, `NoAlpha`, `NoFlip` (missing → default), `SystemBackBuffer` (missing → default),
   `WaitVBlank`, `GDIMouse`, `Resolution`, `FogSpeed`, `ScrollMode`, `ScrollSpeed`, `BuildHelp`, `SoundFormat`,
   `CDAudio` (2×), `Intro`, `VideoFormat`, `MessageLevel`, `GameZoneSelection`, `FirstLogon`, `Font`, `FontSize`,
   `BannerId`, `BannerSubId`, `Mission`, `Plus`, `Gold`, `Tips&Tricks`, `Newbie`, `Language`
4. reopen `General`: `Campaigns`, `CampaignsPlus` (REG_BINARY)
5. `HKCR\CLSID\{0e9f8400-...}` again
6. `HKLM\...\Siedler3\1.0\Index Server` (missing, OK)
7. `RegCreateKeyExA HKLM\...\Siedler3\1.0\Patches`: `InPatch` (missing), `Convert` (REG_SZ), `RegQueryInfoKeyA`

Not read at all: `HKLM\SOFTWARE\Bluebyte\S3Editor`, `S3PLUSCD`, `App Paths\s3.exe`.
Written (after the serial dialog / settings): `SingleNumber`, `Seed`, `Playername`, `ClanFlag`, `WaitVBlank`,
`Resolution`, `Intro`, `GDIMouse`, `GameZoneSelection`, `FirstLogon`, `Font`, `FontSize`, `BannerId`, `BannerSubId`,
`Newbie` … (17 `RegSetValueExA` in total). The fake registry must accept writes (keep them in memory or a file
next to the save games).

## DLLs loaded (S3.EXE process, in order)

Static/implicit: `S3.EXE` (native, at `0x400000`), kernelbase, kernel32, msvcrt, **`libogg-0.dll`, `libvorbis-0.dll`,
`libvorbisfile-3.dll`** (native, from the game dir, pulled in by `_INMM.dll`), ucrtbase, sechost, advapi32, gdi32,
user32, msacm32, ole32 (+combase, rpcrt4), WINMM, **`_INMM.dll`** (native, GOG winmm wrapper), shell32, imm32,
comctl32.

Dynamic, in order: `dinput.dll` (+hid), `opengl32`/`wined3d` (wine internals), `ddraw.dll`, `dplayx.dll` (DirectPlay
COM probe at startup), `dsound.dll` (+mmdevapi), `wsock32.dll` (+ws2_32, iphlpapi), `msvfw32.dll` + `mciavi32.dll` +
**`ir50_32.dll`** (native Indeo 5 codec from the game dir; unloaded when the intro closes).

Not loaded: `ProtectCD.dll`, `wininet.dll`, `comdlg32.dll`, `s3.dll` (Settlers United), `d3dx9_43.dll`.

## Files opened

| When | Files | How |
| --- | --- | --- |
| Startup | `C:\S3\Gfx\siedler3_60.f8007e01f.dat` (FindFirstFile probe) | |
| Startup | 19 `MoveFileA` of old multiplayer map names in `Map\Multi\` (e.g. `A256-2_Quick3.map` → `A256-2-Quick3.map`, `448-2-Island 3.map` → `448-2-island_3.map`). All **failed** (files do not exist in the GOG install) → nothing changed | patch/convert step |
| Startup (`_INMM`) | `MUSIC\Track00.ogg` … `Track65.ogg` (at least) probed; 13 files exist | `_INMM` maps CD tracks to ogg |
| Loading (thread 2) | `Gfx\siedler3_00..17, 20..27, 30..37, 40..48, 60, 61 .f8007e01f.dat` (44 files) | `CreateFileA` + `GetFileSize` + `CreateFileMappingA` + **`MapViewOfFile`** (memory-mapped, read-only) |
| Loading | `Snd\siedler3_00.dat`, `Snd\siedler3_01.dat` | same, memory-mapped |
| Intro | `S3\Add\001\01_00_00.dat` (an AVI despite the extension) | MCI `AVIVideo` |
| Map dialog | `FindFirstFile` over `Map\Single\` (and the others) | |
| Game start | `Map\Single\Beginner I.MAP`; Gfx and Snd files are mapped again; `MUSIC\Track07.ogg` (in-game music) | |

The **Gfx file name encodes the pixel format**: `f8007e01f` = R `0xF800`, G `0x07E0`, B `0x001F` (RGB565, the mode
wine reported). HYPOTHESIS: with a 555 primary the game would open `*.f7c03e01f.dat`; the port should report RGB565.

No file was opened for writing; the game directory listing (mtimes) was identical before and after all runs.
Nothing was written to `Save\` (we did not save).

## DirectDraw

| Item | Value |
| --- | --- |
| Creation | `CoCreateInstance` → IDirectDraw2 (`{b3a6f3e0-…}`), `Initialize(NULL)`; done 3× (probe, menu, game) |
| Probe | first object: `SetCooperativeLevel(NULL, DDSCL_NORMAL)` only, then released |
| Cooperative level | `DDSCL_FULLSCREEN | DDSCL_EXCLUSIVE | DDSCL_ALLOWREBOOT` (0x13) on the main window |
| Enumeration | `DirectDrawEnumerateA` (via GetProcAddress) from the render thread |
| Display modes | **800x600x16** for intro/menus/setup, **640x480x16** in game (registry `Resolution`=0). `SetDisplayMode(w,h,16,0,0)`; called again on every screen change. `GetDisplayMode` once before the game |
| Surfaces (menu) | Primary: caps `DDSCAPS_PRIMARYSURFACE` only (**no back buffer, no flip chain**), 800x600, pitch 1600, RGB565. Offscreen: `DDSCAPS_OFFSCREENPLAIN`, 800x600 |
| Surfaces (game) | Primary 640x480; offscreen `OFFSCREENPLAIN|SYSTEMMEMORY` 640x480 (back buffer); offscreen `OFFSCREENPLAIN|SYSTEMMEMORY` **536x582** (scrolling world buffer) |
| Interfaces | Surfaces are QI'd to IDirectDrawSurface3 (`{da044e00-69b2-11d0-a1d5-00aa00b8dfbb}`) |
| Palette / clipper | none (16-bit; `SetPalette(NULL)` only during release). No `Flip`, no `GetAttachedSurface` called |
| Menu frame | draw into the 800x600 offscreen with `Lock(flags DDLOCK_WAIT)` + **`GetDC`/`ReleaseDC`** (GDI text, ~630 times), then `BltFast(primary, 0,0, offscreen, full rect, DDBLTFAST_WAIT)` only when something changed (21 times in the menu) |
| Game frame | per frame: `Lock` 536x582 buffer (`DDLOCK_WRITEONLY` 0x20), 4× `Blt` from it to the 640x480 back buffer (the buffer wraps around: 4 quadrants), `Lock` back buffer (`DDLOCK_WAIT`, sprites/UI by CPU), `WaitForVerticalBlank(DDWAITVB_BLOCKBEGIN)`, `Blt(primary, (0,0)-(640,480), backbuffer)` flags `0x1020200` = `DDBLT_WAIT|DDBLT_ROP|DDBLT_ASYNC` (SRCCOPY). Also `GetBltStatus` |
| Clear | `Blt` with `DDBLT_COLORFILL|DDBLT_ASYNC|DDBLT_WAIT` (0x1000600) on primary / offscreen |

Port implication: no flipping, no palette. Present = copy the primary (RGB565) to the screen after each `Blt`/`BltFast`
to the primary. `WaitForVerticalBlank` is the frame pacer (registry `WaitVBlank`=1). GetDC on a surface needs a GDI
text renderer into RGB565 memory.

## DirectSound

| Item | Value |
| --- | --- |
| Creation | `DirectSoundCreate` (GetProcAddress), `SetCooperativeLevel(hwnd, DSSCL_PRIORITY)` |
| Primary | `DSBCAPS_PRIMARYBUFFER|DSBCAPS_CTRLVOLUME`, `SetFormat` PCM **stereo 22050 Hz 16-bit**, `Play` |
| Secondary | `DSBCAPS_CTRLFREQUENCY|CTRLPAN|CTRLVOLUME|LOCSOFTWARE` (flags 0xE8), PCM **mono 22050 Hz 16-bit**, sizes 33939 … 251792 bytes (one buffer per sample) |
| Use | `Lock` whole buffer once, copy sample, `Unlock`; `DuplicateSoundBuffer` for overlapping plays; `SetVolume`, `SetPan`, `Play(0,0,0)` (one-shot, no looping), `GetStatus` polling |

Music is **not** DirectSound: `_INMM.dll` decodes `MUSIC\TrackNN.ogg` with libvorbisfile and plays it through
`waveOutOpen` from its own thread (the game thinks it is CD audio, registry `CDAudio`=2).

## DirectInput

`DirectInputCreateA` (version 0x0300), one device: **`GUID_SysMouse`** (`{6f1d2b60-d5a0-11cf-bfc7-444553540000}`),
`SetDataFormat(c_dfDIMouse)` (X, Y, Z axes + 4 buttons), `SetCooperativeLevel(hwnd, DISCL_FOREGROUND|DISCL_NONEXCLUSIVE)`,
`SetProperty(DIPROP_BUFFERSIZE, 128)`, `Acquire`, then buffered `GetDeviceData` (16-byte DIDEVICEOBJECTDATA, called
with `DIGDD_PEEK` first). No keyboard device: the keyboard comes from window messages. Created 3× (startup probe, menu, game).

## MCI (intro video)

Sent with `mciSendStringA` (through `_INMM`, forwarded to winmm):

```
open AVIVideo!.\S3\Add\001\01_00_00.dat alias test
status test window handle
window test handle <hwnd of the game window>
play test window notify
close test wait          (after Escape or at the end)
```

So the intro is in `S3\Add\001\` (resolves unknowns #4). The port can skip it (registry `Intro`=0 or answer
`open` with an error), or decode Indeo 5 later.

## Threads and timing

| Thread | Start | Notes |
| --- | --- | --- |
| main (0120) | entry | creates the window (`"Siedler3 Window Class"`, title `S3`, style `0x90C80000`, ex-style `WS_EX_TOPMOST`, 1024x768 = screen size from `GetSystemMetrics`), a hidden `"CDAUDIO CALLBACKWINDOW"`, runs a **`GetMessageA`** loop (only 1 `PeekMessageA`) |
| 2 × at menu start | `CreateThread(…, 0x622805 /*CRT _beginthreadex*/, …, CREATE_SUSPENDED)` + `ResumeThread` | one is the render/UI thread (loads ddraw, Gfx, Snd, fonts, menu); `SetThreadPriority` 2 (HIGHEST) / 0 toggled around loading |
| 3 × at game start | same | game logic, AI (OutputDebugString "COMP_FoodForMines …", "Center - player: 1, group: N"), render |
| `_INMM` | `CreateThread(…, 0x6ad41280, …)` | music |

Sync: `SetEvent` (~74 k), `WaitForSingleObject` (~42 k), `WaitForMultipleObjects` (~38 k), `ResetEvent` (~5 k)
in ~4 min, so the threads hand off every frame. Timing: `QueryPerformanceCounter`/`Frequency`, `Sleep`, and
`CreateWaitableTimerA`/`SetWaitableTimer`/`CancelWaitableTimer` resolved with GetProcAddress (HYPOTHESIS: frame
timer). **`timeSetEvent` was never called** (wine `+mmtime` showed nothing). `timeGetTime` goes through `_INMM`
(native, not visible in the relay log).

C++ exceptions: `RaiseException(0xE06D7363)` (MSVC throw) ~36 times, all caught by the game — normal control flow.
The CPU emulator must support SEH (`fs:[0]` chain) and `RtlUnwind`.

## CD checks / ProtectCD

`ProtectCD.dll` is never loaded (confirms unknowns #2). `GetVolumeInformationA` is never called. `GetDriveTypeA` is
called ~16 times with `"C:\"` and with the odd path `".\\"` (from `0x413840`); it returned `DRIVE_FIXED` (3) and
the game was happy. HLE: return `DRIVE_FIXED` for everything.

## Network at startup

`wsock32.dll` is loaded at startup and `socket, bind, getsockname, htonl, htons, inet_addr, ntohs, inet_ntoa,
WSAGetLastError, gethostbyname, WSAStartup, WSACleanup, sendto, recvfrom, select, shutdown, closesocket` are resolved
with GetProcAddress (HYPOTHESIS: LAN game discovery). A DirectPlay COM object is created (dplayx loaded) once at startup.
Stubs that fail cleanly should be enough for single player.

## Win32 calls from the game (counts, whole session ≈ 4 min: intro, menu, setup, ~1 min in game)

Hot functions were excluded from the trace (TlsGetValue/TlsSetValue, HeapAlloc/Free/ReAlloc/Size,
Enter/LeaveCriticalSection, Interlocked*, Get/SetLastError, IsBad*Ptr, GetCurrentThread(Id), lstrlenA/lstrcpyA,
RtlUnwind) — these are called very often too.

| DLL | Called (count) |
| --- | --- |
| kernel32 | SetEvent 74314, WaitForSingleObject 42187, WaitForMultipleObjects 37839, ResetEvent 4805, QueryPerformanceCounter 1000, Sleep 267, CloseHandle 196, WideCharToMultiByte 155, CreateFileA/GetFileSize/CreateFileMappingA/MapViewOfFile 141 each, GetLocaleInfoW 116, UnmapViewOfFile 94, MultiByteToWideChar 60, GetProcAddress 48, RaiseException 36, SetCurrentDirectoryA 30, OutputDebugStringA 27, MoveFileA 19, GetDriveTypeA 16, CreateEventA 15, GetFileAttributesA 14, FindFirstFileA 13, GetModuleHandleA 12, GetCurrentDirectoryA 12, LoadLibraryA 11, QueryPerformanceFrequency 10, VirtualAlloc 7, SetThreadPriority 7, FindClose 7, CreateThread 6, GetStringTypeW 5, LCMapStringW 5, FormatMessageA 5, LocalFree 5, ResumeThread 5, GetModuleFileNameA 4, GetSystemTime 4, GetStdHandle 3, GetFileType 3, GetCPInfo 3, GetLocalTime 3, GetLocaleInfoA 3, GetStartupInfoA 2, GetTimeZoneInformation 2, CompareStringW 2, SetFilePointer 2, and 1× each: GetVersion, GetVersionExA, HeapCreate, TlsAlloc, SetHandleCount, GetCommandLineA, GetEnvironmentStringsW, FreeEnvironmentStringsW, GetACP, SetUnhandledExceptionFilter, GetUserDefaultLCID, IsValidCodePage, IsValidLocale, CreateMutexA, CreateSemaphoreA, ReleaseSemaphore |
| user32 | IntersectRect 6363, DrawTextA 1364, IsRectEmpty 1233, PtInRect 304, GetWindowLongA 232, GetMessageA 126, TranslateMessage 125, DispatchMessageA 125, GetMessagePos 75, SetCursor 63, LoadCursorA 29, ClientToScreen 22, PostMessageA 19, SetRect 7, BeginPaint/EndPaint 4, ShowCursor 4, PostThreadMessageA 4, SendMessageA 3, GetSystemMetrics 2, LoadIconA 2, CreateWindowExA 2, ShowWindow 2, and 1×: PeekMessageA, RegisterClassA, RegisterClassExA, SetWindowLongA, GetClientRect, MoveWindow, SendMessageTimeoutA, GetDC, ReleaseDC |
| gdi32 | SetTextColor 3043, SelectObject 2940, DeleteObject 2792, CreateRectRgn 2782, SelectClipRgn 2782, SetBkMode 2779, TextOutA 874, GetTextExtentPoint32A 735, GetTextMetricsA 654, MoveToEx 103, LineTo 103, GetTextExtentExPointA 39, CreateFontIndirectA 11, GetStockObject 6, GetDeviceCaps 2 (index 90 = LOGPIXELSY), CreatePen 2, GdiFlush 2, CreateDIBSection 1 |
| advapi32 | RegQueryValueExA 73, RegSetValueExA 17, RegCloseKey 9, RegOpenKeyExA 8, RegCreateKeyExA 4, RegQueryInfoKeyA 1, GetUserNameA 1 |

Never called in this session (from the static import list): CreateProcessA, CopyFileA, DeleteFileA, CreateDirectoryA,
WriteFile, GetTempFileNameA, VirtualProtect, VirtualQuery, ClipCursor, MsgWaitForMultipleObjects, wvsprintfA,
MessageBoxA (only on the DDERR error), DestroyWindow, RegDeleteKeyA, RegEnum*, CoCreateGuid. Saving a game,
multiplayer and campaign were not exercised.

GDI text is drawn on DirectDraw surface DCs: per text item the pattern is `CreateRectRgn` + `SelectClipRgn` +
`SelectObject(font)` + `SetBkMode(TRANSPARENT)` + `SetTextColor` + `TextOutA`/`DrawTextA` + `DeleteObject`.
Fonts: 4 `CreateFontIndirectA` at menu start (face from registry `Font` = "MS Sans Serif", height from `FontSize`)
and 3 more for the game UI.

## Open / not done

- Save/load, campaign, multiplayer not traced.
- `timeGetTime`/`timeBeginPeriod` calls go to `_INMM.dll` (native), so their counts are unknown.
- Which waitable-timer functions are really called (only GetProcAddress seen).
