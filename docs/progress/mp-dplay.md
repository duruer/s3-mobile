# Progress: mp-dplay (DirectPlay 6 HLE on s3-net) — FINAL 2026-11-05

Goal: s3-dx DirectPlay HLE over s3-net/s3-relay so the game's own MP menu works; two headless s3run + local relay
test (port/scripts/mp-local-test.sh); per-tick state hash comparison; docs. No commits (checkpoint step commits).

## Done
- port/crates/s3-dx/src/dplay_net.rs (new): relay-backed IDirectPlay4A (EnumConnections, InitializeConnection,
  GetCaps/GetPlayerCaps, EnumSessions sync+async, Open, Close, EnumPlayers, Create/DestroyPlayer, Send, Receive,
  GetMessageCount, Get/SetSessionDesc, Get/SetPlayerData, Get/SetPlayerName), service dplay-pump + SetEvent on the
  CreatePlayer events, env S3_RELAY_URL / S3_NET_NAME / S3_DPLAY_TRACE, `set_config`, `report_desync`.
  dplay.rs routes methods; s3-dx Cargo: dep s3-net, dev-deps s3-relay + tokio. Test
  `dplay_two_machines_through_relay`; `dplay_factories_and_stubs` updated.
- s3-desktop: mp_hash.rs (new) + small additive main.rs edits: --mp-hash FILE, --mp-hash-every N, env
  S3_MP_DUMP_STEP, S3_MP_HDR, S3_GAME_LOG, S3_MP_NETDBG. s3-desktop now depends on s3-mods.
- s3-runtime/src/process.rs: env S3_EXE_ARGS appended to the guest command line (debug, e.g. -netmon).
- tools/mp_hash_compare.py, tools/mp_dump_diff.py, port/scripts/mp-local-test.sh.
- Results: game's LAN menu -> our connection -> create/join -> ready -> start works; lockstep runs; no simulation
  desync in long1/long2/long3 (up to 5489 ticks ~5.9 min, commands from both peers); 3 tiles of world byte +0x0f
  differ late (HYPOTHESIS local marker). "Stuck at player2player" = slow random-map generation, not a bug.
- Docs: multiplayer.md sections 5-7 (+1.4 dwUser fix, code map), directx-hle.md, status.md, kernel32.md,
  integration-log.md, AGENTS.md row.
- cargo test --workspace: all pass; s3-dx checks for aarch64-linux-android; single player still reaches in game
  (scratch/mp-dplay/sp-final.png).

## Next (not done)
- Hook game's "Desync!" (0x53ad7c) / ChecksumFailure (0x53b59a) -> dplay_net::report_desync.
- Android: INTERNET permission, relay URL setting / lobby UI; arm64 peer test.
- Identify world byte +0x0f writer (watchpoint) to confirm it is UI-only.

## Verify
cd port && CARGO_TARGET_DIR=$PWD/target cargo test -p s3-dx -p s3-net -p s3-relay -j 4
CARGO_TARGET_DIR=$PWD/target cargo build --release -j 4 -p s3-desktop -p s3-relay
cd .. && port/scripts/mp-local-test.sh scratch/mp-test 120   # RESULT: IDENTICAL
