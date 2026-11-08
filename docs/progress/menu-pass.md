# menu-pass (2026-11-08)

Goal: every screen reachable from the main menu (incl. popups / MP errors / Credits / Help / Tips) looks right at
2316x904 (layouts) and 2176x1812 (full variants); no switch to tiny Fit when a popup appears.

Worktree: `.claude/worktrees/menu-pass`, branch `menu-pass` (separate cargo target in its port/target).

## Done
- Reproduced: Internet -> "Internet connection not detected." msg box over main menu -> no layout -> tiny Fit.
  Same msg box (200,160,400,200) for LAN "provider has to be selected", "First select a game", Campaign: Normal.
- menu_layout.rs: `Popup`, `MSG_BOX`, `POPUPS`, `with_popup` (cached leaked composite "<screen>+msg-box": base
  canvas/background + edge strips + dialog centred), `detect` checks popups first, `matches_outside`, `by_name`,
  `full_variant` of a composite = base full. `CREDITS` (band y144..456) + `credits-full`. Tests added (pass).
- s3-desktop headless: published viewport now via `for_frame_split` (wtap mapped through layouts like wshot).
- Verified wshot 2316x904 + 2176x1812: popup big, OK tap closes it; credits band.
- Exit Game: exits at once (no confirm). Online Help / Tips: ShellExecute MANUAL/TIPS main.htm refused -> nothing.

- Docs (menu-layout.md "Popups", journal), `cargo test -p s3-mods` ok, `build-apk.sh` ok, committed.

## Open
- Online Help / Tips & Tricks: an in-app HTML viewer (Android needs a ContentProvider or WebView; desktop could
  open the default browser) - not done.
- Phone check of the popup layout (GLES path renders pieces generically; not run on the device).


## Verify
- `cargo test -p s3-mods`, `sh port/scripts/build-apk.sh`, wshot shots in scratch/menu-pass/
