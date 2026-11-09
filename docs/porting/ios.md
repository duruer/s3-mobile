# iOS app (no Mac: xtool on WSL)

Status 2026-11-08: the app builds and links without a Mac: `scripts/build-ios.sh` -> `port/ios/xtool/S3Mobile.app`
(Mach-O arm64, 10 MB, iOS 17.0+), SDK from `Xcode_27.2.xip` via `xtool sdk install` (no Apple login needed for
that). Signed and installed on the iPhone 2026-11-09 (`build-ios.sh --install`, patched usbmuxd below); first run pending. Target: iPhone,
iOS 17+, free Apple ID (re-sign every 7 days).

## Pieces

| Part | Where | What |
| --- | --- | --- |
| Shared frontend | `port/crates/s3-frontend` | launcher, upload server, engine thread, `Shared`, touch -> pointer events, audio ring, GLES renderer `glrender` (also used by Android `present.rs`) |
| Rust core | `port/crates/s3-ios` | `libs3ios.a` (staticlib), C API `port/ios/Sources/CS3iOS/include/s3ios.h` |
| Swift shell | `port/ios` | SwiftPM package for xtool: `S3MobileApp.swift` (SwiftUI app, lifecycle), `GameView.swift` (GLKView + CADisplayLink, touches, UIKeyInput soft keyboard, hardware keys -> VK, haptics), `Core.swift` (paths, AVAudioEngine source node) |
| Build | `port/scripts/build-ios.sh` | cargo (Windows) then `xtool dev build` / `run` in WSL Ubuntu |

Why a separate `s3-frontend` crate: `s3-android` is a `cdylib`, and cargo builds every crate type of a dependency,
so depending on it from the iOS staticlib needed an Apple linker. A plain rlib avoids that; `s3-android` re-exports its
modules under the old paths (`crate::launcher`, ...).

Why OpenGL ES and not Metal: iOS 17 still ships OpenGLES.framework (deprecated, works), so the Android GLES renderer
is reused unchanged: GLKit makes the EAGLContext current and binds its framebuffer, then `s3ios_draw` runs
`Composer::draw_gl`. EGL stays Android only. The AI upscaler (GLES 3 compute-like passes) is Android only for now; the
AI filter falls back to sharp bilinear on iOS.

## Flow

- Main thread: CADisplayLink (up to 120 Hz) -> `s3ios_tick` (launcher requests, engine end, haptics, keyboard,
  `redraw` when frame/status/layer/display changed) -> `GLKView.display()` -> `s3ios_draw(drawableW, drawableH)`.
- Touches: UITouch -> ids in touch order; actions down / pointer down / move / pointer up / up / cancel, same as
  Android MotionEvents (`s3_frontend::touch::pointer_events`), window pixels = points * `contentScaleFactor`.
- Keyboard: game text field tapped / serial dialog (show), field lost the focus (hide), three-finger tap (toggle),
  launcher text field -> `keyboard` in `S3Out` (1 show, 2 hide, 3 toggle) -> `becomeFirstResponder` (UIKeyInput).
  Hardware keyboard: `pressesBegan` -> HID usage -> VK (`s3ios_key`) + typed chars (`s3ios_char`).
- Audio: `AVAudioSourceNode` pulls interleaved i16 stereo 44100 Hz (`s3ios_audio_pull`) and converts to float.
- Background: scene phase not active -> `s3ios_set_background(1)`: the game pauses (no background play on iOS yet:
  iOS stops apps without a background mode), audio engine paused.
- Files: `Documents/Settlers3/` = game dir (Files app "On My iPhone > S3 Mobile", `UIFileSharingEnabled`,
  or the launcher's upload server), `Documents/Data/` = settings + `log.txt` of the last run. Both are deleted with
  the app (re-signing after 7 days keeps them).
- Game folder outside the app: launcher > Game files > "Choose game folder..." opens `UIDocumentPickerViewController`
  for a folder; `Core.usePickedFolder` starts security-scoped access, stores a bookmark in UserDefaults
  (`gameFolderBookmark`) and calls `s3ios_set_game_dir(..., 1)`; restored at every start. "Use app folder" forgets
  it. Such a folder (e.g. On My iPhone/Settlers3) survives deleting the app. Avoid iCloud Drive (files can be
  evicted). `S3Out.pick_folder` carries the launcher's `Request::PickFolder` / `UseAppFolder`.
- Pushing the game files from a PC: `python3 -P tools/upload_game_files.py <game folder> "<URL shown on the phone>"`
  (8 parallel connections, skips files already there): 1.6 GB in ~2 min at ~17 MB/s. A curl-per-file loop managed
  only a few files per second (process start + 2 requests per file, 1673 mostly small files).

## Setup (once, in WSL Ubuntu 26.04 aarch64)

Done on this PC 2026-11-08: apt `usbmuxd libimobiledevice-utils` + Swift build deps, swiftly + Swift 6.4.2,
xtool 1.24.0 at `~/.local/bin/xtool`. Remaining (user, needs Apple login):

1. Xcode `.xip` from developer.apple.com/download/all (used: 27.2). SDK: `xtool sdk install <path to .xip>`
   (done 2026-11-08; `swift sdk list` shows `darwin`).
2. Login (user, once): `xtool auth login` (or `xtool setup`) -> "Password" for a free Apple ID. Any Apple ID works, it
   does not have to be the one on the iPhone; xtool suggests a separate one because this login uses private Apple
   APIs. The iPhone is registered as a test device of that account.
3. iPhone over USB into WSL (verified 2026-11-09, iPhone 14 Pro `iPhone15,2`, iOS 27.2):
   - `winget install --exact dorssel.usbipd-win` (5.3.0 arm64), then elevated `usbipd bind --force --busid <id>`
     (`--force`: Windows' own composite/WPD driver holds the phone even without iTunes), `usbipd attach --wsl
     --busid <id>` (a WSL distro must be running).
   - Tap "Trust" on the phone; `ideviceinfo -k DeviceName` answers.
   - **usbmuxd 1.1.1 over usbipd-win breaks app uploads**: the phone logs `asyncReadComplete, message was too large
     (65536 bytes, max = 65535)` and xtool fails with `LockdownClient.Error.muxError` / hangs at Packaging
     (usbmuxd issue #247, open). Cause (HYPOTHESIS, fits the symptom): usbipd-win drops the separate zero-length
     packet usbmuxd sends after a transfer that is a multiple of wMaxPacketSize (512), so the phone merges two
     transfers. Fix: `port/ios/usbmuxd-usbipd-zlp.patch` (on usbmuxd 3ded00c) splits such TCP packets so no
     transfer needs a ZLP. Build in WSL (`./autogen.sh --without-preflight --without-systemd && make`, deps
     `libusb-1.0-0-dev libplist-dev libimobiledevice-dev libimobiledevice-glue-dev`), stop the stock daemon, run
     `sudo ~/usbmuxd-src/src/usbmuxd -f -v`. With it the install went through (Installing/Verifying 100%).
   - The iTunes relay of xtool issue #19 (Windows usbmuxd on port 27015) needs Apple Mobile Device Support, which
     is x64-only drivers: not available on this ARM64 PC.
   - On the phone: Settings > Privacy & Security > Developer Mode (appears after the first install attempt), and
     Settings > General > VPN & Device Management > trust the Apple ID's developer certificate.

## Build

```
port/scripts/build-ios.sh --rust-only   # core only (works now on Windows)
port/scripts/build-ios.sh               # + xtool dev build -> port/ios/xtool/S3Mobile.app
port/scripts/build-ios.sh --install     # + install on the connected iPhone
```

xtool prefixes the bundle id (`XTL-xxxx.com.settlers3port.app`) for free accounts.

## Known gaps / risks

- Only deprecation warnings (GLKView / EAGLContext, OpenGL ES deprecated since iOS 12).
- No JIT on iOS (W^X); the interpreter is the default anyway.
- LAN discovery uses UDP broadcast/multicast; iOS 14+ needs the local network permission (Info.plist text set) and
  multicast may need the `com.apple.developer.networking.multicast` entitlement (Apple approval) — HYPOTHESIS:
  broadcast to the subnet works without it. Joining by IP / relay URL does not need it.
- Launcher texts still talk about Android paths in places (Files page).
