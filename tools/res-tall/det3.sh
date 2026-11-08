#!/bin/sh
# res-tall determinism check: s3det 90000 vms (tools/determinism-test.sh inject) x2 with fast paths + 1 with
# S3_FAST=0; compares per-second hashes. usage: tools/res-tall/det3.sh S3DET GAME_DIR OUT_DIR [VMS]
set -eu
BIN=$1; G=$2; OUT=$3; VMS=${4:-90000}
rm -rf "$OUT" && mkdir -p "$OUT/d1" "$OUT/d2" "$OUT/d3"
I="1000:move 165 235;1200:click 165 235;3000:move 630 467;3200:click 630 467;5000:move 730 567;5200:click 730 567"
I="$I;7000:move 730 567;7200:click 730 567;48000:click 25 167;50000:click 25 200;52000:click 37 237"
I="$I;54000:move 420 300;55000:click 420 300;58000:rclick 300 260"
for n in 1 2 3; do
  if [ $n = 3 ]; then F=0; else F=1; fi
  S3_FAST=$F "$BIN" --game-dir "$G" --data-dir "$OUT/d$n" --vms "$VMS" --hash-every 1000 --hash-out "$OUT/h$n.txt" \
    --inject "$I;$((VMS - 100)):shot $OUT/end$n.png" > "$OUT/log$n.txt" 2>&1 &
done
wait
for n in 2 3; do
  if cmp -s "$OUT/h1.txt" "$OUT/h$n.txt"; then echo "run1 vs run$n: IDENTICAL ($(wc -l < "$OUT/h1.txt") lines)"; else
    echo "run1 vs run$n: DIVERGED"; diff "$OUT/h1.txt" "$OUT/h$n.txt" | head -3; fi
done
tail -n 1 "$OUT/h1.txt"
