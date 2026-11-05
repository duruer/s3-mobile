# verify1 (progress note) — FINAL 2026-11-05

Goal: end-to-end verification on desktop + Android (emulator-5610). Result: DONE, full flow works on both.

## Done
- cargo build --workspace --release + cargo test --workspace: 159 passed, 1 ignored, 0 failed.
- Desktop (scratch/verify1/d*.png, run3.log): menu -> SP -> in game ~17 s -> build list/ghosts -> save -> quit ->
  load -> continues; cursor visible; WAV non-silent in game. One slow-load run (~77 s) not reproduced.
- Android release APK (arm64-v8a + x86_64), emulator-5610: menu, soft keyboard typing (ABC button + adb text + soft key),
  in game, building placed, save, Back (harmless), Home+reopen, drag scroll, quit, stats, Load Game, saved game
  continues; cursor everywhere. Shots scratch/android-shots/verify1-01..23.
- No code changes needed. Docs: status.md, android.md (verification 1 + coordinates; Load Game is 800x600),
  integration-log.md (verify1 section).

## Open
- Esc/Back opens no in-game menu (game behaviour, desktop same). Three-finger tap untested (adb). Real phone untested.
