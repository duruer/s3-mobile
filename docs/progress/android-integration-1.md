# Progress: android-integration-1

Goal: build APK (release, arm64 + x86_64), run on emulator s3_api35 (5610), reach the same point as desktop
(menu -> scenario -> in game), fix Android-specific problems, update docs/porting/android.md.

## Done
- (starting) read android.md, integration-log.md

## Next
- build-apk.sh, install, check game files on device, launch, screenshots, tap test.
- 02:50 APK built (both ABIs), files on device identical (1191 MB), main menu -> scenario -> in game on emulator,
  build menu + construction site placement by tap work. In game ~30 presents/s, ~91 M instr/s, 1 core ~92 %.
- Arrow keys scroll. Right-button drag scrolls the map (view moves in drag direction).
- touch.rs: drag_button_for() (in game, x>=170: right drag), drag_pos() mirrors right drags (grab scroll);
  android.rs clamps MouseMove. Tests pass. NEXT: verify on emulator, then docs.
- 02:58 Verified grab scroll (factor 1/2 mirrored), pause/resume. Final APK (both ABIs) built:
  port/target/apk/settlers3port.apk. docs/porting/android.md updated (status, controls, install, known issues).
## Result
Done. App left running in game on emulator-5610; emulator left running.
