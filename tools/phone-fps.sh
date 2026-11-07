#!/usr/bin/env bash
# Start a random-map scenario on the phone through the menu layouts (Fold4 cover screen 2316x904, landscape) and
# grab FPS-counter crops. Usage: ADB_SERIAL=ip:port tools/phone-fps.sh OUTDIR [wait_s]
# Needs fps_counter != Off in the app's mods-settings.json. SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
OUT=$1; WAIT=${2:-70}
ADB="${ADB:-$LOCALAPPDATA/Android/Sdk/platform-tools/adb.exe}"
A=("$ADB" -s "$ADB_SERIAL")
D=4630946213010294403   # cover display id for screencap
mkdir -p "$OUT"
"${A[@]}" shell am force-stop com.settlers3port.app
"${A[@]}" logcat -c
"${A[@]}" shell am start -n com.settlers3port.app/android.app.NativeActivity >/dev/null
sleep 22
for xy in "355 667" "2142 840" "1589 811" "2142 845"; do "${A[@]}" shell input tap $xy; sleep 4; done
sleep "$WAIT"
for i in 1 2 3 4 5; do "${A[@]}" exec-out screencap -p -d $D > "$OUT/f$i.png"; sleep 3; done
"${A[@]}" logcat -d -s s3port | grep -i "perf hint" | head -5 || true
