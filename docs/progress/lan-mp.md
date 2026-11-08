# lan-mp: LAN multiplayer out of the box (embedded relay + UDP discovery)

Goal: host/join over LAN without a manually started relay (user report 2026-11-08).

## Design (decided)
- No `S3_RELAY_URL` / relay-url.txt => LAN mode. InitializeConnection starts an in-process relay
  (s3-relay lib, own thread + tokio runtime, 0.0.0.0:7777, fallback ephemeral port) and a UDP discovery thread
  (s3-net `lan.rs`, port 47777). Local NetClient -> own relay, so the "Type of Network" entry always works.
- Hosting (Open CREATE on the local relay): discovery announces "relay at port P, session name" (broadcast beacon
  every 1 s + unicast reply to queries). Clients broadcast queries every 1 s from an ephemeral socket (replies are
  unicast => allowed by Windows firewall's "unicast response to broadcast" default).
- EnumSessions merges local relay + every discovered relay; remembers instance -> relay client.
  Open JOIN switches the active client to that relay's client. Close -> back to local.
- Override URL set => old single-relay Internet mode.

## Done
- s3-net/src/lan.rs (Discovery, per-interface sockets, tests), s3-relay `spawn_background`, dplay_net `LanCtx`
  (LAN mode, connection name "Local network / Wi-Fi (Settlers 3 Port)"), s3run `log` output for net crates,
  s3-android wifilock.rs (MulticastLock via JNI hook) + manifest ACCESS_WIFI_STATE / CHANGE_WIFI_MULTICAST_STATE,
  mp-local-test.sh `LAN=1`.
- Desktop+desktop LAN run: `LAN=1 ... mp-local-test.sh scratch/lan3 100` -> joiner found host via UDP, connected to
  host relay, 946 ticks IDENTICAL. (scratch/lan2 run during an APK build: game did not start, timing only.)
- APK built (port/target/apk/settlers3port.apk in this worktree, signed with main debug.keystore).

- Phone (Fold4, host) <-> PC (headless s3run, join) over Wi-Fi: found, joined, 2326 ticks, phone orders executed on
  PC, no game desync (scratch/lanphone/). Docs: multiplayer.md section 8, journal, status.md.

## Status: DONE (committed on branch worktree-agent-a444c46f316eb6eac)
Open: PC-as-host needs a firewall rule (documented, untested); x86_64 <-> arm64 peer; join-by-IP field.
