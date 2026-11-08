# launcher (2026-11-08) — CODE DONE, PHONE TEST PENDING

Goal: own home screen at app start (before the engine): game files status + version, Play, Settings
(mods-settings.json + relay-url.txt), Game files page with a built-in web upload server (phone Wi-Fi IP, folder
upload from a PC browser), optional Android folder picker. Return to the launcher after Exit Game.

Done (committed on the worktree branch):
- `port/crates/s3-android/src/launcher.rs` (pages, touch/scroll, Back, text field, QR), `upload.rs` (HTTP server).
- Presenter shows `Shared::launcher`; android.rs spawns the engine on Play, back to the launcher on
  `Shared::engine_end`; `--ez play true` autoplay; `<data>/game-dir.txt` override.
- Unit tests pass; preview PNGs via `S3_LAUNCHER_PREVIEW=<dir> cargo test -p s3-android --lib launcher_preview -- --ignored`.
- APK builds (`sh port/scripts/build-apk.sh --abi arm64-v8a`). Docs: android.md "Launcher", journal.

Not done / next:
1. Phone test (adb 192.168.1.50:39093 refused all session): `adb install -r` (NEVER uninstall), screenshots of
   both displays, Play -> game -> Exit Game -> launcher -> Play again (in-process engine restart is untested!),
   relay field with the soft keyboard (does Gboard send key events to NativeActivity here?).
2. Upload test: `echo /sdcard/Android/data/com.settlers3port.app/files/uploadtest > .../files/data/game-dir.txt`,
   restart app, Start upload server, `curl -X POST --data-binary @f "http://<ip>:8080/upload?k=<tok>&path=GFX/a.dat"`,
   then delete game-dir.txt.
3. Android folder picker (ACTION_OPEN_DOCUMENT_TREE) needs a Java activity for onActivityResult.
