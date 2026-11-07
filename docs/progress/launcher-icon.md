# launcher-icon (2026-11-07) — DONE

Goal: APK launcher icon = real Settlers III icon (user request), install on the phone over wireless adb.

Done:
- `tools/s3_icon.py`: S3.EXE / .ico -> `drawable-*dpi/ic_launcher.png` (nearest-neighbour).
- `port/scripts/build-apk.sh`: icon overlay step + Windows Git Bash support. Built `--abi arm64-v8a` OK.
- Phone paired + connected (192.168.1.52:40781). New keystore -> signature mismatch -> game files moved aside
  with su, uninstall/install, files moved back (chown/chcon). Game starts; launcher shows the crest.

- Then (legal): default = own adaptive vector "III" icon; game crest only via `--game-icon`. Installed, verified.

Not committed yet.
