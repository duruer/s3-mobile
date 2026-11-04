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
