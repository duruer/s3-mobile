# Online multiplayer (W5): how S3.EXE uses DirectPlay, and our relay stack

Status 2026-11-05 (mp-dplay): **the game's own multiplayer menu works over our relay.** `s3-dx/src/dplay_net.rs`
implements the IDirectPlay4A methods S3.EXE uses on top of `s3-net`; two headless `s3run` instances + a local
`s3-relay` get through "Multi-player Game: LAN" -> our connection -> New Game / Join Game -> ready -> start, load
the same map and run the game in lockstep. A per-simulation-tick state hash (section 6) found **no simulation desync** in
runs of up to ~6 min game time with player commands from both sides (only a 3-tile difference in one world byte,
HYPOTHESIS: local marker, see section 6). Not done yet: lobby launch, the
"Multi-player Game: Internet" menu entry (not DirectPlay, see 5.1), desync hook into the relay, Android settings UI
for the relay URL, cross-host (arm64) runs.

Verify:
```sh
cd port && CARGO_TARGET_DIR=$PWD/target cargo test -p s3-net -p s3-relay -p s3-dx -j 4
CARGO_TARGET_DIR=$PWD/target cargo build --release -j 4 -p s3-desktop -p s3-relay
cd .. && port/scripts/mp-local-test.sh scratch/mp-test 120     # ends with "RESULT: IDENTICAL (no desync)"
```
Run a relay: `port/target/release/s3-relay --listen 0.0.0.0:7777 [--gog-only] [--grace 20] [-v]`; point the game at it
with `S3_RELAY_URL=ws://host:7777/` (default `ws://127.0.0.1:7777/`). Android: one line in `<data dir>/relay-url.txt`
(e.g. `ws://10.0.2.2:7788/` = the PC from the emulator), read at engine start; the APK has the `INTERNET` permission.
Android joining a desktop host is verified (ux-verify1, [integration-log.md](integration-log.md)).

## 1. What S3.EXE does with DirectPlay (static RE, GOG exe sha256 `00a925cf…`)

Sources: REA/Ghidra on `S3.EXE` plus a local `objdump -d -M intel` dump (`scratch/mp/`, gitignored). The game has
debug strings for almost every step (`NET: …`, `DPSYS_… empfangen`, `CDirectPlay::DP…`), which made the mapping easy.
Source file names leak from asserts: `E:\Siedler3\DirectX\Dplay.cpp`, `netsession.cpp`, `Dirk\LobbyManager.cpp`,
`Dirk\LobbyDialogs.cpp`.

### 1.1 Classes and interfaces

| Thing | Where | Notes |
| --- | --- | --- |
| `DirectX::CDirectPlay` | methods `0x5bb000-0x5c2100` | wraps one `IDirectPlay4A` (`this+4`) and one `IDirectPlayLobby2A` (`this+8`); every call goes through `0x5bb890` (throws `CDirectPlayException` with the method name on failure) |
| COM creation | `0x5c07f0` | `CoCreateInstance(CLSID_DirectPlay, IID_IDirectPlay4A)`, sets `this+0xc = 4` (interface version) |
| Lobby creation | `0x5c0830` | `CoCreateInstance(CLSID_DirectPlayLobby, IID_IDirectPlayLobby2A)` |
| HRESULT name table | `0x7a03b4` | (code, "DPERR_…") pairs; copied verbatim into `s3_net::dplay::dperr` |
| `CNetSession` / `CGameNetSession` | game layer `0x46a000-0x472000` | setup (slots, names, GameSetup) and in-game lockstep ("TimeServer", steps, TTP) |
| `CLocalPlayer` / `CGameLocalPlayer` | vtables `0x65cd34`, `0x65ce70` | a local DirectPlay player: `+4` = CDirectPlay, `+8` = DPID; vtable slot 3 = Receive, 4 = Send, 7 = Receive+dispatch |
| `CConnectionShortcut` / `CTCPConnectionShortcut` | strings `0x79eee8`, `0x79ef48` | entries of the connection list; TCP one is labelled "Configured Internet Connection" / "Internet connection to %s" (HYPOTHESIS: built with `CreateCompoundAddress` TCP/IP SP + `DPAID_INet` at `0x5c0a40`) |
| Blue Byte lobby | `CLobbyManager`, `LobbyClient::*`, `Lobby::CSocketError` | original online lobby over **wsock32**, not DirectPlay; uses `dwUser`/LobbyId for league games. Dead server; ignore (Settlers United replaced it) |

IDirectPlay4A vtable offsets used (call sites verified): `+0x18` CreatePlayer, `+0x30` EnumPlayers, `+0x34`
EnumSessions, `+0x38` GetCaps, `+0x60` Open, `+0x64` Receive, `+0x68` Send, `+0x8c` EnumConnections (plus
InitializeConnection, Close, DestroyPlayer, Get/SetSessionDesc, Get/SetPlayerData, GetMessageCount by string).
Lobby: `+0x0c` Connect, `+0x20` GetConnectionSettings, `+0x30` SetConnectionSettings, `+0x38` CreateCompoundAddress.
No groups, no `SendEx`, no chat API, no secure sessions (no strings/calls found).

### 1.2 Startup: lobby probe (OBSERVED, `0x5bb6c0-0x5bb780`)

At startup the game creates the lobby object and calls `GetConnectionSettings(0, NULL, &size)`. If that fails
with **`DPERR_NOTLOBBIED` = 0x8877042E** (compare at `0x5bb730`; this confirms the value our stub already used) it
creates a plain DirectPlay object and continues normally. Otherwise it was **lobby-launched**: `DPLConnect`
(`0x5c0af0`: `IDirectPlayLobby::Connect(0, &dp2, NULL)` then `QueryInterface(IID_IDirectPlay4A)`) and
`EnumPlayers`. `0x4700ff` reads the connection settings again in the game layer (session/player names).

=> Two ways for our HLE to put the player into an online game: (a) let them use the in-game multiplayer menus with
our virtual service provider, or (b) **fake a lobby launch**: our native lobby UI fills a `DPLCONNECTION`
(session desc + player name + host/join), `GetConnectionSettings` returns it and `Connect` opens the relay session,
so the game skips its connection/session dialogs (HYPOTHESIS: needs a runtime test of where the game lands after a
lobby launch; Settlers United/aLobby likely use this path).

### 1.3 Connections (service providers)

`0x5bbb00`: `EnumConnections(guidApp or NULL, cb=0x5bc1a0, ctx, DPCONNECTION_DIRECTPLAY)`; the callback stores
64-byte entries (`this+0x34`, count `this+0x40`); then for each entry `0x5bb810` re-creates the DirectPlay object
and calls `InitializeConnection(entry+0x14 = connection blob, 0)` (HYPOTHESIS: to test it; the UI is
`Menus::priv::CConnectionDialog`). The game expects the usual SPs (IPX, TCP/IP, modem, serial) to be listed with
their names; it does not hard-code SP GUIDs (none found besides the CLSIDs/IIDs). Our HLE reports **one** connection,
"Internet (Settlers 3 Port)" with our own SP GUID `{6A2F4C1E-3B5D-4E8F-9A10-53335056F4E1}`
(`s3_net::dplay::SP_GUID_S3PORT`), whose connection blob is opaque to the game.

### 1.4 Sessions

**Create/join** (`0x46ff80` -> `0x5bb840`), DPSESSIONDESC2 built by the game:

| Field | Value |
| --- | --- |
| `dwSize` | 0x50 |
| `dwFlags` | `DPSESSION_MIGRATEHOST|DPSESSION_KEEPALIVE` (0x44); `|DIRECTPLAYPROTOCOL` (0x2000) if interface version > 3 and the SP caps lack `DPCAPS_GUARANTEEDOPTIMIZED` (0x40) (GetCaps with `DPGETCAPS_GUARANTEED` at `0x5c0a20`); `|DPSESSION_PRIVATE` (0x200) when the 4th arg is set (password game) |
| `guidApplication` | `{0E9F8400-D9A5-11CF-8F4D-00804880B189}` (bytes at `0x6970f8`) |
| `guidInstance` | only for join (from the chosen session) |
| `dwMaxPlayers` | `0x435ac0()` (HYPOTHESIS: chosen map's player count) |
| `lpszSessionName`, `lpszPassword` | from the dialog (empty string `0x6553a0` if none) |
| `dwUser1/2` | globals `0x7a8000/0x7a8004`; runtime value `3c44fe57 3d439f21` in every run, also logged as "Siedler3 Version 1.60: (3c44fe57,3d439f21)" (OBSERVED 2026-11-05: a build/version id, not lobby ids) |
| `dwUser3` | 0xA0 (constant at `0x6970f0`; HYPOTHESIS: game/net version 1.60) |
| `dwUser4` | 1 if `0x437450()` (HYPOTHESIS: Amazons/mission CD flag, cf. "RemoteSetup with AmazonsSupport") |

`Open(&desc, join ? DPOPEN_JOIN : DPOPEN_CREATE)` then immediately `EnumPlayers(NULL, 0x5bc5f0, this, 0)`.

**Enumerate** (`0x5bbd60`, `0x5bbee0`): list = `EnumSessions(&desc, 0, cb=0x5bc310, ctx, flags)`; the callback
returns FALSE on `DPESC_TIMEDOUT` and stores 0x60-byte entries. Finding a specific session (join by instance):
loop `EnumSessions(…, flags|DPENUMSESSIONS_ASYNC)` + `Sleep(250)` for up to 20 s (`0x4e20` ms), then
`DPENUMSESSIONS_STOPASYNC`. Log: "NET: Starting find session" / "Found session" / "Stop find session %i".

### 1.5 Players, Send, Receive

* **CreatePlayer** (`0x5c12d0`): `CreatePlayer(&dpid, &DPNAME, hEvent, data, size, flags)`. **`hEvent` is a real
  Win32 event** (`lpMessageEvent`, created at `0x5c11c3`): DirectPlay must `SetEvent` it whenever a message for the
  player is queued. The HLE has to do the same (the game's network thread probably waits on it — HYPOTHESIS).
  One local player per machine.
* **Send** (`0x5c13f0`): `Send(myDpid, to (0 = all), flags, data, size)`.
  * Setup/control messages: `DPSEND_GUARANTEED` (flag 1), sizes 8, 12, 16, 315 (GameSetup), 2n+4 (to all).
  * **In-game lockstep bundles: flags 0 (not guaranteed)**. The time server sends every bundle **twice**
    (`0x46b8bd`: loop `edi = 2`), clients once; the game has its own acks/retransmits ("Requests retransmitted",
    "NoAck", TTP), so it tolerates loss and duplicates.
* **Receive** (`0x5c1380`): `Receive(&from, &to, DPRECEIVE_ALL, buf, &size)`; `DPERR_NOMESSAGES` (0x887700BE) = no
  message. Buffers: 0x400 bytes during setup (`0x470e41`), 0x9c40 = 40000 bytes in game (`0x46add3`). In-game the
  loop first calls `GetMessageCount` (vtable slot 5).
* **System messages** (`from == 0`, switch at `0x5bcde1`): `DPSYS_CREATEPLAYERORGROUP` (3), `DESTROYPLAYERORGROUP`
  (5), `SESSIONLOST` (0x31), `HOST` (0x101), `SETPLAYERORGROUPDATA` (0x102, only if `dwPlayerType == 1`),
  `SETSESSIONDESC` (0x104). Anything else logs "Unsupported DPSYS Message" (harmless).

### 1.6 Game-level protocol on top of DirectPlay (the payloads the relay forwards)

Setup layer (`CGameNetSession`, dispatcher `0x470e25`, jump table `0x471498`/`0x4714b8`, first payload byte = type):

| Type | Handler | Meaning (from log strings) |
| --- | --- | --- |
| 5 | `0x471325` | KillPlayer received (kick, e.g. "wegen GameFull", "during GameStart") |
| 16 | `0x471214` | NetId request/assignment ("Requesting NetId %i") HYPOTHESIS |
| 20 | `0x470eb0` | player/slot info, then readiness check `0x471df0` HYPOTHESIS |
| 25 | `0x470f04` | Received GameSetup (315 bytes, map, races, …) |
| 26 | `0x470e85` | Slot infos HYPOTHESIS |
| 27 | `0x4712b4` | ? |
| 28 | `0x471356` | LoadGameInfo received (multiplayer save load) |

Map transfer: strings "FTP: …" belong to an **FTP download** of maps from a server (`0x465a45`), not a peer
transfer; maps are matched by checksum ("DLG: Invalid map checksum").

In-game lockstep (`CNetSession`, `0x46a000-0x46f500`): the host is the **TimeServer** ("Ich bin der TimeServer");
messages Request / Announce / Acknowledge / CurrentStep / ExecuteTo / NewHost / ping info; bundles have 4-byte
sub-message headers `type u8 (HYPOTHESIS), 0, size u8, retry u8` (byte 2 is the copy length, byte 3 is incremented on
resend; client bundle header `20 00 08 01`), bundle ≤ ~0x7d dwords.
Latency adaption: "Timings angepaßt an Spieler %i mit Ping von %u. Latency, Stepahead, TransferRate". Host migration:
on DPSYS_HOST the new host logs "Ich bin der neue TimeServer" and sends NewHost (12 bytes, guaranteed, `0x46ec3f`).
**Desync detection** is game-side: per-player checksums ("CheckSum player %u = %u - CheckBuffer[%u][%u]", "Message
ChecksumFailure" at `0x53b59a`, then "Desync!", "Desync Buildings/Settler/Pile/Trade/Economy/World!").
`-netmon` command line flag writes much net traffic to `trace.lis` — use it for runtime RE.

Implications for us:
1. The relay must deliver **per-sender FIFO**, support **host migration** with DPSYS_HOST to exactly one new host,
   and send DESTROYPLAYER for players whose client vanished. Reliability for non-guaranteed traffic is a bonus.
2. **Determinism is the real problem** (lockstep): all peers must compute bit-identical game state. x87 results
   must match across arm64/x86_64 hosts (see mobile-vision.md W5) — otherwise "Desync!" fires. The relay only carries
   bytes; it cannot fix desyncs, it can only report them (we hook the checksum-failure site and send `DesyncReport`).

## 2. Our stack

```
 S3.EXE ──IDirectPlay4A/Lobby (guest COM)──> s3-dx dplay.rs (HLE, build phase)
                                               │ sync calls + event polling, SetEvent(hEvent)
                                               ▼
                                         s3-net::client::NetClient  (background thread, tokio current_thread)
                                               │ WebSocket (binary frames), resumable link
                                               ▼
                                         s3-relay (tokio; hub actor: clients, sessions, players)
```

* **One virtual SP** "Internet (Settlers 3 Port)"; sessions come from the relay (`ListSessions` filtered by
  application GUID **and identical exe sha256**, since lockstep needs identical code).
* **Transport: WebSocket over TCP** (tokio-tungstenite, no TLS yet). Reasons: works through NAT/mobile carriers/proxies,
  trivially hostable behind any HTTPS reverse proxy (wss) later, ordered+reliable. QUIC would avoid head-of-line
  blocking but S3's traffic is tiny (bundles of a few hundred bytes every step), so TCP is fine (HYPOTHESIS to
  re-check with real games on LTE).
* **Relay = authority for sessions** (DirectPlay "client/server-ish" emulation): it assigns instance GUIDs and DPIDs,
  keeps player lists, decides the host. Peers never talk directly (no NAT punching needed).

### 2.1 Semantics (what the relay guarantees)

| DirectPlay concept | Relay behaviour |
| --- | --- |
| Open CREATE | `CreateSession` → new instance GUID, creator = host, reply `Joined` |
| Open JOIN | `JoinSession` checks: exists, same exe, not `JOINDISABLED/NEWPLAYERSDISABLED`, password, `members < dwMaxPlayers` → `Joined{desc, host, players}` |
| EnumSessions | `ListSessions(appGuid)`; password never returned, `DPSESSION_PASSWORDREQUIRED` set instead; `dwCurrentPlayers` = player count |
| CreatePlayer | DPID allocated (never 0/1/0xFFFFFFFF, scrambled counter); creator gets `PlayerCreated` **before** others get `PlayerAdded` (→ DPSYS_CREATEPLAYERORGROUP) |
| DestroyPlayer / client leaves / slot expires | `PlayerRemoved` to the other members (→ DPSYS_DESTROYPLAYERORGROUP) |
| Set/GetPlayerData, SetPlayerName | owner only; broadcast `PlayerData` (→ DPSYS_SETPLAYERORGROUPDATA) / `PlayerName` |
| SetSessionDesc | host only; others get `SessionDescChanged` (→ DPSYS_SETSESSIONDESC) |
| Host leaves | with `MIGRATEHOST`: oldest remaining member becomes host, gets `BecameHost` (→ DPSYS_HOST); without: everyone gets `SessionLost` (→ DPSYS_SESSIONLOST) |
| Send(from, to, flags) | `from` must be owned by the sender (spoofing dropped). `to = 0`: one `Deliver` per other player (addressed to that player, like DirectPlay). Per-sender order preserved for every receiver |
| Send without GUARANTEED | still reliable, but dropped while the receiver has > `soft_queue_limit` (2048) unacked frames (DirectPlay may drop these too) |
| KEEPALIVE / connection loss | client pings every `keepalive` (2 s); relay drops a silent connection after 10 s; the client's slot (players, host role, queue) is kept `resume_grace` (20 s) for a resume; after that it is removed as if it left |
| Desync | `DesyncReport{step, checksum, detail}` logged and forwarded as `Desync` to the other members |

### 2.2 Resumable link

Mobile networks drop TCP connections (Wi-Fi↔LTE handover, sleep). Every non-control frame is numbered per direction
(`seq` 1, 2, …), every frame carries `ack` (highest seq received). Both sides keep unacked frames
(`s3_net::link::Link`). On reconnect the client sends `Hello{resume: token, last_seq}`; the relay answers
`Welcome{resumed: true, last_seq}` and both retransmit what the other has not seen; duplicates (`seq <= last`) are
ignored. The game sees nothing but a short stall (lockstep waits). If the relay no longer knows the token it
answers `resumed: false` and the client reports `SessionLost`. Explicit `Ack` frames are sent at most every 100 ms
when data was received but nothing was sent back. Test: `resume_after_connection_drop_loses_nothing`.

### 2.3 Wire format (protocol v1)

One WebSocket **binary** message = one frame:

```
frame   = seq:var  ack:var  tag:u8  body
var     = unsigned LEB128 (u16/u32/u64; overlong/overflow rejected)
bytes   = len:var raw[len]        (max 65536)
string  = bytes, UTF-8            (max 256 bytes)
guid    = 16 raw bytes, Windows GUID memory order (Data1..3 little-endian)
list<T> = count:var T*            (max 1024)
desc    = flags:var instance:guid application:guid max_players:var current_players:var
          name:string password:string user1..4:var
player  = dpid:var owner:var flags:var short_name:string long_name:string data:bytes
```

seq = 0 marks control frames (never retransmitted). Trailing bytes and unknown tags are errors (connection closed).

Client → relay:

| Tag | Message | Body |
| --- | --- | --- |
| 1 | Hello (control) | proto:var(=1) exe_sha256:32 raw name:string build:string resume:(0 \| 1 token:16 raw last_seq:var) |
| 2 | Ping (control) | nonce:var |
| 3 | Ack (control) | — |
| 4 | Bye (control) | — (leave immediately, no grace) |
| 10 | ListSessions | req:var application:guid |
| 11 | CreateSession | req:var desc |
| 12 | JoinSession | req:var instance:guid password:string |
| 13 | LeaveSession | req:var |
| 14 | SetSessionDesc | req:var desc |
| 20 | CreatePlayer | req:var flags:var short:string long:string data:bytes |
| 21 | DestroyPlayer | req:var dpid:var |
| 22 | SetPlayerData | req:var dpid:var data:bytes |
| 23 | SetPlayerName | req:var dpid:var short:string long:string |
| 30 | Send | from:var to:var flags:var payload:bytes |
| 40 | DesyncReport | step:var checksum:var detail:string |

Relay → client:

| Tag | Message | Body |
| --- | --- | --- |
| 1 | Welcome (control) | proto:var client_id:var token:16 raw resumed:u8 last_seq:var keepalive_ms:var server:string |
| 2 | Reject (control) | code:var message:string (then close) |
| 3 | Pong (control) | nonce:var |
| 4 | Ack (control) | — |
| 10 | Error | req:var code:var message:string |
| 11 | Ok | req:var |
| 12 | SessionList | req:var list\<desc\> |
| 13 | Joined | req:var desc host_client:var list\<player\> |
| 14 | PlayerCreated | req:var dpid:var |
| 20 | PlayerAdded | player |
| 21 | PlayerRemoved | dpid:var |
| 22 | PlayerData | dpid:var data:bytes |
| 23 | PlayerName | dpid:var short:string long:string |
| 24 | SessionDescChanged | desc |
| 25 | BecameHost | — |
| 26 | SessionLost | code:var |
| 30 | Deliver | from:var to:var flags:var payload:bytes |
| 40 | Desync | client_id:var step:var checksum:var detail:string |

Error codes (`ErrorCode`, mapped to HRESULTs by `s3_net::dplay::hresult_for`): 1 BadRequest (INVALIDPARAMS),
2 VersionMismatch / 3 ExeMismatch (UNKNOWNAPPLICATION), 4 ResumeFailed (SESSIONLOST), 5 ServerFull
(CANNOTCREATESERVER), 10 NoSuchSession (NOSESSIONS), 11 SessionFull / 13 JoinDisabled (NONEWPLAYERS),
12 WrongPassword (INVALIDPASSWORD), 14 NotInSession (UNINITIALIZED), 15 AlreadyInSession (ALREADYINITIALIZED),
20 NoSuchPlayer (INVALIDPLAYER), 21 NotOwner (ACCESSDENIED), 30 Timeout (TIMEOUT), 31 Disconnected (CONNECTIONLOST).

Handshake checks: protocol version must equal 1; if the relay runs with `--gog-only`/`--allow-exe`, the exe hash
must be listed. Payload limit 64 KiB per message (S3's largest buffer is 40000 bytes).

### 2.4 Code map

| File | What |
| --- | --- |
| `port/crates/s3-net/src/wire.rs` | varint/bytes/string encoder + bounded decoder |
| `port/crates/s3-net/src/protocol.rs` | `ClientMsg`, `ServerMsg`, `Frame`, `SessionDesc`, `PlayerInfo`, `ErrorCode`, `guid()` |
| `port/crates/s3-net/src/link.rs` | seq/ack/retransmit bookkeeping (both sides) |
| `port/crates/s3-net/src/client.rs` | `NetClient`: blocking requests (`create_session`, `join_session`, `create_player`, …), non-blocking `send`, `poll_event`/`wait_event`, `snapshot()` (session, host, players, rtt), auto-resume |
| `port/crates/s3-net/src/dplay.rs` | DPERR table (from the exe), DPSYS/DPSESSION/DPSEND constants, `SystemMessage::layout(guest_addr)` = DPMSG_* bytes for Receive, `hresult_for` |
| `port/crates/s3-dx/src/dplay_net.rs` | the DirectPlay HLE over `NetClient` (section 5) |
| `port/crates/s3-desktop/src/mp_hash.rs` | `s3run --mp-hash`: per-simulation-tick state hash, exec log, game-log / net debug hooks (section 6) |
| `port/scripts/mp-local-test.sh`, `tools/mp_hash_compare.py`, `tools/mp_dump_diff.py` | two-instance test and its comparison tools |
| `port/crates/s3-relay/src/lib.rs` | `start(RelayConfig) -> RelayHandle` (url, stats, kick for tests); hub actor |
| `port/crates/s3-relay/src/main.rs` | CLI binary |
| `port/crates/s3-relay/tests/two_clients.rs` | create/list/join/players/ordered data/spoofing/data+desc changes/desync/leave; host migration (3 clients) and session loss; password/full/exe rules; exe allow-list; resume after drop without loss; slot expiry |

Dependencies (all MIT/Apache-2.0, GPL-3.0-compatible): tokio, tokio-tungstenite/tungstenite (no TLS features),
futures-util, log. Randomness for GUIDs/tokens comes from std's OS-seeded hasher (HYPOTHESIS: good enough for a
hobby relay; switch to `getrandom` before public deployment).

## 3. HLE plan (original plan; what was built is in section 5)

1. `DirectPlayLobby::GetConnectionSettings`: keep `DPERR_NOTLOBBIED` unless the frontend requested a lobby launch
   (then return a DPLCONNECTION; see 1.2).
2. `EnumConnections`: one callback with `SP_GUID_S3PORT`, name "Internet (Settlers 3 Port)", a small opaque blob.
   `InitializeConnection` with that blob = `NetClient::connect` (relay URL from settings; default
   `ws://127.0.0.1:7777/` for tests). Other SPs: none.
3. `EnumSessions`: `list_sessions(guidApplication)`, call the callback per session (DPSESSIONDESC2 in guest memory),
   then once with `DPESC_TIMEDOUT`. With `DPENUMSESSIONS_ASYNC` return the cached list immediately and refresh in
   the background; `STOPASYNC` stops refreshing.
4. `Open`: CREATE → `create_session`, JOIN → `join_session(instance, password)`; map `NetError::code()` via
   `hresult_for`. `GetSessionDesc`/`SetSessionDesc` from `snapshot()` / `set_session_desc`.
5. `CreatePlayer` → `create_player`; remember the guest **event handle** per local DPID and `SetEvent` it whenever
   a message for that player is queued. `EnumPlayers` from `snapshot().players` (local + remote).
6. `Receive`/`GetMessageCount`: drain `poll_event()` into a guest-side queue: `Event::Message` → data message
   (`from`, `to`); everything else → `SystemMessage::layout(buf)` with `from = DPID_SYSMSG`; `BUFFERTOOSMALL` +
   required size if the game's buffer is short; `Event::Link(Reconnecting/Resumed)` only logged (maybe an overlay
   icon); `Event::Link(Lost)` → DPSYS_SESSIONLOST.
7. `Send` → `NetClient::send` (never blocks). `Close` → `leave_session`.
8. Desync: an `s3-mods` hook at the checksum-failure site (`0x53b59a`, "Message ChecksumFailure") calls
   `report_desync(step, checksum, …)` (HYPOTHESIS: exact hook point and arguments to be confirmed at runtime).
9. Test with two `s3run` instances on one PC against a local relay (desktop), then emulator + desktop.

## 4. PC crossplay later (design notes)

Goal: let Windows players with the real game (Settlers United / aLobby, real `dplayx.dll`) play with ours.
Options, cheapest first:

1. **Bridge client on the PC** (recommended): a small Windows helper registers a DirectPlay **service provider**
   (`dpwsockx`-style SP DLL) or simply hooks `dplayx` via the game's own `CoCreateInstance`, and speaks our relay
   protocol. Windows players then see the same "Internet (Settlers 3 Port)" connection. Requires the same exe build
   (crossplay only with GOG/SU builds whose sha256 we support) and the same determinism (PC x87 is the reference;
   our emulated x87 must match native bit-for-bit).
2. **Real DirectPlay wire protocol in the relay** ("DirectPlay 6 TCP/IP SP": enumeration on UDP/TCP 47624, then
   dynamic ports 2300-2400 (TCP + UDP), messages with the `play` signature). The relay would act as a DirectPlay
   name server/peer so unmodified PCs connect by IP (Settlers United already tunnels this over its VPN-like lobby).
   Big job (DirectPlay's peer-to-peer topology means every PC talks to every peer; the relay must impersonate all
   our mobile players with distinct addresses). Documented by Wine's `dlls/dplayx` and the MS-DPDX spec (HYPOTHESIS:
   check spec coverage).
3. **VPN/LAN emulation**: put mobile clients on a virtual LAN with PCs (e.g. a WireGuard/tun-based overlay) and run
   the real DirectPlay protocol from our HLE. Heavy on mobile (VPN permission, battery).

Our protocol keeps everything the DirectPlay wire model needs (DPIDs, session desc incl. `dwUser`, guaranteed flag,
system events), so a bridge can translate 1:1.

## 5. DirectPlay HLE (implemented 2026-11-05, `port/crates/s3-dx/src/dplay_net.rs`)

`dplay.rs` keeps the COM plumbing (vtables, factories, lobby stubs) and routes these IDirectPlay4A methods to
`dplay_net.rs`; one `NetClient` per machine (`Machine::state::<DpNet>`), shared by all DirectPlay objects the game
creates (it re-creates them per connection test).

| Method | Behaviour |
| --- | --- |
| `EnumConnections` | one callback: SP GUID `SP_GUID_S3PORT`, connection blob `"S3PORTv1"+GUID` (24 bytes), DPNAME "Internet (Settlers 3 Port)", flags `DPCONNECTION_DIRECTPLAY` |
| `InitializeConnection` | checks the blob, connects to the relay once (`NetConfig`: `set_config(m, ..)` or env `S3_RELAY_URL`, `S3_NET_NAME`); relay down -> `DPERR_UNAVAILABLE` |
| `GetCaps` / `GetPlayerCaps` | `GUARANTEEDOPTIMIZED|GUARANTEEDSUPPORTED` (+`ISHOST`), so the game does not add `DIRECTPLAYPROTOCOL`; 65000-byte buffers, 1 local player |
| `EnumSessions` | sync: `list_sessions(app GUID)` + callbacks + `DPESC_TIMEDOUT`; `ASYNC`: cached list, refreshed by a background thread; `STOPASYNC`: no-op. Session names live in stable guest strings (the game may keep the pointer) |
| `Open` | `CREATE` -> `create_session(desc from the game)`, `JOIN` -> `join_session(instance, password)`; errors via `hresult_for` |
| `Close` | `leave_session`, clears local players and the queue |
| `EnumPlayers` | local (flag `DPENUMPLAYERS_LOCAL` 0x8) + remote (0x10) players sorted by DPID |
| `CreatePlayer` / `DestroyPlayer` | relay player; remembers the **Win32 event** per local DPID |
| `Send` | `NetClient::send` (never blocks); to a local DPID: queued locally |
| `Receive` / `GetMessageCount` | drain relay events into a host queue (data -> `from`/`to`; events -> `DPSYS_*` via `SystemMessage::layout` at the game's buffer address, `from = 0`); `BUFFERTOOSMALL` + size; `TOPLAYER`/`FROMPLAYER`/`PEEK` honoured |
| `Get/SetSessionDesc`, `Get/SetPlayerData`, `Get/SetPlayerName` | relay snapshot / requests |
| groups, `Initialize` | succeed with no entries |

Service `dplay-pump` (every scheduler slice) and every Receive/GetMessageCount drain relay events and **SetEvent the
local players' events** when messages are queued. Debug: `S3_DPLAY_TRACE=1` logs every Send/Receive with the first
24 bytes. Test: `s3-dx` `dplay_two_machines_through_relay` (two machines, real relay in-process, guest callbacks:
EnumConnections -> InitializeConnection -> Open CREATE / EnumSessions + Open JOIN -> CreatePlayer -> DPSYS
CREATEPLAYER with `BUFFERTOOSMALL` retry -> ordered data both ways -> event signalled -> GetSessionDesc ->
Close -> DESTROYPLAYER).

### 5.1 Menu flow observed at runtime (GOG exe, 800x600 menu coordinates)

- Main menu "Multi-player Game: **LAN**" (165,275) opens "LAN Game": "Type of Network" lists our connection
  (200,131); "Search for games" (175,240); "Open Games" list (first entry ~200,330); "Join Game" (610,567);
  "New Game" (730,567).
- "Multi-player Game: **Internet**" (165,315) says "Internet connection not detected." without touching DirectPlay
  (HYPOTHESIS: it is the Blue Byte lobby over wsock32, whose `WSAStartup` fails on purpose). The LAN entry is the
  DirectPlay path; a mobile UI should rename/route it.
- New Game -> Game Setup with the map picker (tabs Random 181,133 / Multi-player Map 181,180; "384-2-Artokia" =
  first MP map 165,255; OK 630,467), setup OK (730,567) -> **Open(CREATE)** + CreatePlayer -> waiting room
  ("Status: Waiting for Players"). The joiner appears after Join Game. Each player must press its ready button
  (right of the chat line: host row 723,114, second row 723,134), then the host's OK (730,567) starts. OK before
  everyone is ready shows "Not all players are ready".
- Session desc the game creates (OBSERVED): flags 0x44, `dwMaxPlayers` 2 (from the map/setup), name
  "Player's game", `dwUser1/2` = `3c44fe57 3d439f21` (constant across runs; the game logs
  "Siedler3 Version 1.60: (3c44fe57,3d439f21)" — a build/version id), `dwUser3` 0xA0, `dwUser4` 1.
- Setup traffic (OBSERVED with `S3_DPLAY_TRACE`): host -> joiner GameSetup (type 0x19, 315 B, guaranteed) and slot
  infos (0x1a, 53 B, to all); ping pairs type 0x10 (12 B, `10 00 0c 00 ...` with a `timeGetTime` value, echoed);
  joiner `SetPlayerData` (-> DPSYS 0x102 at the host); host `SetSessionDesc` at game start (-> DPSYS 0x104);
  final slot info type 0x14. "LOGIC: GameLogic random seed N" is the same on both peers (seed from GameSetup).

### 5.2 In-game lockstep protocol (OBSERVED + STATIC)

- Every in-game message is a **bundle**: dword CRC32 (table at `session+0xe78`, `0x46dbd0`, over the rest) then
  4-byte sub-message headers `type, 0, size, retry` + payload. Receive loop `0x46adaa`: GetMessageCount, Receive
  into a 40000-byte buffer, DPID -> slot (`0x434ec0`), drop unless `session+0x23d[slot]` (active) and not
  `session+0x251[slot]` (left), drop on CRC mismatch, then dispatch by type (`0x46afe8` / `0x46afb8`):
  0x06 `0x46c8b0`, 0x10 `0x45c380` (ping), 0x11 `0x46d370` (**ExecuteTo**: `session+0x224 = max(.., value)`),
  0x12 `0x46cbd0`, 0x13 `0x46d190`, 0x16 `0x46d6a0`, 0x17 `0x46d7c0`, 0x18 `0x46d820`,
  0x20 `0x46d610` (ack: `session+0x1d4[slot] = max(.., value)`), 0x21 `0x46c8d0` (player request), 0x22 `0x46db40`.
- Start: the host (TimeServer) sends `11 00 08 01 | 0x11` (ExecuteTo 17) twice per round, the joiner answers
  `20 00 08 01 | 0`, until both logic threads run; then the joiner's first request `21 fb 10 01 ...` arrives and
  normal traffic starts. While a peer is still building the map both screens show "Initiating player2player
  communication....". With a **random map** this phase took 30-100+ s in our runs ("Position gefunden nach ~30000
  Versuchen" start-position search, slow in the interpreter): runs cut at 50-80 s looked "stuck" there but were
  only slow. The test script therefore uses the fixed map 384-2-Artokia.
- Logic side: `CLogicThread` loop `0x53dxxx` increments the **frame counter** `gd+0x2d2d8` by 4 every pass
  (`0x53dda5`), executes queued logic messages (`0x535990`; switch on the class byte at `0x535a59`) and runs the
  **simulation tick** (`0x53e5a1`: `gd` tick calls `0x4e7810`, `0x4e05e0`, `0x4e9670`, then AI `0x550f80` per slot)
  only when `word [0x7ace62] == 0`. The frame counter keeps running while lockstep pauses the simulation, so it
  differs between peers; the number of simulation ticks does not.
- Logic message classes seen: `0x01` = player commands from the network (`01 cmd len 0 ...`, e.g. `0x03`
  selection, `0x09` move, `0x01` place building, `0xfb` sent by each player every ~256 ticks with a
  checksum-like value: HYPOTHESIS: the game's own sync check); `0x02` = local timing (`02 00 08 01 v`, value differs
  per peer); `0x22` (0x58 bytes, local, executed at slightly different ticks per peer, bytes 4..5 differ).
- `gd+0x2d2dc` = simulation RNG state (changes exactly on simulation ticks; equal on both peers).
- Game debug log: function `0x428720` (printf-like). `S3_GAME_LOG=1` makes s3run print it (`[game] ...`);
  `S3_EXE_ARGS=-netmon` passes the game's net-monitor switch (more `NET:` lines, e.g. "Erstelle Zufallswelt").

## 6. Local two-instance test and desync check

`port/scripts/mp-local-test.sh [OUT] [SECONDS]` starts `s3-relay` on 127.0.0.1:$PORT (7787), two
`s3run --headless` (env `S3_RELAY_URL`, `S3_NET_NAME=host|join`), injects the menu clicks of 5.1 (host creates on
Artokia, joiner searches/joins at 17-20 s, both ready, host starts at 29 s), takes screenshots every 20 s and
compares the hash files with `tools/mp_hash_compare.py`. Env: `MAP=random`, `HOST_EXTRA`/`JOIN_EXTRA` (more inject
items, e.g. in-game build/select/move clicks), `HASH_EVERY`, `PORT`.

**Hash** (`s3run --mp-hash FILE [--mp-hash-every N]`, `port/crates/s3-desktop/src/mp_hash.rs`): a hook at the start of
every simulation tick (`0x53e5a1`) writes `step=TICK rng=.. n=settlers/buildings/piles settlers=.. buildings=..
piles=.. trade=.. econ=.. players=.. [world=.. every 64 ticks] frame=.. local=..` (FNV-1a over the tables of
game-state-units.md). World tile bytes `+0x09 +0x0b +0x0c +0x0d` are masked: they differ per peer from the first
tick (+9/+d, one region: HYPOTHESIS fog / local view) or around the local player's land in build mode (+b/+c:
HYPOTHESIS placement markers) while all simulation tables stay identical. A hook on the logic executor
(`0x535a59`) writes `exec at=TICK frame=.. msg=HEX` for every executed logic message; the compare tool checks
that both peers execute the same class-0x01 messages at the same ticks. Debug: `S3_MP_DUMP_STEP=t1,t2` writes the
raw tables before those ticks (`tools/mp_dump_diff.py DIR TICK` lists differing per-entry offsets), `S3_MP_HDR=1`
adds `gd+0x2d240..` dwords, `S3_MP_NETDBG=1` logs the receive filter and CRC failures.

Results (2026-11-05, x86_64 desktop, both peers on this PC; logs/hashes in `scratch/mp-dplay/`, local only):

| Run | Setup | Result |
| --- | --- | --- |
| long1 | random 384 map, 330 s, select+move by both players, 1 building placed | 3979 ticks (~4.2 min game time at 64 ms/tick), 5 player commands executed at identical ticks, all simulation tables identical at every tick; world differed only in bytes masked since (+b/+c) |
| a1 | Artokia, 60 s | 391 ticks, IDENTICAL |
| long2 | Artokia, 400 s, build/select/move by both players | 5489 ticks (~5.9 min), 8 player commands (4 building sites, 2 selections, 2 moves) at identical ticks, settlers/buildings/piles/trade/econ/players identical at **every** tick; world hash differs from tick 4992 |
| long3 | same as long2 + raw dumps | 5052 ticks, same picture; world differs from tick 3392: dump at tick 4800 = only the masked bytes plus **tile byte +0x0f on 3 tiles** ((293,345), (301,335), (301,341): 0x18 on one peer, 0 on the other) |

Conclusion: no simulation desync found in ~16 min of combined two-peer game time. The remaining world difference is
3 tiles of byte +0x0f while every settler/building/pile/economy/player record stays bit-identical for thousands of
ticks afterwards; HYPOTHESIS: another local-only marker (e.g. a move-target / selection marker drawn for the local
player), not simulation state. To confirm: watch writes to those tiles (add a write watchpoint to s3run) and check
whether the game's own sync messages (`0xfb`) still agree. The compare tool reports this case as
`RESULT: SIMULATION TABLES IDENTICAL; world tiles differ` (exit code 3).

Pitfall found on the way: hashing at the frame counter (`gd+0x2d2d8`) instead of the simulation tick reported
"desyncs" from step ~84 in about half the runs; the sequences of distinct states were identical — the peers had just
paused at different frames. Always key comparisons by simulation tick.

## 7. Open questions / next steps

- Hook the game's own desync detection ("Desync!" `0x53ad7c`, "Message ChecksumFailure" `0x53b59a`) and call
  `dplay_net::report_desync` so the relay forwards it (the relay side exists).
- Android: the APK manifest has no `android.permission.INTERNET` yet (needed before `InitializeConnection` can reach a
  relay; s3-dx with s3-net/tokio already compiles for `aarch64-linux-android`).
- Lobby launch (1.2) from a native mobile lobby, and a frontend setting for the relay URL (Android: today only the
  env var / `dplay_net::set_config`).
- Cross-host: run the same test with one peer on arm64 (emulator-5610 / phone) — determinism.md golden vectors first.
- Exact meaning of setup types 16/20/26/27, of class-0x22 logic messages and of the per-peer tile bytes
  (+0x09/+0x0b/+0x0c/+0x0d masked; +0x0f differs on a few tiles after ~3400-5000 ticks, unmasked).
- Host migration / player leaving in game (relay supports it; game reaction untested).
- More than 2 players; the 20-slot `dwMaxPlayers` path.

