# Progress: overlay-window (window-resolution overlay layer)

Goal: mods (Mobile HUD, touch feedback, FPS counter, Classic floating UI) draw into a separate RGBA layer at WINDOW
resolution (constant on-screen size + sharp at every zoom); frontends composite it; guest frame = game pixels only.
Classic panel drawn at a constant on-screen scale (presenter + Viewport split). Lower zoom minimum (HUD).
Branch: worktree-agent-a4fc43414e521fc52. Scratch: C:\Users\<user>\Projects\settlers-3\scratch\overlay-window\.

## Design (decided 2026-11-08)
- Mods record a display list (`s3-mods/src/layer.rs`: `Layer` recorder, `DrawList` of commands with hashes) on the
  engine thread at present/refresh time (`MachineExt::window_overlay`); unchanged list = nothing to do. The list is
  published to a `SceneSlot` sink registered by the frontend; the frontend rasterizes it (`LayerRaster`, premultiplied
  RGBA at window size, only the dirty rect of changed commands) and composites (GL 2nd texture / CPU blend).
- `HostEvent::Pointer` gets window coords `wx, wy`; mods hit-test HUD in window px (`crate::touch_window_pos`).
- `Viewport.panel: Option<PanelSplit>`: Classic + NativeWide + zoom on + in game: panel region x<pw,y<art_h shown at
  win_h/art_h, map fills the rest; zoom width computed so the map keeps its aspect.

## Status
- 2026-11-08: reading code, design done. Next: s3-runtime event + ext changes.
- 2026-11-08 ~08:30: implemented (not committed yet): s3-runtime (Pointer wx/wy, ext.window_overlay, refresh
  only re-presents for frame overlays), s3-mods layer.rs (recorder + raster + SceneSlot; Canvas shares the
  rasterizer), display.rs (PanelSplit, Viewport::new_split/for_frame_split/quads/panel_scale, frame_viewport,
  panel_split), lib.rs (draw_window, draw_viewport, touch_window_pos, set_layer_sink, last_layer), mods ported:
  fps, debug_overlay, touch/touch_fx, ui_classic, ui_hud(+minimap); zoom: levels 200/240 added (DEFAULT 10,
  CLASSIC_MIN 7), res::MIN_H 200, classic_width + anchor_camera_map; s3-desktop: window layer blend, `wshot`,
  `w`-prefixed touch injects (viewport frozen while fingers down); s3-android: present.rs layer texture + split
  quads, touch window coords, Shared.layer. `cargo test -p s3-mods` OK; android `cargo check --target
  aarch64-linux-android` OK. Determinism s3det 90000: IDENTICAL to baseline (fast + JIT).
- 2026-11-08 ~09:30: verified headless (HUD 1536x600 / pinch in 760x296 / out; HUD min 512x200 with icons via
  harvest >= 480; Classic 1232x480 .. 2952x1200 panel constant, magnifier, panel tap through split viewport, Menu
  button). Fixed: Layout::read 2048 cap (Classic overlay missing > 2048 wide). fps desktop HUD 374 -> 536.
  APK arm64 builds. Docs: mods, display, resolution-zoom, ui-hud, ui-classic, touch, multicore, performance,
  journal, AGENTS. Open: phone test (main session), game mouse cursor still in the frame (scales with zoom),
  s3-runtime vfs test fails on Windows (path separator, pre-existing).
- Verify: scratch/vis.sh (headless 2316x904, wshot) -> scratch/overlay-window/vis/<name>/.
