# input-ok-bug (progress) — FINAL

Goal: "first OK click on the players screen is ignored" (desktop + Android). Plus GetProcAddress names
kernel32 `IsProcessorFeaturePresent`, user32 `GetLastActivePopup`.

## Result
- **No input bug.** Single Player has three OK screens: map-selection popup (OK 630,467) over Game Setup,
  Game Setup (OK 730,567), players screen (OK 730,567, same spot). Earlier notes counted the popup OK as
  "Game Setup OK", so the Game Setup OK looked like an ignored players-screen OK. Proof: S3_TRACE + a temporary guest
  stack dump: the "ignored" click ran the Game Setup OK handler 0x488300 (CoCreateGuid in 0x431ab0); the next click
  ran the players handler 0x4955b0 -> PostThreadMessageA(0x801) -> loading. Headless
  `--inject "1000:click 165 235;2000:click 630 467;3000:click 730 567;4000:click 730 567"` reaches in game at 25 s.
- How S3 reads menu clicks: window messages on t0 (WM_MOUSEMOVE/LBUTTONDOWN/UP -> SetEvent wakes GUI thread t2);
  DirectInput GetDeviceData with rgdod=NULL only flushes. Documented in docs/porting/user32-gdi32.md.
- Added `kernel32!IsProcessorFeaturePresent` (TRUE for 2 CX8 and 8 RDTSC, matching our CPUID; else FALSE) and
  `user32!GetLastActivePopup` (returns hwnd). No more "not declared" warnings.
- Test `s3-win32 user_gdi_tests::tap_gives_one_press_per_tap` (tap -> MOUSEMOVE, LBUTTONDOWN, LBUTTONUP, twice at
  the same spot; GetProcAddress + behaviour of the two new functions).
- Docs: status.md (known issues rows removed), android.md (flow + tap coords), integration-log.md (recipe comment),
  kernel32.md, user32-gdi32.md.
- Temporary S3_STACK stack dump in s3-runtime machine.rs was removed again (not kept).

## Verify
CARGO_TARGET_DIR=port/target cargo test -j 4 -p s3-win32 -p s3-dx  (all pass)
s3run headless with the 4-click recipe above -> in game.
