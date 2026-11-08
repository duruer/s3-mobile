# Progress: ios (iOS build via xtool, no Mac)

Goal: run the port on the user's iPhone (iOS 17+, free Apple ID = 7-day signing) built on this Windows PC via
WSL Ubuntu + xtool (github.com/xtool-org/xtool).

## Done (2026-11-08)
- `rustup target add aarch64-apple-ios`; `cargo check --target aarch64-apple-ios -p s3-engine -p s3-mods -p s3-net
  -p s3-relay` passes unchanged.
- WSL Ubuntu 26.04 aarch64: apt `usbmuxd libimobiledevice-utils` + Swift deps; swiftly + Swift 6.4.2
  (`~/.local/share/swiftly`); xtool 1.24.0 AppImage at `~/.local/bin/xtool`.

## Waiting on user
- Download Xcode 27.1 `.xip` (developer.apple.com, login), then run `xtool setup` in WSL (Apple ID password login +
  path to the xip) — user does this, credentials never pass through us.
- iPhone USB into WSL: `usbipd` (or iTunes relay, xtool issue #19).

## Plan
1. Split the GLES drawing out of `s3-android/src/present.rs` into a platform-neutral module usable with any current
   GL context (iOS: EAGLContext from Swift; OpenGLES.framework has the same gl* symbols). EGL stays Android only.
2. New crate `crates/s3-ios` (staticlib `libs3ios.a`): C API for Swift (start, draw, touch, pause/resume, audio pull,
   launcher). Reuses s3-android's platform-neutral modules (frontend Shared/run_engine, launcher, upload, audio ring).
3. `port/ios/`: SwiftPM + `xtool.yml` app shell (UIKit, GLKView, AudioUnit, touches, Files-app visible Documents
   folder for the game files).
4. Build: `cargo build --target aarch64-apple-ios -p s3-ios --release` (archiving needs no linker) -> SwiftPM links it;
   `xtool dev` builds/signs/installs.

## In progress
- Done: steps 1-4; app builds + links (`scripts/build-ios.sh`). Next: user `xtool auth login`, iPhone via usbipd, `build-ios.sh --install`, test on device.

## Verify
- `cargo check --target aarch64-apple-ios -p s3-ios`; desktop/Android builds still pass; later on device.
