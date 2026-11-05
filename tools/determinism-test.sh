#!/bin/sh
# Determinism test (W5): run S3.EXE twice headless with s3det (virtual clock, fixed epoch, deterministic GUIDs)
# in parallel, same inject script, and compare per-second state hashes. See docs/porting/determinism.md.
#
# usage: tools/determinism-test.sh [s3det binary] [virtual ms (75000)]
# Output (small) goes to scratch/determinism/run/. Exit 0 = identical.
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
BIN=${1:-$ROOT/port/target/release/s3det}
VMS=${2:-75000}
G=$ROOT/game-files/gog-s3-ultimate
OUT=$ROOT/scratch/determinism/run
rm -rf "$OUT" && mkdir -p "$OUT/d1" "$OUT/d2"
# Menu -> Single Player Scenario -> OK x3 -> random map 384 (in game by ~40 000 vms), then open the build menu,
# pick the first building and place it near the castle, and right-click (cancel) once.
I="1000:move 165 235;1200:click 165 235;3000:move 630 467;3200:click 630 467;5000:move 730 567;5200:click 730 567"
I="$I;7000:move 730 567;7200:click 730 567;48000:click 25 167;50000:click 25 200;52000:click 37 237"
I="$I;54000:move 420 300;55000:click 420 300;58000:rclick 300 260"
for n in 1 2; do
  "$BIN" --game-dir "$G" --data-dir "$OUT/d$n" --vms "$VMS" --hash-every 1000 --hash-out "$OUT/h$n.txt" \
    --inject "$I;$((VMS - 100)):shot $OUT/end$n.png" > "$OUT/log$n.txt" 2>&1 &
done
wait
if diff "$OUT/h1.txt" "$OUT/h2.txt" > "$OUT/diff.txt"; then
  echo "determinism: IDENTICAL ($(wc -l < "$OUT/h1.txt") hash lines, $(tail -n 1 "$OUT/h1.txt"))"
else
  echo "determinism: DIVERGED, first differing line:"; grep -m1 '^<' "$OUT/diff.txt"
  echo "dump memory at that vms in both runs (--dump-at V --dump f) and compare with tools/memdiff.py"
  exit 1
fi
