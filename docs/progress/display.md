# display (workflow wf_a446e364-b12, 2026-11-05) — FINISHED (build phase; device verify pending)

Goal: GPU present path on Android (EGL/GLES, high refresh, render thread), display modes Fit/Fill/Stretch/Native wide,
shared screen->game transform, FPS counter (s3-mods/src/fps.rs), desktop s3run modes + --fps, docs/porting/display.md.

## API for the other agents (stable)
- `s3_mods::display` (port/crates/s3-mods/src/display.rs): `DisplayMode {Fit, Fill, Stretch, NativeWide}`,
  `ScaleFilter`, `FpsCounter {Off, Fps, Detailed}`, `DisplaySettings` = `Settings.display` (JSON `display`).
- `display::Viewport` = THE screen<->game transform (`window_to_game`, `game_to_window`, `visible_game_rect`, `bars`,
  `Viewport::native_wide_size(win_w, win_h, game_h)`).
- Machine helpers: `viewport(m)`, `window_size(m)`, `visible_game_rect(m)`, `display_settings(m)`, `set_display`,
  `override_display` (not saved), `cycle_mode/cycle_filter/cycle_fps(m)`.

## Done
- s3-mods: display.rs, fps.rs (FpsMod added last in install), settings `display` field, Classic menu Screen/FPS
  toggles + floating buttons anchored to the visible rect, touch ✕ anchored (View.vis, cancel_button_in).
- s3-android: gl.rs, present.rs (render thread, EGL/GLES2, sharp-bilinear shader, dlsym setFrameRate*, CPU fallback),
  overlay layout_vp/hit_vp/button_image, frontend Shared presenter fields + sync_display, android.rs uses Presenter,
  uithread display_prefs (cutout ALWAYS, preferredDisplayModeId highest refresh). s3-mods now a non-optional dep.
- s3-desktop: --display/--filter/--fps/--window, resizable window with CPU scaling (display::scale_to_xrgb),
  headless --window publishes a simulated viewport.
- docs: docs/porting/display.md (new), android.md, mods.md, ui-classic.md, AGENTS.md row.
- Verified: cargo test s3-mods (55) / s3-android (25) / s3-desktop (8); cargo ndk check + scripts/build-apk.sh
  --abi arm64-v8a OK (libEGL/libGLESv2 NEEDED); headless in-game HUD + Classic with --window 2229x904 --display fill
  --fps (screenshots scratch/display/a.png, c2.png); short windowed s3run run OK.

## Next (Verify phase, device)
- Phone: check logcat "present: GLES surface", "setFrameRate...", "display: modes [...]"; FPS counter "disp x/120 Hz";
  touch mapping in Fill/Stretch; window lifecycle (home/back, fold/unfold) — set_window waits for surface destroy.
- If EGL fails on a device, the CPU fallback (Fit only) is logged "using the CPU path".

## Verify
- `CARGO_TARGET_DIR=port/target cargo test -p s3-mods -p s3-android -p s3-desktop -j 4`
- `port/scripts/build-apk.sh --abi arm64-v8a`
