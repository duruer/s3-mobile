# DirectX HLE (`port/crates/s3-dx`)

Status 2026-11-05: DirectDraw, DirectSound, DirectInput 3 and DirectPlay stubs implemented; 16 unit tests
(`cargo test -p s3-dx -j 4`). Exercised by the real S3.EXE since integration round 1 (menu 800x600x16, in game
640x480x16, WaitForVerticalBlank pacing, DirectInput mouse, DirectSound mixer; see
[integration-log.md](integration-log.md)). DirectPlay is still only stubs (multiplayer menus not exercised). What the game uses comes from [directx-surface.md](../reverse-engineering/directx-surface.md)
(static) and [wine-trace.md](../reverse-engineering/wine-trace.md) (runtime).

## Files

| File | Contents |
| --- | --- |
| `lib.rs` | `register_all(reg)` (exports + every interface method get a handler), `register_factories(m)` (CoCreateInstance classes + the two services below), `stubs!` macro for constant-result methods, HRESULT constants |
| `ddraw.rs` | DirectDraw: IDirectDraw, IDirectDraw2, IDirectDrawSurface/2/3 (one vtable), IDirectDrawPalette, IDirectDrawClipper; exports DirectDrawCreate, DirectDrawEnumerateA, DirectDrawCreateClipper; factory CLSID_DirectDraw |
| `blit.rs` | pure pixel code: `fill`, `blt` (clipping, nearest stretch, mirror, src/dst colour key ranges, memmove-safe overlap), `to_framebuffer` (16-bit 565/555, 8-bit + palette, 24/32-bit -> RGB565) |
| `dsound.rs` | DirectSound: IDirectSound, IDirectSoundBuffer; exports DirectSoundCreate, DirectSoundEnumerateA; `mixer_service` |
| `mixer.rs` | pure mixer core: dB/pan -> gain, `mix_voice` (8/16-bit, mono/stereo, linear resampling, looping), `mix_stream` (external PCM queues), clamping |
| `dinput.rs` | DirectInput 3: IDirectInputA, IDirectInputDeviceA (SysMouse, SysKeyboard); export DirectInputCreateA; `pump_service`; VK -> DIK table |
| `dplay.rs` | DirectPlay 6 stubs: IDirectPlay4A, IDirectPlayLobby3A (per-method arg counts tables); factories CLSID_DirectPlay / CLSID_DirectPlayLobby; exports |
| `tests.rs` | vtable offsets vs S3 call sites, every method has a handler, blit/mixer unit tests, full COM flows through `call_guest` |

## Object model

- Every interface object is guest memory `[vtbl][refcount][...]` from `Machine::com_new_object`; vtables come from
  `Machine::com_vtable(dll, iface, METHODS)` (thunks named `ddraw.dll!IDirectDrawSurface::Blt` etc., so
  `S3_TRACE=thunks` shows COM calls by name).
- Host-side state per module in `Machine::state::<T>()`: `ddraw::DdState` (display mode, surfaces, palettes,
  clippers, the visible primary, a reused FrameBuffer), `dsound::DsState`, `dinput::DiState`. Keyed by the guest
  interface pointer. Guards are never held across `call_guest`.
- IDirectDraw objects store a "core" pointer at `+8`; `QueryInterface(IID_IDirectDraw/2)` returns a new interface
  object with the other vtable. IDirectDraw v1 has its own `SetDisplayMode` handler (3 args vs 5 in v2).
- Surfaces: `QueryInterface` for IID_IDirectDrawSurface/2/3 returns the same pointer (vtables are prefix
  compatible: v1 = 36 methods, v2 +3, v3 +SetSurfaceDesc).
- Release: surfaces free their pixel memory (VirtualAlloc region) and a flip chain when the refcount reaches 0;
  sound buffers free PCM memory when the last duplicate goes; DirectDraw/DirectSound/DirectInput/DirectPlay
  objects themselves are never freed (few, may be aliased).

## DirectDraw behaviour

- Display mode: starts as 800x600x16 (`DEFAULT_MODE`); `SetDisplayMode` accepts 8/16/32 bpp and 320..2048 x
  200..2048. `EnumDisplayModes` offers 640x480, 800x600, 1024x768 at 8 and 16 bpp (filtered by the DDSD).
  16 bpp = RGB565 (masks F800/07E0/001F), which makes the game choose `GFX/*.f8007e01f.dat`.
- `CreateSurface`: primary takes the current mode size; `DDSCAPS_FLIP` + `DDSD_BACKBUFFERCOUNT` builds a chain
  (back buffer reachable with `GetAttachedSurface(DDSCAPS_BACKBUFFER)`); offscreen needs WIDTH+HEIGHT and may give
  a pixel format (bpp, 555 detection). Reported caps get `SYSTEMMEMORY` or `VIDEOMEMORY|LOCALVIDMEM`, primaries
  `VISIBLE` (+`FRONTBUFFER` with a chain). Pitch = width*bytes rounded up to 4.
- `Lock` returns the pixel pointer (offset by the rect), the full DDSD and never fails/blocks; `Unlock`,
  `GetDC`/`ReleaseDC` are always allowed (no busy tracking).
- **Presentation**: whenever the visible primary is written by `Blt`, `BltFast`, `Unlock`, `ReleaseDC`,
  `SetPalette`/palette `SetEntries` (8-bit), or `Flip` (rotates the pixel pointers through the chain), the primary
  is converted to RGB565 and passed to `Machine::present` (counted in `stats.presents`). S3's menu path is
  BltFast(offscreen -> primary), the game path Blt(back buffer -> primary) after WaitForVerticalBlank.
- `Blt`: `DDBLT_COLORFILL` (fx+0x50), ROP BLACKNESS/WHITENESS as fills, other ROPs (SRCCOPY) copy;
  `KEYSRC`/`KEYDEST` use the surface keys, `*OVERRIDE` the DDBLTFX keys (ranges), `DDBLT_DDFX` mirroring,
  stretching nearest-neighbour, destination clipped to the surface (also without a clipper; real DirectDraw would
  return DDERR_INVALIDRECT without one — lenient on purpose). `BltFast` honours `SRCCOLORKEY`/`DESTCOLORKEY`.
  `DDBLT_WAIT/ASYNC` are irrelevant (synchronous). `GetBltStatus`/`GetFlipStatus`/`IsLost`/`Restore` = DD_OK.
- `WaitForVerticalBlank(BLOCKBEGIN|BLOCKEND)` blocks the calling guest thread (ThunkResult::Block) until the next
  simulated 60 Hz boundary after the call started — S3's frame pacer (`WaitVBlank`=1). `GetScanLine` /
  `GetVerticalBlankStatus` are derived from the same clock. `BLOCKBEGINEVENT` -> DDERR_UNSUPPORTED.
- `GetDC` uses the gdi32 bridge `s3_runtime::gdi::create_dc(DcTarget{addr,pitch,width,height,bpp,rgb565})`
  (installed by s3-win32 gdi32); returns DDERR_CANTCREATEDC if no hook is installed. `ReleaseDC` ->
  `gdi::release_dc`.
- Not implemented (E_NOTIMPL): overlays, BltBatch, DuplicateSurface. Palettes: 2/4/16/256 entries
  (DDPCAPS_1/2/4/8BIT), PALETTEENTRY dwords. Clippers only remember the HWND; `GetClipList` -> DDERR_NOCLIPLIST.

## DirectSound behaviour

- `CreateSoundBuffer`: primary (`DSBCAPS_PRIMARYBUFFER`, no data; its volume is the master volume; `SetFormat`
  stored, default 22050/2/16) or secondary (PCM tag 1, 1-2 channels, 8/16 bit; data in the process heap).
  `DuplicateSoundBuffer` shares the PCM memory (refcounted), own position/volume/pan/frequency.
- `Lock` handles wrap-around (two pointers), `DSBLOCK_FROMWRITECURSOR`, `DSBLOCK_ENTIREBUFFER`; primary Lock ->
  DSERR_PRIOLEVELNEEDED. `Play(LOOPING)`, `Stop` (keeps position), `GetStatus` (PLAYING|LOOPING),
  `Get/SetCurrentPosition` (write cursor = play + 15 ms), `Set/GetVolume` (-10000..0), `Set/GetPan`,
  `Set/GetFrequency` (0 = original, 100..200000), `GetFormat`, `GetCaps`.
- **Mixer** (`dsound::mixer_service`, registered as scheduler service `dsound-mixer`, runs once per
  `run_slice`): wall-clock paced, produces audio up to `LATENCY_MS` (60 ms) ahead of real time, at most 250 ms
  per call (longer stalls are skipped, not replayed). Output 44100 Hz stereo i16 -> `Host::queue_audio`. Gains:
  `10^(vol/2000)`, pan attenuates the opposite side the same way, master = primary volume. Play cursors therefore
  advance in real time even headless. Paused (`m.input.paused`) -> no output, pacing restarts on resume.
- **Device drift guard** (audio-verify, 2026-11-05): if the host reports a device queue
  (`Host::audio_queued_frames`, Android AAudio ring, s3run `--audio-play`), the mixer never lets it exceed
  `MAX_HOST_QUEUE_MS` (250 ms): frames above it are not produced this call (voices are delayed, not skipped;
  counted in `DsState::drift_cut_frames`). Nominal queue is ~60 ms, so this only acts on a slow/stalled device
  clock. Hosts without a device report 0 (pure wall-clock pacing).
- Every secondary `Play` is logged at Debug (`dsound: Play <buf> vol pan rate ch/bit bytes flags`, `s3run -v`) and
  counted in `DsState::voices_started`.
- **Other audio sources** (winmm waveOut / CD music): push interleaved i16 into
  `m.state::<s3_runtime::audio::AudioStreams>()` (`streams[id] = AudioStream { rate, channels, samples, gain }`);
  the mixer resamples and mixes them. This keeps a single producer for the host audio sink.
  (`s3-runtime/src/audio.rs` was added for this by the directx agent.)

## Audio verification (s3run `--audio-wav`, 2026-11-05)

How: `s3run ... --audio-wav ../scratch/audio/x.wav [--audio-wav-seconds 300]` writes exactly what reaches
`Host::queue_audio` (44100 Hz stereo i16; header rewritten every second, so a killed run still leaves a valid
file) and prints `audio timing: ... lead min/max/last, drift since 2 s, max gap between calls` at exit (lead =
audio produced minus wall time since the first block). Analyse with `python3 -I tools/wav_stats.py x.wav [--from
S] [--to S] [--window-ms N]` (RMS dBFS per window L/R, peak, clipped samples, silent windows, 2-200 ms zero gaps
inside signal). `S3_MUSIC_VOLUME=0` mutes the CD music to isolate effects. Optional real playback: build with
`--features audio-cpal` (cpal 0.15/ALSA, needs `pkg-config alsa`; off by default) and pass `--audio-play`.

Findings (GOG S3.EXE, menu -> random map in game, ~45 s runs):

- **Timing**: lead constant 59-60 ms from start to end (= `LATENCY_MS`), drift 0 ms over 40 s, 44.07 s audio for
  44.01 s wall; largest gap between mixer calls 39-67 ms (busy loading); no zero gaps inside the signal. The mixer
  keeps up headless at release speed. (A debug build is too slow: lead dips to -42 ms.)
- **Music** (MCI cdaudio track 7, from ~16 s when the game starts): -31 dBFS RMS L / -28 R, peaks ~8000, no
  clipping, continuous. The menu is silent: S3 plays no music there (same under wine: only Track07 at game start).
- **Effects** (music muted): first sounds 1-2 s after the map appears; ambient/worker sounds -35..-55 dBFS. The
  game creates 11 mono 22050 Hz 16-bit buffers (33939..251792 bytes) + `DuplicateSoundBuffer` per overlapping play,
  and plays them with `SetVolume(-2000)` `SetPan(+-...)` `Play(0)` (no looping, no `GetCurrentPosition`/streaming
  buffers: the play cursor is only relevant for `GetStatus` end detection). Pan 820 -> left 8 dB lower, visible in
  the per-channel RMS. Mixed peak with music < 8100: no clipping headroom issue.
- **Bug fixed**: winmm's music service queued music to `Host::queue_audio` *itself* while the DirectSound mixer
  also queued (two producers: music blocks were appended between mixer blocks, i.e. twice the audio per wall
  second, music and effects not overlapped, sink queues growing). Music now goes through
  `AudioStreams[MUSIC_STREAM_ID]` (see [user32-gdi32.md](user32-gdi32.md)); the mixer is the only producer.
- Quality checks (unit tests `mixer_resamples_22k_voice_smoothly_with_pan`,
  `mixer_passes_stream_music_through_unchanged_and_gapless`, `mixer_drift_guard_caps_host_queue`): 22050 -> 44100
  linear upsampling of a 441 Hz sine stays within 25 LSB of the ideal; 44100 Hz streams pass bit-exact; dB curve
  `10^(v/2000)` and pan law (opposite side attenuated by |pan|/100 dB) match DirectSound.

## DirectInput behaviour

- `CreateDevice(GUID_SysMouse | GUID_SysKeyboard)`, else DIERR_DEVICENOTREG. `SetDataFormat`,
  `SetCooperativeLevel` accepted (format not interpreted: mouse = DIMOUSESTATE layout, keyboard = 256 bytes).
- `SetProperty(DIPROP_BUFFERSIZE)` sets the buffer (0 = unbuffered, no events kept); overflow drops the oldest
  and returns DI_BUFFEROVERFLOW once.
- `Acquire` starts reading `m.input` from "now" (own cursor) and takes the current host mouse position as the
  reference for relative deltas. Host `MouseMove{x,y}` -> `DIMOFS_X/Y` deltas (same sequence number),
  `MouseButton` -> `DIMOFS_BUTTON0+i` (0x80 / 0), `Key{vk}` -> DIK scan code (table `vk_to_dik`).
- `pump_service` (service `dinput-pump`, also called at the start of GetDeviceData/GetDeviceState) converts new
  host events for every acquired device and sets the `SetEventNotification` event (handle table Event ->
  signaled), which wakes S3's main thread.
- `GetDeviceData(cb>=16, rgdod, &n, DIGDD_PEEK?)`, `GetDeviceState` (accumulated deltas, reset on read),
  `GetCapabilities`, `GetDeviceInfo`, `EnumDevices`. Not acquired -> DIERR_NOTACQUIRED.
- HYPOTHESIS: S3 moves its own cursor from the relative DirectInput deltas. As long as host positions are
  absolute and the game cursor started at the host position, the deltas reproduce the touch position; if the
  game clamps differently the cursor could drift (check on device; fix would be to resync on touch-down).

## DirectPlay behaviour

Both CLSIDs create objects for any requested IID (IDirectPlay2..4 A/W share the IDirectPlay4A vtable prefix;
lobby 1..3). Enumerations (`EnumConnections`, `EnumSessions`, `EnumPlayers`, ...) succeed with no callbacks,
`Close`/`Initialize` succeed, `GetMessageCount` = 0, `Receive` = DPERR_NOMESSAGES, lobby
`GetConnectionSettings` = DPERR_NOTLOBBIED, everything else DPERR_UNAVAILABLE. `DirectPlayCreate` fails with
DPERR_UNAVAILABLE, `DirectPlayLobbyCreateA` returns a lobby object. Error code values are from memory of
dplay.h (HYPOTHESIS until multiplayer menus are exercised). Update 2026-11-05: S3.EXE's own HRESULT table (`0x7a03b4`)
confirms the values (DPERR_NOTLOBBIED = 0x8877042E); the full RE of the game's DirectPlay use and the plan for the
real implementation over our relay are in [multiplayer.md](multiplayer.md).

## Open points

- Runs against S3.EXE (menu + in game, 2026-11-05). Still watch for: unexpected QueryInterface IIDs
  (logged as warnings), Lock/GetDC on the primary, unknown Blt flags.
- Present happens on every primary write; if a frame is assembled from several Blts to the primary, the host sees
  intermediate frames (S3 does one full-screen Blt per frame, so fine).
- 16-bit 555 surfaces are only created if the game asks with a 555 pixel format; the display is always 565.
