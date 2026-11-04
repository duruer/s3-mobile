# USER32, GDI32, ADVAPI32, WINMM and the small DLLs (HLE design)

Status: implemented 2026-11-05 (task `user-gdi-misc`). Code: `port/crates/s3-win32/src/`
(`user32.rs`, `gdi32.rs`, `font.rs`, `advapi32.rs`, `registry_store.rs`, `winmm.rs`, `audio_music.rs`, `ole32.rs`,
`rpcrt4.rs`, `shell32.rs`, `wsock32.rs`, `wininet.rs`, `comctl32.rs`, `comdlg32.rs`; tests in `user_gdi_tests.rs`).
General HLE rules: [architecture.md](architecture.md).

**First results (2026-11-05, headless `s3run` against the GOG install):**

- The game reaches the **main menu**, with all GDI text drawn by our font.
- An injected click on "Single Player: Scenario" opens **Game Setup** with no serial-number prompt.
- Clicking OK through the setup and lobby screens (`--inject "25000:click 165 235;45000:click 630 467;60000:click 730
  567;90000:click 730 567"`) starts a **random 384 map in game** at 640x480. Settlers, buildings and UI are
  visible after ~220 s wall time (10.7 G instructions).
- The game then plays CD track 7 through MCI (`play tracks 7..8`, notify to the CDAUDIO window), the same track
  as in the wine reference run.

## USER32

| Area | Behaviour |
| --- | --- |
| Classes | `RegisterClassA/ExA` (atom `0xC000+n`, also looked up as `#atomXXXX`), `UnregisterClassA` |
| Windows | `CreateWindowExA` sends `WM_GETMINMAXINFO`, `WM_NCCREATE` (FALSE = fail), `WM_NCCALCSIZE`, `WM_CREATE` (-1 = fail), `WM_SIZE`, `WM_MOVE`, all via `call_guest` on the creating thread. With `WS_VISIBLE` (or later `ShowWindow`) it sends `WM_SHOWWINDOW`. The **first visible top-level window** also gets `WM_ACTIVATEAPP(1)`, `WM_NCACTIVATE`, `WM_ACTIVATE(WA_ACTIVE)` and `WM_SETFOCUS`, and becomes active and focused. HWNDs are `0x10010 + 0x10*n`; the desktop is `0x10008` |
| Geometry | Windows sit at (0,0). The client area is the whole window rect (no caption or borders, HYPOTHESIS: harmless under exclusive full-screen DirectDraw). `CW_USEDEFAULT` means full screen. `GetSystemMetrics(SM_CX/CYSCREEN)` returns the current display mode (default 800x600) |
| Display mode | s3-dx calls `user32::set_screen_size(m, w, h, bpp)` on `SetDisplayMode`. Full-screen top-level windows are resized and get a **posted** `WM_DISPLAYCHANGE` (HYPOTHESIS: Windows sends it synchronously) |
| Window longs | `GWL_WNDPROC/HINSTANCE/HWNDPARENT/ID/STYLE/EXSTYLE/USERDATA` plus `cbWndExtra` bytes (the S3 WndProc `0x610040` keeps its object pointer there) |
| Queues | One queue per guest thread (by tid), created on demand. `PeekMessage(PM_NOREMOVE)` creates it too (S3 uses this trick). Messages go to the queue of the thread that owns the window. `PostMessage(0, …)` is a thread message, `HWND_BROADCAST` posts to all top-level windows |
| Retrieval | Order: posted messages (with hwnd and range filter), then `WM_QUIT` (from `PostQuitMessage`), then `WM_PAINT` (a visible window that needs painting; the flag is cleared when the message is taken, so a WndProc that ignores it cannot cause a loop), then `WM_TIMER` (`SetTimer`; a timer proc goes in lParam and `DispatchMessage` calls it) |
| Blocking | `GetMessageA` and `WaitMessage` return `ThunkResult::Block` when nothing is queued. `MsgWaitForMultipleObjects` takes a signaled handle first, then returns `WAIT_OBJECT_0+n` if any message is queued (no "new since last check" rule), and honours its timeout. Host quit makes `GetMessage` return `WM_QUIT` |
| Sent messages | `SendMessageA/TimeoutA` call the WndProc directly on the calling thread, also for windows owned by other threads (HYPOTHESIS: good enough for S3). No WndProc means our `DefWindowProc` |
| DefWindowProc | `WM_NCCREATE`→1, `WM_ERASEBKGND`→1, `WM_NCACTIVATE`→1, `WM_NCHITTEST`→HTCLIENT, `WM_PAINT` validates, `WM_CLOSE`→DestroyWindow, Alt+F4 posts `WM_CLOSE`, everything else 0 |
| Input | Service `user32-input`, also run at every message call. New `HostEvent`s from `m.input` become `WM_MOUSEMOVE` (coalesced), `WM_L/R/MBUTTONDOWN/UP` (plus `DBLCLK` when the class has `CS_DBLCLKS`, within 500 ms and 4 px), `WM_KEYDOWN/UP` (`WM_SYSKEY*` while Alt is held or for F10) and `WM_CHAR`. They go to the capture window, else the active window. Mouse wParam = MK_* flags, lParam = client coordinates. Key lParam = repeat 1, set-1 scan code from a VK table, extended bit, bit 29 for Alt, bits 30/31 on key-up. Events from before the first window are dropped |
| Chars | `TranslateMessage` builds `WM_CHAR` from `WM_KEYDOWN` with a US layout (Shift, Caps Lock, Ctrl+letter). It stops doing this once the host has sent a `HostEvent::Char`, to avoid duplicate characters |
| Key state | `GetAsyncKeyState` and `GetKeyState` read `m.input` (0xFFFF8000 when down; toggle bit for Caps, Num and Scroll Lock). `GetKeyboardState`, `MapVirtualKeyA` |
| Cursor | `SetCursor` and `ShowCursor` only keep state (frontends may read `UserState::cursor/cursor_count` later). `GetCursorPos` is the host mouse position clamped to `ClipCursor`. `SetCursorPos` moves only the emulated position. `LoadCursorA/LoadIconA/LoadImageA` return fake handles `0xCC0000+id` |
| Painting | `BeginPaint/EndPaint/GetDC/ReleaseDC` give **pixel-less** window DCs (drawing on them is a no-op; S3 draws on DirectDraw surface DCs). `InvalidateRect/ValidateRect/UpdateWindow` |
| Text | `DrawTextA`: `DT_LEFT/CENTER/RIGHT`, `DT_VCENTER/BOTTOM` (single line), `DT_WORDBREAK` (also splits words longer than the line), `DT_SINGLELINE`, `DT_CALCRECT`, `DT_NOCLIP`, `&` prefixes (`DT_NOPREFIX`). Returns the text height |
| Misc | Rect helpers (`SetRect`, `IntersectRect`, `PtInRect` with POINT by value = 3 stack args, …), `wsprintfA` (cdecl) and `wvsprintfA` (flags `-0#`, width, precision, `h/l`, `d i u x X c C s S %`, 1023-char limit). `MessageBoxA` logs at Warn level and answers IDOK/IDYES/IDIGNORE, or IDCANCEL for retry/cancel boxes. `GetSysColor`, `SystemParametersInfoA(SPI_GETWORKAREA)`, `CharUpper/LowerA` |

Exposed for other crates: `user32::screen_size`, `set_screen_size`, `post_message`, `send`, `pump_input`, `def_window_proc`.

## GDI32

- **Surface DCs (the interface with s3-dx).** `s3_runtime::gdi` (`port/crates/s3-runtime/src/gdi.rs`) defines
  `DcTarget { addr, pitch, width, height, bpp, rgb565 }`. In `IDirectDrawSurface::GetDC`, s3-dx calls
  `s3_runtime::gdi::create_dc(m, target)` on the locked surface memory. In `ReleaseDC` it calls
  `s3_runtime::gdi::release_dc(m, hdc)`. `gdi32::register` installs the implementation as a process-global hook.
- **Objects.** GDI handles are `0x00C0_0000 + 4n`; stock objects are `0x00BF_0000 + index`. Supported: fonts, solid
  brushes, pens, rectangular regions, bitmaps (DIB sections, compatible bitmaps) and palettes (dummies).
  `SelectObject` returns the previous object of the same kind. New DCs hold the stock 1x1 `DEFAULT_BITMAP`
  (`0x00BF_0015`), so the first `SelectObject(memDC, bitmap)` returns non-zero (S3.EXE at `0x4a8233` throws a C++
  exception -> "abnormal program termination" if it gets 0; found by integration r1 when opening the build menu).
  Selecting a region sets the clip. `DeleteObject`
  never deletes stock objects or DCs. `SaveDC/RestoreDC`.
- **Clipping.** `CreateRectRgn` + `SelectClipRgn` (the region is copied, NULL removes clipping) and `IntersectClipRect`.
  Every drawing call clips to the target bounds ∩ clip rect.
- **Text.** `TextOutA` and `ExtTextOutA` (`ETO_OPAQUE` fills the rect with the bk colour, `ETO_CLIPPED`) honour
  `SetTextAlign` (`TA_LEFT/RIGHT/CENTER`, `TA_TOP/BOTTOM/BASELINE`, `TA_UPDATECP`), `SetBkMode` (OPAQUE fills
  the text cell), `SetTextColor/SetBkColor` and the underline flag. Glyph coverage is alpha-blended into
  RGB565/555/24/32-bit targets (8-bit: threshold). Strings are Windows-1252.
- **Metrics.** `GetTextMetricsA` (TEXTMETRICA 56 bytes), `GetTextExtentPoint32A`, `GetTextExtentExPointA`
  (fit count + partial extents), `GetTextFaceA`.
- **Drawing.** `MoveToEx/LineTo/Polyline` (Bresenham, end point excluded, pen colour, `PS_NULL`), `Rectangle`,
  `FillRect` (also registered in user32, accepts `COLOR_x+1`), `PatBlt` (PATCOPY/BLACKNESS/WHITENESS),
  `Set/GetPixel`, `BitBlt/StretchBlt` (SRCCOPY with format conversion, nearest-neighbour stretch), `Ellipse`
  (filled bounding box, HYPOTHESIS: unused).
- **DIBs.** `CreateDIBSection` allocates the bits on the guest process heap. Bottom-up DIBs use a negative pitch.
  `BI_BITFIELDS` with green mask 0x7E0 means RGB565. `CreateCompatibleDC` + `SelectObject(bitmap)` draws into the
  bitmap.
- `GetDeviceCaps`: 16 bpp, 96 dpi, `NUMCOLORS` -1, no `RC_PALETTE`, HORZRES/VERTRES = target or screen size.

### Fonts (`font.rs`)

The bundled font is **Liberation Sans** Regular/Bold (`port/crates/s3-win32/assets/fonts/`, SIL OFL 1.1, license
file next to it; copied from the system `ttf-liberation`). It is metric-compatible with Arial, which is close to
MS Sans Serif. It is rasterized with **fontdue**, and glyphs are cached per (size, bold, char).

- Every face name maps to it. Weight >= 600 uses the bold face.
- `lfHeight < 0` = em size in px, `> 0` = cell height (ascent+descent), `0` = 13 px cell.
- The registry `Font`/`FontSize` = "MS Sans Serif"/15 gives tmHeight 15.
- Advances are rounded per glyph like GDI; there is no kerning.

Observed: menu buttons, "Version 1.60" and the Game Setup labels render cleanly
(`scratch/ugdi-shot.png`, `ugdi-click.png`, local only). HYPOTHESIS: the original bitmap MS Sans Serif is a little
narrower. If long labels overflow, scale `px` down (one place: `font::metrics`).

## ADVAPI32 registry (`registry_store.rs`, `advapi32.rs`)

Lazy load on the first Reg* call. Later sources override earlier ones:

1. Built-in defaults: the `regs.cmd` values from [registry.md](../reverse-engineering/registry.md) **without**
   the serial number, plus `General\Playername`, `...\1.0\Patches\Convert`, and
   `HKCR\CLSID\{0e9f8400-d9a5-11cf-8f4d00804880b189}\Seed = 0x6a0f`.
2. **`regs.cmd` from the user's game dir, parsed at runtime.** It supplies `SingleNumber`; we never embed the
   serial. `%locpath%` becomes `C:\S3`.
3. `data_dir/registry.json`: values the game wrote. The file is pretty-printed, one object per key path, with
   `{"type": "dword"|"sz"|"binary"|n, "data": ...}`.

Rules:

- Lookups are case-insensitive. `HKLM\SOFTWARE\Wow6432Node\X` is folded to `HKLM\SOFTWARE\X`.
- Key handles are kernel handles (`KernelObject::Other("regkey")`). Predefined HKEYs are roots.
- `RegQueryValueExA` follows the size protocol: NULL buffer = size only; too small = `ERROR_MORE_DATA` (234) plus
  the needed size; missing value = 2.
- `RegSetValueExA` saves the JSON at once (atomic rename).
- A write of `General\SingleNumber` is **ignored when a value exists**. The game rewrites it encrypted and the GOG
  exe cannot decode that (see [wine-trace.md](../reverse-engineering/wine-trace.md)).
- Also implemented: `RegCreateKeyExA` (disposition), `RegOpenKeyA`, `RegCreateKeyA`, `RegQueryValueA`,
  `RegSetValueA`, `RegDeleteKeyA`, `RegDeleteValueA`, `RegQueryInfoKeyA`, `RegEnumKeyExA`, `RegEnumValueA`,
  `RegFlushKey`.
- `GetUserNameA` returns the registry `Playername` (default "Player").

Without `regs.cmd` the game asks for the serial number in single player. The rewrite is then stored encrypted, so it
would ask again next time (HYPOTHESIS). Frontends should make sure `regs.cmd` is present.

## WINMM / `_INMM` (`winmm.rs`, `audio_music.rs`)

`_inmm.dll` is a registry alias of `winmm.dll`, so the game's `_INMM` imports land here, and the GOG wrapper DLL is
never loaded.

- **Time.** `timeGetTime` = host ms. `timeBeginPeriod/EndPeriod/GetDevCaps`, `timeGetSystemTime`.
- **`timeSetEvent`** (one-shot and periodic, `TIME_CALLBACK_FUNCTION` / `EVENT_SET` / `EVENT_PULSE`).
  - The first call creates a **guest timer thread** whose entry is our thunk `winmm.dll!__s3port_timer_thread`.
  - The thunk fires the due timers: `call_guest(cb, [id, 0, user, 0, 0])` or it sets the event. Then it returns
    `Block`, so the scheduler retries it every round.
  - Periodic timers do not burst more than 4 periods to catch up.
  - S3 only uses `timeSetEvent` when `GetProcAddress("CreateWaitableTimerA")` fails
    ([static-startup.md](../reverse-engineering/static-startup.md)). `timeKillEvent` stops a timer.
- **MCI CD audio**, used by S3 through `mciSendCommandA` (code at `0x615740`–`0x615fe6`):
  - `MCI_OPEN` with flags `0x2100`, type "cdaudio".
  - `MCI_SET` time format TMSF (10).
  - `MCI_STATUS` item 3 (number of tracks).
  - `MCI_PLAY` with `FROM` = track and `TO` = track+1 (if below the track count), plus `MCI_NOTIFY` to the hidden
    "CDAUDIO CALLBACKWINDOW" (WndProc `0x615bd0`, which forwards `MM_MCINOTIFY 0x3B9`).
  - `MCI_STOP`, `MCI_CLOSE`.
- **Our side of CD audio:**
  - Track N = `MUSIC\TrackNN.ogg` through the VFS. The GOG install has Track02–14, so the number of tracks is
    14 and track 1 is "data".
  - Playback decodes with lewton and plays tracks `from..to`.
  - When playback ends, `MM_MCINOTIFY(MCI_NOTIFY_SUCCESSFUL, devid)` is posted to the callback window.
    `SUPERSEDED` is posted on a new play, `ABORTED` on stop/close.
  - Status covers: length (via the last Ogg granule; no decoding), position, mode, media present/ready,
    current track, and track type.
  - The same commands work as **MCI strings** (`open cdaudio alias x`, `set x time format tmsf`, `play x from 3 to 4
    notify`, `status x number of tracks|mode|length [track n]|position`, `stop`, `pause`, `resume`, `close`,
    `close all`).
- **Videos** (`open AVIVideo!<file> alias test`, `status test window handle`, `window test handle …`,
  `play test window notify`, `close test wait`; also the briefing videos):
  - Any non-cdaudio device type is a **fake video**: open, window, status and `close` all succeed.
  - `play … notify` posts `MM_MCINOTIFY(SUCCESSFUL)` immediately, so the game continues as if the video ended.
  - OBSERVED: if the open fails instead, S3 shows *"Error during video play, please check for DirectX Media 6.0 and
    Intel Indeo video codec 5.06 or above (Error code 266)"* over the main menu.
- **Music output.** `audio_music::MusicPlayer::mix` adds music into any interleaved i16 buffer with linear
  resampling and a volume (default 0.6, HYPOTHESIS).
  - By default the service `winmm-music` queues 44100 Hz stereo through `Host::queue_audio`, paced by the wall
    clock with about 150 ms lead.
  - **If DirectSound (s3-dx) owns the host audio stream**, it must call
    `s3_win32::winmm::set_external_music_mixer(m, true)` once and `s3_win32::winmm::mix_music(m, &mut buf, rate,
    channels)` for every block it mixes. Otherwise there are two producers with different rates.
  - `auxSetVolume` sets the music volume.
- Other: `waveOutGetNumDevs`=1, `auxGetNumDevs`/`joyGetNumDevs`=0, `joyGetPosEx`=JOYERR_UNPLUGGED,
  `PlaySoundA`/`sndPlaySoundA` do nothing and return TRUE.

## Small DLLs

| DLL | Behaviour |
| --- | --- |
| ole32 | `CoInitialize(Ex)`/`OleInitialize`, `CoUninitialize`, `CoCreateInstance` (CLSID factories in `m.com`), `CoCreateGuid` (random v4), `CoTaskMemAlloc/Free` (process heap) |
| rpcrt4 | `UuidToStringA` (lower case, no braces, string on the process heap), `RpcStringFreeA`, `UuidCreate` |
| shell32 | `ShellExecuteExA`/`ShellExecuteA` are logged and refused (`SE_ERR_NOASSOC`); `SHGetSpecialFolderPathA` = `C:\S3` |
| wsock32 | `WSAStartup` fails with `WSASYSNOTREADY` (10091); socket calls fail with `WSAENETDOWN`; `htons/htonl/ntoh*`, `inet_addr`, `inet_ntoa`, `gethostname` ("s3port") work. OBSERVED: the game logs nothing and continues to the menu |
| wininet | every call fails with `ERROR_INTERNET_CANNOT_CONNECT`; `InternetGetConnectedState` = offline |
| comctl32 | `InitCommonControls(Ex)` succeed |
| comdlg32 | file dialogs return "cancelled" |

## Tests

`cargo test -p s3-win32 -j 4` (with `CARGO_TARGET_DIR=port/target`). Tests for this area:

- `user_gdi_tests`:
  - A guest message loop with a hand-assembled WndProc: Register, Create, Post, GetMessage, Translate, Dispatch,
    PostQuitMessage, `WM_CREATE`, `WM_ACTIVATEAPP`.
  - Input to messages: mouse coalescing, MK flags, key lParam, `TranslateMessage` WM_CHAR, timers, thread
    messages, quit ordering.
  - Rects and wsprintf through the thunks.
  - The registry API: size protocol, `ERROR_MORE_DATA`, JSON persistence, the SingleNumber rule, the HKCR Seed.
  - GDI text on a surface DC: metrics, extent, clip, DrawText center/calc, opaque fill, lines, release.
  - MCI strings and commands: fake video, track count, errors.
  - `timeSetEvent` callbacks on the timer thread.
  - Decoding Track02.ogg of the local install, only if it is present.
- Unit tests in `registry_store`, `font`, `user32` (wsprintf, word wrap, VK chars), `winmm` (MCI parsing),
  `rpcrt4` and `wsock32`.

## Open / HYPOTHESIS

- Cross-thread `SendMessage` runs on the sender's thread.
- `MsgWaitForMultipleObjects` wakes for any queued message (not only new ones).
- No non-client area.
- Cursor shape is not shown by frontends yet. `GDIMouse=1` means the game expects an OS cursor; set `GDIMouse=0` in
  `registry.json` to get the game's software cursor (HYPOTHESIS).
- Music output has only been exercised headless (`NullHost` discards audio). The MCI play path is OBSERVED in game;
  whether it is audible depends on the frontend's `queue_audio` and on the DirectSound mixer integration.
