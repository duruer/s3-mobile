# Tooling on this PC (for this project)

Checked 2026-11-05.

## Reverse engineering

| Tool | Where | Notes |
| --- | --- | --- |
| REA MCP (`mcp__rea__*`) 6.5.0 | `~/.local/bin/rea` | Load the `reverse-engineer-anything` skill first. Backend: Ghidra 12.1.2 at `/opt/ghidra` (JDK 21). |
| Ghidra zip (backup) | `~/Projects/reverse-engineering/tools/ghidra.zip` | Same Ghidra, not needed while `/opt/ghidra` exists |
| rizin, objdump, python-pefile | system | `tools/pe_imports.py` uses pefile |
| wine 11.x + wine-mono + winetricks | system | Run S3.EXE on Linux as reference; `WINEDEBUG=+relay,+ddraw,+dsound,+dinput` for API traces. Working setup (Xvfb + wine virtual desktop required, relay filtering): [wine-trace](reverse-engineering/wine-trace.md) |
| Blockheads RE workspace | `~/Projects/reverse-engineering/` | Other project. `docs/REA_USAGE.md` has general REA + gdb tips |

**REA sharing rule.** `~/Projects/reverse-engineering/docs/REA_USAGE.md` describes a setup where **one REA server
is shared by many agents**. In that setup, `open_binary` on another target or `close_binary` breaks their Ghidra session.
In a normal Claude Code session the REA server is a stdio child of **this** session (check `binary_session` →
`server_identity.server.started_at` ≈ session start). Then opening/closing S3.EXE is safe. If in doubt, check before
calling `open_binary`. S3.EXE auto-analysis takes about 1-2 minutes.

## Android

| Item | Value |
| --- | --- |
| SDK | `~/Android/Sdk` (emulator, cmdline-tools/latest, build-tools 35/36, platforms 34-37, NDK 27.2.12479018) |
| adb | `/opt/android-sdk/platform-tools/adb` (on PATH) |
| System images | android-30 and android-35 `google_apis` **x86_64** |
| Existing AVDs | `bhmod_m7_api30`, `bhmod_m9_api30`, `bhwt_api30`, `fold_compat_30/35`, `kabuk_api35`, `pulsar_api35` — they belong to other projects; **do not reuse or kill them** |
| Running now | `bhwt_api30` headless on `emulator-5600` (Blockheads world-tool). Leave it alone |
| Our AVD | `s3_api35` (ours): API 35 x86_64, pixel_6, 3 GB RAM, 6 GB data, console port 5610 (`emulator-5610`). Managed by `port/scripts/run-emulator.sh`, see [porting/android.md](porting/android.md) |

ABI notes: the API 30 x86_64 image reports `x86_64,x86,arm64-v8a,armeabi-v7a,armeabi` (it has ARM translation).
The API 35 image reports only `x86_64`. Plan: build our app for **arm64-v8a + x86_64**, test x86_64 on the emulator,
and test arm64 on a real phone or the API 30 image.

Create our own AVD when needed (no root required):

```
~/Android/Sdk/cmdline-tools/latest/bin/avdmanager create avd -n s3_api35 \
  -k "system-images;android-35;google_apis;x86_64" -d pixel_6
~/Android/Sdk/emulator/emulator -avd s3_api35 -port 5610 -gpu host &   # use a free port, not 5600
adb -s emulator-5610 ...
```

REA also has adb tools (`list_adb_devices`, `install_adb_package`, `capture_adb_screen`, `read_adb_logcat`).

## Build tools

cmake, ninja, Java (system), Rust via rustup, bun. Frida is **not** installed.

## Real test phone (2026-11-05)

- **Samsung Galaxy Z Fold4 (SM-F936B)**, Android 14, arm64-v8a, **rooted** (user-confirmed). Reached over wireless adb
  (`adb devices` shows e.g. `10.0.0.50:<port>`; the port changes when wireless debugging is restarted).
- Install and push files: `ADB_SERIAL=<ip:port> port/scripts/run-emulator.sh install push launch`.
  Samsung/Android 14 refuses directories created by `adb push` under `Android/data` (`secure_mkdirs() failed`);
  the script creates them with `adb shell mkdir -p` first.
- Root is for debugging only (e.g. `su -c simpleperf`, reading other app dirs). Do not depend on it for normal installs.
  Ask the user before changing system settings with root.
- The cover screen is narrow and portrait (904x2229). Test on the inner screen (unfolded) or in landscape.
