#!/bin/sh
# Usage: sh tools/re-messages/run.sh OUTDIR MAXSEC "extra inject" [extra s3run args]
# Starts the tutorial (main menu "Tutorial" button at 165 35; in game after ~3 s at 800x600) and runs the extra
# inject commands. Relative paths in the inject resolve against scratch/re-messages. Log -> OUTDIR/log.txt.
# START=scen starts a random single-player map instead (re-ui recipe, in game after ~17-45 s).
# S3RUN overrides the binary (default: scratch/re-messages/target/release/s3run.exe, built from this worktree).
set -e
S=/c/Users/<user>/Projects/settlers-3/scratch/re-messages
cd "$S"
OUT=$1; shift; MAX=$1; shift; EXTRA=$1; shift
mkdir -p "$OUT" data
PRE="1000:move 165 35;1200:click 165 35"
if [ "$START" = scen ]; then
  PRE="1000:move 165 235;1200:click 165 235;3000:move 630 467;3200:click 630 467;5000:move 730 567;5200:click 730 567;7000:move 730 567;7200:click 730 567"
fi
exec timeout $((MAX + 120)) "${S3RUN:-$S/target/release/s3run.exe}" --headless \
  --game-dir C:/Users/<user>/Projects/settlers-3/game-files/gog-s3-ultimate --data-dir data --max-seconds "$MAX" \
  --inject "$PRE;$EXTRA" "$@" > "$OUT/log.txt" 2>&1
