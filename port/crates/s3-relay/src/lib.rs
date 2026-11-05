//! s3-relay: lobby/relay server for Settlers 3 port online play.
//!
//! One async "hub" task owns all state (clients, sessions, players) and processes events from per-connection
//! tasks in arrival order, so ordering per sender is preserved without locks. Each connection task only
//! decodes/encodes WebSocket frames. See docs/porting/multiplayer.md for protocol and semantics.
//!
//! Semantics mirror DirectPlay 6 as S3.EXE uses it: sessions with a host (migrates when the host leaves, because
//! S3 sets `DPSESSION_MIGRATEHOST`), players with DPIDs, system events (player added/removed, data, host,
//! session lost) and `Send(from, to, flags, payload)`. Delivery is reliable and ordered (TCP); messages *without*
//! `DPSEND_GUARANTEED` may be dropped when a receiver falls far behind, which DirectPlay also allows and S3's
//! lockstep layer tolerates (it retransmits on its own).

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use s3_net::dplay::{DPSESSION_JOINDISABLED, DPSESSION_MIGRATEHOST, DPSESSION_NEWPLAYERSDISABLED, DPSESSION_PASSWORDREQUIRED, DPSEND_GUARANTEED};
use s3_net::link::{Accept, Link};
use s3_net::protocol::*;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message as WsMessage;

#[derive(Debug, Clone)]
pub struct RelayConfig {
    pub listen: SocketAddr,
    /// Empty = accept any exe (sessions still only match identical exes).
    pub allowed_exes: Vec<Sha256>,
    pub max_clients: usize,
    /// Interval the client should ping at (sent in Welcome).
    pub keepalive: Duration,
    /// Drop a connection that sent nothing for this long.
    pub idle_timeout: Duration,
    /// Keep a dropped client's slot (players, host role, queued messages) this long for a resume.
    pub resume_grace: Duration,
    /// Non-guaranteed messages to a client are dropped while it has more unacked frames than this.
    pub soft_queue_limit: usize,
    /// A client with more unacked frames than this is disconnected (and its slot freed).
    pub hard_queue_limit: usize,
    pub server_name: String,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:0".parse().unwrap(),
            allowed_exes: Vec::new(),
            max_clients: 1024,
            keepalive: Duration::from_secs(2),
            idle_timeout: Duration::from_secs(10),
            resume_grace: Duration::from_secs(20),
            soft_queue_limit: 2048,
            hard_queue_limit: 50_000,
            server_name: concat!("s3-relay ", env!("CARGO_PKG_VERSION")).into(),
        }
    }
}

/// Counters for tests and the log.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    pub clients: usize,
    pub sessions: usize,
    pub delivered: u64,
    pub dropped_unguaranteed: u64,
    pub desync_reports: u64,
    pub resumes: u64,
    pub host_migrations: u64,
}

pub struct RelayHandle {
    pub local_addr: SocketAddr,
    hub: mpsc::UnboundedSender<HubEvent>,
    task: tokio::task::JoinHandle<()>,
}

impl RelayHandle {
    pub fn url(&self) -> String {
        format!("ws://{}/", self.local_addr)
    }

    pub async fn stats(&self) -> Stats {
        let (tx, rx) = oneshot::channel();
        if self.hub.send(HubEvent::Stats(tx)).is_err() {
            return Stats::default();
        }
        rx.await.unwrap_or_default()
    }

    /// Test helper: cut a client's TCP connection without a Bye (simulates a network drop).
    pub fn kick_connection(&self, client_id: u32) {
        let _ = self.hub.send(HubEvent::Kick(client_id));
    }

    pub fn shutdown(self) {
        self.task.abort();
    }
}

/// Bind and start the relay on the current tokio runtime.
pub async fn start(cfg: RelayConfig) -> std::io::Result<RelayHandle> {
    let listener = TcpListener::bind(cfg.listen).await?;
    let local_addr = listener.local_addr()?;
    let (hub_tx, hub_rx) = mpsc::unbounded_channel();
    let hub_tx2 = hub_tx.clone();
    let idle = cfg.idle_timeout;
    tokio::spawn(Hub::new(cfg).run(hub_rx));
    let task = tokio::spawn(async move {
        let mut next_conn: u64 = 1;
        loop {
            let (stream, peer) = match listener.accept().await {
                Ok(x) => x,
                Err(e) => {
                    log::warn!("accept: {e}");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
            };
            let _ = stream.set_nodelay(true);
            let conn = next_conn;
            next_conn += 1;
            tokio::spawn(connection(stream, peer, conn, hub_tx2.clone(), idle));
        }
    });
    log::info!("s3-relay listening on {local_addr}");
    Ok(RelayHandle { local_addr, hub: hub_tx, task })
}

enum Out {
    Frame(Vec<u8>),
    Close,
}

enum HubEvent {
    Open { conn: u64, peer: SocketAddr, out: mpsc::UnboundedSender<Out> },
    Frame { conn: u64, data: Vec<u8> },
    Closed { conn: u64 },
    Stats(oneshot::Sender<Stats>),
    Kick(u32),
    Tick,
}

async fn connection(stream: TcpStream, peer: SocketAddr, conn: u64, hub: mpsc::UnboundedSender<HubEvent>, idle: Duration) {
    let ws = match tokio::time::timeout(Duration::from_secs(10), tokio_tungstenite::accept_async(stream)).await {
        Ok(Ok(ws)) => ws,
        _ => return,
    };
    let (mut sink, mut stream) = ws.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel();
    if hub.send(HubEvent::Open { conn, peer, out: out_tx }).is_err() {
        return;
    }
    let writer = tokio::spawn(async move {
        while let Some(o) = out_rx.recv().await {
            match o {
                Out::Frame(b) => {
                    if sink.send(WsMessage::binary(b)).await.is_err() {
                        break;
                    }
                }
                Out::Close => break,
            }
        }
        let _ = sink.close().await;
    });
    loop {
        match tokio::time::timeout(idle, stream.next()).await {
            Ok(Some(Ok(WsMessage::Binary(b)))) => {
                if hub.send(HubEvent::Frame { conn, data: b.to_vec() }).is_err() {
                    break;
                }
            }
            Ok(Some(Ok(WsMessage::Close(_)))) | Ok(None) | Ok(Some(Err(_))) => break,
            Ok(Some(Ok(_))) => {}
            Err(_) => {
                log::info!("conn {conn} ({peer}): idle timeout");
                break;
            }
        }
        if writer.is_finished() {
            break;
        }
    }
    let _ = hub.send(HubEvent::Closed { conn });
    writer.abort();
}

struct Client {
    id: u32,
    token: [u8; 16],
    name: String,
    exe: Sha256,
    link: Link<ServerMsg>,
    conn: Option<(u64, mpsc::UnboundedSender<Out>)>,
    disconnected_at: Option<Instant>,
    session: Option<Guid>,
}

struct Session {
    desc: SessionDesc,
    exe: Sha256,
    host: u32,
    /// Client ids in join order (host migration picks the oldest member).
    members: Vec<u32>,
    players: BTreeMap<u32, PlayerInfo>,
}

impl Session {
    fn public_desc(&self) -> SessionDesc {
        let mut d = self.desc.clone();
        if !d.password.is_empty() {
            d.flags |= DPSESSION_PASSWORDREQUIRED;
        }
        d.password.clear();
        d.current_players = self.players.len() as u32;
        d
    }
}

struct Hub {
    cfg: RelayConfig,
    clients: HashMap<u32, Client>,
    by_token: HashMap<[u8; 16], u32>,
    /// conn -> client id (None until Hello)
    conns: HashMap<u64, (Option<u32>, mpsc::UnboundedSender<Out>, SocketAddr)>,
    sessions: HashMap<Guid, Session>,
    next_client: u32,
    dpid_counter: u32,
    dpid_salt: u32,
    stats: Stats,
}

impl Hub {
    fn new(cfg: RelayConfig) -> Self {
        let salt: [u8; 4] = s3_net::random_bytes();
        Self {
            cfg,
            clients: HashMap::new(),
            by_token: HashMap::new(),
            conns: HashMap::new(),
            sessions: HashMap::new(),
            next_client: 1,
            dpid_counter: 0,
            dpid_salt: u32::from_le_bytes(salt),
            stats: Stats::default(),
        }
    }

    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<HubEvent>) {
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        loop {
            let ev = tokio::select! {
                ev = rx.recv() => match ev { Some(ev) => ev, None => break },
                _ = tick.tick() => HubEvent::Tick,
            };
            match ev {
                HubEvent::Open { conn, peer, out } => {
                    self.conns.insert(conn, (None, out, peer));
                }
                HubEvent::Frame { conn, data } => self.on_frame(conn, &data),
                HubEvent::Closed { conn } => self.on_closed(conn),
                HubEvent::Stats(tx) => {
                    let mut s = self.stats.clone();
                    s.clients = self.clients.len();
                    s.sessions = self.sessions.len();
                    let _ = tx.send(s);
                }
                HubEvent::Kick(cid) => {
                    if let Some(conn) = self.clients.get(&cid).and_then(|c| c.conn.as_ref().map(|x| x.0)) {
                        if let Some((_, out, _)) = self.conns.get(&conn) {
                            let _ = out.send(Out::Close);
                        }
                        self.on_closed(conn);
                    }
                }
                HubEvent::Tick => self.sweep(),
            }
        }
    }

    fn raw_send(out: &mpsc::UnboundedSender<Out>, f: Frame<ServerMsg>) {
        let _ = out.send(Out::Frame(f.encode()));
    }

    /// Sequence and send (or queue while disconnected) a message to a client.
    fn send(&mut self, cid: u32, msg: ServerMsg) {
        let Some(c) = self.clients.get_mut(&cid) else { return };
        let f = c.link.stamp(msg);
        if let Some((_, out)) = &c.conn {
            Self::raw_send(out, f);
        }
        if c.link.unacked_len() > self.cfg.hard_queue_limit {
            log::warn!("client {cid}: {} unacked frames, dropping", c.link.unacked_len());
            self.remove_client(cid);
        }
    }

    fn reply_err(&mut self, cid: u32, req: u32, code: ErrorCode, message: &str) {
        self.send(cid, ServerMsg::Error { req, code, message: message.into() });
    }

    fn on_frame(&mut self, conn: u64, data: &[u8]) {
        let Some((cid, out, peer)) = self.conns.get(&conn).map(|(c, o, p)| (*c, o.clone(), *p)) else { return };
        let frame = match Frame::<ClientMsg>::decode(data) {
            Ok(f) => f,
            Err(e) => {
                log::warn!("conn {conn} ({peer}): bad frame: {e}");
                let _ = out.send(Out::Close);
                return;
            }
        };
        let Some(cid) = cid else {
            match frame.msg {
                ClientMsg::Hello { proto, exe_sha256, name, build, resume } => {
                    self.on_hello(conn, out, peer, proto, exe_sha256, name, build, resume)
                }
                _ => {
                    let _ = out.send(Out::Close);
                }
            }
            return;
        };
        let Some(c) = self.clients.get_mut(&cid) else { return };
        c.link.on_ack(frame.ack);
        match c.link.accept(frame.seq) {
            Accept::New => {}
            Accept::Duplicate => return,
            Accept::Gap { expected, got } => {
                log::warn!("client {cid}: gap expected {expected} got {got}");
                let _ = out.send(Out::Close);
                return;
            }
        }
        self.on_msg(cid, frame.msg);
    }

    #[allow(clippy::too_many_arguments)]
    fn on_hello(
        &mut self,
        conn: u64,
        out: mpsc::UnboundedSender<Out>,
        peer: SocketAddr,
        proto: u16,
        exe: Sha256,
        name: String,
        build: String,
        resume: Option<Resume>,
    ) {
        let reject = |code: ErrorCode, message: String| {
            Self::raw_send(&out, Frame { seq: 0, ack: 0, msg: ServerMsg::Reject { code, message } });
            let _ = out.send(Out::Close);
        };
        if proto != PROTOCOL_VERSION {
            return reject(ErrorCode::VersionMismatch, format!("relay speaks protocol v{PROTOCOL_VERSION}, client v{proto}"));
        }
        if !self.cfg.allowed_exes.is_empty() && !self.cfg.allowed_exes.contains(&exe) {
            return reject(ErrorCode::ExeMismatch, format!("S3.EXE {} is not allowed on this relay", s3_net::hex(&exe[..8])));
        }
        // Resume an existing identity?
        if let Some(r) = resume {
            if let Some(&cid) = self.by_token.get(&r.token) {
                let c = self.clients.get_mut(&cid).unwrap();
                if c.exe == exe {
                    if let Some((old, old_out)) = c.conn.take() {
                        let _ = old_out.send(Out::Close);
                        self.conns.remove(&old);
                    }
                    c.conn = Some((conn, out.clone()));
                    c.disconnected_at = None;
                    let welcome = ServerMsg::Welcome {
                        proto: PROTOCOL_VERSION,
                        client_id: cid,
                        token: c.token,
                        resumed: true,
                        last_seq: c.link.last_recv,
                        keepalive_ms: self.cfg.keepalive.as_millis() as u32,
                        server: self.cfg.server_name.clone(),
                    };
                    Self::raw_send(&out, Frame { seq: 0, ack: c.link.last_recv, msg: welcome });
                    for f in c.link.retransmit_after(r.last_seq) {
                        Self::raw_send(&out, f);
                    }
                    if let Some(e) = self.conns.get_mut(&conn) {
                        e.0 = Some(cid);
                    }
                    self.stats.resumes += 1;
                    log::info!("client {cid} resumed from {peer}");
                    return;
                }
            }
            // Unknown/expired token: continue as a fresh client; `resumed: false` tells it the old state is gone.
        }
        if self.clients.len() >= self.cfg.max_clients {
            return reject(ErrorCode::ServerFull, "relay full".into());
        }
        let cid = self.next_client;
        self.next_client = self.next_client.wrapping_add(1).max(1);
        let token: [u8; 16] = s3_net::random_bytes();
        let link = Link::new();
        let welcome = ServerMsg::Welcome {
            proto: PROTOCOL_VERSION,
            client_id: cid,
            token,
            resumed: false,
            last_seq: 0,
            keepalive_ms: self.cfg.keepalive.as_millis() as u32,
            server: self.cfg.server_name.clone(),
        };
        Self::raw_send(&out, Frame { seq: 0, ack: 0, msg: welcome });
        log::info!("client {cid} '{name}' ({build}) from {peer}, exe {}", s3_net::hex(&exe[..8]));
        self.clients.insert(cid, Client { id: cid, token, name, exe, link, conn: Some((conn, out)), disconnected_at: None, session: None });
        self.by_token.insert(token, cid);
        if let Some(e) = self.conns.get_mut(&conn) {
            e.0 = Some(cid);
        }
    }

    fn on_closed(&mut self, conn: u64) {
        let Some((cid, _, _)) = self.conns.remove(&conn) else { return };
        let Some(cid) = cid else { return };
        if let Some(c) = self.clients.get_mut(&cid) {
            if c.conn.as_ref().map(|x| x.0) == Some(conn) {
                c.conn = None;
                c.disconnected_at = Some(Instant::now());
                log::info!("client {cid}: connection lost, keeping slot for {:?}", self.cfg.resume_grace);
            }
        }
    }

    fn sweep(&mut self) {
        let expired: Vec<u32> = self
            .clients
            .values()
            .filter(|c| c.disconnected_at.is_some_and(|t| t.elapsed() > self.cfg.resume_grace))
            .map(|c| c.id)
            .collect();
        for cid in expired {
            log::info!("client {cid}: resume grace expired");
            self.remove_client(cid);
        }
    }

    fn remove_client(&mut self, cid: u32) {
        self.leave(cid);
        if let Some(c) = self.clients.remove(&cid) {
            self.by_token.remove(&c.token);
            if let Some((conn, out)) = c.conn {
                let _ = out.send(Out::Close);
                self.conns.remove(&conn);
            }
        }
    }

    fn alloc_dpid(&mut self, g: &Guid) -> u32 {
        loop {
            self.dpid_counter = self.dpid_counter.wrapping_add(1);
            let id = self.dpid_counter.wrapping_mul(0x9e37_79b1) ^ self.dpid_salt;
            if id > 1 && id != u32::MAX && !self.sessions[g].players.contains_key(&id) {
                return id;
            }
        }
    }

    fn members_except(&self, g: &Guid, cid: u32) -> Vec<u32> {
        self.sessions.get(g).map(|s| s.members.iter().copied().filter(|&m| m != cid).collect()).unwrap_or_default()
    }

    /// Remove a client from its session: destroy its players, migrate host, delete empty sessions.
    fn leave(&mut self, cid: u32) {
        let Some(g) = self.clients.get_mut(&cid).and_then(|c| c.session.take()) else { return };
        let Some(s) = self.sessions.get_mut(&g) else { return };
        s.members.retain(|&m| m != cid);
        let gone: Vec<u32> = s.players.values().filter(|p| p.owner == cid).map(|p| p.dpid).collect();
        for d in &gone {
            s.players.remove(d);
        }
        let was_host = s.host == cid;
        let members = s.members.clone();
        let migrate = s.desc.flags & DPSESSION_MIGRATEHOST != 0;
        if members.is_empty() {
            self.sessions.remove(&g);
            log::info!("session {} closed", s3_net::hex(&g[..4]));
            return;
        }
        for &m in &members {
            for &d in &gone {
                self.send(m, ServerMsg::PlayerRemoved { dpid: d });
            }
        }
        if was_host {
            if migrate {
                let new_host = members[0];
                self.sessions.get_mut(&g).unwrap().host = new_host;
                self.stats.host_migrations += 1;
                log::info!("session {}: host {cid} left, new host {new_host}", s3_net::hex(&g[..4]));
                self.send(new_host, ServerMsg::BecameHost);
            } else {
                for &m in &members {
                    if let Some(c) = self.clients.get_mut(&m) {
                        c.session = None;
                    }
                    self.send(m, ServerMsg::SessionLost { code: ErrorCode::Disconnected });
                }
                self.sessions.remove(&g);
            }
        }
    }

    fn on_msg(&mut self, cid: u32, msg: ClientMsg) {
        match msg {
            ClientMsg::Hello { .. } => {}
            ClientMsg::Ping { nonce } => {
                if let Some(c) = self.clients.get_mut(&cid) {
                    let f = c.link.stamp(ServerMsg::Pong { nonce });
                    if let Some((_, out)) = &c.conn {
                        Self::raw_send(out, f);
                    }
                }
            }
            ClientMsg::Ack => {}
            ClientMsg::Bye => {
                log::info!("client {cid}: bye");
                self.remove_client(cid);
            }
            ClientMsg::ListSessions { req, application } => {
                let exe = self.clients[&cid].exe;
                let mut sessions: Vec<SessionDesc> = self
                    .sessions
                    .values()
                    .filter(|s| s.exe == exe && s.desc.application == application)
                    .map(|s| s.public_desc())
                    .collect();
                sessions.sort_by(|a, b| a.name.cmp(&b.name));
                self.send(cid, ServerMsg::SessionList { req, sessions });
            }
            ClientMsg::CreateSession { req, mut desc } => {
                if self.clients[&cid].session.is_some() {
                    return self.reply_err(cid, req, ErrorCode::AlreadyInSession, "already in a session");
                }
                desc.instance = s3_net::random_bytes();
                desc.current_players = 0;
                let g = desc.instance;
                let s = Session { desc, exe: self.clients[&cid].exe, host: cid, members: vec![cid], players: BTreeMap::new() };
                let public = s.public_desc();
                log::info!("client {cid} created session {} '{}'", s3_net::hex(&g[..4]), public.name);
                self.sessions.insert(g, s);
                self.clients.get_mut(&cid).unwrap().session = Some(g);
                self.send(cid, ServerMsg::Joined { req, desc: public, host: cid, players: Vec::new() });
            }
            ClientMsg::JoinSession { req, instance, password } => {
                if self.clients[&cid].session.is_some() {
                    return self.reply_err(cid, req, ErrorCode::AlreadyInSession, "already in a session");
                }
                let exe = self.clients[&cid].exe;
                let Some(s) = self.sessions.get_mut(&instance) else {
                    return self.reply_err(cid, req, ErrorCode::NoSuchSession, "no such session");
                };
                let err = if s.exe != exe {
                    Some((ErrorCode::ExeMismatch, "different S3.EXE than the host"))
                } else if s.desc.flags & (DPSESSION_JOINDISABLED | DPSESSION_NEWPLAYERSDISABLED) != 0 {
                    Some((ErrorCode::JoinDisabled, "session closed for joining"))
                } else if !s.desc.password.is_empty() && s.desc.password != password {
                    Some((ErrorCode::WrongPassword, "wrong password"))
                } else if s.desc.max_players != 0 && s.members.len() as u32 >= s.desc.max_players {
                    Some((ErrorCode::SessionFull, "session full"))
                } else {
                    None
                };
                if let Some((code, m)) = err {
                    return self.reply_err(cid, req, code, m);
                }
                s.members.push(cid);
                let (desc, host, players) = (s.public_desc(), s.host, s.players.values().cloned().collect());
                self.clients.get_mut(&cid).unwrap().session = Some(instance);
                self.send(cid, ServerMsg::Joined { req, desc, host, players });
            }
            ClientMsg::LeaveSession { req } => {
                if self.clients[&cid].session.is_none() {
                    return self.reply_err(cid, req, ErrorCode::NotInSession, "not in a session");
                }
                self.leave(cid);
                self.send(cid, ServerMsg::Ok { req });
            }
            ClientMsg::SetSessionDesc { req, desc } => {
                let Some(g) = self.clients[&cid].session else {
                    return self.reply_err(cid, req, ErrorCode::NotInSession, "not in a session");
                };
                let s = self.sessions.get_mut(&g).unwrap();
                if s.host != cid {
                    return self.reply_err(cid, req, ErrorCode::NotOwner, "only the host may change the session");
                }
                s.desc.flags = desc.flags;
                s.desc.max_players = desc.max_players;
                s.desc.name = desc.name;
                s.desc.password = desc.password;
                s.desc.user = desc.user;
                let public = s.public_desc();
                for m in self.members_except(&g, cid) {
                    self.send(m, ServerMsg::SessionDescChanged { desc: public.clone() });
                }
                self.send(cid, ServerMsg::Ok { req });
            }
            ClientMsg::CreatePlayer { req, flags, short_name, long_name, data } => {
                let Some(g) = self.clients[&cid].session else {
                    return self.reply_err(cid, req, ErrorCode::NotInSession, "not in a session");
                };
                let s = &self.sessions[&g];
                if s.desc.max_players != 0 && s.players.len() as u32 >= s.desc.max_players {
                    return self.reply_err(cid, req, ErrorCode::SessionFull, "session full");
                }
                let dpid = self.alloc_dpid(&g);
                let p = PlayerInfo { dpid, owner: cid, flags, short_name, long_name, data };
                self.sessions.get_mut(&g).unwrap().players.insert(dpid, p.clone());
                // Reply first so the creator knows its DPID before any message from others can mention it.
                self.send(cid, ServerMsg::PlayerCreated { req, dpid });
                for m in self.members_except(&g, cid) {
                    self.send(m, ServerMsg::PlayerAdded { player: p.clone() });
                }
            }
            ClientMsg::DestroyPlayer { req, dpid } => match self.owned_player(cid, dpid) {
                Err(code) => self.reply_err(cid, req, code, "cannot destroy player"),
                Ok(g) => {
                    self.sessions.get_mut(&g).unwrap().players.remove(&dpid);
                    for m in self.members_except(&g, cid) {
                        self.send(m, ServerMsg::PlayerRemoved { dpid });
                    }
                    self.send(cid, ServerMsg::Ok { req });
                }
            },
            ClientMsg::SetPlayerData { req, dpid, data } => match self.owned_player(cid, dpid) {
                Err(code) => self.reply_err(cid, req, code, "cannot set player data"),
                Ok(g) => {
                    self.sessions.get_mut(&g).unwrap().players.get_mut(&dpid).unwrap().data = data.clone();
                    for m in self.members_except(&g, cid) {
                        self.send(m, ServerMsg::PlayerData { dpid, data: data.clone() });
                    }
                    self.send(cid, ServerMsg::Ok { req });
                }
            },
            ClientMsg::SetPlayerName { req, dpid, short_name, long_name } => match self.owned_player(cid, dpid) {
                Err(code) => self.reply_err(cid, req, code, "cannot set player name"),
                Ok(g) => {
                    let p = self.sessions.get_mut(&g).unwrap().players.get_mut(&dpid).unwrap();
                    p.short_name = short_name.clone();
                    p.long_name = long_name.clone();
                    for m in self.members_except(&g, cid) {
                        self.send(m, ServerMsg::PlayerName { dpid, short_name: short_name.clone(), long_name: long_name.clone() });
                    }
                    self.send(cid, ServerMsg::Ok { req });
                }
            },
            ClientMsg::Send { from, to, flags, payload } => self.on_send(cid, from, to, flags, payload),
            ClientMsg::DesyncReport { step, checksum, detail } => {
                self.stats.desync_reports += 1;
                let name = self.clients[&cid].name.clone();
                log::warn!("DESYNC reported by client {cid} '{name}': step {step} checksum {checksum:#x}: {detail}");
                if let Some(g) = self.clients[&cid].session {
                    for m in self.members_except(&g, cid) {
                        self.send(m, ServerMsg::Desync { client_id: cid, step, checksum, detail: detail.clone() });
                    }
                }
            }
        }
    }

    fn owned_player(&self, cid: u32, dpid: u32) -> Result<Guid, ErrorCode> {
        let g = self.clients[&cid].session.ok_or(ErrorCode::NotInSession)?;
        let p = self.sessions[&g].players.get(&dpid).ok_or(ErrorCode::NoSuchPlayer)?;
        if p.owner != cid {
            return Err(ErrorCode::NotOwner);
        }
        Ok(g)
    }

    fn on_send(&mut self, cid: u32, from: u32, to: u32, flags: u32, payload: Vec<u8>) {
        let Some(g) = self.clients[&cid].session else { return };
        let s = &self.sessions[&g];
        if s.players.get(&from).map(|p| p.owner) != Some(cid) {
            log::debug!("client {cid}: Send from foreign/unknown player {from:#x} ignored");
            return;
        }
        let targets: Vec<(u32, u32)> = if to == DPID_ALL {
            s.players.values().filter(|p| p.dpid != from).map(|p| (p.dpid, p.owner)).collect()
        } else {
            match s.players.get(&to) {
                Some(p) => vec![(p.dpid, p.owner)],
                None => return,
            }
        };
        for (dpid, owner) in targets {
            if flags & DPSEND_GUARANTEED == 0
                && self.clients.get(&owner).is_some_and(|c| c.link.unacked_len() > self.cfg.soft_queue_limit)
            {
                self.stats.dropped_unguaranteed += 1;
                continue;
            }
            self.stats.delivered += 1;
            self.send(owner, ServerMsg::Deliver { from, to: dpid, flags, payload: payload.clone() });
        }
    }
}
