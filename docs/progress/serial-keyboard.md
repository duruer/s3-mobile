# serial-keyboard

Goal: user report (Reddit, 2026-11-09, Motorola Android phone): the game reaches the main menu, then asks for the
serial number, but the soft keyboard never shows, so the player is stuck.

Facts:
- The serial prompt appears when the game dir has no `regs.cmd` (non-GOG install, or GOG files copied without it;
  `registry_store.rs` logs "no regs.cmd in the game dir"). Reproduced on desktop with a link copy of the GOG dir
  without `regs.cmd` (scratchpad `noregs/`, hardlinks + junctions; never commit). Enter = OK in that dialog; the
  key must be typed with dashes.
- Since 2026-11-07 (letterbox buttons removed) the only way to open the soft keyboard on Android was a three-finger
  tap, never tested on a real device. HYPOTHESIS: Motorola's "three-finger screenshot" gesture eats it.
- RE (docs/reverse-engineering/game-state-ui.md section 8): `CEditControl`, vtable slots, event types, focus
  handler slot 25, flags `+0x0c` read-only, `+0x45` number field.

Done (code, all compiling, tests green):
- `s3-mods/src/text_input.rs`: hooks 0x419b30 (tap on field -> Show), 0x419b10 (focus: number field gets it ->
  Show; owner loses it -> Hide), 0x421f50 (dtor of owner -> Hide). Installed for any exe whose bytes match the
  1.60 build (not only GOG sha), skipped with `patches_enabled: false`.
- `s3_mods::KeyboardRequest {Show=1, Hide=2, Toggle=3}`, `request_keyboard`, `take_keyboard_request -> Option`.
  Three-finger tap = Toggle. `Shared::keyboard_req` is now `AtomicU8` (same codes as iOS `S3Out::keyboard`);
  Android maps 1/2/3 -> `soft_keyboard(Some(true)/Some(false)/None)`.
- Tests: `tests::text_field_hooks_request_the_soft_keyboard`, `text_field_hooks_need_the_160_code`.
- Desktop verified (s3run logs): serial dialog -> show; Enter -> hide; invalid -> OK -> dialog again -> show.
  Game Setup: tap name -> show; tap Map field -> hide; tap name -> show; OK -> hide.

Docs updated (game-state-ui §9, android, status, touch, ios, README, journal, AGENTS). APK builds (both ABIs).
No emulator on the ARM64 dev machine and no phone attached: not seen on Android yet.

Next:
1. Build APK (`port/scripts/build-apk.sh`), check on our AVD with a game dir without `regs.cmd`
   (`game-dir.txt` override in the data dir) that the keyboard appears by itself.
2. DONE in code (2026-11-09, second commit): keyboard pan. `s3-frontend/src/ime.rs` `keyboard_pan(vp, field,
   ime_h)`; `Shared::{ime_h, text_field, pan_y}`; `GlRenderer::draw(.., pan_y)` -> `glViewport(0, pan_y, ..)`
   (frame + layer move up); Android touch adds `pan_y` to the window y; `UiThread::refresh_keyboard` reads
   `getInsets(ime()).bottom` (API < 30: half the view height), polled every 100 ms for 2 s after a request.
   Tests `ime::tests::*` (incl. the real `main-menu+msg-box` layout at 2316x904). Not seen on a phone.
   iOS: shared draw path, but nothing sets `ime_h` yet (no move).
3. GOG exe without regs.cmd: the typed serial is stored encrypted and the GOG exe (decoder NOPed) cannot read it
   back -> HYPOTHESIS: asks again every start. Possible fix: decode the write (XOR, `imul 0FED42531h`, HKCR Seed).
