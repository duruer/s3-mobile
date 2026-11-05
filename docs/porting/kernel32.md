# KERNEL32 HLE and the runtime pieces under it

Code: `port/crates/s3-win32/src/kernel32.rs` (+ `kernel32_{sync,file,time,locale}.rs`, tests in
`kernel32_tests.rs`), runtime support in `port/crates/s3-runtime` (`machine.rs` waits, `handles.rs`, `vfs.rs`,
`mapfile.rs`, `hosttime.rs`, `resources.rs`, `seh.rs`). Status 2026-11-05: all 130 static KERNEL32 imports plus the
GetProcAddress targets have handlers; S3.EXE reaches the main menu and Game Setup headless (see "Integration").

## Identity the guest sees

| Question | Answer | Why |
| --- | --- | --- |
| OS version | Windows 98: `GetVersion` = `0xC0000A04`, `GetVersionExA` = 4.10 build 1998, platform 1 (`" A"`) | game targets Win9x; consistent with the foundation choice |
| CPU / system | 1 × Pentium (586, level 5), 4 KiB pages, 64 KiB granularity, user range 0x10000-0x7FFEFFFF | `GetSystemInfo` |
| Memory | 256 MiB RAM, 192 MiB free (`GlobalMemoryStatus`; all values < 2 GiB for 32-bit math) | |
| Locale | LCID 0x0409, ANSI cp 1252, OEM cp 437; `GetLocaleInfoA/W` table for en-US (incl. `LOCALE_RETURN_NUMBER`) | game language comes from registry `Language` (registry.md), not the locale — HYPOTHESIS |
| Install dir | `C:\S3`, exe `C:\S3\S3.EXE`, command line `S3.EXE` (+ env `S3_EXE_ARGS` if set, debug only, e.g. `-netmon`), cwd `C:\S3` | VFS (architecture.md) |
| Drives | `GetDriveTypeA` = DRIVE_FIXED for every path, `GetLogicalDrives` = C: only, volume label `S3`, FAT32, 1 GiB free | wine-trace: fixed drive is accepted; no CD check |
| Temp dir | `C:\S3\TEMP\` (overlay) | |
| HLE DLL paths | `GetModuleFileNameA(hHLE)` = `C:\WINDOWS\SYSTEM\<NAME>.DLL` | |
| Std handles | fake `0x10/0x14/0x18`, `GetFileType` = FILE_TYPE_CHAR, writes go to the log | MSVC CRT `_ioinit` probes them |
| Time | `GetTickCount` = host monotonic ms; QPC = 1 MHz host µs, strictly increasing; wall clock from the host with the host's UTC offset as time zone bias (no DST rules, TIME_ZONE_ID_UNKNOWN) | QPC must advance (blit benchmark 0x5c4280) |

## Threads, waits, timers (cooperative scheduler)

- `CreateThread` → `Machine::create_thread` (own stack + TEB; `CREATE_SUSPENDED` honoured). The stack argument is the
  initial **commit** like on Windows: the stack size is the PE reserve (1 MiB), or the argument rounded up to 1 MiB
  when it is larger, or the argument itself with `STACK_SIZE_PARAM_IS_A_RESERVATION` (0x10000). S3.EXE's
  `_beginthreadex` passes 0x1000; until 2026-11-05 that gave 64 KiB stacks, and the save-slot reader (256 KiB of
  locals, `FUN_004a5c80` via `_chkstk`) ran below the stack, so its SEH frames failed the frame check and the
  CFileNotFound exception for a missing `slot N.sav` became "unhandled" (crash when opening the Save menu).
  `ResumeThread`/`SuspendThread` count, `GetExitCodeThread`, `TerminateThread` (other threads), priorities are stored only.
- A blocking call returns `ThunkResult::Block`; the scheduler retries the handler every round. Generic wait:
  **`Machine::poll_wait(handles, wait_all, timeout_ms) -> WaitPoll::{Ready(i), Timeout, Failed, Pending}`** — also
  meant for user32 `MsgWaitForMultipleObjects` (check the message queue first, then call `poll_wait`; on `Pending`
  return `Block`). Timeouts count from the first call (`Thread::block_since`).
- Objects: events (manual/auto), mutexes (owner + recursion, `ERROR_NOT_OWNER`), semaphores (`ERROR_TOO_MANY_POSTS`),
  thread handles, **waitable timers** (`KernelObject::Timer`, due time in host ms, periodic re-arm, missed periods are
  not replayed). Named objects share one namespace (`ERROR_ALREADY_EXISTS` when re-created).
- `PulseEvent` releases blocked waiters (all for manual-reset, one for auto) via `Thread::pulsed`, then leaves the
  event reset (`Machine::pulse_event`).
- **Waitable timers are provided** (CreateWaitableTimerA/OpenWaitableTimerA/SetWaitableTimer/CancelWaitableTimer), so the
  game takes its waitable-timer path (static-startup.md) and does not need `timeSetEvent`. Verified at runtime: both
  menu threads create a timer with period 500 ms (`SetWaitableTimer(h, due, 0x1f4, 0, 0, 0)` from 0x60d0e7).
  Completion routines (APCs) are ignored with a warning.
- `Sleep(n)` always yields (also `Sleep(0)`) and returns after ≥ n host ms.
- Critical sections live in the guest struct (`LockCount` -1 = free, `RecursionCount`, `OwningThread` = tid). A
  contended `Enter` blocks; `Leave` **hands ownership directly** to one blocked waiter (round robin from the leaving
  thread's slot) so a thread looping Enter/Leave cannot starve others under cooperative scheduling. A section owned by
  an exited thread is taken over (logged).
- TLS: 64 slots in the TEB (`TlsAlloc` zeroes the slot in all threads; `TlsGetValue` clears LastError like Windows).
- `DuplicateHandle` turns the `GetCurrentThread`/`GetCurrentProcess` pseudo handles into real handles.

## Files (all through the VFS)

- `CreateFileA` implements all five dispositions with the Win32 error codes (`ERROR_FILE_NOT_FOUND` vs
  `ERROR_PATH_NOT_FOUND`, `ERROR_FILE_EXISTS`, `ERROR_ALREADY_EXISTS` as LastError on success). Read-only opens use the
  game file directly; any write access first copies the file into `data_dir/overlay` (`Vfs::copy_up`); creates go to the
  overlay. Directories open only with `FILE_FLAG_BACKUP_SEMANTICS`. Device names (`CON`, `\\.\`) fail.
- `DeleteFileA` / `MoveFileA` / `RemoveDirectoryA` on files that exist only in the game dir record a **whiteout**
  (`data_dir/overlay-whiteouts.txt`, lower-case game-relative paths); the game dir is never modified. The 19 `MoveFileA`
  calls in `Map\Multi` at startup (`A256-2_Quick3.map` -> `A256-2-Quick3.map` ...) fail with ERROR_FILE_NOT_FOUND
  (wine) or ERROR_ALREADY_EXISTS (our GOG copy has both names); the game ignores the result.
- `FindFirstFileA` lists overlay + game dir merged (case-insensitive, overlay wins), with `.`/`..` first for wildcard
  patterns like Windows; WIN32_FIND_DATAA attributes are DIRECTORY or ARCHIVE, times = host mtime.
- `SetFileAttributesA` accepts and ignores attributes. `GetFileTime` = host mtime/atime; `SetFileTime` is a no-op.
- `S3_FILES=1` logs every CreateFileA / FindFirstFileA / Delete / Move / Copy / Create- and RemoveDirectoryA with the
  guest path, result and LastError at Info (`[files] ...`); filter out `Gfx|Snd|siedler` for readable output.

### Save / load (verified 2026-11-05, desktop headless)

What S3.EXE does (runtime, `S3_FILES=1`; code `FUN_0050ac40`, `FUN_0050b670`, `FUN_00509980`, file class
`FUN_004a5c80`/`FUN_004a5600` -> `FUN_00619610` = CreateFileA wrapper that throws `Win32::CFileNotFound*`, caught in
`FUN_004a5600`):

| Step | Calls |
| --- | --- |
| In-game: computer icon (bottom left) -> panel "Save Game" | reads `C:\S3\Save\slot 1.sav` .. `slot 8.sav` (GENERIC_READ, OPEN_EXISTING); missing slots fail with ERROR_FILE_NOT_FOUND, are caught and shown as empty lines |
| Click a slot line | `CreateDirectoryA("C:\S3\Save")`, `("C:\S3\Save\Multi")` (ERROR_ALREADY_EXISTS: the GOG game dir has `Save\Multi`), then `CreateFileA("C:\S3\Save\slot N.sav", GENERIC_READ\|WRITE, CREATE_ALWAYS)` -> overlay `data_dir/overlay/Save/slot N.sav` (~4.6 MB for a fresh random 384 map), panel says "Game Saved". **No name is asked**: slots are fixed, the list shows the save time + map name |
| Main menu -> Load Game | `FindFirstFileA("C:\S3\Save\*.sav")` (merged overlay + game dir), reads each listed file's header; list shows "Game Clock" (host local time of the save) and "Map Name" |
| Click the entry | reads the slot file, switches to 640x480 and continues the saved game |

File type tag in the header is 0x0b (`FUN_004a5c80` accepts 10 or 11; a checksum over the data follows the tag).
`Save\Multi\*.mps` (the user's multiplayer saves) stay visible through the merged listing; they are used by the
multiplayer load path only (`Save\Multi\`, `Multi\temp.mps`), not by the single-player Load Game list.
Test: `kernel32_tests::save_slots_overlay_and_game_dir`. Evidence (local): `scratch/keep/rt1-saved.png`,
`rt2-menu.png`, `rt3-loadlist.png`, `rt4-loaded.png`.
- **Memory-mapped files**: `CreateFileMappingA` + `MapViewOfFile` `mmap` the host file (MAP_PRIVATE|MAP_FIXED) into a
  64 KiB-aligned guest region (`s3_runtime::mapfile`), so the ~230 MB of GFX files cost no RAM until touched (important
  on Android). Pages wholly past EOF stay anonymous zero pages. Writes through a view are private (not written back) —
  HYPOTHESIS: the game never writes through views. Mapping a size larger than the file does not grow it (logged).
  Pagefile-backed mappings (`INVALID_HANDLE_VALUE`) share one zeroed block between views. `UnmapViewOfFile` puts
  anonymous pages back and releases the region. Falls back to copying when `mmap` fails.

## Memory, process, errors

- `VirtualAlloc/Free/Query/Protect`, `Heap*` map onto `s3-mem` (foundation). `LocalAlloc`/`GlobalAlloc` families are
  fixed blocks from the process heap (handle == pointer). `IsBad*Ptr` check that every page is committed and not in the
  NULL guard.
- `GetModuleHandleA`/`LoadLibraryA`/`GetProcAddress` know the exe and the HLE modules only (other DLLs fail with
  `ERROR_MOD_NOT_FOUND`); `GetProcAddress` returns 0 for names no HLE module declares (logged `not declared`).
- `IsProcessorFeaturePresent` (CRT looks it up via GetProcAddress) matches our CPUID (Pentium P54C): TRUE only for
  `PF_COMPARE_EXCHANGE_DOUBLE` (2) and `PF_RDTSC_INSTRUCTION_AVAILABLE` (8); FALSE for FDIV errata (0), FPU
  emulated (1), MMX (3), SSE/3DNow and unknown features. `user32!GetLastActivePopup(hwnd)` returns hwnd.
- Environment: `GetEnvironmentStrings[W]` return the block built by `process.rs`; `Get/SetEnvironmentVariableA` work on a
  host-side copy (the block is not rebuilt).
- `CreateProcessA` fails with `ERROR_FILE_NOT_FOUND` (only used for the 565→555 converter / external tools);
  `TerminateProcess(self)` = exit.
- `RaiseException` → `seh::raise_exception` (guest-side dispatch over FS:[0]; continuable exceptions return to the
  caller when a handler answers ContinueExecution); `RtlUnwind` → `seh::rtl_unwind`. Tested with hand-assembled
  handlers (nested frames, unwind callbacks, `__except`-style jump to a continuation).
- `SetUnhandledExceptionFilter` stores the filter; `UnhandledExceptionFilter` logs code + address, calls the stored
  filter, otherwise returns EXCEPTION_EXECUTE_HANDLER (the CRT then exits).
- `OutputDebugStringA` → host log (`[guest tN] ...`, first 2000 lines at Info, then Debug).
- `FormatMessageA`: system codes → short English text or `Error N`; `FROM_STRING` copies without inserts (HYPOTHESIS:
  enough for the game's error dialogs); `ALLOCATE_BUFFER` supported.

## Resources

`s3_runtime::resources` (moved from s3-pe, which re-exports it): `find_resource`, `find_resource_entry` (HRSRC = guest
address of the `IMAGE_RESOURCE_DATA_ENTRY`), `load_string`, `ResId`, `rt`. kernel32 `FindResourceA` / `LoadResource` /
`LockResource` / `SizeofResource` use them; user32 `LoadStringA`/`LoadIconA`/`LoadCursorA` should too.

## Strings and code pages

cp1252 tables both ways (0x80-0x9F mapped like Windows; CP_UTF8 honoured); `MultiByteToWideChar`/`WideCharToMultiByte`
with the usual size-query and `ERROR_INSUFFICIENT_BUFFER` semantics, default char + used-default flag;
`GetStringTypeA/W` (CT_CTYPE1 via Unicode classes, CT_CTYPE2 approximate); `LCMapStringA/W` (upper/lower, a simple
sort key: 2 bytes per unit, upper-cased with NORM_IGNORECASE); `CompareStringA/W` (ordinal, optional ignore-case —
not linguistic, HYPOTHESIS: fine for the CRT's `strcoll`/`stricmp` uses); `lstr*`.

## Tests

`cargo test -p s3-win32 -j 4` runs `kernel32_tests.rs` (no game bytes): thread + event handoff, wait timeouts and
WaitForMultipleObjects any/all, named objects, mutex recursion, critical-section mutual exclusion with 2 threads
yielding inside the lock, SEH raise + RtlUnwind of nested frames, continuable RaiseException, PulseEvent, Sleep, TLS,
periodic waitable timer, QPC monotonicity, files (case-insensitive read, overlay writes, cwd, find, move/delete
whiteouts, mapped views), save slots in the overlay next to the game dir's `Save\Multi`, CreateThread stack
reserve, version/module/locale/string conversions/memory checks.

## Integration (headless, 2026-11-05)

With kernel32 complete, the CRT start-up and WinMain run through; the next stops were in other modules
(`ole32!CoInitializeEx`, `wsock32!htons/htonl/WSACleanup`, `winmm!mciSendCommandA`, `ole32!CoCreateGuid`). With
temporary local stubs for those (not committed), S3.EXE reaches the **main menu** (800x600x16, 3 threads, the intro
video fails with the game's own "Error during video play" box because MCI was stubbed to fail) and, with injected clicks,
**Game Setup**, and with two more clicks (map OK, setup OK, lobby OK) an **actual in-game map** (Random 384, 640x480x16,
6 guest threads, 8.1 G instructions / 150 s, ~2000 presents, 55 caught C++ exceptions, ~10 k WaitForSingleObject /
7 k SetEvent per 150 s). 44+ GFX files are memory-mapped at startup; menu threads are woken by 500 ms waitable timers.
After the temporary stubs were removed (the owning agents had implemented those functions meanwhile) a plain
headless run reaches the menu again without stopping.
