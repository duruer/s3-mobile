#!/usr/bin/env bash
# Run the Settlers 3 Port on OUR emulator (AVD s3_api35, console port 5610 -> adb serial emulator-5610).
# Never touches other AVDs/emulators (e.g. emulator-5600 belongs to another project).
#
# Usage: scripts/run-emulator.sh [command...]     (default: start install launch)
#   create-avd        create AVD s3_api35 (pixel_6, API 35 google_apis x86_64, 3 GB RAM, 6 GB data) if missing
#   start             boot the AVD headless on port 5610 if not running, wait for boot
#   install           install port/target/apk/settlers3port.apk (build it with scripts/build-apk.sh)
#   push [--minimal]  copy the user's game files to /sdcard/Android/data/com.settlers3port.app/files/game
#                     (GAME_DIR env, default ../game-files/gog-s3-ultimate). Only what the game reads; never Save/.
#                     --minimal skips S3QOTA, S3MCD, Map/MULTI and Map/User (~600 MB instead of ~1.2 GB).
#   launch            (re)start the app
#   stop              force-stop the app (the emulator keeps running)
#   logcat [secs]     print the app's log (tag s3port, last run) for up to secs seconds (default: dump and exit)
#   shot [name]       screenshot to scratch/android-shots/<name>.png
#   tap X Y           tap at screen coordinates (adb input)
#   status            show device, app pid, game dir contents
#   kill-emulator     shut the AVD down (only ours)
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail

PORT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="$(cd "$PORT_DIR/.." && pwd)"
SDK="${ANDROID_HOME:-$HOME/Android/Sdk}"
AVD=s3_api35
CONSOLE_PORT=5610
SERIAL="${ADB_SERIAL:-emulator-$CONSOLE_PORT}"   # ADB_SERIAL=<ip:port> targets a real phone
PKG=com.settlers3port.app
APK="$PORT_DIR/target/apk/settlers3port.apk"
GAME_DIR="${GAME_DIR:-$ROOT/game-files/gog-s3-ultimate}"
DEV_GAME="/sdcard/Android/data/$PKG/files/game"
SHOTS="$ROOT/scratch/android-shots"
ADB="adb -s $SERIAL"

running() { adb devices | grep -q "^$SERIAL[[:space:]]*device"; }

create_avd() {
  if "$SDK/emulator/emulator" -list-avds | grep -qx "$AVD"; then
    echo "AVD $AVD exists"
  else
    echo "== creating AVD $AVD"
    echo no | "$SDK/cmdline-tools/latest/bin/avdmanager" create avd -n "$AVD" \
      -k "system-images;android-35;google_apis;x86_64" -d pixel_6
  fi
  local cfg="$HOME/.android/avd/$AVD.avd/config.ini"
  # Our resource limits: 3 GB RAM, 6 GB data partition, 4 cores, no camera.
  for kv in "hw.ramSize=3072" "disk.dataPartition.size=6G" "hw.cpu.ncore=4" "hw.camera.back=none" \
            "hw.camera.front=none" "hw.keyboard=yes" "hw.gpu.enabled=yes" "hw.gpu.mode=swiftshader_indirect"; do
    local k="${kv%%=*}"
    if grep -q "^$k" "$cfg"; then sed -i "s|^$k.*|$kv|" "$cfg"; else echo "$kv" >>"$cfg"; fi
  done
}

start() {
  if running; then echo "$SERIAL already running"; return; fi
  create_avd
  local avail_kb; avail_kb=$(df -Pk "$HOME" | awk 'NR==2 {print $4}')
  if [ "$avail_kb" -lt $((3 * 1024 * 1024)) ]; then echo "error: < 3 GB free disk, not starting emulator" >&2; exit 1; fi
  echo "== starting $AVD on port $CONSOLE_PORT (headless)"
  local audio=(-no-audio)
  [ "${S3_EMU_AUDIO:-0}" = 1 ] && audio=()
  mkdir -p "$ROOT/scratch"
  nohup "$SDK/emulator/emulator" -avd "$AVD" -port "$CONSOLE_PORT" -no-window -gpu swiftshader_indirect \
    -no-boot-anim -no-snapshot-save -memory 3072 "${audio[@]}" >"$ROOT/scratch/emulator-$AVD.log" 2>&1 &
  echo "emulator pid $!"
  $ADB wait-for-device
  for _ in $(seq 1 180); do
    [ "$($ADB shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = 1 ] && { echo "booted"; return; }
    sleep 2
  done
  echo "error: boot timeout" >&2; exit 1
}

install() {
  [ -f "$APK" ] || { echo "error: $APK missing; run scripts/build-apk.sh" >&2; exit 1; }
  $ADB install -r -g "$APK"
}

push() {
  local minimal=0
  [ "${1:-}" = --minimal ] && minimal=1
  [ -f "$GAME_DIR/S3.EXE" ] || { echo "error: $GAME_DIR/S3.EXE not found" >&2; exit 1; }
  # Build the list: S3.EXE, small top-level data files, folders the game reads (wine-trace.md).
  local items=(S3.EXE)
  for f in regs.cmd DATA.DAT S3.DAT S3CD1.DAT S3CD2.DAT S3GOLD1.DAT S3GOLD2.DAT S3MCD1.DAT S3QOTA1.DAT; do
    [ -e "$GAME_DIR/$f" ] && items+=("$f")
  done
  local dirs=(SND MUSIC S3 Map/SINGLE Map/TUTORIAL)
  [ $minimal = 0 ] && dirs+=(S3QOTA S3MCD Map/MULTI Map/User)
  local bytes=0
  for i in "${items[@]}" "${dirs[@]}"; do bytes=$((bytes + $(du -sb "$GAME_DIR/$i" | cut -f1))); done
  # GFX: only the RGB565 set (*.f8007e01f.dat); the port always reports a 565 primary surface.
  local gfx=()
  while IFS= read -r f; do gfx+=("$f"); done < <(cd "$GAME_DIR/GFX" && ls | grep -i '\.f8007e01f\.dat$')
  for f in "${gfx[@]}"; do bytes=$((bytes + $(stat -c %s "$GAME_DIR/GFX/$f"))); done
  local free_kb
  free_kb=$($ADB shell df -k /sdcard | awk 'NR==2 {print $4}' | tr -d '\r')
  echo "== pushing $((bytes / 1048576)) MB to $DEV_GAME (device free: $((free_kb / 1024)) MB)"
  if [ $((bytes / 1024 + 200000)) -gt "$free_kb" ]; then echo "error: not enough space on the device" >&2; exit 1; fi
  # The app creates files/ on first start; create it from the shell too (works on API 30+ for adb shell).
  $ADB shell mkdir -p "$DEV_GAME/GFX" "$DEV_GAME/Map"
  for i in "${items[@]}"; do $ADB push -q "$GAME_DIR/$i" "$DEV_GAME/$i" >/dev/null; done
  for f in "${gfx[@]}"; do $ADB push -q "$GAME_DIR/GFX/$f" "$DEV_GAME/GFX/$f" >/dev/null; done
  # Real phones (e.g. Samsung, Android 14) refuse `adb push` creating directories under Android/data
  # ("secure_mkdirs() failed: Operation not permitted"), but `adb shell mkdir` works. So create every
  # directory first, then push file by file; files already present with the same size are skipped.
  for d in "${dirs[@]}"; do
    echo "   $d"
    local mk=()
    while IFS= read -r sub; do mk+=("'$DEV_GAME/$sub'"); done < <(cd "$GAME_DIR" && find "$d" -type d)
    $ADB shell "mkdir -p ${mk[*]}" </dev/null
    while IFS= read -r f; do
      local want have
      want=$(stat -c %s "$GAME_DIR/$f")
      have=$($ADB shell "stat -c %s '$DEV_GAME/$f' 2>/dev/null" </dev/null | tr -d '\r' || true)
      [ "$want" = "$have" ] && continue
      $ADB push -q "$GAME_DIR/$f" "$DEV_GAME/$f" >/dev/null </dev/null
    done < <(cd "$GAME_DIR" && find "$d" -type f)
  done
  $ADB shell du -sh "$DEV_GAME"
}

launch() {
  $ADB shell am force-stop "$PKG" || true
  $ADB logcat -c || true
  $ADB shell am start -n "$PKG/android.app.NativeActivity"
}

stop_app() { $ADB shell am force-stop "$PKG"; }

logcat() {
  if [ -n "${1:-}" ]; then
    timeout "$1" $ADB logcat -v time -s s3port:V AndroidRuntime:E DEBUG:V libc:F || true
  else
    $ADB logcat -d -v time -s s3port:V AndroidRuntime:E DEBUG:V libc:F | tail -n 300
  fi
}

shot() {
  mkdir -p "$SHOTS"
  local name="${1:-shot-$(date +%H%M%S)}"
  $ADB exec-out screencap -p >"$SHOTS/$name.png"
  echo "$SHOTS/$name.png"
}

status() {
  adb devices
  echo "pid: $($ADB shell pidof $PKG || echo none)"
  $ADB shell ls -la "$DEV_GAME" 2>/dev/null | head -30 || true
}

kill_emulator() { running && $ADB emu kill || echo "$SERIAL not running"; }

[ $# -eq 0 ] && set -- start install launch
while [ $# -gt 0 ]; do
  cmd="$1"; shift
  case "$cmd" in
    create-avd) create_avd ;;
    start) start ;;
    install) install ;;
    push) if [ "${1:-}" = --minimal ]; then push --minimal; shift; else push; fi ;;
    launch) launch ;;
    stop) stop_app ;;
    logcat) if [[ "${1:-}" =~ ^[0-9]+$ ]]; then logcat "$1"; shift; else logcat; fi ;;
    shot) if [ $# -gt 0 ] && [[ "$1" != -* ]] && ! [[ "$1" =~ ^(start|install|push|launch|stop|logcat|tap|status)$ ]]; then shot "$1"; shift; else shot; fi ;;
    tap) $ADB shell input tap "$1" "$2"; shift 2 ;;
    status) status ;;
    kill-emulator) kill_emulator ;;
    -h|--help|help) sed -n '2,22p' "$0" ;;
    *) echo "unknown command $cmd" >&2; exit 2 ;;
  esac
done
