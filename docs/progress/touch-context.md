# touch-context (W3 context-aware touch) — FINAL 2026-11-05

Goal: context-aware touch shared by both UI modes (mobile-vision.md W3). Doc: docs/porting/touch.md.
Owns: s3-mods/src/touch*.rs, s3-android/src/touch.rs (+ android.rs/frontend.rs input path), s3-desktop/src/touch_inject.rs.

## Done
- s3-mods: touch.rs (mod + HandleInput hook), touch_logic.rs (pure + 16 tests), touch_game.rs (GOG addrs), touch_fx.rs.
  lib.rs: modules, `right_next()`, touch::install before ui_classic/ui_hud (UI modes on top). fallback.rs: push/click pub(crate).
  settings.rs: `touch.context` (default true).
- desktop: touch_inject.rs (tap dtap tap2 tap3 drag drag2 fling longpress lpdrag pinch); main.rs delegates.
- android: touch.rs = MotionEvent -> HostEvent::Pointer (+ PointerFilter); android.rs uses it; frontend.rs Shared
  rmb_toggle/rmb_armed/keyboard_req synced with s3-mods in drive(); s3-mods optional dep (engine feature).
  `cargo ndk -t x86_64 check -p s3-android` OK. NOT run on emulator (task did not grant emulator use).
- Verified headless (scratch/touch-context/r*.png, z2.png): box select, move flag, rings, deselect, camera drag (exact),
  fling glide, ✕ + 2-finger cancel, long-press ring, pinch 640<->800.
- Tests: s3-mods, s3-desktop, s3-android pass. Workspace: only s3-dx dplay_factories_and_stubs fails (mp-dplay's WIP, not touched).

## Open
- attack on enemy, double-tap same-type select: logic unit-tested only.
- pinch right after a resolution switch is ignored by the game (cool-down, HYPOTHESIS).
- Android multi-touch on a device/emulator untested.
- ESC/ABC/RMB letterbox buttons kept (no UI-mode equivalent for keyboard yet).
