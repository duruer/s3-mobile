//! Synchronous relay client for the DirectPlay HLE.
//!
//! The emulator thread calls blocking methods ([`NetClient::create_session`], ...) and polls events
//! ([`NetClient::poll_event`]) from `Receive`/`GetMessageCount`. A background thread runs a single-threaded tokio
//! runtime that owns the WebSocket, answers keepalives, acks frames and transparently resumes the link after a
//! network drop (mobile networks switch Wi-Fi <-> LTE). Nothing here blocks the game except explicit requests.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::link::{Accept, Link};
use crate::protocol::*;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// `ws://host:port/` of the relay.
    pub url: String,
    pub exe_sha256: Sha256,
    /// Shown in session lists / logs (not the in-game player name).
    pub name: String,
    pub build: String,
    /// How long blocking requests wait for the relay.
    pub request_timeout: Duration,
    /// Try to resume a dropped link for this long before reporting the session as lost (zero = never resume).
    pub resume_window: Duration,
    /// Declare the link dead if nothing is received for this long (relay pings us back every keepalive).
    pub idle_timeout: Duration,
}

impl ClientConfig {
    pub fn new(url: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            exe_sha256: crate::GOG_EXE_SHA256,
            name: name.into(),
            build: concat!("s3-net ", env!("CARGO_PKG_VERSION")).into(),
            request_timeout: Duration::from_secs(10),
            resume_window: Duration::from_secs(20),
            idle_timeout: Duration::from_secs(15),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetError {
    Connect(String),
    Rejected(ErrorCode, String),
    Relay(ErrorCode, String),
    Timeout,
    Closed,
    Protocol(String),
}

impl NetError {
    pub fn code(&self) -> ErrorCode {
        match self {
            NetError::Rejected(c, _) | NetError::Relay(c, _) => *c,
            NetError::Timeout => ErrorCode::Timeout,
            _ => ErrorCode::Disconnected,
        }
    }
}

impl std::fmt::Display for NetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for NetError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    Connected,
    Reconnecting,
    /// Link resumed after a drop, nothing lost.
    Resumed,
    /// Gave up (or relay forgot us); the session is gone.
    Lost,
}

/// What the HLE turns into DirectPlay messages (data messages or `DPSYS_*`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Message { from: u32, to: u32, flags: u32, payload: Vec<u8> },
    PlayerAdded(PlayerInfo),
    PlayerRemoved(PlayerInfo),
    PlayerData { dpid: u32, data: Vec<u8> },
    PlayerName { dpid: u32, short_name: String, long_name: String },
    SessionDesc(SessionDesc),
    BecameHost,
    SessionLost(ErrorCode),
    Desync { client_id: u32, step: u32, checksum: u32, detail: String },
    Link(LinkState),
}

/// Session state mirrored from relay events (for EnumPlayers / GetPlayerData / GetSessionDesc).
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub client_id: u32,
    pub session: Option<SessionDesc>,
    pub host: u32,
    pub players: HashMap<u32, PlayerInfo>,
    pub rtt_ms: Option<u32>,
}

impl Snapshot {
    pub fn is_host(&self) -> bool {
        self.session.is_some() && self.host == self.client_id
    }
}

#[derive(Debug, Clone)]
pub struct Joined {
    pub desc: SessionDesc,
    pub host: u32,
    pub players: Vec<PlayerInfo>,
}

enum Cmd {
    Request(ClientMsg, std_mpsc::Sender<Result<ServerMsg, NetError>>),
    Fire(ClientMsg),
    Close,
}

pub struct NetClient {
    cmd: mpsc::UnboundedSender<Cmd>,
    events: Mutex<std_mpsc::Receiver<Event>>,
    snapshot: Arc<Mutex<Snapshot>>,
    next_req: AtomicU32,
    timeout: Duration,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl NetClient {
    /// Connect and handshake (blocking). Fails with [`NetError::Rejected`] on version/exe mismatch.
    pub fn connect(cfg: ClientConfig) -> Result<NetClient, NetError> {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (ev_tx, ev_rx) = std_mpsc::channel();
        let (ready_tx, ready_rx) = std_mpsc::channel();
        let snapshot = Arc::new(Mutex::new(Snapshot::default()));
        let snap2 = snapshot.clone();
        let timeout = cfg.request_timeout;
        let thread = std::thread::Builder::new()
            .name("s3-net".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = ready_tx.send(Err(NetError::Connect(e.to_string())));
                        return;
                    }
                };
                rt.block_on(async move {
                    let mut link = Link::<ClientMsg>::new();
                    match handshake(&cfg, None, &mut link).await {
                        Err(e) => {
                            let _ = ready_tx.send(Err(e));
                        }
                        Ok((ws, w)) => {
                            {
                                let mut s = snap2.lock().unwrap();
                                s.client_id = w.client_id;
                            }
                            let _ = ready_tx.send(Ok(()));
                            Task { cfg, link, token: w.token, keepalive: w.keepalive, cmd_rx, ev_tx, snapshot: snap2, pending: HashMap::new() }
                                .run(ws)
                                .await;
                        }
                    }
                });
            })
            .map_err(|e| NetError::Connect(e.to_string()))?;
        match ready_rx.recv_timeout(timeout + Duration::from_secs(1)) {
            Ok(Ok(())) => Ok(NetClient {
                cmd: cmd_tx,
                events: Mutex::new(ev_rx),
                snapshot,
                next_req: AtomicU32::new(1),
                timeout,
                thread: Some(thread),
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(NetError::Timeout),
        }
    }

    fn request(&self, make: impl FnOnce(u32) -> ClientMsg) -> Result<ServerMsg, NetError> {
        let req = self.next_req.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = std_mpsc::channel();
        self.cmd.send(Cmd::Request(make(req), tx)).map_err(|_| NetError::Closed)?;
        match rx.recv_timeout(self.timeout) {
            Ok(Ok(ServerMsg::Error { code, message, .. })) => Err(NetError::Relay(code, message)),
            Ok(r) => r,
            Err(std_mpsc::RecvTimeoutError::Timeout) => Err(NetError::Timeout),
            Err(_) => Err(NetError::Closed),
        }
    }

    fn expect_ok(r: Result<ServerMsg, NetError>) -> Result<(), NetError> {
        match r? {
            ServerMsg::Ok { .. } => Ok(()),
            other => Err(NetError::Protocol(format!("unexpected reply {other:?}"))),
        }
    }

    fn expect_joined(r: Result<ServerMsg, NetError>) -> Result<Joined, NetError> {
        match r? {
            ServerMsg::Joined { desc, host, players, .. } => Ok(Joined { desc, host, players }),
            other => Err(NetError::Protocol(format!("unexpected reply {other:?}"))),
        }
    }

    pub fn list_sessions(&self, application: Guid) -> Result<Vec<SessionDesc>, NetError> {
        match self.request(|req| ClientMsg::ListSessions { req, application })? {
            ServerMsg::SessionList { sessions, .. } => Ok(sessions),
            other => Err(NetError::Protocol(format!("unexpected reply {other:?}"))),
        }
    }

    /// DPOPEN_CREATE. The relay assigns the instance GUID (returned in `Joined::desc`).
    pub fn create_session(&self, desc: SessionDesc) -> Result<Joined, NetError> {
        Self::expect_joined(self.request(|req| ClientMsg::CreateSession { req, desc }))
    }

    /// DPOPEN_JOIN.
    pub fn join_session(&self, instance: Guid, password: &str) -> Result<Joined, NetError> {
        Self::expect_joined(self.request(|req| ClientMsg::JoinSession { req, instance, password: password.into() }))
    }

    pub fn leave_session(&self) -> Result<(), NetError> {
        Self::expect_ok(self.request(|req| ClientMsg::LeaveSession { req }))
    }

    pub fn set_session_desc(&self, desc: SessionDesc) -> Result<(), NetError> {
        Self::expect_ok(self.request(|req| ClientMsg::SetSessionDesc { req, desc }))
    }

    pub fn create_player(&self, short_name: &str, long_name: &str, data: &[u8], flags: u32) -> Result<u32, NetError> {
        let r = self.request(|req| ClientMsg::CreatePlayer {
            req,
            flags,
            short_name: short_name.into(),
            long_name: long_name.into(),
            data: data.to_vec(),
        })?;
        match r {
            ServerMsg::PlayerCreated { dpid, .. } => Ok(dpid),
            other => Err(NetError::Protocol(format!("unexpected reply {other:?}"))),
        }
    }

    pub fn destroy_player(&self, dpid: u32) -> Result<(), NetError> {
        Self::expect_ok(self.request(|req| ClientMsg::DestroyPlayer { req, dpid }))
    }

    pub fn set_player_data(&self, dpid: u32, data: &[u8]) -> Result<(), NetError> {
        Self::expect_ok(self.request(|req| ClientMsg::SetPlayerData { req, dpid, data: data.to_vec() }))
    }

    pub fn set_player_name(&self, dpid: u32, short_name: &str, long_name: &str) -> Result<(), NetError> {
        Self::expect_ok(self.request(|req| ClientMsg::SetPlayerName {
            req,
            dpid,
            short_name: short_name.into(),
            long_name: long_name.into(),
        }))
    }

    /// IDirectPlay4::Send. Non-blocking; ordering per sender is preserved. `to = 0` = all other players.
    pub fn send(&self, from: u32, to: u32, flags: u32, payload: &[u8]) -> Result<(), NetError> {
        if payload.len() > crate::wire::MAX_BYTES {
            return Err(NetError::Protocol("payload too big".into()));
        }
        self.cmd.send(Cmd::Fire(ClientMsg::Send { from, to, flags, payload: payload.to_vec() })).map_err(|_| NetError::Closed)
    }

    pub fn report_desync(&self, step: u32, checksum: u32, detail: &str) -> Result<(), NetError> {
        self.cmd
            .send(Cmd::Fire(ClientMsg::DesyncReport { step, checksum, detail: detail.into() }))
            .map_err(|_| NetError::Closed)
    }

    pub fn poll_event(&self) -> Option<Event> {
        self.events.lock().unwrap().try_recv().ok()
    }

    pub fn wait_event(&self, timeout: Duration) -> Option<Event> {
        self.events.lock().unwrap().recv_timeout(timeout).ok()
    }

    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.lock().unwrap().clone()
    }

    pub fn client_id(&self) -> u32 {
        self.snapshot.lock().unwrap().client_id
    }

    /// Say goodbye and stop the background thread.
    pub fn close(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        let _ = self.cmd.send(Cmd::Close);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for NetClient {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct WelcomeInfo {
    client_id: u32,
    token: [u8; 16],
    keepalive: Duration,
    resumed: bool,
    last_seq: u64,
}

async fn send_frame(ws: &mut Ws, f: Frame<ClientMsg>) -> Result<(), NetError> {
    ws.send(WsMessage::binary(f.encode())).await.map_err(|e| NetError::Connect(e.to_string()))
}

async fn handshake(cfg: &ClientConfig, token: Option<[u8; 16]>, link: &mut Link<ClientMsg>) -> Result<(Ws, WelcomeInfo), NetError> {
    let connect = tokio_tungstenite::connect_async(cfg.url.as_str());
    let (mut ws, _) = tokio::time::timeout(cfg.request_timeout, connect)
        .await
        .map_err(|_| NetError::Timeout)?
        .map_err(|e| NetError::Connect(e.to_string()))?;
    let hello = ClientMsg::Hello {
        proto: PROTOCOL_VERSION,
        exe_sha256: cfg.exe_sha256,
        name: cfg.name.clone(),
        build: cfg.build.clone(),
        resume: token.map(|token| Resume { token, last_seq: link.last_recv }),
    };
    let f = link.stamp(hello);
    send_frame(&mut ws, f).await?;
    let deadline = Instant::now() + cfg.request_timeout;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let msg = tokio::time::timeout(left, ws.next()).await.map_err(|_| NetError::Timeout)?;
        let data = match msg {
            Some(Ok(WsMessage::Binary(b))) => b,
            Some(Ok(WsMessage::Close(_))) | None => return Err(NetError::Closed),
            Some(Ok(_)) => continue,
            Some(Err(e)) => return Err(NetError::Connect(e.to_string())),
        };
        let frame = Frame::<ServerMsg>::decode(&data).map_err(|e| NetError::Protocol(e.to_string()))?;
        return match frame.msg {
            ServerMsg::Welcome { proto, client_id, token, resumed, last_seq, keepalive_ms, .. } => {
                if proto != PROTOCOL_VERSION {
                    return Err(NetError::Rejected(ErrorCode::VersionMismatch, format!("relay speaks v{proto}")));
                }
                Ok((
                    ws,
                    WelcomeInfo {
                        client_id,
                        token,
                        keepalive: Duration::from_millis(keepalive_ms.clamp(200, 60_000) as u64),
                        resumed,
                        last_seq,
                    },
                ))
            }
            ServerMsg::Reject { code, message } => Err(NetError::Rejected(code, message)),
            other => Err(NetError::Protocol(format!("expected Welcome, got {other:?}"))),
        };
    }
}

struct Task {
    cfg: ClientConfig,
    link: Link<ClientMsg>,
    token: [u8; 16],
    keepalive: Duration,
    cmd_rx: mpsc::UnboundedReceiver<Cmd>,
    ev_tx: std_mpsc::Sender<Event>,
    snapshot: Arc<Mutex<Snapshot>>,
    pending: HashMap<u32, (Pending, std_mpsc::Sender<Result<ServerMsg, NetError>>)>,
}

/// What to update in the [`Snapshot`] when a request succeeds.
#[derive(Clone, Copy)]
enum Pending {
    Other,
    Leave,
    Destroy(u32),
}

enum Exit {
    Closed,
    Dropped,
}

impl Task {
    async fn run(mut self, mut ws: Ws) {
        loop {
            match self.connected(&mut ws).await {
                Exit::Closed => {
                    let _ = ws.close(None).await;
                    break;
                }
                Exit::Dropped => {
                    let _ = self.ev_tx.send(Event::Link(LinkState::Reconnecting));
                    match self.reconnect().await {
                        Some(new_ws) => {
                            ws = new_ws;
                            let _ = self.ev_tx.send(Event::Link(LinkState::Resumed));
                        }
                        None => {
                            self.lost();
                            break;
                        }
                    }
                }
            }
        }
        for (_, (_, tx)) in self.pending.drain() {
            let _ = tx.send(Err(NetError::Closed));
        }
    }

    fn lost(&mut self) {
        let in_session = self.snapshot.lock().unwrap().session.take().is_some();
        if in_session {
            let _ = self.ev_tx.send(Event::SessionLost(ErrorCode::Disconnected));
        }
        let _ = self.ev_tx.send(Event::Link(LinkState::Lost));
    }

    async fn reconnect(&mut self) -> Option<Ws> {
        if self.cfg.resume_window.is_zero() {
            return None;
        }
        let give_up = Instant::now() + self.cfg.resume_window;
        let mut backoff = Duration::from_millis(100);
        while Instant::now() < give_up {
            match handshake(&self.cfg, Some(self.token), &mut self.link).await {
                Ok((mut ws, w)) => {
                    if !w.resumed {
                        log::warn!("s3-net: relay did not resume our session");
                        let _ = ws.close(None).await;
                        return None;
                    }
                    for f in self.link.retransmit_after(w.last_seq) {
                        if send_frame(&mut ws, f).await.is_err() {
                            break;
                        }
                    }
                    return Some(ws);
                }
                Err(NetError::Rejected(code, msg)) => {
                    log::warn!("s3-net: resume rejected {code:?}: {msg}");
                    return None;
                }
                Err(e) => {
                    log::info!("s3-net: reconnect failed: {e}");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(2));
                }
            }
        }
        None
    }

    async fn connected(&mut self, ws: &mut Ws) -> Exit {
        let mut ping = tokio::time::interval(self.keepalive);
        let mut ack_tick = tokio::time::interval(Duration::from_millis(100));
        let mut last_rx = Instant::now();
        let mut nonce: u64 = 0;
        let mut ping_sent: HashMap<u64, Instant> = HashMap::new();
        loop {
            tokio::select! {
                cmd = self.cmd_rx.recv() => {
                    let msg = match cmd {
                        Some(Cmd::Request(msg, tx)) => {
                            if let Some(req) = request_id(&msg) {
                                let kind = match &msg {
                                    ClientMsg::LeaveSession { .. } => Pending::Leave,
                                    ClientMsg::DestroyPlayer { dpid, .. } => Pending::Destroy(*dpid),
                                    _ => Pending::Other,
                                };
                                self.pending.insert(req, (kind, tx));
                            }
                            msg
                        }
                        Some(Cmd::Fire(msg)) => msg,
                        Some(Cmd::Close) | None => {
                            let f = self.link.stamp(ClientMsg::Bye);
                            let _ = send_frame(ws, f).await;
                            return Exit::Closed;
                        }
                    };
                    let f = self.link.stamp(msg);
                    if send_frame(ws, f).await.is_err() {
                        return Exit::Dropped;
                    }
                }
                msg = ws.next() => {
                    let data = match msg {
                        Some(Ok(WsMessage::Binary(b))) => b,
                        Some(Ok(WsMessage::Close(_))) | None | Some(Err(_)) => return Exit::Dropped,
                        Some(Ok(_)) => continue,
                    };
                    last_rx = Instant::now();
                    let frame = match Frame::<ServerMsg>::decode(&data) {
                        Ok(f) => f,
                        Err(e) => {
                            log::warn!("s3-net: bad frame from relay: {e}");
                            return Exit::Dropped;
                        }
                    };
                    self.link.on_ack(frame.ack);
                    match self.link.accept(frame.seq) {
                        Accept::New => {}
                        Accept::Duplicate => continue,
                        Accept::Gap { expected, got } => {
                            log::warn!("s3-net: frame gap (expected {expected}, got {got})");
                            return Exit::Dropped;
                        }
                    }
                    if let ServerMsg::Pong { nonce } = frame.msg {
                        if let Some(t) = ping_sent.remove(&nonce) {
                            self.snapshot.lock().unwrap().rtt_ms = Some(t.elapsed().as_millis() as u32);
                        }
                        continue;
                    }
                    self.dispatch(frame.msg);
                }
                _ = ack_tick.tick() => {
                    if self.link.ack_due() {
                        let f = self.link.stamp(ClientMsg::Ack);
                        if send_frame(ws, f).await.is_err() {
                            return Exit::Dropped;
                        }
                    }
                }
                _ = ping.tick() => {
                    if last_rx.elapsed() > self.cfg.idle_timeout {
                        log::info!("s3-net: relay silent for {:?}, reconnecting", last_rx.elapsed());
                        return Exit::Dropped;
                    }
                    nonce += 1;
                    ping_sent.retain(|_, t| t.elapsed() < Duration::from_secs(60));
                    ping_sent.insert(nonce, Instant::now());
                    let f = self.link.stamp(ClientMsg::Ping { nonce });
                    if send_frame(ws, f).await.is_err() {
                        return Exit::Dropped;
                    }
                }
            }
        }
    }

    fn reply(&mut self, req: u32, msg: ServerMsg) {
        if let Some((kind, tx)) = self.pending.remove(&req) {
            if let ServerMsg::Ok { .. } = msg {
                let mut s = self.snapshot.lock().unwrap();
                match kind {
                    Pending::Leave => {
                        s.session = None;
                        s.players.clear();
                    }
                    Pending::Destroy(dpid) => {
                        s.players.remove(&dpid);
                    }
                    Pending::Other => {}
                }
            }
            let _ = tx.send(Ok(msg));
        }
    }

    fn dispatch(&mut self, msg: ServerMsg) {
        let ev = |s: &Self, e: Event| {
            let _ = s.ev_tx.send(e);
        };
        match msg {
            ServerMsg::Welcome { .. } | ServerMsg::Reject { .. } | ServerMsg::Pong { .. } | ServerMsg::Ack => {}
            ServerMsg::Error { req, .. } | ServerMsg::Ok { req } | ServerMsg::SessionList { req, .. } | ServerMsg::PlayerCreated { req, .. } => {
                if let ServerMsg::PlayerCreated { dpid, .. } = &msg {
                    let mut s = self.snapshot.lock().unwrap();
                    let owner = s.client_id;
                    s.players.entry(*dpid).or_insert(PlayerInfo { dpid: *dpid, owner, ..Default::default() });
                }
                self.reply(req, msg);
            }
            ServerMsg::Joined { req, ref desc, host, ref players } => {
                {
                    let mut s = self.snapshot.lock().unwrap();
                    s.session = Some(desc.clone());
                    s.host = host;
                    s.players = players.iter().map(|p| (p.dpid, p.clone())).collect();
                }
                self.reply(req, msg);
            }
            ServerMsg::PlayerAdded { player } => {
                self.snapshot.lock().unwrap().players.insert(player.dpid, player.clone());
                ev(self, Event::PlayerAdded(player));
            }
            ServerMsg::PlayerRemoved { dpid } => {
                let p = self.snapshot.lock().unwrap().players.remove(&dpid).unwrap_or(PlayerInfo { dpid, ..Default::default() });
                ev(self, Event::PlayerRemoved(p));
            }
            ServerMsg::PlayerData { dpid, data } => {
                if let Some(p) = self.snapshot.lock().unwrap().players.get_mut(&dpid) {
                    p.data = data.clone();
                }
                ev(self, Event::PlayerData { dpid, data });
            }
            ServerMsg::PlayerName { dpid, short_name, long_name } => {
                if let Some(p) = self.snapshot.lock().unwrap().players.get_mut(&dpid) {
                    p.short_name = short_name.clone();
                    p.long_name = long_name.clone();
                }
                ev(self, Event::PlayerName { dpid, short_name, long_name });
            }
            ServerMsg::SessionDescChanged { desc } => {
                self.snapshot.lock().unwrap().session = Some(desc.clone());
                ev(self, Event::SessionDesc(desc));
            }
            ServerMsg::BecameHost => {
                {
                    let mut s = self.snapshot.lock().unwrap();
                    s.host = s.client_id;
                }
                ev(self, Event::BecameHost);
            }
            ServerMsg::SessionLost { code } => {
                {
                    let mut s = self.snapshot.lock().unwrap();
                    s.session = None;
                    s.players.clear();
                }
                ev(self, Event::SessionLost(code));
            }
            ServerMsg::Deliver { from, to, flags, payload } => ev(self, Event::Message { from, to, flags, payload }),
            ServerMsg::Desync { client_id, step, checksum, detail } => ev(self, Event::Desync { client_id, step, checksum, detail }),
        }
    }

}

fn request_id(msg: &ClientMsg) -> Option<u32> {
    use ClientMsg::*;
    match msg {
        ListSessions { req, .. }
        | CreateSession { req, .. }
        | JoinSession { req, .. }
        | SetSessionDesc { req, .. }
        | CreatePlayer { req, .. }
        | DestroyPlayer { req, .. }
        | SetPlayerData { req, .. }
        | SetPlayerName { req, .. } => Some(*req),
        LeaveSession { req } => Some(*req),
        _ => None,
    }
}
