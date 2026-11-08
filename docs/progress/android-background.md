# android-background (2026-11-08)

Goal: game keeps running when the app goes to the background in a game; foreground service + ongoing notification
with game time (chronometer); in-game message lines posted as notifications while backgrounded.

## Done (code complete, compiles on host; APK build running)
- `s3-mods/src/messages.rs`: `take_new_lines()` (process-wide queue of newly shown message texts, composite hook).
- `port/android/java/com/settlers3port/app/GameService.java` (START/UPDATE/EVENT intents, wake lock, channels).
- `port/android/AndroidManifest.xml` (hasCode=true, service specialUse, permissions), `res/drawable/ic_notify.xml`.
- `port/scripts/build-apk.sh`: javac --release 11 + d8 -> classes.dex.
- `s3-android`: `background.rs` (coalescing + drift, tests), `bgservice.rs` (JNI intents, permission request),
  `frontend.rs` (`in_game`, `game_ms`, `background`, `bg_lines`, `sync_background`, `background_fps`),
  `android.rs` (Pause in game -> background play; Resume stops it).

## Next
- Install + test on phone (adb 192.168.1.52:43103), then docs (android.md "Background play", journal), commit.

## Verify
- `port/scripts/build-apk.sh --abi arm64-v8a`, install, start game, `adb shell input keyevent KEYCODE_HOME`,
  `adb shell dumpsys notification --noredact | grep -A5 settlers`, `adb logcat -s s3port`.
