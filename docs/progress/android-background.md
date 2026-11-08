# android-background (2026-11-08)

Goal: game keeps running when the app goes to the background in a game; foreground service + ongoing notification
with game time (chronometer); in-game message lines posted as notifications while backgrounded.

## Status: DONE, verified on the Fold4 (see docs/journal.md 2026-11-08 "Background play")

Code: `port/android/java/.../GameService.java`, `AndroidManifest.xml`, `res/drawable/ic_notify.xml`,
`scripts/build-apk.sh` (javac + d8), `s3-android/src/{background,bgservice,android,frontend}.rs`,
`s3-mods/src/messages.rs` (`take_new_lines`). Docs: `docs/porting/android.md` "Background play".

Note: a fresh worktree generates a new debug keystore; re-sign with the main repo's `port/target/debug.keystore`
(or copy it first) or `install -r` fails and an uninstall would delete the game files on the phone.

## Open
- Not exercised: chronometer re-sync at fast speed, 5 s out-of-game pause in the background, swipe-away.

## Verify
- `port/scripts/build-apk.sh --abi arm64-v8a`, install, start a game, `adb shell input keyevent KEYCODE_HOME`,
  `adb shell dumpsys activity services com.settlers3port.app`, `adb logcat -s s3port` (`game time N s (background)`).
