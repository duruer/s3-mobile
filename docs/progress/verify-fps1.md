# verify-fps1 (verification 1 of fps/display/res-zoom/hud-polish workflow, 2026-11-05) — FINAL

Goal: verify perf-cpu, display, res-zoom, hud-polish on desktop + phone (10.0.0.50) / emulator-5610;
phone >= 60 fps in game default zoom mode A; fix hotspots/regressions; update status/performance/display docs.

## Result
DONE: phone Classic default zoom 60 fps (cap); HUD default 48-58. All 6 user items verified (pinch only on desktop:
adb cannot inject multi-touch). Code: s3-mods fast_snap.rs, fast_obj.rs, fast_land.rs (+ fast.rs/lib.rs registration,
fast_tests.rs differential tests), fps.rs (HUD: counter top centre). Docs: status.md, performance.md (Phone
measurements), display.md, resolution-zoom.md. cargo test --workspace --release: 257 passed, 1 ignored.
User's phone settings restored from scratch/vfps/phone/settings-user-backup.json; app force-stopped.

## Open
- HUD default 48-58 fps: next fast path = 0x4b26c0 tile loop (46 % in HUD).
- Seen once on the phone: after a Classic session (menu zoom/screen taps) mods-settings.json said ui_mode MobileHud,
  zoom_level 9 - not reproduced/explained (maybe a mis-tap on "UI: Mobile HUD"/zoom buttons). HYPOTHESIS only.
- Classic at render heights between panel variants: area below the panel art shows stale pixels ("Quit Game" text
  repeated) - known res-zoom open item.

## Log
- 16:05 workspace build+tests green; desktop HUD headless OK (minimap tap->full map->tap jumps, settings page, pinch 1040x780) scratch/vfps/hud/*.png
- 16:11 APK installed on phone (install -r -g), launches, GLES 2316x904 @120 (setFrameRate ok), full width.
- Phone HUD in game 1536x600 default: 36 fps, 28 ms, 250 MIPS, disp 35/120 (scratch/vfps/phone/fps1.png). User settings backup scratch/vfps/phone/settings-user-backup.json (was MobileHud).
- Phone Classic Wide: lvl7 1536x600 38-40 fps (250 MIPS), lvl6 40, lvl5 44, Fit lvl5 60 (cap). Pinning to cpu7 no real gain.
- Desktop profile (scratch/vfps/prof): per frame ~10k snapshot tiles; 0x51f110 snapshot fill 23%, 4b26c0 18, 54cbd0 17, 4d3050 17.
- NEW fast path s3-mods/src/fast_snap.rs (snap_tiles 0x522219..0x52303e, 133 instr common path) + differential test
  fast_tests::differential_snap_tiles (passes). NEXT: s3det on/off identity, fps measure, APK, phone.
- NEW fast path s3-mods/src/fast_obj.rs (obj_empty: 0x4d3050 per-tile object drawer, empty-tile returns, 110-116 instr)
  + differential_obj_empty. s3det S3_FAST=0 vs all fast paths: IDENTICAL (scratch/vfps/det, insns=11817005518).
  Desktop 2316x904 Classic uncapped: 35 -> 43-45 fps (snap only). NEXT: phone measurement with new APK.
- obj_empty extended (land 2..7 early returns); NEW fast_land.rs (land_clean: 0x54ce53 landscape triangle loop, clean
  triangles) + differential_land_clean. s3det all fast paths vs S3_FAST=0: IDENTICAL; s3det wall 55.9 -> 41 s.
  Desktop 2316x904 Classic uncapped 71 fps. Phone (snap+obj v1) Classic lvl6 1536x600: 54-56 fps. NEXT: phone with land.
- PHONE with snap+obj+land: Classic lvl6 1536x600 60/62/60 fps at cap (engine thread 94% busy), 412-419 MIPS-equiv.
  HUD lvl6: 58/56/48 fps (48 with lots of animated water on screen). scratch/vfps/phone/cB6-*, hB6-*.
- MP two-instance (with zoom pinches, all fast paths): IDENTICAL (scratch/vfps/mp). Desktop save/load round trip OK (scratch/vfps/sl/all.png).
- Phone: HUD minimap tap -> full map OK, tap jumps camera OK. Found: FPS counter covered the map's Close button ->
  fps.rs draws top centre in Mobile HUD. NEXT: rebuild APK (both ABIs), zoom-level fps table, restore user settings, docs.
