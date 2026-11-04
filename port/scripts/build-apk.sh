#!/usr/bin/env bash
# Build the Settlers 3 Port APK without Gradle.
#   cargo ndk (libs3android.so for arm64-v8a + x86_64) -> aapt2 link (manifest + res) -> add stripped libs
#   (stored, 16 KiB page aligned) -> zipalign -> apksigner (debug keystore in port/target, generated once).
# The APK never contains game files.
#
# Usage: scripts/build-apk.sh [--abi arm64-v8a|x86_64|all] [--debug] [--shell]
#   --shell: build without the engine feature (status screen/input/audio only; for when other crates are broken)
# Output: port/target/apk/settlers3port.apk
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail

PORT="$(cd "$(dirname "$0")/.." && pwd)"
SDK="${ANDROID_HOME:-$HOME/Android/Sdk}"
BT="$SDK/build-tools/36.0.0"
PLATFORM_JAR="$SDK/platforms/android-35/android.jar"
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$SDK/ndk/27.2.12479018}"
export CARGO_TARGET_DIR="$PORT/target"
export PATH="$HOME/.cargo/bin:$PATH"
STRIP="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-strip"
JOBS="${JOBS:-4}"

ABIS=(arm64-v8a x86_64)
PROFILE=release
FEATURES=()
while [ $# -gt 0 ]; do
  case "$1" in
    --abi) shift; if [ "$1" = all ]; then ABIS=(arm64-v8a x86_64); else ABIS=("$1"); fi ;;
    --debug) PROFILE=debug ;;
    --shell) FEATURES=(--no-default-features) ;;
    -h|--help) sed -n '2,11p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done

avail_kb=$(df -Pk "$PORT" | awk 'NR==2 {print $4}')
if [ "$avail_kb" -lt $((2 * 1024 * 1024)) ]; then
  echo "error: less than 2 GB free on $(df -P "$PORT" | awk 'NR==2 {print $6}'); refusing to build" >&2
  exit 1
fi

# 1. Native libraries.
targs=()
for a in "${ABIS[@]}"; do targs+=(-t "$a"); done
cargo_flags=(-p s3-android -j "$JOBS" ${FEATURES[@]+"${FEATURES[@]}"})
[ "$PROFILE" = release ] && cargo_flags+=(--release)
echo "== cargo ndk ${ABIS[*]} ($PROFILE)"
(cd "$PORT" && cargo ndk "${targs[@]}" --platform 26 build "${cargo_flags[@]}")

# 2. Resources + manifest.
OUT="$PORT/target/apk"
STAGE="$OUT/stage"
rm -rf "$STAGE"
mkdir -p "$STAGE"
echo "== aapt2"
"$BT/aapt2" compile --dir "$PORT/android/res" -o "$OUT/res.zip"
"$BT/aapt2" link -o "$OUT/unsigned.apk" -I "$PLATFORM_JAR" \
  --manifest "$PORT/android/AndroidManifest.xml" \
  --min-sdk-version 26 --target-sdk-version 35 \
  "$OUT/res.zip"

# 3. Native libs: stripped copies, stored uncompressed (extractNativeLibs=false).
for a in "${ABIS[@]}"; do
  case "$a" in
    arm64-v8a) triple=aarch64-linux-android ;;
    x86_64) triple=x86_64-linux-android ;;
    *) echo "unsupported ABI $a" >&2; exit 2 ;;
  esac
  so="$PORT/target/$triple/$PROFILE/libs3android.so"
  mkdir -p "$STAGE/lib/$a"
  "$STRIP" --strip-debug -o "$STAGE/lib/$a/libs3android.so" "$so"
  echo "   lib/$a/libs3android.so $(du -h "$STAGE/lib/$a/libs3android.so" | cut -f1)"
done
(cd "$STAGE" && zip -q -0 -r "$OUT/unsigned.apk" lib)

# 4. Align + sign.
"$BT/zipalign" -f -P 16 4 "$OUT/unsigned.apk" "$OUT/aligned.apk"
KS="$PORT/target/debug.keystore"
if [ ! -f "$KS" ]; then
  echo "== generating debug keystore $KS"
  keytool -genkeypair -keystore "$KS" -storepass android -keypass android -alias androiddebugkey \
    -keyalg RSA -keysize 2048 -validity 10000 -dname "CN=Settlers3Port Debug,O=Debug,C=US" >/dev/null 2>&1
fi
"$BT/apksigner" sign --ks "$KS" --ks-pass pass:android --key-pass pass:android \
  --out "$OUT/settlers3port.apk" "$OUT/aligned.apk"
rm -f "$OUT/unsigned.apk" "$OUT/aligned.apk" "$OUT/res.zip" "$OUT/settlers3port.apk.idsig"
"$BT/apksigner" verify "$OUT/settlers3port.apk"
echo "== built $OUT/settlers3port.apk ($(du -h "$OUT/settlers3port.apk" | cut -f1))"
