#!/usr/bin/env bash
# Build (and optionally install) the iOS app without a Mac (docs/porting/ios.md).
#   1. Rust core: cargo build --target aarch64-apple-ios -p s3-ios (static library; archiving needs no Apple linker,
#      so this runs on Windows or Linux).
#   2. Swift shell port/ios: xtool (WSL Ubuntu on Windows, or Linux) builds, signs and packages it with the iOS SDK it
#      made from Xcode.xip (`xtool setup`, once).
# The app never contains game files.
#
# Usage: scripts/build-ios.sh [--debug] [--rust-only] [--install]
#   --rust-only: only step 1 (check the core builds for iOS)
#   --install:   `xtool dev run` (build + install on the USB-connected iPhone) instead of `xtool dev build`
# Output: port/ios/xtool/S3Mobile.app (and .ipa with `xtool dev build --ipa`)
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail

PORT="$(cd "$(dirname "$0")/.." && pwd)"
PROFILE=release
RUST_ONLY=0
XTOOL_CMD="build"
for a in "$@"; do
  case "$a" in
    --debug) PROFILE=debug ;;
    --rust-only) RUST_ONLY=1 ;;
    --install) XTOOL_CMD="run" ;;
    *) echo "unknown option $a" >&2; exit 2 ;;
  esac
done

export CARGO_TARGET_DIR="$PORT/target"
export IPHONEOS_DEPLOYMENT_TARGET=17.0
# No debug info in the archive (it is ~85 MB with it); the app links only what it uses.
export CARGO_PROFILE_RELEASE_DEBUG=0
flags=(-p s3-ios --target aarch64-apple-ios)
[ "$PROFILE" = release ] && flags+=(--release)
echo "== cargo build ${flags[*]}"
(cd "$PORT" && cargo build "${flags[@]}")
LIB="$PORT/target/aarch64-apple-ios/$PROFILE/libs3ios.a"
if [ "$PROFILE" = debug ]; then
  # Package.swift links from target/aarch64-apple-ios/release.
  mkdir -p "$PORT/target/aarch64-apple-ios/release"
  cp "$LIB" "$PORT/target/aarch64-apple-ios/release/libs3ios.a"
fi
ls -la "$PORT/target/aarch64-apple-ios/release/libs3ios.a"
[ "$RUST_ONLY" = 1 ] && exit 0

echo "== xtool dev $XTOOL_CMD (port/ios)"
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*)
    # Windows: xtool runs in WSL Ubuntu (swiftly + ~/.local/bin/xtool), on the same files via /mnt/c.
    WPATH="$(wsl -d Ubuntu -- wslpath -a "$(cygpath -w "$PORT/ios")" | tr -d '\r')"
    wsl -d Ubuntu -- bash -lc ". ~/.local/share/swiftly/env.sh; export PATH=\$HOME/.local/bin:\$PATH; cd '$WPATH' && xtool dev $XTOOL_CMD"
    ;;
  *)
    (cd "$PORT/ios" && xtool dev "$XTOOL_CMD")
    ;;
esac
