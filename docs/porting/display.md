# Display: GPU presenter, full-screen modes, FPS counter

Status 2026-11-05 (workflow `wf_a446e364-b12`, agent `display`). User request: high FPS and full-screen display
options like Settlers United, an FPS counter setting. Related: [resolution-zoom.md](resolution-zoom.md) (custom game
resolutions, zoom), [android.md](android.md), [mods.md](mods.md), [ui-hud.md](ui-hud.md).

Everything here is **presentation only**: it never changes guest-visible memory, so lockstep multiplayer
([determinism.md](determinism.md)) is unaffected.

## Display modes (`s3_mods::display`)

| Mode | What it does | Touch mapping |
| --- | --- | --- |
| `Fit` | aspect kept, whole frame visible, black bars (old behaviour) | bars clamp to the frame edge |
| `Fill` | aspect kept, fills the screen, overflow cropped (centred) | cropped strip is unreachable; mods anchor UI with `visible_game_rect` |
| `Stretch` | fills the screen, aspect distorted | linear per axis |
| `Native wide` (default) | the game renders at a custom resolution with the screen's aspect (res-zoom picks it with `Viewport::native_wide_size`), shown like `Fit` → no bars. Known 800x600 menu screens use a full-screen menu layout ([menu-layout.md](menu-layout.md)); other frames of another aspect are shown like `Fit` | like `Fit` / through the layout pieces |

The old ESC/ABC/RMB letterbox buttons were removed 2026-11-07; both UI modes have
their own menu buttons (Classic: floating Menu, HUD: menu sheet), and the RMB/keyboard functions are gestures
(two-finger tap = cancel, three-finger tap = keyboard), see [touch.md](touch.md).

Filters: `Nearest` ("Pixel"), `Linear` ("Smooth"), `SharpBilinear` ("Sharp", default): nearest inside each source
pixel, a one-screen-pixel linear band at the pixel edges → crisp but evenly sized pixels at non-integer scales
(800x600 → 2229x904 is 1.507x). At an exact integer scale it equals nearest; when downscaling it uses linear.

### The shared transform

`s3_mods::display::Viewport` is the **only** screen↔game transform. Frontends build it per shown frame
(`Viewport::for_frame_split(mode, pixels, game_w, game_h, win_w, win_h, scene.panel)`), map touches/mouse with
`window_to_game`, and publish it to the mods with `display::set_viewport(m, Some(vp))`. Mods read:

- `display::viewport(m)` / `display::window_size(m)` (None headless without `--window`),
- `display::visible_game_rect(m)`: part of the frame that is on screen (game px; the whole frame except in Fill),
- `display::display_settings(m)`; menus change settings with `set_display` / `cycle_mode` / `cycle_filter` /
  `cycle_fps` (persisted in `mods-settings.json`); `override_display` changes them for one run only (CLI).
- `display::frame_viewport(m)` / `s3_mods::draw_viewport(m)`: the viewport of the frame being drawn (published window
  size + mode + this frame's panel split), for the window layer.

### Classic panel at a constant size (`PanelSplit`, 2026-11-08 overlay-window)

UI mode A shows the original left panel. With zoom it used to scale with the render size (big when zoomed in, tiny
zoomed out). Now, in `NativeWide` with the custom resolution on, in game (`display::panel_split`), the scene
published with each frame carries `PanelSplit { w: [0x730f24], h: art height of the layout variant }`
and `Viewport::new_split` draws:

- the panel part (frame `x < w`, `y < h`) at `win_h / h`: the art always fills the screen height (Fold4 cover:
  136 px art at 480 -> 256 px, 176 at 600 -> 265 px, 216 at 768 -> 254 px wide on screen); the black strip the game
  leaves below the art at heights between the variants is not shown any more;
- the map part (`x >= w`) in the rest of the window. The zoom picks render widths so this part keeps the map's
  aspect (`zoom::classic_width`: `w + (win_w - w*win_h/h) * H / win_h`, multiple of 8; < 1.2 % off), and pinch / menu
  zoom anchor in map-view fractions (`zoom::anchor_camera_map`, `ResState::anchor_map`).

**One panel variant at every size (2026-11-08, phone report "the panel flickers while zooming").** Two causes,
both fixed and verified on the phone (screen recording of injected pinches, 1045 frames, panel unchanged):

1. With a variant per height the game swapped the panel art when the zoom crossed 600 / 768 (256 / 265 / 254 px on
   screen and a different arrangement), so pinching around a border made the panel jump. UI mode A now uses the
   640x480 panel at every custom size (`res::CLASSIC_VARIANT`, `res::variant_for_mode`; the only one that fits the
   whole Classic range from 480 up). Costs some sharpness (136 px art scaled to 256 px on the Fold4 cover).
2. Single frames without the split: the presenter only used the split when the latest scene was made for the
   frame's size (a frame of a new size often arrives first: now `display::panel_for_frame` carries the split over),
   and `panel_split` needed a game frame within the last second (`res::in_game`), which a big zoom step (full
   landscape redraw) can exceed: now `res::in_game_frame` (up to 10 s while the shown frame has a custom size).
   `S3_PANEL_TRACE=1` prints why `panel_split` returned none.

**Panel art past the panel width (2026-11-08).** The 640x480 panel's minimap frame has a diagonal right edge (up
to x 207 at the top) and leaves that reach a few px past `w = 136`; they lie in the map part and scaled with the
zoom ("the diagonal edge still moves"). `PanelSplit::overhang` lists them as 4-row bands (`y0, y1, x_end`, measured
from a frame with a black map behind; numbers only; runs of rows with the same `x_end`, since 4-row bands also
carried map pixels that moved during zoom); `quads()` adds one panel-scale quad per band on top of the
map part, and `window_to_game` / `game_to_window` use the panel scale there (`PanelSplit::art_right`). Classic
never goes below the art's height (480), so the map part is never larger than the panel scale and no copy of the
edge shows next to it. The Classic context chip starts right of the overhang.

**Zoom below the panel art (2026-11-08).** The 640x480 panel needs 480 frame rows, so UI mode A could not zoom in
past 480 while the Mobile HUD goes to 200. Now heights below 480 keep a 480 high frame and show only `crop_h`
rows of the map part (centred, matching width) over the whole map area (`zoom::classic_crop` ->
`PanelSplit::crop_h`, `Viewport::map_src`). Those are exactly the pixels a smaller render would show, at the same
scale; it costs a 480 high render and the crop is centred (pinching inside the crop zooms about the screen centre,
and the outer rows of the 480 view cannot be shown at the map's edges). Verified desktop + phone (200 high, 60 fps).

**Leaves at the panel edge (2026-11-08, second fix).** The leaves below the minimap reach up to x 144; the frame
showed them a second time in the map part at the map's scale, at other heights than the panel's, so they seemed to
move while zooming. Now only the minimap frame rows (< 156) are overhang bands; the map part starts at
`PanelSplit::map_x0` (`w + 9`, `display::map_skip`; `zoom::classic_width` adds the 9 columns) and the leaf tips are
cut at the panel edge. Phone recording: the edge is identical in all frames.

**Native wide fills the window (2026-11-08).** The render width is a multiple of 8, so the frame's aspect differed
from the window's by up to ~0.5 % and `Viewport::new` left 1-4 px bars whose width changed with every zoom step;
everything anchored to the shown frame (Mobile HUD bars, ✕, FPS counter) shifted by a few px per step ("the HUD
jitters"). Within 2 % `NativeWide` now stretches the frame to the whole window.

**xBR upscaling filter (2026-11-08).** Zoom shows fewer game pixels scaled up (4.5x at 200 px height), so the
1998 pixel art looks blocky. `ScaleFilter::Xbr` ("xBR (HD edges)") is a GPU pixel-art upscaler in the Android
presenter: `gl::FS_XBR`, our GLSL ES 1.00 version of Hyllian's xBR level 2 (single pass at any scale, 21 nearest
taps, luma-weighted edge detection, 30/45/60 degree edge blends with a one-output-pixel smoothstep; bvec logic as
0/1 floats). Falls back to sharp bilinear when the shader does not compile (logged) and on the desktop CPU scaler.
Fold4 cover at 240 px height: compiles on Adreno, 120 fps with xBR and with sharp (no visible cost); edges of
units, buildings and rocks become smooth, ground texture looks "painted". Cycle it with "Filter" in the HUD
settings / Classic (CLI `--filter xbr`).

**AI upscaling filter (2026-11-08).** `ScaleFilter::Ai` ("AI (Anime4K)"): the small CNN x2 upscaler of Anime4K
v3.2 (`Anime4K_Upscale_CNN_x2_S`, https://github.com/bloc97/Anime4K, **MIT**, license text kept in the generated
file) on the GPU. `tools/anime4k_to_gles.py` converts the mpv user shader into GLSL ES 3.00 fragment shaders
(`s3-android/src/anime4k.rs`, generated: 4 conv passes + depth-to-space); `s3-android/src/upscale_ai.rs` runs
them per presented game frame: 4 passes at the frame size into RGBA16F textures (ping-pong, FBO), then
depth-to-space into an RGBA8 texture of twice the frame size (= bilinear frame + the network's luma residual),
which the presenter draws instead of the frame (same normalized coordinates, so the Classic split / quads work).
Needs a GLES 3 context: the presenter now asks for an ES3-capable config + context and falls back to GLES 2
(then and on any shader / framebuffer error: sharp bilinear, logged once). Used only when the frame is shown at
>= 1.2x and not for menu layouts. Fold4 inner screen (2176x1812) at 400 px height: 120 fps like sharp bilinear;
units, buildings and rocks look smooth and detailed instead of blocky (softer than xBR). Cycle with "Filter" in
the HUD settings or the Classic menu's floating buttons (new "Filter" button there).

**Plan, not started: HD sprites (option 2).** If shader upscaling is not enough: upscale the game's sprites and
landscape textures 2-4x with an AI model (Real-ESRGAN class) **generated locally from the user's own files** (never
distributed: derived game assets), and render the game at 2x internally. Needs a native draw path for sprites,
landscape, shadows, selection marks in a 2x frame (our fast paths `fast_tile`, `fast_land_rows`, sprite rows are the
starting points), hit testing / mouse at 2x, and per-animation consistency (AI upscaling flickers between frames,
palette). Generation is slow on a phone (desktop tool + copy is more realistic). Biggest work item of the port so far.

**FPS limit and FPS counter position (2026-11-08).** `DisplaySettings::fps_limit` (30 / 60 / 120 / 144, plus the
display's refresh rate when it is above 144, `display::fps_limit_options`; default 60) is applied to the
DirectDraw frame limiter (`s3_dx::ddraw::set_fps_cap`) at install and on change (`display::apply_fps_limit`); a
developer override wins (env `S3_FPS_CAP`; Android `fps-cap.txt` sets that env). `DisplaySettings::fps_pos`
(auto / top left / top centre / top right / bottom right / bottom centre / bottom left) places the counter in the
free area: right of the Classic panel, below the HUD's resource strip, above the HUD's bottom bar. Both are
cycled with one tap: Mobile HUD Menu > Settings ("FPS limit", "FPS position"), Classic Menu floating buttons
("FPS limit", "FPS pos"). Desktop: 144 -> 82-86 fps (desktop speed), counter bottom right above the bar.

**Cursor in the window layer (2026-11-08).** `s3_runtime::cursor` drew the game's Windows cursor into the presented
frame, so it grew, shrank and moved with the zoom. With a window layer, `cursor_layer::CursorMod` sets
`CursorState::disabled` and blits the same image (RGBA copy per HCURSOR, XOR pixels as their colour) at
`game_to_window(mouse)` minus the hot spot, at `Layer::ui` scale (its size in a 600 px high frame), last (on top).
The window position is kept while the game position does not change, so a zoom does not move it on screen.
Without a window layer (old frontends) the frame cursor stays.

`window_to_game` / `game_to_window` / `scale()` (= map part) / `quads()` (source -> destination rectangles: one, or
panel + map) handle it, so touch mapping, the GPU presenter (one quad per part, sharp-bilinear scale per part), the
CPU scaler (`scale_to_xrgb`) and the mods agree. Fit, Fill, Stretch and menus (layouts) are unchanged.
`S3_PANEL_SPLIT=0` turns it off.

### Window overlay layer

Mods draw their UI at window resolution ([mods.md](mods.md) "Window layer"); the presenter blends the layer over the
game quad (Android: 2nd RGBA texture, only changed rows uploaded; s3run: CPU blend). The game frame contains only
game pixels (and the game's mouse cursor).

Gestures stay in game pixels (as before); thresholds (`touch.tap_slop` ...) are therefore in game pixels in every mode.

### Settings JSON (`<data dir>/mods-settings.json`)

```json
"display": { "mode": "NativeWide", "filter": "SharpBilinear", "fps_counter": "Off", "refresh_hz": 0 }
```

`mode`: `Fit` | `Fill` | `Stretch` | `NativeWide`; `filter`: `Nearest` | `Linear` | `SharpBilinear`;
`fps_counter`: `Off` | `Fps` | `Detailed`; `refresh_hz`: 0 = highest the panel offers, else e.g. 60 / 90 / 120.
Missing fields use the defaults (old files keep loading).

## FPS counter (`s3-mods/src/fps.rs`)

A mod (`FpsMod`, installed last so it draws on top of the HUD) that counts **real guest presents**
(`Machine::stats.presents`; overlay-only refreshes are not counted) and guest instructions over 500 ms windows.
Drawn small in the top-right corner of the visible game area (in the Mobile HUD at the top centre, since
2026-11-05 verify-fps1: top right it covered the full-screen map's "Close" and the "Back to HUD" button) (Bold 11 px, dark backdrop; green ≥ 55 fps, yellow
≥ 30, red below). `Fps`: "60 fps". `Detailed`: + "16.7 ms (max 33)" frame time avg/worst, "100 MIPS", and on Android
"disp 120/120 Hz" = presenter swaps per second / panel refresh rate. Toggles: settings JSON, `s3run --fps`, and the
UI modes' menus (they call `display::cycle_fps`).

## Android presenter (`s3-android/src/present.rs`, `gl.rs`)

Before: the activity thread copied the frame into an `ANativeWindow_lock`ed RGB565 buffer sized with the window's
aspect (letterbox) and the compositor scaled it; Fit only, and drawing ran on the input thread.

Now a **render thread** (`s3-present`):

1. The engine's `Host::present` copies the frame into `Shared::frame` and signals `Shared::frame_cv`; it never waits
   for the display (no vsync wait, no GL on the engine thread). The activity thread is no longer woken per frame.
2. The render thread waits on `frame_cv` (or 250 ms), copies the newest frame, uploads it as an RGB565 texture
   (`glTexImage2D` once per size, then `glTexSubImage2D`, `GL_UNSIGNED_SHORT_5_6_5`: no CPU conversion or scaling),
   draws one quad placed by the `Viewport` with the selected filter (sharp bilinear = fragment shader
   `gl::FS_SHARP` on a `GL_LINEAR` texture), the letterbox buttons as small textures (re-rendered only when size /
   state change), then `eglSwapBuffers` with swap interval 1. Only new frames are drawn (no redraws of the same
   frame), so at 60 game fps on a 120 Hz panel it swaps 60 times a second.
3. High refresh: after each new surface it calls `ANativeWindow_setFrameRateWithChangeStrategy(win, hz, DEFAULT,
   ALWAYS)` (API 31; `ANativeWindow_setFrameRate` on API 30; looked up with dlsym so the APK still runs on API 26).
   The activity thread additionally sets `WindowManager.LayoutParams.preferredDisplayModeId` / `preferredRefreshRate`
   to the mode with the current resolution and the highest refresh rate (or closest to `refresh_hz`), via JNI
   (`uithread.rs` `display_prefs`). The chosen rate is shown by the detailed FPS counter.
4. Full screen: the same JNI job sets `layoutInDisplayCutoutMode = ALWAYS` (API 30; `SHORT_EDGES` on 28-29), so the
   window covers the camera-cutout strip too (on the Fold4 cover screen the window was 2229 of 2316 px wide).
5. Window lifecycle: `Presenter::set_window(Some/None)` from `InitWindow` / `TerminateWindow` / resize waits until
   the render thread destroyed / created its EGL surface (Android requires the surface gone before
   `onNativeWindowDestroyed` returns). The EGL context survives window changes.
6. Fallback: if EGL/GLES init or surface creation fails, the render thread uses the old CPU path (Fit only) and
   publishes its `Letterbox` so touch mapping and button hits keep matching.

Touch mapping (`android.rs`): `Shared::viewport` (written by the render thread) → `Viewport::window_to_game`;
buttons via `overlay::hit_vp`. The engine thread forwards the viewport and display rates to the mods once per slice
(`frontend.rs` `sync_display`) and pushes the display settings to the presenter (`Shared::set_display`).

Measured on the phone (verify-fps1, 2026-11-05, Galaxy Z Fold4 cover screen): logcat `display: modes
[8:904x2316@120 9:...@96 10:...@60 ...] -> Some((8, 120))`, `present: GLES surface 2316x904 (window 2316x904)` (the
full panel width now, was 2229), `ANativeWindow_setFrameRateWithChangeStrategy(120) -> 0`; detailed counter
"disp 60/120 Hz" at 60 game fps. The `s3-present` thread uses ~10-14 % of one core (top) and 6.6 % of the process
cycles (simpleperf), the engine thread never waits for it. Native wide on the cover screen gives 1536x600 at the
default zoom (aspect 2316/904); HUD and Classic both fill the whole screen; Back closes the HUD full-screen map.
fps per mode and zoom: [performance.md](performance.md#phone-measurements). The FPS gain comes from the engine side
(perf-cpu + verify-fps1 fast paths), not from the presenter. The presenter's job is
to not get in the way: no blocking of the engine thread, vsync-paced swaps at the panel's high refresh rate.

## Desktop `s3run`

Window is resizable; the frame is scaled on the CPU into a window-size buffer with the same `Viewport` and filters
(`display::scale_to_xrgb`, per-axis tap tables; `minifb` `ScaleMode::UpperLeft`), mouse positions go through
`window_to_game`. Options (override `mods-settings.json` for this run, not saved):

```text
--display fit|fill|stretch|native   --filter nearest|linear|sharp   --fps off|fps|full   --window WxH
```

Headless with `--window WxH`: a simulated viewport for that screen size is published to the mods (so Native wide
and Fill can be tested headless). `shot f.png` writes the game frame at game resolution; `wshot f.png` writes the
window as the player sees it (frame scaled with the display mode / panel split + the mods' window layer) at the
`--window` size. Touch injects with a `w` prefix (`wtap`, `wpinch`, ...) take window pixels.

Example: `s3run --game-dir game-files/gog-s3-ultimate --data-dir scratch/d --window 2229x904 --display fill --fps full`.

## Tests

- `cargo test -p s3-mods display:: fps::` — viewport math for all modes on the Fold4 cover size, CPU scaler
  (nearest / sharp = nearest at integer scale / linear), sharp tap monotonicity, enum parsing, FPS windows.
- `cargo test -p s3-android` — viewport button layout / hit / button image, GL clip rect.
- Android build: `scripts/build-apk.sh --abi arm64-v8a` (cargo ndk); device verification is a separate phase.
