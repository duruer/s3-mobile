# DirectX / COM surface used by S3.EXE (GOG)

Source: static analysis with REA/Ghidra 12.1.2 on 2026-11-05 (S3.EXE sha256 `00a925cf…`).
The game wraps **every** DirectX call in a helper that takes an error-context string like
`"CSurface::DDBlt"`. Those strings give an almost complete list of the methods used. Indirect vtable
calls can still hide extra methods, so confirm with a runtime trace (Wine `+ddraw,+dsound,+dinput,+dplay`).

## How each component is created (OBSERVED)

| Component | Creation | Code |
| --- | --- | --- |
| DirectDraw | `CoCreateInstance(CLSID_DirectDraw {D7B70EE0-4340-11CF-B063-0020AFC2CD35}, NULL, CLSCTX_ALL=7, IID_IDirectDraw2 {B3A6F3E0-2B43-11CF-A2DE-00AA00B93356})`, then vtable `+0x48` = `IDirectDraw2::Initialize(guid)` | `FUN_005bec00` |
| DirectDraw enumeration | `LoadLibraryA("ddraw.dll")` + `GetProcAddress("DirectDrawEnumerateA")` | `FUN_005beb90` |
| DirectSound | `LoadLibraryA("dsound.dll")` + `DirectSoundCreate` | `FUN_005c1d20` |
| DirectInput | `LoadLibraryA("dinput.dll")` + `DirectInputCreateA(hinst, 0x0300 /*DX3*/, …)`, then `CreateDevice` (vtable +0x0C) | `FUN_005b9ec0`, called from `0x411540`, `0x5ba230` |
| DirectPlay | COM `CoCreateInstance` at `0x5c0808` / `0x5c0847` (CLSIDs not decoded yet). Error string `"DirectPlay 6.0 nicht installiert"` → needs **DirectPlay 6** (`IDirectPlay4` + lobby `IDirectPlayLobby3`, HYPOTHESIS) | |

So the HLE must provide a **COM `CoCreateInstance`** (ole32) that hands out our own DirectDraw/DirectPlay objects,
plus fake `ddraw.dll` / `dsound.dll` / `dinput.dll` modules for `LoadLibraryA` + `GetProcAddress`.

## Methods named in error strings (OBSERVED)

| Interface | Methods |
| --- | --- |
| IDirectDraw2 | Initialize, SetCooperativeLevel, SetDisplayMode, GetDisplayMode, RestoreDisplayMode, EnumDisplayModes, CreateSurface, WaitForVerticalBlank (+ DirectDrawEnumerateA) |
| IDirectDrawSurface | Lock, Unlock, Blt, BltFast, Flip, GetAttachedSurface, GetSurfaceDesc, IsLost, Restore, GetDC, ReleaseDC |
| IDirectDrawPalette | used (RTTI `CPalette`), methods not named — check |
| IDirectSound | SetCooperativeLevel, CreateSoundBuffer, DuplicateSoundBuffer |
| IDirectSoundBuffer | Lock, Unlock, Play, GetStatus, Restore, GetVolume, SetVolume, SetPan |
| IDirectInput / Device (DX3) | CreateDevice, SetDataFormat, SetCooperativeLevel, SetProperty, SetEventNotification, Acquire, Unacquire, GetDeviceData (buffered input) |
| IDirectPlay(4?) | EnumConnections, InitializeConnection, EnumSessions, Open, Close, GetSessionDesc, SetSessionDesc, GetCaps, CreatePlayer, DestroyPlayer, EnumPlayers, GetPlayerData, SetPlayerData, Send, Receive, GetMessageCount |
| IDirectPlayLobby | Connect, CreateCompoundAddress, GetConnectionSettings, SetConnectionSettings |

`GetDC`/`ReleaseDC` on surfaces → GDI text drawing onto DirectDraw surfaces (fonts from registry `Font`/`FontSize`).
Our GDI HLE must rasterise text into the surface memory.

## Port implications

- DirectDraw part is a classic 2D software-rendered game: lock surface, write pixels, flip. HLE = surfaces in guest
  memory + upload the front buffer to a GPU texture on Flip.
- DirectInput is real (buffered mouse/keyboard via `GetDeviceData`). Touch must be translated into
  DIDEVICEOBJECTDATA events, **and** window messages (registry `GDIMouse`=1 suggests the cursor is GDI-drawn).
- DirectPlay only for multiplayer → stub with "not available" first (keep single player working), implement later.

## Static details for the HLE (2026-11-05, static RE pass; see also [static-startup.md](static-startup.md))

All addresses are VAs in GOG `S3.EXE`. Wrapper functions throw a C++ exception (`CDirectXException`-like, via
`0x5be180` + `__CxxThrowException 0x61f027`) on any failing HRESULT except `DDERR_WASSTILLDRAWING (0x8876021C)`,
which the callers retry.

### DirectDraw

- **SetCooperativeLevel** (`0x404e43`, IDirectDraw2 vtbl `+0x50`): `(hwnd, 0x13)` =
  `DDSCL_FULLSCREEN | DDSCL_ALLOWREBOOT | DDSCL_EXCLUSIVE`.
- **SetDisplayMode** (IDirectDraw2 vtbl `+0x54`, 5 args `w, h, bpp, refresh=0, flags=0`):
  - first call at startup `0x404ebd` uses the settings object `0x7a7af0`: width `+0x8C`, height `+0x90`, bpp `+0x94`;
    defaults set at `0x40d25a`: **800 × 600 × 16**. Command line `-messe` (`0x410e23`) forces **640 × 480**.
  - mode switch by index (`0x405c80`, used for the registry value **`Resolution`**, read at `0x40deb8` into settings
    `+0x70` and clamped to 0..2): **0 → 640×480×16, 1 → 800×600×16, 2 → 1024×768×16**. Always **16 bpp**.
  - `GetDisplayMode` first; if the current mode already matches it logs `"GFX: Skipping SetDisplayMode"` (`0x42cf2b`).
- **No 8-bit/palette path in the main renderer**: no `CreatePalette`/`SetPalette`/`SetColorKey` calls exist in the
  DirectDraw module (`0x5be000-0x5c0000`); vtable offsets used there are only Initialize, SetCooperativeLevel,
  SetDisplayMode, GetDisplayMode, RestoreDisplayMode, EnumDisplayModes, CreateSurface, WaitForVerticalBlank and
  surface Blt/BltFast/Flip/GetAttachedSurface/GetSurfaceDesc/Lock/Unlock/IsLost/Restore/GetDC/ReleaseDC.
  Sprites are drawn in software (the game's own blitters, incl. shadow/alpha with masks `0x7BEF/0x39E7/…`), so
  **no colour keys** are needed. (RTTI `CPalette` exists but no call site was found — HYPOTHESIS: dead or video-only.)
- **Surface creation** (`CreateSurface` IDirectDraw2 vtbl `+0x18`, wrapper `0x5bec80`; DDSURFACEDESC `dwSize=0x6C`):

  | Ctor / create | dwFlags | ddsCaps | Use |
  | --- | --- | --- | --- |
  | `0x5bf800` primary | `DDSD_CAPS` (+`DDSD_PIXELFORMAT` 0x1000 if a format is given) | `DDSCAPS_PRIMARYSURFACE` (0x200) \| extra | front buffer |
  | `0x5bf9b0` flipping primary | `DDSD_CAPS\|DDSD_BACKBUFFERCOUNT` (0x21), `dwBackBufferCount=1` | `PRIMARYSURFACE\|FLIP\|COMPLEX` (0x218) | `lpFlipPrimary`; back buffer via `GetAttachedSurface` (`0x5bfa52`) |
  | `0x5bfaf0` / `0x5bfbc0` offscreen | `DDSD_CAPS\|DDSD_HEIGHT\|DDSD_WIDTH` (7) (+PIXELFORMAT) | `DDSCAPS_OFFSCREENPLAIN` (0x40) \| extra, e.g. **`DDSCAPS_SYSTEMMEMORY` (0x800)** | back buffer / work surfaces |

- **Presentation mode** (draw thread, `0x42e4f0`–`0x42e830`; log `"DRAW: Flags %i %i %i %i"`), chosen by settings bytes
  `0x7a7af0+0x5A` (getter `0x4122b0`) and `+0x5B` (getter `0x4122c0`), both default 1 (`0x40d1f4/0x40d1f8`) and
  overridden from the registry (`0x40daa2`… — HYPOTHESIS: the `NoFlip` value; string `0x695a38`):
  1. `+0x5B` set (**default**): primary surface + **offscreen back buffer in system memory (caps 0x800)**; the game
     renders into the back buffer (Lock) and copies it to the primary with **`Blt`** (`0x5bf0f8`, flags
     `|= DDBLT_ASYNC|DDBLT_WAIT` = `0x1000200`) of rect `(0,0,w,h)` (`0x730f0c` = width, `0x730f10` = height);
     before that, if `WaitVBlank` (`+0x5C`, getter `0x4122e0`) is set, `WaitForVerticalBlank(DDWAITVB_BLOCKBEGIN, 0)`
     (`0x42cddb`); if that throws, the catch block clears `+0x5C`.
  2. `+0x5B` clear, `+0x5A` set: primary + offscreen (no forced system memory), Blt.
  3. both clear: flipping chain (`0x5bf9b0`), **`Flip(NULL, 0)`** retried with `Sleep(0)` while
     `DDERR_WASSTILLDRAWING` (`0x5bf710`/`0x5bf740`).
  HLE plan: implement surfaces as plain guest memory; Blt to primary or Flip = "present" → upload the 16-bit
  front image to a GPU texture. `BltFast` (`0x5bf15d`) adds `DDBLTFAST_WAIT (0x10)`.
- **Pixel-format check → GFX file choice** (GFX loader `0x5f9ff2`): reads the R/G/B masks of the display format
  (object fields `+0x14/+0x18/+0x1C`):
  - `F800/07E0/001F` (RGB565) → shadow/blend masks `0x7BEF, 0x39E7, 0x18E3, 0x0861, 0x0020` (`0x7e8dd8…`);
  - `7C00/03E0/001F` (RGB555) → masks `0x3DEF, 0x1CE7, 0x0C63, 0x0421, 0`;
  - anything else → error path (`0x5fa106`).
  File names are `"siedler3_" + NN + "." + mask suffix + ".dat"` in `.\Gfx` (strings `0x7a2d8c/0x7a2d98/0x7a2da4`,
  wildcard `"siedler3_??."` at `0x7a2dac`); the suffix is the three masks in lowercase hex without padding:
  **`f8007e01f` = 565, `7c003e01f` = 555** (HYPOTHESIS for the exact formatting code; matches the file names on disk:
  47 `*.f8007e01f.dat` + 45 `*.7c003e01f.dat`, ≈117 MB each set). On disk the 565 set is `Siedler3_NN…` and the 555
  set `siedler3_NN…` → **the HLE file system must be case-insensitive**. The files are **memory-mapped**
  (`CreateFileMappingA` + `MapViewOfFile`, class at `0x61a5xx`) and pre-faulted with the `VirtualProtect` page-touch
  helper `0x61ab60`. Recommendation: report RGB565 (masks `F800/07E0/001F`) — it matches Android `RGB_565`.
- CPU-dependent surface copy: `0x5c3740` benchmarks 4 (5 with MMX) copy routines with QPC and keeps the fastest
  ([static-isa.md](static-isa.md) §8).

### DirectSound (`dsound.dll` via `LoadLibraryA`, `DirectSoundCreate` at `0x5c1d4e`)

- `SetCooperativeLevel(hwnd, DSSCL_PRIORITY = 2)` (`0x5fe96d`, IDirectSound vtbl `+0x18`).
- **Primary buffer** (`0x5c21c0`): `DSBUFFERDESC{dwSize=0x14, dwFlags=0x81 = DSBCAPS_PRIMARYBUFFER|DSBCAPS_CTRLVOLUME,
  dwBufferBytes=0, lpwfxFormat=NULL}`. **No `SetFormat` call** (no wrapper/error string, no vtbl `+0x38` call) →
  the primary format is whatever the device default is; the game only uses it for master volume.
- **Secondary buffers** (`0x5c2220`): `dwFlags=0xE0 = CTRLFREQUENCY|CTRLPAN|CTRLVOLUME`, size and `WAVEFORMATEX`
  from the sound data; `DuplicateSoundBuffer` (vtbl `+0x14`) for repeated effects. Methods: Lock/Unlock/Play/
  GetStatus/Restore/GetVolume/SetVolume/SetPan. A down-conversion mode exists (`0x5c26d8`: when the `SoundFormat`
  mode is 4 and the sample is ≥ 22050 Hz 16-bit, it halves rate and converts to 8-bit — HYPOTHESIS on the registry link).

### DirectInput (DirectX 3, `dinput.dll`)

- One device: **`GUID_SysMouse` {6F1D2B60-D5A0-11CF-BFC7-444553540000}** (`0x65b388`, created at `0x5ba233`).
  No keyboard device (keys come from `WM_KEYDOWN/UP/CHAR`).
- `SetDataFormat` with the in-exe copy of **`c_dfDIMouse`** (`0x62ef60`: `dwSize 0x18, dwObjSize 0x10,
  dwFlags DIDF_RELAXIS (2), dwDataSize 16`, 7 objects: X/Y/Z axes at 0/4/8, buttons 0–3 at 12–15).
- `SetCooperativeLevel(hwnd, 6 = DISCL_NONEXCLUSIVE | DISCL_FOREGROUND)` (`0x404f81`).
- `SetEventNotification(hEvent)` and `SetProperty(DIPROP_BUFFERSIZE (1), DIPROPDWORD{0x14,0x10,0,DIPH_DEVICE,
  dwData = 128})` (`0x404f18`–`0x404f9a`) → **buffered, 128 events**.
- Main thread reads with `GetDeviceData(sizeof(DIDEVICEOBJECTDATA)=16, buf, &n /*2*/, 0)` (`0x5ba290`) whenever
  the event is signalled (§3 of static-startup). Touch → relative X/Y axis events + button 0/1 events, *and*
  `WM_MOUSEMOVE/WM_LBUTTON*` window messages (cursor position).

### DirectPlay (multiplayer only)

| Site | rclsid | riid |
| --- | --- | --- |
| `0x5c0808` | `{D1EB6D20-8923-11D0-9D97-00A0C90A43CB}` = **CLSID_DirectPlay** | `{0AB1C531-4745-11D1-A7A1-0000F803ABFC}` = **IID_IDirectPlay4A** |
| `0x5c0847` | `{2FE8F810-B2A5-11D0-A787-0000F803ABFC}` = **CLSID_DirectPlayLobby** | `{1BB4AF80-A303-11D0-9C4F-00A0C905425E}` = **IID_IDirectPlayLobby2A** |
| `0x5bec16` | `{D7B70EE0-4340-11CF-B063-0020AFC2CD35}` = CLSID_DirectDraw | `{B3A6F3E0-2B43-11CF-A2DE-00AA00B93356}` = IID_IDirectDraw2 |

All with `CLSCTX_ALL (7)`. Names from `dplay.h`/`dplobby.h`/`ddraw.h` (memory; GUID values decoded from the exe with
`python3 -I` + `pefile` + `uuid.UUID(bytes_le=…)`). HLE: `CoCreateInstance` for the DirectPlay CLSIDs can return
`REGDB_E_CLASSNOTREG`; the game then shows "DirectPlay 6.0 nicht installiert" only when entering multiplayer
(HYPOTHESIS: confirm that single player does not create it at startup).
