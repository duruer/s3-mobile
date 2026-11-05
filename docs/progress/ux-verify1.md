# ux-verify1 (progress note)
Goal: verify touch-context + ui-classic + ui-hud + mp-dplay together: cargo test, desktop inject (both modes),
mp-local-test.sh, Android (emulator-5610) both modes + join desktop-hosted MP via 10.0.2.2; update status.md + integration-log.md.
## Done
- (started 2026-11-05)
## Next
- build/test workspace
- cargo build --workspace --release OK; cargo test --workspace --release: 231 passed, 0 failed, 1 ignored (dplay test passes now)
- Fix: Android had no INTERNET permission (manifest) and no way to set relay URL -> added
  `<data>/relay-url.txt` read in s3-android frontend.rs run_engine (sets S3_RELAY_URL). APK build started.
- Desktop mode A run: scratch/ux-verify1/A (in progress)
- Desktop mode A (scratch/ux-verify1/A): box select, tap move, tap2 deselect, drag2 pan, panel taps -> placement by tap, Menu -> save -> quit -> load: ALL OK
- Desktop mode B first run (B): BUG: touch-context inactive in HUD (touch_game::layout required panel_w>0, HUD sets 0) -> taps were plain left clicks.
  FIX touch_game.rs layout(): allow panel_w >= 0. B2: move/deselect/pan now OK. Save/quit/load in HUD OK (B).
- Missing: no way Classic -> HUD in game. FIX ui_classic.rs: FloatBtn::HudMode "UI: Mobile HUD" above Close while menu (page 7) open.
- Cosmetic FIX touch.rs draw: hide ✕ cancel when ui_mode == MobileHud (HUD banner has Cancel; ✕ overlapped resource strip).
- Android mode A (old APK + INTERNET fix): tap select, tap move, panel build + tap place: OK (ux1-03..09). adb draganddrop = pan (no long-press box).
- Running: B3 (HUD placement), A2 (Classic->HUD switch, pinch), APK rebuild.
- Desktop B4: HUD tap placement OK (site placed, slot 1->0). A2: Classic->HUD switch OK (cover mode). Android: mode A, switch A->HUD (cover), HUD full-screen after restart, HUD->A all OK, tap select/move/place OK both modes.
- Desktop MP (scratch/ux-verify1/mp): 3104 ticks identical, both in game, both built.
- Android joined desktop host via relay ws://10.0.2.2:7788 (relay-url.txt): session listed, joined, both in waiting room, ready, game started on both.
- BUG found: in MP [0x7e3408] gd global = 0 -> touch-context "not in game" -> taps = plain left clicks (move impossible).
  FIX touch_game.rs: gd() falls back to [logic+0x3a0] recorded by HandleInput hooks (note_logic; touch.rs + ui_hud_game.rs logic_hook);
  ui_hud_game::gd uses the same fallback. Desktop MP mp5: tap -> Command (move), units move, IDENTICAL hashes.
- Added S3_TOUCH_TRACE eprintln of tap decisions in touch.rs.
- Next: APK rebuild (running), Android MP retest with tap move, full cargo test, docs.
- Android MP retest with fixed APK (mp-android2): join desktop host, tap select + move OK in mode A and HUD; desktop host moved 14 soldiers by tap.
- Final: cargo build --workspace --release OK; cargo test --workspace --release 232 passed / 0 failed / 1 ignored; SP desktop regression run (final/) in game, tap -> Command.
- Docs updated: status.md, integration-log.md (ux-verify1 section), touch.md, ui-hud.md, ui-classic.md, multiplayer.md, android.md, game-state-ui.md.
- Emulator left with ui_mode ClassicTouch, relay-url.txt = ws://10.0.2.2:7788/ (harmless when no relay).
## Status: DONE
