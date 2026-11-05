# Online multiplayer (W5): how S3.EXE uses DirectPlay, and our relay stack

Status 2026-11-05: **design + skeleton done.** Static RE of the game's DirectPlay usage (below), protocol v1,
`port/crates/s3-net` (protocol, resumable link, sync client, DirectPlay constants/system-message layouts) and
`port/crates/s3-relay` (tokio WebSocket relay, library + binary) implemented with 18 tests including two and three
real clients talking through a local relay. **Not wired into the game yet:** s3-dx `dplay.rs` is still the stub
(the build phase replaces it, see "HLE plan"). Nothing here has been exercised by S3.EXE at runtime, so every
statement about game behaviour is static RE (OBSERVED in code) or marked HYPOTHESIS.

Verify: `cd port && CARGO_TARGET_DIR=$PWD/target cargo test -p s3-net -p s3-relay -j 4`
Run a relay: `port/target/debug/s3-relay --listen 0.0.0.0:7777 [--gog-only] [--grace 20] [-v]`

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
| `dwUser1/2` | globals `0x7a8000/0x7a8004` (runtime values; HYPOTHESIS: lobby/league ids) |
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
| `port/crates/s3-relay/src/lib.rs` | `start(RelayConfig) -> RelayHandle` (url, stats, kick for tests); hub actor |
| `port/crates/s3-relay/src/main.rs` | CLI binary |
| `port/crates/s3-relay/tests/two_clients.rs` | create/list/join/players/ordered data/spoofing/data+desc changes/desync/leave; host migration (3 clients) and session loss; password/full/exe rules; exe allow-list; resume after drop without loss; slot expiry |

Dependencies (all MIT/Apache-2.0, GPL-3.0-compatible): tokio, tokio-tungstenite/tungstenite (no TLS features),
futures-util, log. Randomness for GUIDs/tokens comes from std's OS-seeded hasher (HYPOTHESIS: good enough for a
hobby relay; switch to `getrandom` before public deployment).

## 3. HLE plan for the build phase (s3-dx `dplay.rs`)

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

## 5. Open questions (runtime)

- Exact meaning of setup types 16/20/26/27, `dwUser1/2/4`, and `dwMaxPlayers` source — capture with `-netmon` and a
  two-instance desktop run.
- Does the game's network loop wait on the CreatePlayer event (MsgWaitForMultipleObjects) or poll?
- How the in-game multiplayer menus react to a connection list with one entry; whether `InitializeConnection`
  failures hide entries.
- Lobby-launch landing screen (1.2).
- Cross-host determinism of our x87 (blocking issue for real online play, separate workstream).
