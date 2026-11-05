#!/bin/sh
# Usage: tools/re-ui/run.sh MAXSEC "extra inject" [extra s3run args]
# Starts a single-player random map (in game after ~17-45 s) and runs extra inject commands.
# Output dir: scratch/re-ui (relative paths in inject resolve against port/).
set -e
cd ~/Projects/settlers-3/port
MAX=$1; shift; EXTRA=$1; shift
I="1000:move 165 235;1200:click 165 235;3000:move 630 467;3200:click 630 467;5000:move 730 567;5200:click 730 567;7000:move 730 567;7200:click 730 567"
exec timeout $((MAX + 60)) ./target/release/s3run --headless --game-dir ../game-files/gog-s3-ultimate --data-dir ../scratch/data --max-seconds "$MAX" --inject "$I;$EXTRA" "$@"
