# android-ux (progress note) — FINAL 2026-11-05

Goal: Android UX: on-screen keyboard (IME) with VK + WM_CHAR, immersive fullscreen, Back=Esc, 3 letterbox
buttons (Esc / keyboard / right-click toggle), clear first-run screen.

## Done (all in port/crates/s3-android + docs/porting/android.md)
- keys.rs: char_for_key / vk_for_char, comma/period VKs. android.rs sends Key + Char (KeyCharacterMap).
- touch.rs: two-finger right click now fires on lift; three-finger tap -> keyboard request; right_next mode.
- overlay.rs (new): letterbox buttons ESC / ABC / RMB, layout, hit test, drawing (host compose step).
- uithread.rs (new, android only): JNI jobs on the Java main looper (socket pair + add_fd_with_callback):
  immersive (WindowInsetsController / setSystemUiVisibility), IME show/hide/visibility, Activity.finish.
- frontend.rs: REQUIRED_PARTS / missing_parts wait, first-run text; guest_exited -> app finishes + exit(0).
- screen.rs: first_run_screen.
- Cargo.toml: jni = "0.22" (android target only; same version android-activity uses).

## Verified (emulator-5610, x86_64 APK)
- immersive (bars hidden), buttons drawn, typing into Game Setup name field via soft keyboard + adb input text,
  Back hides IME, ABC highlight clears, RMB mode, Back in main menu exits game -> app closes, relaunch OK,
  in game + build menu ghost sites, first-run screen + auto start after files appear.
- cargo test -p s3-android: 28 pass. s3run headless reaches in game.

## Open
- Back/Esc does not open an in-game menu (game/engine: Esc does nothing in game on desktop too; the panel's
  computer-icon menu draws empty). Needs engine/HLE work.
- Three-finger tap not tried on a device (adb has no multi-touch).
