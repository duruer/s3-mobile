#!/usr/bin/env bash
# Build the Settlers 3 Port APK without Gradle.
#   cargo ndk (libs3android.so for arm64-v8a + x86_64) -> aapt2 link (manifest + res) -> add stripped libs
#   (stored, 16 KiB page aligned) -> zipalign -> apksigner (debug keystore in port/target, generated once).
# The APK never contains game files.
#
# Usage: scripts/build-apk.sh [--abi arm64-v8a|x86_64|all] [--debug] [--shell] [--game-icon]
#   --shell: build without the engine feature (status screen/input/audio only; for when other crates are broken)
#   --game-icon: PERSONAL builds only: launcher icon from your own S3.EXE (game artwork, never publish such an APK).
#                Default: our own adaptive icon (res/drawable/ic_launcher*.xml).
# Output: port/target/apk/settlers3port.apk
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail

PORT="$(cd "$(dirname "$0")/.." && pwd)"
# Linux/WSL: ~/Android/Sdk. Windows (Git Bash): %LOCALAPPDATA%\Android\Sdk, tools have .exe/.bat suffixes.
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) DEF_SDK="$(cygpath -u "$LOCALAPPDATA")/Android/Sdk"; EXE=.exe; BAT=.bat; HOSTTAG=windows-x86_64; DEF_PY=python ;;
  *) DEF_SDK="$HOME/Android/Sdk"; EXE=; BAT=; HOSTTAG=linux-x86_64; DEF_PY=python3 ;;
esac
SDK="${ANDROID_HOME:-$DEF_SDK}"
BT="$SDK/build-tools/36.0.0"
PY="${PYTHON:-$DEF_PY}"
# --game-icon: the user's own S3.EXE icon becomes the launcher icon (extracted at build time, never committed).
GAME_EXE="${S3_ICON_EXE:-$PORT/../game-files/gog-s3-ultimate/S3.EXE}"
PLATFORM_JAR="$SDK/platforms/android-35/android.jar"
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$SDK/ndk/27.2.12479018}"
export CARGO_TARGET_DIR="$PORT/target"
export PATH="$HOME/.cargo/bin:$PATH"
STRIP="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$HOSTTAG/bin/llvm-strip$EXE"
JOBS="${JOBS:-4}"

ABIS=(arm64-v8a x86_64)
PROFILE=release
FEATURES=()
GAME_ICON=0
while [ $# -gt 0 ]; do
  case "$1" in
    --abi) shift; if [ "$1" = all ]; then ABIS=(arm64-v8a x86_64); else ABIS=("$1"); fi ;;
    --debug) PROFILE=debug ;;
    --shell) FEATURES=(--no-default-features) ;;
    --game-icon) GAME_ICON=1 ;;
    -h|--help) sed -n '2,13p' "$0"; exit 0 ;;
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
"$BT/aapt2$EXE" compile --dir "$PORT/android/res" -o "$OUT/res.zip"
ICON_FLAGS=()
if [ "$GAME_ICON" = 1 ]; then
  [ -f "$GAME_EXE" ] || { echo "error: --game-icon: no $GAME_EXE (set S3_ICON_EXE)" >&2; exit 1; }
  echo "   --game-icon: game artwork in the APK, personal use only, do not publish"
  # Game icon as drawable-*dpi PNGs; density-specific PNGs win over the adaptive drawable/ic_launcher.xml.
  rm -rf "$OUT/icon-res"
  "$PY" -I "$PORT/../tools/s3_icon.py" "$GAME_EXE" "$OUT/icon-res"
  "$BT/aapt2$EXE" compile --dir "$OUT/icon-res" -o "$OUT/icon-res.zip"
  ICON_FLAGS=(-R "$OUT/icon-res.zip")
fi
"$BT/aapt2$EXE" link -o "$OUT/unsigned.apk" -I "$PLATFORM_JAR" \
  --manifest "$PORT/android/AndroidManifest.xml" \
  --min-sdk-version 26 --target-sdk-version 35 \
  "$OUT/res.zip" ${ICON_FLAGS[@]+"${ICON_FLAGS[@]}"}

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
# Stored (no compression) so the libs can be mmapped from the APK; python zipfile instead of zip (not on Windows).
(cd "$STAGE" && "$PY" -I -c '
import os, sys, zipfile
with zipfile.ZipFile(sys.argv[1], "a", zipfile.ZIP_STORED) as z:
    for root, _, files in os.walk("lib"):
        for f in files:
            p = os.path.join(root, f)
            z.write(p, p.replace(os.sep, "/"))
' "$OUT/unsigned.apk")

# 4. Align + sign.
"$BT/zipalign$EXE" -f -P 16 4 "$OUT/unsigned.apk" "$OUT/aligned.apk"
KS="$PORT/target/debug.keystore"
if [ ! -f "$KS" ]; then
  echo "== generating debug keystore $KS"
  keytool -genkeypair -keystore "$KS" -storepass android -keypass android -alias androiddebugkey \
    -keyalg RSA -keysize 2048 -validity 10000 -dname "CN=Settlers3Port Debug,O=Debug,C=US" >/dev/null 2>&1
fi
"$BT/apksigner$BAT" sign --ks "$KS" --ks-pass pass:android --key-pass pass:android \
  --out "$OUT/settlers3port.apk" "$OUT/aligned.apk"
rm -f "$OUT/unsigned.apk" "$OUT/aligned.apk" "$OUT/res.zip" "$OUT/icon-res.zip" "$OUT/settlers3port.apk.idsig"
"$BT/apksigner$BAT" verify "$OUT/settlers3port.apk"
echo "== built $OUT/settlers3port.apk ($(du -h "$OUT/settlers3port.apk" | cut -f1))"
