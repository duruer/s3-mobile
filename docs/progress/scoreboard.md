# scoreboard (2026-11-08)

Goal: end-of-game statistics screen fills the window (wide cover / tall inner layouts, touch mapping), plus
Settlers-United-style improvements.

## Done
- RE: stats records `0x7acfac + id*0x44` (20 players), score formula in `0x53e720` (game-state-ui.md).
- SU evidence: `s3.dll` strings "View Extended Game Stats", `#gamestats-container`, Points/Settlers/.../Battles,
  "Count", "Game Time"; launcher saves stats screenshots (settlers-united/README.md).
- Layouts `statistics`, `statistics-20` (+ `-full`) in `menu_layout.rs`; test `statistics_screen`.
- Mod `end_stats.rs` (sampling + chart sheet), registered in `lib.rs`; tests pass.

## Verified (headless, scratch/scoreboard/r3 2316x904, r4 2176x1812)
- statistics screen full window (layout `statistics` / `statistics-full`), button opens sheet, tab switch,
  tap inside sheet consumed, Close back, tap elsewhere -> main menu. APK builds.

## Open
- not tried on the phone; >10 players (`statistics-20`) only unit-tested; history not saved; no stats images.

## Verify
- `cargo test -p s3-mods --lib`; `s3run --layout-preview <shot> out.png 2316x904`.
