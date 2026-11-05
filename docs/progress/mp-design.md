# Progress: mp-design (W5 multiplayer design + skeleton) — FINAL 2026-11-05

Goal: RE how S3.EXE uses DirectPlay 6; design our online stack; implement `port/crates/s3-net` (protocol + client)
and `port/crates/s3-relay` (tokio WebSocket relay server) with tests; document in docs/porting/multiplayer.md.
Did NOT edit s3-dx (build phase does). No commits.

## Done
- RE (REA + local objdump dump in scratch/mp/, gitignored): CDirectPlay wrapper + vtable offsets, lobby probe
  (DPERR_NOTLOBBIED 0x8877042E confirmed, lobby-launch path exists), EnumConnections + InitializeConnection test loop,
  DPSESSIONDESC2 the game builds (flags 0x44, app GUID {0E9F8400-D9A5-11CF-8F4D-00804880B189}, dwUser3 0xA0),
  async EnumSessions polling (250 ms, 20 s), CreatePlayer passes a Win32 event handle, Send flags (setup guaranteed,
  in-game lockstep unguaranteed and sent twice by the host), Receive buffers (0x400 / 0x9c40), DPSYS switch
  (3, 5, 0x31, 0x101, 0x102, 0x104), setup message type switch, lockstep/TimeServer/desync strings, DPERR table
  at 0x7a03b4. All in docs/porting/multiplayer.md section 1.
- Crates: s3-net (wire.rs, protocol.rs, link.rs, client.rs, dplay.rs), s3-relay (lib.rs hub actor, main.rs CLI,
  tests/two_clients.rs). Workspace Cargo.toml: members + `s3-net` workspace dep (additive).
- Tests: s3-net 12 unit tests, s3-relay 6 integration tests (2-3 real clients via local relay: session flow, ordered
  data, spoofing, host migration, session loss, join rules, exe allow-list, resume without loss, slot expiry).
  5 repeated runs stable; clippy clean. Binary smoke-tested.
- Docs: docs/porting/multiplayer.md (new), AGENTS.md map link, one-line pointer in directx-hle.md.

## Next (build phase, other agents)
- Replace s3-dx dplay.rs stubs per multiplayer.md section 3 (SetEvent on the CreatePlayer event!).
- Runtime RE with `-netmon` (trace.lis) and two s3run instances against a local relay.
- Determinism of x87 across hosts (W5 blocker), desync hook at 0x53b59a (HYPOTHESIS).

## Verify
`cd port && CARGO_TARGET_DIR=$PWD/target cargo test -p s3-net -p s3-relay -j 4`
