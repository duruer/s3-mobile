# Progress: android-shell

Goal: s3-android NativeActivity frontend (render RGB565 letterboxed, touch->mouse, AAudio, lifecycle, status screen),
scripts/build-apk.sh (no Gradle), scripts/run-emulator.sh (AVD s3_api35 on port 5610), docs/porting/android.md.

## Done
- crate: layout.rs, touch.rs, screen.rs, audio.rs, keys.rs, frontend.rs (host-tested, 18 tests), android.rs.
- feature `engine` (default) makes s3-engine optional; `build-apk.sh --shell` builds without it.
- AVD s3_api35 created and running (emulator-5610). Shell APK verified: status screen, letterbox 1334x600,
  tap/long-press/drag mapping, pause/resume, Back -> VK_ESCAPE, AAudio opens 44100/2ch.
- Game files pushed (1.2 GB) to /sdcard/Android/data/com.settlers3port.app/files/game.

## In progress
- waiting for s3-dx / s3-win32 (other agents) to compile, then full APK with the engine.

## Verify
- `CARGO_TARGET_DIR=port/target cargo test -p s3-android -j 4`
- `port/scripts/build-apk.sh --abi x86_64 && port/scripts/run-emulator.sh install launch shot test`

## Result (end of task)
- Full engine APK (arm64-v8a + x86_64, 9.3 MB) built; on the emulator S3.EXE reaches the main menu; tap closes
  the intro-video error dialog; screenshot scratch/android-shots/main-menu.png. App stopped, emulator left running.
- regs.cmd added to run-emulator.sh push (registry HLE imports it).
