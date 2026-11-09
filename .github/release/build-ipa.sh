#!/usr/bin/env bash
# Build an UNSIGNED (ad-hoc) iOS app for version $1 on a Mac with Xcode, without xtool or an Apple account:
# Rust core (libs3ios.a) -> swiftc (port/ios/Sources/S3Mobile) -> S3Mobile.app -> dist/s3-mobile-<v>-ios-unsigned.ipa.
# Users sign it themselves when installing (Sideloadly / AltStore / SideStore with their own Apple ID).
# The bundle mirrors what `xtool dev build` makes (Info.plist keys, AppIcon.png as CFBundleIconFile).
# Used by .github/workflows/ios.yml (docs/porting/releases.md).
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
VERSION="$1"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PORT="$ROOT/port"
IOS="$PORT/ios"
B="$ROOT/build-ios"
APP="$B/Payload/S3Mobile.app"
rm -rf "$B"
mkdir -p "$B/cmod" "$APP" "$ROOT/dist"

# 1. Rust core, and the system libraries it needs at link time.
export S3_VERSION="$VERSION"
export CARGO_TARGET_DIR="$PORT/target"
export IPHONEOS_DEPLOYMENT_TARGET=17.0
export CARGO_PROFILE_RELEASE_DEBUG=0
(cd "$PORT" && cargo rustc -p s3-ios --target aarch64-apple-ios --release --crate-type staticlib \
  -- --print native-static-libs) 2> "$B/cargo.log" || { cat "$B/cargo.log"; exit 1; }
NATIVE="$(sed -n 's/.*native-static-libs: //p' "$B/cargo.log" | tail -1)"
echo "== native libs: $NATIVE"
LIBDIR="$PORT/target/aarch64-apple-ios/release"
ls -l "$LIBDIR/libs3ios.a"

# 2. Swift shell. The C API module (SwiftPM target CS3iOS) is just the header.
cat > "$B/cmod/module.modulemap" <<EOF
module CS3iOS {
    header "$IOS/Sources/CS3iOS/include/s3ios.h"
    export *
}
EOF
SDK="$(xcrun --sdk iphoneos --show-sdk-path)"
# shellcheck disable=SC2086 # NATIVE is a list of linker flags
xcrun --sdk iphoneos swiftc -O -swift-version 5 -target arm64-apple-ios17.0 -sdk "$SDK" \
  -parse-as-library -module-name S3Mobile -I "$B/cmod" \
  "$IOS"/Sources/S3Mobile/*.swift \
  -L "$LIBDIR" -ls3ios $NATIVE \
  -framework OpenGLES -framework GLKit -framework AVFoundation -framework AudioToolbox \
  -o "$APP/S3Mobile"

# 3. Bundle: our Info.plist plus the keys xtool adds.
cp "$IOS/Resources/AppIcon.png" "$APP/AppIcon.png"
python3 - "$IOS/Info.plist" "$APP/Info.plist" "$VERSION" <<'EOF'
import plistlib, sys
src, dst, version = sys.argv[1:4]
with open(src, "rb") as f:
    d = plistlib.load(f)
d.update({
    "CFBundleDevelopmentRegion": "en",
    "CFBundleExecutable": "S3Mobile",
    "CFBundleIconFile": "AppIcon",
    "CFBundleIdentifier": "com.settlers3port.app",
    "CFBundleInfoDictionaryVersion": "6.0",
    "CFBundleName": "S3Mobile",
    "CFBundlePackageType": "APPL",
    "CFBundleShortVersionString": version.split("-")[0],
    "CFBundleVersion": version.split("-")[0],
    "CFBundleSupportedPlatforms": ["iPhoneOS"],
    "LSRequiresIPhoneOS": True,
    "MinimumOSVersion": "17.0",
    "UIDeviceFamily": [1, 2],
    "UILaunchScreen": {},
    "UIRequiredDeviceCapabilities": ["arm64"],
})
with open(dst, "wb") as f:
    plistlib.dump(d, f, fmt=plistlib.FMT_BINARY)
EOF
printf 'APPL????' > "$APP/PkgInfo"
# Ad-hoc signature only (no identity, no provisioning profile): installers re-sign with the user's Apple ID.
codesign --force --sign - --timestamp=none "$APP"

# 4. .ipa = zip of Payload/.
OUT="$ROOT/dist/s3-mobile-$VERSION-ios-unsigned.ipa"
(cd "$B" && zip -qry "$OUT" Payload)
ls -l "$OUT"
