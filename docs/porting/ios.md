# iOS app (no Mac: xtool on WSL)

Status 2026-11-08: the app builds and links without a Mac: `scripts/build-ios.sh` -> `port/ios/xtool/S3Mobile.app`
(Mach-O arm64, 10 MB, iOS 17.0+), SDK from `Xcode_27.2.xip` via `xtool sdk install` (no Apple login needed for
that). Not yet signed / installed / run: needs `xtool auth` (Apple ID) and the iPhone over USB. Target: iPhone,
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
- Keyboard: three-finger tap / launcher text field -> `keyboard` in `S3Out` -> `becomeFirstResponder` (UIKeyInput).
  Hardware keyboard: `pressesBegan` -> HID usage -> VK (`s3ios_key`) + typed chars (`s3ios_char`).
- Audio: `AVAudioSourceNode` pulls interleaved i16 stereo 44100 Hz (`s3ios_audio_pull`) and converts to float.
- Background: scene phase not active -> `s3ios_set_background(1)`: the game pauses (no background play on iOS yet:
  iOS stops apps without a background mode), audio engine paused.
- Files: `Documents/Settlers3/` = game dir (Files app "On My iPhone > Settlers 3 Mobile", `UIFileSharingEnabled`,
  or the launcher's upload server), `Documents/Data/` = settings + `log.txt` of the last run.

## Setup (once, in WSL Ubuntu 26.04 aarch64)

Done on this PC 2026-11-08: apt `usbmuxd libimobiledevice-utils` + Swift build deps, swiftly + Swift 6.4.2,
xtool 1.24.0 at `~/.local/bin/xtool`. Remaining (user, needs Apple login):

1. Xcode `.xip` from developer.apple.com/download/all (used: 27.2). SDK: `xtool sdk install <path to .xip>`
   (done 2026-11-08; `swift sdk list` shows `darwin`).
2. Login (user, once): `xtool auth login` (or `xtool setup`) -> "Password" for a free Apple ID. Any Apple ID works, it
   does not have to be the one on the iPhone; xtool suggests a separate one because this login uses private Apple
   APIs. The iPhone is registered as a test device of that account.
3. iPhone over USB into WSL: `usbipd` (learn.microsoft.com/windows/wsl/connect-usb), or the iTunes relay from xtool
   issue #19 if `AFCClient.Error.muxError`. Developer Mode on the iPhone; trust the developer certificate in
   Settings after the first install.

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
