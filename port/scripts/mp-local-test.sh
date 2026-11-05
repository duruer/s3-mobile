#!/usr/bin/env bash
# Two headless s3run instances + a local s3-relay: host creates a LAN/relay game, the second instance joins,
# both start the same map. See docs/porting/multiplayer.md ("Local two-instance test").
#
# usage: port/scripts/mp-local-test.sh [OUT_DIR] [SECONDS]
#   env: S3RUN (default port/target/release/s3run), RELAY (port/target/release/s3-relay), PORT (7787),
#        GAME_DIR, HOST_EXTRA / JOIN_EXTRA (extra inject items, ';'-separated, times in ms), HASH_EVERY (ticks),
#        MAP=artokia (default: multi-player map "384-2-Artokia", loads in a few seconds) | random (random 384 map;
#        its start-position search can take 30-100 s, during which both peers show "Initiating player2player
#        communication" and wait for each other)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${1:-$ROOT/scratch/mp-test}"
SECS="${2:-60}"
S3RUN="${S3RUN:-$ROOT/port/target/release/s3run}"
RELAY="${RELAY:-$ROOT/port/target/release/s3-relay}"
PORT="${PORT:-7787}"
GAME_DIR="${GAME_DIR:-$ROOT/game-files/gog-s3-ultimate}"
HASH_EVERY="${HASH_EVERY:-1}"
MAP="${MAP:-artokia}"
mkdir -p "$OUT"
rm -rf "$OUT/host-data" "$OUT/join-data"
"$RELAY" --listen "127.0.0.1:$PORT" -v >"$OUT/relay.log" 2>&1 &
RELAY_PID=$!
trap 'kill $RELAY_PID 2>/dev/null || true' EXIT
sleep 0.5
export S3_RELAY_URL="ws://127.0.0.1:$PORT/"

# Menu coordinates (800x600): main menu "Multi-player Game: LAN" 165,275; LAN screen: network list entry 200,131,
# "Search for games" 175,240, first open game 200,330, "Join Game" 610,567, "New Game" 730,567;
# map picker: tab "Multi-player Map" 181,180, first map "384-2-Artokia" 165,255, OK 630,467; game setup OK 730,567; waiting room: ready buttons (right of the chat line) 723,114 (host
# row) / 723,134 (joiner row), OK (start) 730,567.
if [ "$MAP" = random ]; then PICK="9500:click 630 467"; else PICK="9000:click 181 180;10000:click 165 255;11000:click 630 467"; fi
HOST="1500:click 165 275;5000:click 200 131;6500:click 730 567;$PICK;12500:click 730 567;16000:shot $OUT/host-wait.png"
JOIN="1500:click 165 275;5000:click 200 131;17000:click 175 240;19000:shot $OUT/join-list.png;19500:click 200 330;20500:click 610 567;24000:shot $OUT/join-wait.png"
HOST="$HOST;26500:click 723 114;28000:shot $OUT/host-ready.png;29000:click 730 567;${HOST_EXTRA:-}"
JOIN="$JOIN;25000:click 723 134;${JOIN_EXTRA:-}"
for t in $(seq 40 20 "$SECS"); do
  HOST="$HOST;$((t*1000)):shot $OUT/host-$t.png"
  JOIN="$JOIN;$((t*1000)):shot $OUT/join-$t.png"
done

S3_NET_NAME=host "$S3RUN" --headless --game-dir "$GAME_DIR" --data-dir "$OUT/host-data" --max-seconds "$SECS" \
  --mp-hash "$OUT/host.hash" --mp-hash-every "$HASH_EVERY" --inject "$HOST" >"$OUT/host.log" 2>&1 &
H=$!
S3_NET_NAME=join "$S3RUN" --headless --game-dir "$GAME_DIR" --data-dir "$OUT/join-data" --max-seconds "$SECS" \
  --mp-hash "$OUT/join.hash" --mp-hash-every "$HASH_EVERY" --inject "$JOIN" >"$OUT/join.log" 2>&1 &
J=$!
wait $H $J || true
grep -h "dplay:" "$OUT/host.log" | sed 's/^/host: /' | head -40 || true
grep -h "dplay:" "$OUT/join.log" | sed 's/^/join: /' | head -40 || true
if [ -s "$OUT/host.hash" ] && [ -s "$OUT/join.hash" ]; then
  python3 -I "$ROOT/tools/mp_hash_compare.py" "$OUT/host.hash" "$OUT/join.hash"
else
  echo "no hash files (game did not start?)"
  exit 1
fi
