//! DirectPlay 6 HLE on top of our relay (`s3-net`): the IDirectPlay4A methods S3.EXE uses, backed by one
//! [`NetClient`] per machine. Design and RE: docs/porting/multiplayer.md (sections 1 and 3).
//!
//! * `EnumConnections` reports one connection, "Internet (Settlers 3 Port)" (`SP_GUID_S3PORT`).
//! * `InitializeConnection` connects to the relay (URL from [`NetConfig`], default env `S3_RELAY_URL` or
//!   `ws://127.0.0.1:7777/`).
//! * Sessions / players map 1:1 to relay sessions / players; `Receive` hands out data messages and `DPSYS_*`
//!   system messages from a host-side queue filled by the `dplay-pump` service, which also `SetEvent`s the
//!   Win32 event each local player passed to `CreatePlayer`.
//!
//! All guest structures are written exactly like dplay.h lays them out (32-bit, ANSI).

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use s3_net::client::{ClientConfig, Event, LinkState, NetClient};
use s3_net::dplay::{self as dp, dperr, SystemMessage};
use s3_net::protocol::{Guid, PlayerInfo, SessionDesc};
use s3_runtime::{KernelObject, LogLevel, Machine, ThunkResult};

use crate::{put_u32, ret};

/// Name shown in the game's "Type of Network" list.
pub const CONNECTION_NAME: &str = "Internet (Settlers 3 Port)";
/// Same entry in LAN mode (embedded relay + discovery, no relay URL configured).
pub const LAN_CONNECTION_NAME: &str = "Local network / Wi-Fi (Settlers 3 Port)";
/// Opaque connection blob handed to the game (it copies it and passes it back to InitializeConnection).
const BLOB_MAGIC: &[u8; 8] = b"S3PORTv1";

/// Relay settings. Frontends call [`set_config`]; otherwise the environment is used.
#[derive(Debug, Clone)]
pub struct NetConfig {
    pub relay_url: String,
    /// LAN mode (no relay URL configured): embedded relay + UDP discovery, `relay_url` unused. See [`LanCtx`].
    pub lan: bool,
    /// Client name shown in relay logs (the in-game player name comes from the game's own dialog).
    pub client_name: String,
}

impl NetConfig {
    pub fn from_env() -> Self {
        let url = std::env::var("S3_RELAY_URL").ok().filter(|u| !u.trim().is_empty());
        NetConfig {
            lan: url.is_none() && std::env::var("S3_LAN").map(|v| v != "0").unwrap_or(true),
            relay_url: url.unwrap_or_else(|| "ws://127.0.0.1:7777/".into()),
            client_name: std::env::var("S3_NET_NAME").unwrap_or_else(|_| "s3port".into()),
        }
    }
}

/// Override the relay settings (before the game opens its multiplayer menu).
pub fn set_config(m: &mut Machine, cfg: NetConfig) {
    let st = state(m);
    st.borrow_mut().config = Some(cfg);
}

/// Counters for logs/tests.
#[derive(Debug, Default, Clone, Copy)]
pub struct NetStats {
    pub sent: u64,
    pub sent_bytes: u64,
    pub received: u64,
    pub received_bytes: u64,
    pub sysmsgs: u64,
}

struct LocalPlayer {
    dpid: u32,
    event: u32,
    short_name: String,
    long_name: String,
    data: Vec<u8>,
}

enum Body {
    Data(Vec<u8>),
    Sys(SystemMessage),
}

struct Queued {
    from: u32,
    /// Local destination DPID (0 = any local player: system messages).
    to: u32,
    body: Body,
}

#[derive(Default)]
pub struct DpNet {
    config: Option<NetConfig>,
    /// Active relay client (LAN mode: the local relay, or the host's relay after a JOIN).
    client: Option<Arc<NetClient>>,
    lan: Option<Arc<LanCtx>>,
    sessions: Arc<Mutex<Vec<SessionDesc>>>,
    refreshing: Arc<AtomicBool>,
    /// Guest copies of session names (the game may keep the pointer from EnumSessions).
    name_strs: HashMap<(Guid, String), u32>,
    local: Vec<LocalPlayer>,
    queue: VecDeque<Queued>,
    in_session: bool,
    scratch: u32,
    pub stats: NetStats,
}

const SCRATCH_SIZE: u32 = 0x400;

/// TCP port the embedded LAN relay tries first (then any free port; the discovery beacon carries the real one).
pub const LAN_RELAY_PORT: u16 = 7777;

/// LAN mode (docs/porting/multiplayer.md section 8): every instance runs an embedded relay and the discovery
/// service. Hosting = creating the session on the own relay (and announcing it); joining = talking to the relay of
/// the host the session was found on.
pub struct LanCtx {
    pub local: Arc<NetClient>,
    pub relay_port: u16,
    pub disc: Arc<s3_net::lan::Discovery>,
    name: String,
    remotes: Mutex<HashMap<std::net::SocketAddr, Arc<NetClient>>>,
    routes: Mutex<HashMap<Guid, Arc<NetClient>>>,
}

/// The process-wide embedded relay (port), started on first use.
fn embedded_relay() -> std::io::Result<u16> {
    static PORT: Mutex<Option<u16>> = Mutex::new(None);
    let mut g = PORT.lock().unwrap();
    if let Some(p) = *g {
        return Ok(p);
    }
    let first = std::env::var("S3_LAN_RELAY_PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(LAN_RELAY_PORT);
    let addrs = [std::net::SocketAddr::from(([0, 0, 0, 0], first)), std::net::SocketAddr::from(([0, 0, 0, 0], 0))];
    let a = s3_relay::spawn_background(&addrs, s3_relay::RelayConfig::default())?;
    *g = Some(a.port());
    Ok(a.port())
}

impl LanCtx {
    fn start(name: &str) -> Result<LanCtx, String> {
        let port = embedded_relay().map_err(|e| format!("embedded relay: {e}"))?;
        let mut cc = ClientConfig::new(format!("ws://127.0.0.1:{port}/"), name.to_string());
        cc.request_timeout = Duration::from_secs(5);
        let disc = s3_net::lan::Discovery::global(&cc.exe_sha256);
        let local = NetClient::connect(cc).map_err(|e| e.to_string())?;
        Ok(LanCtx {
            local: Arc::new(local),
            relay_port: port,
            disc,
            name: name.to_string(),
            remotes: Mutex::new(HashMap::new()),
            routes: Mutex::new(HashMap::new()),
        })
    }

    /// Relay client for a session found by [`LanCtx::list`] (default: the local relay).
    fn route(&self, instance: Guid) -> Arc<NetClient> {
        self.routes.lock().unwrap().get(&instance).cloned().unwrap_or_else(|| self.local.clone())
    }

    /// Client for a host's relay: cached, else the first candidate address that accepts a connection.
    fn remote(&self, h: &s3_net::lan::LanHost) -> Option<(std::net::SocketAddr, Arc<NetClient>)> {
        {
            let r = self.remotes.lock().unwrap();
            if let Some(a) = h.relays.iter().find(|a| r.contains_key(a)) {
                return Some((*a, r[a].clone()));
            }
        }
        for a in &h.relays {
            let mut cc = ClientConfig::new(s3_net::lan::LanHost::url(a), self.name.clone());
            cc.request_timeout = Duration::from_secs(2);
            match NetClient::connect(cc) {
                Ok(c) => {
                    log::info!("dplay: LAN: connected to host relay {a} ('{}')", h.name);
                    let c = Arc::new(c);
                    self.remotes.lock().unwrap().insert(*a, c.clone());
                    return Some((*a, c));
                }
                Err(e) => log::warn!("dplay: LAN: host relay {a} unreachable: {e}"),
            }
        }
        None
    }

    /// Sessions on the own relay plus on every host found on the LAN (waits up to `wait` for the first host).
    fn list(&self, app: Guid, wait: Duration) -> Vec<SessionDesc> {
        let mut out = Vec::new();
        let mut routes = HashMap::new();
        if let Ok(l) = self.local.list_sessions(app) {
            for d in l {
                routes.insert(d.instance, self.local.clone());
                out.push(d);
            }
        }
        for h in self.disc.wait_hosts(wait) {
            let Some((addr, c)) = self.remote(&h) else { continue };
            match c.list_sessions(app) {
                Ok(l) => {
                    for d in l {
                        if !routes.contains_key(&d.instance) {
                            routes.insert(d.instance, c.clone());
                            out.push(d);
                        }
                    }
                }
                Err(e) => {
                    log::warn!("dplay: LAN: list on {addr} failed: {e}");
                    self.remotes.lock().unwrap().remove(&addr);
                }
            }
        }
        self.routes.lock().unwrap().extend(routes);
        out
    }
}


fn state(m: &mut Machine) -> s3_runtime::Shared<DpNet> {
    m.state::<DpNet>()
}

pub fn stats(m: &mut Machine) -> NetStats {
    state(m).borrow_mut().stats
}

fn scratch(m: &mut Machine) -> u32 {
    let st = state(m);
    let s = st.borrow_mut().scratch;
    if s != 0 {
        return s;
    }
    let p = m.mem.alloc(SCRATCH_SIZE);
    st.borrow_mut().scratch = p;
    p
}

fn client(m: &mut Machine) -> Option<Arc<NetClient>> {
    state(m).borrow_mut().client.clone()
}

fn trace_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("S3_DPLAY_TRACE").is_some())
}

fn hexs(b: &[u8]) -> String {
    b.iter().take(24).map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ")
}

fn log(m: &Machine, msg: &str) {
    m.log(LogLevel::Info, &format!("dplay: {msg}"));
}

fn read_guid(m: &Machine, addr: u32) -> Guid {
    let mut g = [0u8; 16];
    g.copy_from_slice(&m.mem.read_bytes(addr, 16));
    g
}

fn read_ansi(m: &Machine, p: u32) -> String {
    if p == 0 {
        return String::new();
    }
    // Latin-1 -> char (the game is German / Windows-1252).
    m.mem.read_cstr_bytes(p).into_iter().map(|b| b as char).collect()
}

/// DPSESSIONDESC2 (guest) -> SessionDesc.
fn read_session_desc(m: &Machine, p: u32) -> SessionDesc {
    let r = |o: u32| m.mem.read_u32(p + o);
    SessionDesc {
        flags: r(4),
        instance: read_guid(m, p + 8),
        application: read_guid(m, p + 0x18),
        max_players: r(0x28),
        current_players: r(0x2c),
        name: read_ansi(m, r(0x30)),
        password: read_ansi(m, r(0x34)),
        user: [r(0x40), r(0x44), r(0x48), r(0x4c)],
    }
}

/// Write a DPSESSIONDESC2 at `p` whose name points to a stable guest string.
fn write_session_desc(m: &mut Machine, p: u32, d: &SessionDesc) {
    let key = (d.instance, d.name.clone());
    let st = state(m);
    let cached = st.borrow_mut().name_strs.get(&key).copied();
    let name = match cached {
        Some(a) => a,
        None => {
            let bytes = dp::to_ansi(&d.name);
            let a = m.mem.alloc(bytes.len() as u32 + 1);
            m.mem.write_cstr_bytes(a, &bytes);
            st.borrow_mut().name_strs.insert(key, a);
            a
        }
    };
    let w = |m: &mut Machine, o: u32, v: u32| m.mem.write_u32(p + o, v);
    w(m, 0, dp::DPSESSIONDESC2_SIZE as u32);
    w(m, 4, d.flags);
    m.mem.write_bytes(p + 8, &d.instance);
    m.mem.write_bytes(p + 0x18, &d.application);
    w(m, 0x28, d.max_players);
    w(m, 0x2c, d.current_players);
    w(m, 0x30, name);
    for o in [0x34, 0x38, 0x3c] {
        w(m, o, 0);
    }
    for (i, u) in d.user.iter().enumerate() {
        w(m, 0x40 + 4 * i as u32, *u);
    }
}

/// DPNAME + strings: returns the byte image for guest address `base`.
fn dpname_bytes(base: u32, short: &str, long: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut tail = Vec::new();
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for s in [short, long] {
        if s.is_empty() {
            out.extend_from_slice(&0u32.to_le_bytes());
        } else {
            let ptr = base + 16 + tail.len() as u32;
            out.extend_from_slice(&ptr.to_le_bytes());
            tail.extend(dp::to_ansi(s));
            tail.push(0);
        }
    }
    out.extend(tail);
    out
}

fn read_dpname(m: &Machine, p: u32) -> (String, String) {
    if p == 0 {
        return (String::new(), String::new());
    }
    (read_ansi(m, m.mem.read_u32(p + 8)), read_ansi(m, m.mem.read_u32(p + 12)))
}

fn hr_for(e: &s3_net::client::NetError) -> u32 {
    dp::hresult_for(e.code())
}

fn set_guest_event(m: &mut Machine, h: u32) {
    if h == 0 {
        return;
    }
    if let Some(KernelObject::Event { signaled, .. }) = m.handles.get_mut(h) {
        *signaled = true;
    }
}

/// Signal the events of local players that have queued messages.
fn signal(m: &mut Machine) {
    let st = state(m);
    let evs: Vec<u32> = {
        let s = st.borrow_mut();
        s.local
            .iter()
            .filter(|p| s.queue.iter().any(|q| q.to == 0 || q.to == p.dpid))
            .map(|p| p.event)
            .collect()
    };
    for e in evs {
        set_guest_event(m, e);
    }
}

/// Move relay events into the receive queue. Returns true if anything arrived.
fn pump(m: &mut Machine) -> bool {
    let st = state(m);
    let Some(c) = st.borrow_mut().client.clone() else { return false };
    let mut got = false;
    let mut logs = Vec::new();
    while let Some(ev) = c.poll_event() {
        got = true;
        let mut s = st.borrow_mut();
        let sys = |s: &mut DpNet, msg: SystemMessage| {
            s.stats.sysmsgs += 1;
            s.queue.push_back(Queued { from: dp::DPID_SYSMSG, to: 0, body: Body::Sys(msg) });
        };
        match ev {
            Event::Message { from, to, payload, .. } => {
                s.stats.received += 1;
                s.stats.received_bytes += payload.len() as u64;
                s.queue.push_back(Queued { from, to, body: Body::Data(payload) });
            }
            Event::PlayerAdded(player) => {
                let n = c.snapshot().players.len() as u32;
                logs.push(format!("player added {:#x} '{}'", player.dpid, player.short_name));
                sys(&mut s, SystemMessage::CreatePlayer { player, current_players: n });
            }
            Event::PlayerRemoved(player) => {
                logs.push(format!("player removed {:#x}", player.dpid));
                sys(&mut s, SystemMessage::DestroyPlayer { player });
            }
            Event::PlayerData { dpid, data } => sys(&mut s, SystemMessage::SetPlayerData { dpid, data }),
            Event::PlayerName { .. } => {}
            Event::SessionDesc(desc) => sys(&mut s, SystemMessage::SetSessionDesc { desc }),
            Event::BecameHost => {
                logs.push("we are the new host".into());
                sys(&mut s, SystemMessage::Host)
            }
            Event::SessionLost(code) => {
                logs.push(format!("session lost ({code:?})"));
                s.in_session = false;
                sys(&mut s, SystemMessage::SessionLost)
            }
            Event::Desync { client_id, step, checksum, detail } => {
                logs.push(format!("DESYNC reported by client {client_id}: step {step} checksum {checksum:#x} {detail}"));
            }
            Event::Link(LinkState::Lost) => {
                logs.push("link lost".into());
                if s.in_session {
                    s.in_session = false;
                    sys(&mut s, SystemMessage::SessionLost)
                }
            }
            Event::Link(l) => logs.push(format!("link {l:?}")),
        }
    }
    for l in logs {
        log(m, &l);
    }
    got
}

/// Pump and signal the players' events if anything arrived (DirectPlay sets the event for every message).
fn pump_signal(m: &mut Machine) {
    if pump(m) {
        signal(m);
    }
}

/// Host service: run once per scheduler slice.
pub fn pump_service(m: &mut Machine) {
    pump_signal(m);
}

// ---------------------------------------------------------------------------------------------------------
// IDirectPlay4A methods

/// EnumConnections(this, lpguidApplication, lpEnumCallback, lpContext, dwFlags).
pub fn enum_connections(m: &mut Machine) -> ThunkResult {
    let (cb, ctx, flags) = (m.arg_u32(2), m.arg_u32(3), m.arg_u32(4));
    if cb == 0 {
        return ret(dperr::INVALIDPARAMS, 5);
    }
    if flags != 0 && flags & dp::DPCONNECTION_DIRECTPLAY == 0 {
        return ret(dp::DP_OK, 5);
    }
    let s = scratch(m);
    let guid = s;
    let blob = s + 0x10;
    let name = s + 0x40;
    m.mem.write_bytes(guid, &dp::SP_GUID_S3PORT);
    let mut b = BLOB_MAGIC.to_vec();
    b.extend_from_slice(&dp::SP_GUID_S3PORT);
    m.mem.write_bytes(blob, &b);
    let st = state(m);
    let lan = st.borrow_mut().config.get_or_insert_with(NetConfig::from_env).lan;
    let label = if lan { LAN_CONNECTION_NAME } else { CONNECTION_NAME };
    let nb = dpname_bytes(name, label, label);
    m.mem.write_bytes(name, &nb);
    m.call_guest(cb, &[guid, blob, b.len() as u32, name, dp::DPCONNECTION_DIRECTPLAY, ctx]);
    ret(dp::DP_OK, 5)
}

/// InitializeConnection(this, lpConnection, dwFlags): connect to the relay (once per machine).
pub fn initialize_connection(m: &mut Machine) -> ThunkResult {
    let conn = m.arg_u32(1);
    if conn == 0 || m.mem.read_bytes(conn, 8) != BLOB_MAGIC {
        return ret(dperr::INVALIDPARAMS, 3);
    }
    if client(m).is_some() {
        return ret(dp::DP_OK, 3);
    }
    let st = state(m);
    let cfg = st.borrow_mut().config.get_or_insert_with(NetConfig::from_env).clone();
    if cfg.lan {
        return match LanCtx::start(&cfg.client_name) {
            Ok(l) => {
                log(m, &format!("LAN mode: embedded relay on port {}, connected as client {}", l.relay_port, l.local.client_id()));
                let mut s = st.borrow_mut();
                s.client = Some(l.local.clone());
                s.lan = Some(Arc::new(l));
                ret(dp::DP_OK, 3)
            }
            Err(e) => {
                m.log(LogLevel::Warn, &format!("dplay: LAN relay unavailable: {e}"));
                ret(dperr::UNAVAILABLE, 3)
            }
        };
    }
    let mut cc = ClientConfig::new(cfg.relay_url.clone(), cfg.client_name.clone());
    cc.request_timeout = Duration::from_secs(5);
    match NetClient::connect(cc) {
        Ok(c) => {
            log(m, &format!("connected to relay {} as client {}", cfg.relay_url, c.client_id()));
            st.borrow_mut().client = Some(Arc::new(c));
            ret(dp::DP_OK, 3)
        }
        Err(e) => {
            m.log(LogLevel::Warn, &format!("dplay: relay {} unavailable: {e}", cfg.relay_url));
            ret(dperr::UNAVAILABLE, 3)
        }
    }
}

/// GetCaps(this, lpDPCaps, dwFlags) / GetPlayerCaps(this, id, lpDPCaps, dwFlags).
fn write_caps(m: &mut Machine, p: u32) -> u32 {
    if p == 0 || m.mem.read_u32(p) < 40 {
        return dperr::INVALIDPARAMS;
    }
    let host = client(m).map(|c| c.snapshot().is_host()).unwrap_or(false);
    let vals = [
        40,
        dp::DPCAPS_GUARANTEEDOPTIMIZED | dp::DPCAPS_GUARANTEEDSUPPORTED | if host { 0x2 } else { 0 },
        65_000, // dwMaxBufferSize
        0,      // dwMaxQueueSize
        64,     // dwMaxPlayers
        10_000, // dwHundredBaud (1 Mbit/s)
        50,     // dwLatency ms
        1,      // dwMaxLocalPlayers
        20,     // dwHeaderLength
        5000,   // dwTimeout
    ];
    for (i, v) in vals.iter().enumerate() {
        m.mem.write_u32(p + 4 * i as u32, *v);
    }
    dp::DP_OK
}

pub fn get_caps(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(1);
    ret(write_caps(m, p), 3)
}

pub fn get_player_caps(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(2);
    ret(write_caps(m, p), 4)
}

/// EnumSessions(this, lpsd, dwTimeout, lpEnumSessionsCallback2, lpContext, dwFlags).
pub fn enum_sessions(m: &mut Machine) -> ThunkResult {
    let (sd, cb, ctx, flags) = (m.arg_u32(1), m.arg_u32(3), m.arg_u32(4), m.arg_u32(5));
    let Some(c) = client(m) else { return ret(dperr::UNINITIALIZED, 6) };
    if flags & dp::DPENUMSESSIONS_STOPASYNC != 0 {
        return ret(dp::DP_OK, 6);
    }
    let app = if sd != 0 { read_guid(m, sd + 0x18) } else { s3_net::S3_APP_GUID };
    let st = state(m);
    let (cache, refreshing, lan) = {
        let s = st.borrow_mut();
        (s.sessions.clone(), s.refreshing.clone(), s.lan.clone())
    };
    let list = if flags & dp::DPENUMSESSIONS_ASYNC != 0 {
        // Return what we have; refresh in the background (the game polls every 250 ms).
        if !refreshing.swap(true, Ordering::AcqRel) {
            let (c2, cache2, r2, lan2) = (c.clone(), cache.clone(), refreshing.clone(), lan.clone());
            std::thread::spawn(move || {
                let l = match &lan2 {
                    Some(lan) => Ok(lan.list(app, Duration::ZERO)),
                    None => c2.list_sessions(app),
                };
                if let Ok(l) = l {
                    *cache2.lock().unwrap() = l;
                }
                r2.store(false, Ordering::Release);
            });
        }
        cache.lock().unwrap().clone()
    } else if let Some(lan) = &lan {
        let l = lan.list(app, Duration::from_millis(1200));
        *cache.lock().unwrap() = l.clone();
        l
    } else {
        match c.list_sessions(app) {
            Ok(l) => {
                *cache.lock().unwrap() = l.clone();
                l
            }
            Err(e) => return ret(hr_for(&e), 6),
        }
    };
    if cb == 0 {
        return ret(dp::DP_OK, 6);
    }
    let s = scratch(m);
    let (desc, timeout) = (s + 0x100, s + 0x1f0);
    m.mem.write_u32(timeout, m.arg_u32(2));
    for d in &list {
        write_session_desc(m, desc, d);
        if m.call_guest(cb, &[desc, timeout, 0, ctx]) == 0 || m.is_stopped() {
            return ret(dp::DP_OK, 6);
        }
    }
    if flags & dp::DPENUMSESSIONS_ASYNC == 0 {
        m.call_guest(cb, &[0, timeout, dp::DPESC_TIMEDOUT, ctx]);
    }
    ret(dp::DP_OK, 6)
}

/// Open(this, lpsd, dwFlags).
pub fn open(m: &mut Machine) -> ThunkResult {
    let (sd, flags) = (m.arg_u32(1), m.arg_u32(2));
    let Some(mut c) = client(m) else { return ret(dperr::UNINITIALIZED, 3) };
    if sd == 0 {
        return ret(dperr::INVALIDPARAMS, 3);
    }
    let desc = read_session_desc(m, sd);
    let lan = state(m).borrow_mut().lan.clone();
    if let Some(lan) = &lan {
        // CREATE on our own relay; JOIN on the relay the session was found on.
        c = if flags & dp::DPOPEN_CREATE != 0 { lan.local.clone() } else { lan.route(desc.instance) };
        state(m).borrow_mut().client = Some(c.clone());
    }
    let r = if flags & dp::DPOPEN_CREATE != 0 {
        c.create_session(desc.clone())
    } else {
        c.join_session(desc.instance, &desc.password)
    };
    match r {
        Ok(j) => {
            log(
                m,
                &format!(
                    "{} session '{}' flags {:#x} max {} user {:x?}; host client {}, {} players",
                    if flags & dp::DPOPEN_CREATE != 0 { "created" } else { "joined" },
                    j.desc.name,
                    j.desc.flags,
                    j.desc.max_players,
                    j.desc.user,
                    j.host,
                    j.players.len()
                ),
            );
            if let (Some(lan), true) = (&lan, flags & dp::DPOPEN_CREATE != 0) {
                lan.disc.set_hosting(Some((lan.relay_port, j.desc.name.clone())));
            }
            let st = state(m);
            let mut s = st.borrow_mut();
            s.in_session = true;
            s.queue.clear();
            ret(dp::DP_OK, 3)
        }
        Err(e) => {
            if let Some(lan) = &lan {
                state(m).borrow_mut().client = Some(lan.local.clone());
            }
            m.log(LogLevel::Warn, &format!("dplay: Open({flags:#x}) failed: {e}"));
            ret(hr_for(&e), 3)
        }
    }
}

/// Close(this).
pub fn close(m: &mut Machine) -> ThunkResult {
    let st = state(m);
    let (c, was, lan) = {
        let mut s = st.borrow_mut();
        let was = s.in_session;
        s.in_session = false;
        s.local.clear();
        s.queue.clear();
        (s.client.clone(), was, s.lan.clone())
    };
    if let (Some(c), true) = (c, was) {
        let _ = c.leave_session();
        log(m, "left session");
    }
    if let Some(lan) = lan {
        lan.disc.set_hosting(None);
        st.borrow_mut().client = Some(lan.local.clone());
    }
    ret(dp::DP_OK, 1)
}

/// EnumPlayers(this, lpguidInstance, lpEnumPlayersCallback2, lpContext, dwFlags).
pub fn enum_players(m: &mut Machine) -> ThunkResult {
    let (cb, ctx, flags) = (m.arg_u32(2), m.arg_u32(3), m.arg_u32(4));
    let Some(c) = client(m) else { return ret(dperr::UNINITIALIZED, 5) };
    if cb == 0 {
        return ret(dperr::INVALIDPARAMS, 5);
    }
    let snap = c.snapshot();
    let st = state(m);
    let mut list: Vec<(u32, String, String, bool)> = {
        let s = st.borrow_mut();
        if !s.in_session {
            return ret(dp::DP_OK, 5);
        }
        let mut v: Vec<_> = s.local.iter().map(|p| (p.dpid, p.short_name.clone(), p.long_name.clone(), true)).collect();
        for p in snap.players.values() {
            if !s.local.iter().any(|l| l.dpid == p.dpid) {
                v.push((p.dpid, p.short_name.clone(), p.long_name.clone(), false));
            }
        }
        v
    };
    list.sort_by_key(|p| p.0);
    let name = scratch(m) + 0x200;
    for (dpid, short, long, local) in list {
        if flags & 0x8 != 0 && !local || flags & 0x10 != 0 && local {
            continue;
        }
        let nb = dpname_bytes(name, &short, &long);
        m.mem.write_bytes(name, &nb);
        let f = if local { 0x8 } else { 0x10 };
        if m.call_guest(cb, &[dpid, dp::DPPLAYERTYPE_PLAYER, name, f, ctx]) == 0 || m.is_stopped() {
            break;
        }
    }
    ret(dp::DP_OK, 5)
}

/// CreatePlayer(this, lpidPlayer, lpPlayerName, hEvent, lpData, dwDataSize, dwFlags).
pub fn create_player(m: &mut Machine) -> ThunkResult {
    let (out, pname, ev, data, size, flags) =
        (m.arg_u32(1), m.arg_u32(2), m.arg_u32(3), m.arg_u32(4), m.arg_u32(5), m.arg_u32(6));
    let Some(c) = client(m) else { return ret(dperr::UNINITIALIZED, 7) };
    let (short, long) = read_dpname(m, pname);
    let data = if data != 0 { m.mem.read_bytes(data, size as usize) } else { Vec::new() };
    match c.create_player(&short, &long, &data, flags) {
        Ok(dpid) => {
            log(m, &format!("CreatePlayer '{short}' -> dpid {dpid:#x} (event {ev:#x})"));
            put_u32(m, out, dpid);
            let st = state(m);
            st.borrow_mut().local.push(LocalPlayer { dpid, event: ev, short_name: short, long_name: long, data });
            ret(dp::DP_OK, 7)
        }
        Err(e) => ret(hr_for(&e), 7),
    }
}

/// DestroyPlayer(this, idPlayer).
pub fn destroy_player(m: &mut Machine) -> ThunkResult {
    let id = m.arg_u32(1);
    let Some(c) = client(m) else { return ret(dperr::UNINITIALIZED, 2) };
    let st = state(m);
    st.borrow_mut().local.retain(|p| p.dpid != id);
    match c.destroy_player(id) {
        Ok(()) => ret(dp::DP_OK, 2),
        Err(e) => ret(hr_for(&e), 2),
    }
}

/// Send(this, idFrom, idTo, dwFlags, lpData, dwDataSize).
pub fn send(m: &mut Machine) -> ThunkResult {
    let (from, to, flags, data, size) = (m.arg_u32(1), m.arg_u32(2), m.arg_u32(3), m.arg_u32(4), m.arg_u32(5));
    let Some(c) = client(m) else { return ret(dperr::UNINITIALIZED, 6) };
    if data == 0 && size != 0 {
        return ret(dperr::INVALIDPARAMS, 6);
    }
    let payload = m.mem.read_bytes(data, size as usize);
    if trace_on() {
        log(m, &format!("Send {from:#x} -> {to:#x} flags {flags:#x} {size} B: {}", hexs(&payload)));
    }
    let st = state(m);
    {
        let mut s = st.borrow_mut();
        if !s.in_session {
            return ret(dperr::UNINITIALIZED, 6);
        }
        s.stats.sent += 1;
        s.stats.sent_bytes += size as u64;
        // To one of our own local players: deliver locally (the relay only routes to other clients).
        if to != 0 && s.local.iter().any(|p| p.dpid == to) {
            s.queue.push_back(Queued { from, to, body: Body::Data(payload) });
            drop(s);
            signal(m);
            return ret(dp::DP_OK, 6);
        }
    }
    match c.send(from, to, flags, &payload) {
        Ok(()) => ret(dp::DP_OK, 6),
        Err(e) => ret(hr_for(&e), 6),
    }
}

fn matches(q: &Queued, flags: u32, from: u32, to: u32) -> bool {
    if flags & dp::DPRECEIVE_TOPLAYER != 0 && !(q.to == to || q.to == 0) {
        return false;
    }
    if flags & dp::DPRECEIVE_FROMPLAYER != 0 && q.from != from {
        return false;
    }
    true
}

/// Receive(this, lpidFrom, lpidTo, dwFlags, lpData, lpdwDataSize).
pub fn receive(m: &mut Machine) -> ThunkResult {
    let (pfrom, pto, flags, data, psize) = (m.arg_u32(1), m.arg_u32(2), m.arg_u32(3), m.arg_u32(4), m.arg_u32(5));
    if psize == 0 {
        return ret(dperr::INVALIDPARAMS, 6);
    }
    pump_signal(m);
    let want_from = if pfrom != 0 { m.mem.read_u32(pfrom) } else { 0 };
    let want_to = if pto != 0 { m.mem.read_u32(pto) } else { 0 };
    let st = state(m);
    let mut s = st.borrow_mut();
    let Some(i) = s.queue.iter().position(|q| matches(q, flags, want_from, want_to)) else {
        return ret(dperr::NOMESSAGES, 6);
    };
    let q = &s.queue[i];
    let bytes = match &q.body {
        Body::Data(d) => d.clone(),
        Body::Sys(msg) => msg.layout(data),
    };
    let cap = m.mem.read_u32(psize);
    m.mem.write_u32(psize, bytes.len() as u32);
    if data == 0 || (cap as usize) < bytes.len() {
        return ret(dperr::BUFFERTOOSMALL, 6);
    }
    let from = q.from;
    let to = if q.to != 0 { q.to } else { s.local.first().map(|p| p.dpid).unwrap_or(0) };
    if trace_on() {
        m.log(LogLevel::Info, &format!("dplay: Receive(flags {flags:#x}) {from:#x} -> {to:#x} {} B: {}", bytes.len(), hexs(&bytes)));
    }
    m.mem.write_bytes(data, &bytes);
    put_u32(m, pfrom, from);
    put_u32(m, pto, to);
    if flags & dp::DPRECEIVE_PEEK == 0 {
        s.queue.remove(i);
    }
    ret(dp::DP_OK, 6)
}

/// GetMessageCount(this, idPlayer, lpdwCount).
pub fn get_message_count(m: &mut Machine) -> ThunkResult {
    let (id, p) = (m.arg_u32(1), m.arg_u32(2));
    pump_signal(m);
    let st = state(m);
    let n = st.borrow_mut().queue.iter().filter(|q| id == 0 || q.to == 0 || q.to == id).count() as u32;
    put_u32(m, p, n);
    ret(dp::DP_OK, 3)
}

/// Copy `bytes` (laid out for address `data`) into a caller buffer with DirectPlay's size protocol.
fn give(m: &mut Machine, data: u32, psize: u32, bytes: &[u8]) -> u32 {
    if psize == 0 {
        return dperr::INVALIDPARAMS;
    }
    let cap = m.mem.read_u32(psize);
    m.mem.write_u32(psize, bytes.len() as u32);
    if data == 0 || (cap as usize) < bytes.len() {
        return dperr::BUFFERTOOSMALL;
    }
    m.mem.write_bytes(data, bytes);
    dp::DP_OK
}

/// GetSessionDesc(this, lpData, lpdwDataSize).
pub fn get_session_desc(m: &mut Machine) -> ThunkResult {
    let (data, psize) = (m.arg_u32(1), m.arg_u32(2));
    let Some(c) = client(m) else { return ret(dperr::UNINITIALIZED, 3) };
    let Some(d) = c.snapshot().session else { return ret(dperr::NOCONNECTION, 3) };
    // SystemMessage::SetSessionDesc = dwType + DPSESSIONDESC2 + name; drop the dwType.
    let full = SystemMessage::SetSessionDesc { desc: d }.layout(data.wrapping_sub(4));
    ret(give(m, data, psize, &full[4..]), 3)
}

/// SetSessionDesc(this, lpSessDesc, dwFlags).
pub fn set_session_desc(m: &mut Machine) -> ThunkResult {
    let sd = m.arg_u32(1);
    let Some(c) = client(m) else { return ret(dperr::UNINITIALIZED, 3) };
    let d = read_session_desc(m, sd);
    match c.set_session_desc(d) {
        Ok(()) => ret(dp::DP_OK, 3),
        Err(e) => ret(hr_for(&e), 3),
    }
}

fn player_info(m: &mut Machine, id: u32) -> Option<PlayerInfo> {
    let st = state(m);
    if let Some(p) = st.borrow_mut().local.iter().find(|p| p.dpid == id) {
        return Some(PlayerInfo {
            dpid: id,
            owner: 0,
            flags: 0,
            short_name: p.short_name.clone(),
            long_name: p.long_name.clone(),
            data: p.data.clone(),
        });
    }
    client(m)?.snapshot().players.get(&id).cloned()
}

/// GetPlayerData(this, idPlayer, lpData, lpdwDataSize, dwFlags).
pub fn get_player_data(m: &mut Machine) -> ThunkResult {
    let (id, data, psize) = (m.arg_u32(1), m.arg_u32(2), m.arg_u32(3));
    let Some(p) = player_info(m, id) else { return ret(dperr::INVALIDPLAYER, 5) };
    ret(give(m, data, psize, &p.data), 5)
}

/// SetPlayerData(this, idPlayer, lpData, dwDataSize, dwFlags).
pub fn set_player_data(m: &mut Machine) -> ThunkResult {
    let (id, data, size) = (m.arg_u32(1), m.arg_u32(2), m.arg_u32(3));
    let Some(c) = client(m) else { return ret(dperr::UNINITIALIZED, 5) };
    let bytes = if data != 0 { m.mem.read_bytes(data, size as usize) } else { Vec::new() };
    let st = state(m);
    if let Some(p) = st.borrow_mut().local.iter_mut().find(|p| p.dpid == id) {
        p.data = bytes.clone();
    }
    match c.set_player_data(id, &bytes) {
        Ok(()) => ret(dp::DP_OK, 5),
        Err(e) => ret(hr_for(&e), 5),
    }
}

/// GetPlayerName(this, idPlayer, lpData, lpdwDataSize).
pub fn get_player_name(m: &mut Machine) -> ThunkResult {
    let (id, data, psize) = (m.arg_u32(1), m.arg_u32(2), m.arg_u32(3));
    let Some(p) = player_info(m, id) else { return ret(dperr::INVALIDPLAYER, 4) };
    let b = dpname_bytes(data, &p.short_name, &p.long_name);
    ret(give(m, data, psize, &b), 4)
}

/// SetPlayerName(this, idPlayer, lpPlayerName, dwFlags).
pub fn set_player_name(m: &mut Machine) -> ThunkResult {
    let (id, pname) = (m.arg_u32(1), m.arg_u32(2));
    let Some(c) = client(m) else { return ret(dperr::UNINITIALIZED, 4) };
    let (short, long) = read_dpname(m, pname);
    let st = state(m);
    if let Some(p) = st.borrow_mut().local.iter_mut().find(|p| p.dpid == id) {
        p.short_name = short.clone();
        p.long_name = long.clone();
    }
    match c.set_player_name(id, &short, &long) {
        Ok(()) => ret(dp::DP_OK, 4),
        Err(e) => ret(hr_for(&e), 4),
    }
}

/// Report a desync to the relay (called from the s3run desync hook / tests).
pub fn report_desync(m: &mut Machine, step: u32, checksum: u32, detail: &str) {
    if let Some(c) = client(m) {
        let _ = c.report_desync(step, checksum, detail);
    }
}

/// Is this machine in a relay session (for frontends / overlays)?
pub fn in_session(m: &mut Machine) -> bool {
    state(m).borrow_mut().in_session
}
