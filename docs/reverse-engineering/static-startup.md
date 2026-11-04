# S3.EXE startup, window, message loop, threads, frame pacing (static)

Date: 2026-11-05. Source: REA/Ghidra 12.1.2 + objdump on GOG `S3.EXE` (sha256 `00a925cf…`). All addresses are VAs.
Everything here is **static** (not observed at runtime) unless marked otherwise. Helper: `tools/dis.py` (disassembly
with API names), `tools/iat_xrefs.py` (all IAT references).

## 1. Entry → WinMain (standard MSVC6 `WinMainCRTStartup`)

`entry` `0x621bc6`, SEH frame with `_except_handler3` = **`0x622230`**, scope table `0x65de38`:

| Step | Address | MSVC6 CRT name (by shape) |
| --- | --- | --- |
| `GetVersion` → `_osver/_winver/_winmajor/_winminor` at `0x7e9350..5c` | | |
| `FUN_00628496(1)` | `0x628496` | `_heap_init` (`HeapCreate`; small-block heap uses `VirtualAlloc` at `0x628be5/0x628c71`) |
| `FUN_006228d3` | `0x6228d3` | `_mtinit` (`TlsAlloc`, per-thread data `_ptd`, 0x74 bytes) |
| `FUN_006232b1` | `0x6232b1` | `_ioinit` (`GetStartupInfoA`, `GetStdHandle`, `GetFileType`, `SetHandleCount`) |
| `GetCommandLineA`, `FUN_00629f98` | | `__crtGetEnvironmentStringsA` |
| `FUN_00629d4b`, `FUN_00629c92` | | `_setargv`, `_setenvp` |
| `FUN_0061f06a` | `0x61f06a` | `_cinit` → runs C++ static constructors (thousands; e.g. global `CString`s and float constants) |
| `GetStartupInfoA`, `FUN_00629c3a` | | `_wincmdln` |
| **`FUN_0045a120(GetModuleHandleA(0), 0, cmdline, nShow)`** | `0x45a120` | **WinMain** |
| `FUN_0061f097(ret)` | `0x61f097` | `exit` (atexit table, then `ExitProcess`) |

WinMain `0x45a120`:
1. `GetSystemTime`; **if `wYear > 2037` → MessageBox "The Settlers III cannot be started…" and exit**
   (HLE: return a sane date).
2. `setlocale(LC_ALL, "")` (`0x620001`).
3. Construct the application object (`0x4043d0`, `this` = global; virtual inheritance, `CThread` base).
4. `FUN_0060f000` → `0x60e2d0` = `CThread::Execute` on the **main thread**: state=5, `vtbl+0x10` **Init**
   (`0x40b3f0` via thunk `0x40a790`), `vtbl+0x14` **Run** (`0x404860`), `vtbl+0x18` **Exit** (`0x404b10`).
5. Destructors (`0x60bfe0`×3, `0x5b59d0`, `0x610330`, `0x60dff0`), return exit code.

Init `0x40b3f0` (main thread): version banner (`"Siedler3 Version %i.%i: (%x,%x)"`, `0x7a8000/4` from `0x60eff0`),
`0x60ca30`, settings (`0x40d700`, registry, see §6), `GetModuleFileNameA`, command line (`0x410da0`: options
`-messe` = 640x480 trade-show mode, `-mouse`, …), `SetThreadPriority(main, 2 = HIGHEST)` (`0x60e9d0` called at
`0x40b9cf`), window + DirectX setup (`0x405b00`/`0x404d..`), `0x49d8f0`, `0x5b5a30`.

## 2. Window

Generic window class code (namespace "DirectX"/`Win32` library, `0x60ff00`, called from `0x45ae40` and `0x5b5db1`):

- `RegisterClassA` with WNDCLASS from virtual `GetWndClass` (default `0x60ffa0`): `style=0`, **`lpfnWndProc =
  0x610040`**, `cbClsExtra=0`, `cbWndExtra=0` (the S3 subclass sets extra bytes, `DAT_007e9170` = window-long index),
  `hIcon=LoadIconA(0, IDI_APPLICATION)`, `hCursor=LoadCursorA(0, IDC_ARROW)`, `hbrBackground=6`,
  **class name `"CWindow"`** (`0x7a4598`).
- `CreateWindowExA` from a CREATESTRUCT filled by virtual `Create`:
  - S3 window (`0x45ade0`, vtable `0x657238`): **exStyle `WS_EX_TOPMOST` (8)**, **style `0x90C80000` =
    `WS_POPUP|WS_VISIBLE|WS_CAPTION|WS_SYSMENU`** (set at `0x45ace7`), x=y=0,
    **cx=`GetSystemMetrics(SM_CXSCREEN)`, cy=`GetSystemMetrics(SM_CYSCREEN)`**, no parent/menu, title from the object
    (default `""`). `ShowCursor(FALSE)` if `0x7a8018` set.
  - Base DirectX window (`0x5b5d50`, vtable `0x65c9c8`): same but style `0x90080000` (`WS_POPUP|WS_VISIBLE|WS_SYSMENU`).
  - `ShowWindow(hwnd, SW_SHOW)` at `0x60fe46`.
- **WndProc `0x610040`**: `GetWindowLongA(hwnd, idx)`; on the first message (`WM_GETMINMAXINFO`/`WM_NCCREATE`, object
  not yet attached) it stores the creating object (`0x7e9174`) with `SetWindowLongA` and sets `obj->hwnd`; then calls
  virtual `obj->vtbl[+0xC](hwnd, msg, wParam, lParam)`. S3: `0x45c820` → `vtbl[+0x78]` = message switch **`0x45c860`**.
- Messages handled by `0x45c860` (everything else → `DefWindowProcA`):
  `WM_ACTIVATEAPP 0x1C`, `WM_SETCURSOR 0x20`, `WM_DISPLAYCHANGE 0x7E`, `WM_NCPAINT 0x85`, `WM_NCACTIVATE 0x86`,
  `WM_NCMOUSEMOVE 0xA0`, `WM_NCLBUTTON*/NCRBUTTON* 0xA1–0xA6`, `WM_KEYDOWN 0x100`, `WM_KEYUP 0x101`, `WM_CHAR 0x102`,
  `WM_SYSKEYDOWN 0x104`, `WM_SYSKEYUP 0x105`, `WM_MOUSEMOVE 0x200`, `WM_LBUTTONDOWN/UP/DBLCLK 0x201–0x203`,
  `WM_RBUTTONDOWN/UP/DBLCLK 0x204–0x206`, `MM_MCINOTIFY 0x3B9` (music/video end), `WM_APP+2..+4 0x802–0x804`
  (internal). **So keyboard comes only via window messages; mouse comes via window messages and DirectInput.**
- Second window class: `RegisterClassExA` `0x615831` + `CreateWindowExA` `0x6159e4` (module `0x615xxx`; HYPOTHESIS:
  hidden helper window of the lobby/network code — not on the main menu path).

## 3. Main-thread loop (input + message pump)

Run `0x404860` (main thread, priority HIGHEST):
```
loop:
  while game_active (0x4120d0):
      vtbl call (… +8)(-1, 0xFF)                       ; HYPOTHESIS: signal/refresh
      while DI mouse has data (0x5ba260):              ; DirectInput event handle signalled
          n = GetDeviceData(buf, 2, 0) (0x5ba290)      ; DIDEVICEOBJECTDATA, 16 bytes each, 2 per call
          for each: 0x45ce10(obj)                      ; mouse event → game
          0x45ce90()
      0x45ce90()
      while PeekMessageA(&m,0,0,0,PM_NOREMOVE) (0x60d9e0 → 0x60d9f0):
          if GetMessage-step (0x60d740) == WM_QUIT: 0x45d2a0(); PostQuitMessage(code)
  if 0x5b8c40(): drain DI buffer
  if 0x60d740() == WM_QUIT: return wParam               ; blocking GetMessageA
```
`0x60d740` = one `GetMessageA(&m, this->hwndFilter, 0, 0)` + `TranslateMessage` + (thread messages with `hwnd==0` go
to `vtbl+8`) + `DispatchMessageA`. Returns 1 on `WM_QUIT`.

The four `PeekMessageA` sites:

| Site | Function | Purpose |
| --- | --- | --- |
| `0x60d6d5` | `0x60d6d0` | idle loop: `while(!PeekMessage(NOREMOVE)) vtbl[+4]()` (OnIdle); then `0x60d740` |
| `0x60d9f0` | `0x60d9e0` | "message waiting?" test used by Run above |
| `0x60f452` | `0x60f430` | `PeekMessage(NOREMOVE)` at the start of a new thread → **creates the thread message queue** (classic trick), then signals the creator (`0x42cf80` = `SetEvent`) |
| `0x60f893` | `0x60f880` | same, other thread class |

HLE consequence: each guest thread needs its own message queue (`PostThreadMessageA` is imported, 1 site);
`PeekMessage` with `PM_NOREMOVE` must create it.

## 4. Threads

All threads go through CRT **`_beginthreadex` = `0x62279a`** (the only `CreateThread` caller, `0x6227db`);
`_threadstartex` = `0x622805` (sets TLS `_ptd`, installs SEH, calls `[ptd+0x48](arg)`, then `_endthreadex`).
`_beginthreadex` has exactly one caller: **`CThread::Start` `0x60e060`**:
`_beginthreadex(NULL, 0x1000 /*stack commit hint*/, 0x60ec20, this, CREATE_SUSPENDED, &this->tid)`, then
`ResumeThread`. Thread proc **`0x60ec20`**: log `"Thread gestartet"`, `this->vtbl[+0x1C]()` (thread body), log
`"Thread beendet"`, return. Priorities via `SetThreadPriority` wrapper `0x60e9d0`.

`CGameThread::Start` = `0x5b6130` (passes the app's message thread as parent); per-thread init `0x5b6160`
(`CoInitialize`, see `"CGameThread::~CGameThread:: iCoInitOpen != 0"`).

| Thread (RTTI) | Created at | Object size / ctor | Pacing |
| --- | --- | --- | --- |
| Main (app, `CConsoleThread`?) | process main thread | `0x4043d0` | blocking `GetMessageA` / DirectInput event |
| `CLogicThread` (`lpLogicThread`) | `0x4058cc`, started `0x405954` | 0x678 B, `0x53c5e0` | **periodic timer 64 ms** (`0x53d761`: `SetPeriodic(0x40)`), priority +1 (`0x53d743`) |
| `CDrawThread` / `DirectX::CGameDrawThread` (`lpDrawThread`) | `0x40583f` | 0x74 B, `0x42c2d0` | draw as fast as possible; `WaitForVerticalBlank(DDWAITVB_BLOCKBEGIN)` if `WaitVBlank` (§DirectDraw); `Flip` retry `Sleep(0)` |
| `CNetThread` (`lpNetThread`) | `0x4057c0`, started `0x40582c` | `0x46f4e0` | periodic 500 ms? (`0x493617`: `SetPeriodic(0x1F4)`) |
| `Menus::CNetThread` (`lpMenuNetThread`) | `0x4055a6` area, started `0x4055b6` | | periodic **500 ms** (`0x45f3b7`) |
| `Menus::CDrawThread` (`lpMenuDrawThread`) | `0x40553f` area | | menu rendering |
| `Menus::priv::CFTPThread`, `CNetBounceThread`, `LobbyClient::priv::CHelperThread` | network/lobby only | | |

HYPOTHESIS: in the main menu the running threads are main + `Menus::CDrawThread` + `Menus::CNetThread`; in a game
main + `CLogicThread` + `CDrawThread` + `CNetThread`. Threads talk through events (`SetEvent`/`PulseEvent` 11 sites/
`ResetEvent`), semaphores, critical sections and `PostThreadMessageA`.

### Waiting / timers (`Win32::CWin32Waitable`, `CWin32WaitableBag`)

- Generic thread loop `0x60da30` (`CMessagedThread`): builds a handle array from its waitable bag and loops on
  **`MsgWaitForMultipleObjects(n, handles, FALSE, timeout, wakeMask)`** (`0x60db22`); index `n` = message → `0x60d740`;
  `WAIT_TIMEOUT (0x102)` → throws (`"CWin32Waitable::WaitForXYZ"`); otherwise dispatches signalled object `0x463db0`.
  Same with no message queue: `0x60dcd0` (`MsgWaitFor…` at `0x60ddaf`) and `WaitForMultipleObjects` at `0x4631db`.
- Timer object (`0x60ce..`): at construction it does `GetProcAddress(kernel32, "CreateWaitableTimerA" /
  "OpenWaitableTimerA" / "SetWaitableTimer" / "CancelWaitableTimer")` (`0x60ceb5`–`0x60cee8`).
  - **If `CreateWaitableTimerA` is found**: waitable-timer path. `SetPeriodic(ms)` `0x60d3a0` →
    `0x60d0a0`: `SetWaitableTimer(h, &due, period=ms, NULL, NULL, resume)` with **`due = +ms*10000`** (positive =
    absolute time in 1601 → already past → fires immediately, then every `ms`).
  - **Else (Win95 path)**: `timeBeginPeriod(res)` (`0x60cef5`), event object, and
    `timeSetEvent(ms, 0, 0x60d640, this, TIME_PERIODIC)` (`0x60d34c`); callback `0x60d640` → `SetEvent` (`0x42cf80`).
  - HLE choice: returning NULL from `GetProcAddress("CreateWaitableTimerA")` selects the `timeSetEvent` path
    (needs a host timer thread that sets an event). Providing waitable timers is equally simple. Either way the
    logic tick is **64 ms (15.6 ticks/s)** — HYPOTHESIS: game speed setting changes this period.
- `timeGetTime` is called from 31 sites (animation, scrolling, network timing): needs millisecond resolution.
  `QueryPerformanceCounter/Frequency` only in the blit benchmark `0x5c4280`. `GetTickCount` 2 sites.

### Android lifecycle (pause/resume) — HYPOTHESIS

Pausing = stop scheduling all guest threads and stop the host timer thread; on resume, the 64 ms timer simply
continues. Because `due` is relative to "now" only at arm time, no catch-up burst happens with `timeSetEvent`.

## 5. Exceptions (SEH + C++ EH) — how much emulation is needed

- **C++ exceptions are used for normal error handling** (DirectX failures, waits, file errors):
  `__CxxThrowException@8` = **`0x61f027`** (149 call sites, 69 of them rethrow `throw;`), which calls
  `RaiseException(0xE06D7363, EXCEPTION_NONCONTINUABLE, 3, {magic, pObj, pThrowInfo})` (`0x61f055`).
  `__CxxFrameHandler` = **`0x61ec26`**; 1 653 per-function stubs `mov eax, FuncInfo; jmp 0x61ec26`;
  1 661 functions push an SEH record on `fs:[0]`. Example: `WaitForVerticalBlank` failure is caught in the draw
  thread (`0x42cddb`) and the catch block turns the option off.
- Plain SEH `__try/__except` with **`_except_handler3` `0x622230`** (24 frames): CRT startup, thread start
  `0x622805`, CPUID/MMX probes `0x5c3900`/`0x5c3970`, others.
- `RtlUnwind` (thunk `0x62ef78`) is used by both handlers; `RaiseException` also at `0x62e3bf` (CRT);
  `SetUnhandledExceptionFilter` `0x624b7f/0x624b91`, `UnhandledExceptionFilter` `0x629bf6`.
- **Needed in the HLE**: a real Win32 SEH dispatcher — `RaiseException` builds `EXCEPTION_RECORD` + `CONTEXT` on the
  guest stack, walks the `fs:[0]` chain calling each guest handler (`handler(rec, frame, ctx, dispctx)`),
  honours `ExceptionContinueSearch/ContinueExecution`; `RtlUnwind(targetFrame, targetIp, rec, retval)` calls handlers
  with `EH_UNWINDING` (flag 2) and pops `fs:[0]`. The CRT's `__CxxFrameHandler` then does all C++ work (catch
  matching, destructors, `_JumpToContinuation`) in guest code — no C++ knowledge needed in the host.
  Hardware faults (#DE, access violation) only need to be turned into SEH exceptions if we want the game's own
  error dialogs; for bring-up, abort with a host log.

## 6. Kernel32 hot path vs rare (static call-site counts, `call [IAT]` only; `mov reg,[IAT]` sites come on top)

Hot (per frame / per tick / per allocation): `timeGetTime` (31), `EnterCriticalSection`/`LeaveCriticalSection`
(game + CRT locks), `SetEvent`/`PulseEvent`/`ResetEvent`, `WaitForSingleObject`, `WaitForMultipleObjects`,
`MsgWaitForMultipleObjects`, `PeekMessageA`/`GetMessageA`/`DispatchMessageA`, `InterlockedIncrement/Decrement/
Exchange`, `HeapAlloc`/`HeapFree`/`HeapReAlloc` (via CRT `malloc`; MSVC6 small-block heap sits on top),
`TlsGetValue` (CRT `_getptd`, every `errno`/`rand`/`strtok`), `GetLastError`/`SetLastError` (146 sites, error checks
after every API), `Sleep(0)` (Flip retry), `OutputDebugStringA` (35 sites, debug log `0x428720`).
File I/O during loading: `CreateFileA`, `ReadFile`, `SetFilePointer`, `GetFileSize`, `CloseHandle`, `FindFirstFileA/
FindNextFileA/FindClose`, **`CreateFileMappingA` + `MapViewOfFile`** (GFX `.dat` files are memory-mapped by the
`CMappedFile` class `0x61a5xx`/`0x61a80c` and pre-faulted with `VirtualProtect`/`VirtualQuery` page-touch helper
`0x61ab60`, called from the GFX loader `0x5fa833`, `0x5ff2a6` and `0x61b358`, `0x61b4e1`; one more mapping at
`0x5a9d17`).
Rare (startup/shutdown/locale): everything else (`GetVersion`, `GetStartupInfoA`, locale/codepage functions,
environment, `CreateProcessA` (patch converter `565to555`, `ShellExecuteExA`), `GetTempPathA`, `CopyFileA`, …).
