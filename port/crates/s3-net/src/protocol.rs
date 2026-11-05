//! Relay protocol v1: messages between a game client (our DirectPlay HLE via [`crate::client`]) and `s3-relay`.
//!
//! Transport: one WebSocket **binary** message = one [`Frame`]. Frame layout:
//! `seq:varint  ack:varint  tag:u8  body...` (see docs/porting/multiplayer.md, "Wire format").
//!
//! * `seq` = 0 for control frames (Hello/Welcome/Reject/Ping/Pong/Ack/Bye). All other frames are numbered
//!   1, 2, 3, ... per direction and per *client identity* (survives reconnects), so a dropped connection can be
//!   resumed without loss or duplication: the sender keeps unacknowledged frames and retransmits them after a
//!   resume, the receiver drops `seq <= last_received`.
//! * `ack` = highest sequenced frame received from the other side so far (piggybacked on every frame).
//!
//! The model mirrors DirectPlay 6 sessions (what S3.EXE uses): a session has a host, players with 32-bit DPIDs,
//! player data/names, a session description with 4 user dwords, and Send(from, to, flags, payload).

use crate::wire::{DecodeError, Reader, Writer};

/// Bump on any incompatible wire change. The relay rejects other versions with [`ErrorCode::VersionMismatch`].
pub const PROTOCOL_VERSION: u16 = 1;

/// 16-byte GUID in Windows memory order (`GUID` struct bytes as stored by the game).
pub type Guid = [u8; 16];
pub type Sha256 = [u8; 32];

/// `DPID_ALLPLAYERS` (also `DPID_SYSMSG` as a sender): broadcast target.
pub const DPID_ALL: u32 = 0;

/// DirectPlay-compatible session description (subset of `DPSESSIONDESC2` that crosses the wire).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionDesc {
    /// `DPSESSION_*` flags (S3: `MIGRATEHOST|KEEPALIVE` = 0x44, + `PRIVATE` 0x200 / `DIRECTPLAYPROTOCOL` 0x2000).
    pub flags: u32,
    /// Session instance GUID; assigned by the relay on create (value sent by the creator is ignored).
    pub instance: Guid,
    /// Application GUID (S3: `{0E9F8400-D9A5-11CF-8F4D-00804880B189}`).
    pub application: Guid,
    pub max_players: u32,
    /// Filled in by the relay.
    pub current_players: u32,
    pub name: String,
    /// Only sent by the creator / joiner; the relay never sends it back (empty in listings).
    pub password: String,
    /// `dwUser1..4` (S3: build/version dwords, see multiplayer.md).
    pub user: [u32; 4],
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlayerInfo {
    pub dpid: u32,
    /// Relay client id owning the player.
    pub owner: u32,
    /// `DPPLAYER_*` flags as given to CreatePlayer.
    pub flags: u32,
    pub short_name: String,
    pub long_name: String,
    /// Remote player data (`DPSET_REMOTE`).
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum ErrorCode {
    BadRequest = 1,
    VersionMismatch = 2,
    ExeMismatch = 3,
    ResumeFailed = 4,
    ServerFull = 5,
    NoSuchSession = 10,
    SessionFull = 11,
    WrongPassword = 12,
    JoinDisabled = 13,
    NotInSession = 14,
    AlreadyInSession = 15,
    NoSuchPlayer = 20,
    NotOwner = 21,
    Timeout = 30,
    Disconnected = 31,
    Unknown = 0xffff,
}

impl ErrorCode {
    pub fn from_u16(v: u16) -> ErrorCode {
        use ErrorCode::*;
        match v {
            1 => BadRequest,
            2 => VersionMismatch,
            3 => ExeMismatch,
            4 => ResumeFailed,
            5 => ServerFull,
            10 => NoSuchSession,
            11 => SessionFull,
            12 => WrongPassword,
            13 => JoinDisabled,
            14 => NotInSession,
            15 => AlreadyInSession,
            20 => NoSuchPlayer,
            21 => NotOwner,
            30 => Timeout,
            31 => Disconnected,
            _ => Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resume {
    pub token: [u8; 16],
    /// Highest sequenced server frame this client received.
    pub last_seq: u64,
}

/// Client -> relay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientMsg {
    // control (seq 0)
    Hello { proto: u16, exe_sha256: Sha256, name: String, build: String, resume: Option<Resume> },
    Ping { nonce: u64 },
    Ack,
    Bye,
    // requests (sequenced; `req` echoes in the reply)
    ListSessions { req: u32, application: Guid },
    CreateSession { req: u32, desc: SessionDesc },
    JoinSession { req: u32, instance: Guid, password: String },
    LeaveSession { req: u32 },
    SetSessionDesc { req: u32, desc: SessionDesc },
    CreatePlayer { req: u32, flags: u32, short_name: String, long_name: String, data: Vec<u8> },
    DestroyPlayer { req: u32, dpid: u32 },
    SetPlayerData { req: u32, dpid: u32, data: Vec<u8> },
    SetPlayerName { req: u32, dpid: u32, short_name: String, long_name: String },
    // fire-and-forget (sequenced)
    Send { from: u32, to: u32, flags: u32, payload: Vec<u8> },
    DesyncReport { step: u32, checksum: u32, detail: String },
}

/// Relay -> client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerMsg {
    // control (seq 0)
    Welcome { proto: u16, client_id: u32, token: [u8; 16], resumed: bool, last_seq: u64, keepalive_ms: u32, server: String },
    Reject { code: ErrorCode, message: String },
    Pong { nonce: u64 },
    Ack,
    // replies (sequenced)
    Error { req: u32, code: ErrorCode, message: String },
    Ok { req: u32 },
    SessionList { req: u32, sessions: Vec<SessionDesc> },
    Joined { req: u32, desc: SessionDesc, host: u32, players: Vec<PlayerInfo> },
    PlayerCreated { req: u32, dpid: u32 },
    // events (sequenced) -> DPSYS_* system messages / data messages in the HLE
    PlayerAdded { player: PlayerInfo },
    PlayerRemoved { dpid: u32 },
    PlayerData { dpid: u32, data: Vec<u8> },
    PlayerName { dpid: u32, short_name: String, long_name: String },
    SessionDescChanged { desc: SessionDesc },
    /// This client is now the session host (`DPSYS_HOST`).
    BecameHost,
    /// `DPSYS_SESSIONLOST`.
    SessionLost { code: ErrorCode },
    Deliver { from: u32, to: u32, flags: u32, payload: Vec<u8> },
    Desync { client_id: u32, step: u32, checksum: u32, detail: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame<M> {
    pub seq: u64,
    pub ack: u64,
    pub msg: M,
}

pub trait Message: Sized {
    fn encode_body(&self, w: &mut Writer);
    fn decode_body(tag: u8, r: &mut Reader) -> Result<Self, DecodeError>;
    /// Control messages travel with seq 0 and are never retransmitted.
    fn is_control(&self) -> bool;
}

impl<M: Message> Frame<M> {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u64(self.seq).u64(self.ack);
        self.msg.encode_body(&mut w);
        w.finish()
    }
    pub fn decode(data: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(data);
        let seq = r.u64()?;
        let ack = r.u64()?;
        let tag = r.u8()?;
        let msg = M::decode_body(tag, &mut r)?;
        r.end()?;
        Ok(Frame { seq, ack, msg })
    }
}

fn put_desc(w: &mut Writer, d: &SessionDesc) {
    w.u32(d.flags).raw(&d.instance).raw(&d.application).u32(d.max_players).u32(d.current_players);
    w.str(&d.name).str(&d.password);
    for u in d.user {
        w.u32(u);
    }
}

fn get_desc(r: &mut Reader) -> Result<SessionDesc, DecodeError> {
    Ok(SessionDesc {
        flags: r.u32()?,
        instance: r.raw()?,
        application: r.raw()?,
        max_players: r.u32()?,
        current_players: r.u32()?,
        name: r.str()?,
        password: r.str()?,
        user: [r.u32()?, r.u32()?, r.u32()?, r.u32()?],
    })
}

fn put_player(w: &mut Writer, p: &PlayerInfo) {
    w.u32(p.dpid).u32(p.owner).u32(p.flags).str(&p.short_name).str(&p.long_name).bytes(&p.data);
}

fn get_player(r: &mut Reader) -> Result<PlayerInfo, DecodeError> {
    Ok(PlayerInfo {
        dpid: r.u32()?,
        owner: r.u32()?,
        flags: r.u32()?,
        short_name: r.str()?,
        long_name: r.str()?,
        data: r.bytes()?,
    })
}

impl Message for ClientMsg {
    fn is_control(&self) -> bool {
        matches!(self, ClientMsg::Hello { .. } | ClientMsg::Ping { .. } | ClientMsg::Ack | ClientMsg::Bye)
    }

    fn encode_body(&self, w: &mut Writer) {
        use ClientMsg::*;
        match self {
            Hello { proto, exe_sha256, name, build, resume } => {
                w.u8(1).u16(*proto).raw(exe_sha256).str(name).str(build);
                match resume {
                    None => {
                        w.u8(0);
                    }
                    Some(r) => {
                        w.u8(1).raw(&r.token).u64(r.last_seq);
                    }
                }
            }
            Ping { nonce } => {
                w.u8(2).u64(*nonce);
            }
            Ack => {
                w.u8(3);
            }
            Bye => {
                w.u8(4);
            }
            ListSessions { req, application } => {
                w.u8(10).u32(*req).raw(application);
            }
            CreateSession { req, desc } => {
                w.u8(11).u32(*req);
                put_desc(w, desc);
            }
            JoinSession { req, instance, password } => {
                w.u8(12).u32(*req).raw(instance).str(password);
            }
            LeaveSession { req } => {
                w.u8(13).u32(*req);
            }
            SetSessionDesc { req, desc } => {
                w.u8(14).u32(*req);
                put_desc(w, desc);
            }
            CreatePlayer { req, flags, short_name, long_name, data } => {
                w.u8(20).u32(*req).u32(*flags).str(short_name).str(long_name).bytes(data);
            }
            DestroyPlayer { req, dpid } => {
                w.u8(21).u32(*req).u32(*dpid);
            }
            SetPlayerData { req, dpid, data } => {
                w.u8(22).u32(*req).u32(*dpid).bytes(data);
            }
            SetPlayerName { req, dpid, short_name, long_name } => {
                w.u8(23).u32(*req).u32(*dpid).str(short_name).str(long_name);
            }
            Send { from, to, flags, payload } => {
                w.u8(30).u32(*from).u32(*to).u32(*flags).bytes(payload);
            }
            DesyncReport { step, checksum, detail } => {
                w.u8(40).u32(*step).u32(*checksum).str(detail);
            }
        }
    }

    fn decode_body(tag: u8, r: &mut Reader) -> Result<Self, DecodeError> {
        use ClientMsg::*;
        Ok(match tag {
            1 => Hello {
                proto: r.u16()?,
                exe_sha256: r.raw()?,
                name: r.str()?,
                build: r.str()?,
                resume: if r.bool()? { Some(Resume { token: r.raw()?, last_seq: r.u64()? }) } else { None },
            },
            2 => Ping { nonce: r.u64()? },
            3 => Ack,
            4 => Bye,
            10 => ListSessions { req: r.u32()?, application: r.raw()? },
            11 => CreateSession { req: r.u32()?, desc: get_desc(r)? },
            12 => JoinSession { req: r.u32()?, instance: r.raw()?, password: r.str()? },
            13 => LeaveSession { req: r.u32()? },
            14 => SetSessionDesc { req: r.u32()?, desc: get_desc(r)? },
            20 => CreatePlayer {
                req: r.u32()?,
                flags: r.u32()?,
                short_name: r.str()?,
                long_name: r.str()?,
                data: r.bytes()?,
            },
            21 => DestroyPlayer { req: r.u32()?, dpid: r.u32()? },
            22 => SetPlayerData { req: r.u32()?, dpid: r.u32()?, data: r.bytes()? },
            23 => SetPlayerName { req: r.u32()?, dpid: r.u32()?, short_name: r.str()?, long_name: r.str()? },
            30 => Send { from: r.u32()?, to: r.u32()?, flags: r.u32()?, payload: r.bytes()? },
            40 => DesyncReport { step: r.u32()?, checksum: r.u32()?, detail: r.str()? },
            t => return Err(DecodeError::BadTag { what: "client message", tag: t as u64 }),
        })
    }
}

impl Message for ServerMsg {
    fn is_control(&self) -> bool {
        matches!(self, ServerMsg::Welcome { .. } | ServerMsg::Reject { .. } | ServerMsg::Pong { .. } | ServerMsg::Ack)
    }

    fn encode_body(&self, w: &mut Writer) {
        use ServerMsg::*;
        match self {
            Welcome { proto, client_id, token, resumed, last_seq, keepalive_ms, server } => {
                w.u8(1).u16(*proto).u32(*client_id).raw(token).bool(*resumed).u64(*last_seq).u32(*keepalive_ms);
                w.str(server);
            }
            Reject { code, message } => {
                w.u8(2).u16(*code as u16).str(message);
            }
            Pong { nonce } => {
                w.u8(3).u64(*nonce);
            }
            Ack => {
                w.u8(4);
            }
            Error { req, code, message } => {
                w.u8(10).u32(*req).u16(*code as u16).str(message);
            }
            Ok { req } => {
                w.u8(11).u32(*req);
            }
            SessionList { req, sessions } => {
                w.u8(12).u32(*req).u32(sessions.len() as u32);
                for s in sessions {
                    put_desc(w, s);
                }
            }
            Joined { req, desc, host, players } => {
                w.u8(13).u32(*req);
                put_desc(w, desc);
                w.u32(*host).u32(players.len() as u32);
                for p in players {
                    put_player(w, p);
                }
            }
            PlayerCreated { req, dpid } => {
                w.u8(14).u32(*req).u32(*dpid);
            }
            PlayerAdded { player } => {
                w.u8(20);
                put_player(w, player);
            }
            PlayerRemoved { dpid } => {
                w.u8(21).u32(*dpid);
            }
            PlayerData { dpid, data } => {
                w.u8(22).u32(*dpid).bytes(data);
            }
            PlayerName { dpid, short_name, long_name } => {
                w.u8(23).u32(*dpid).str(short_name).str(long_name);
            }
            SessionDescChanged { desc } => {
                w.u8(24);
                put_desc(w, desc);
            }
            BecameHost => {
                w.u8(25);
            }
            SessionLost { code } => {
                w.u8(26).u16(*code as u16);
            }
            Deliver { from, to, flags, payload } => {
                w.u8(30).u32(*from).u32(*to).u32(*flags).bytes(payload);
            }
            Desync { client_id, step, checksum, detail } => {
                w.u8(40).u32(*client_id).u32(*step).u32(*checksum).str(detail);
            }
        }
    }

    fn decode_body(tag: u8, r: &mut Reader) -> Result<Self, DecodeError> {
        use ServerMsg::*;
        Ok(match tag {
            1 => Welcome {
                proto: r.u16()?,
                client_id: r.u32()?,
                token: r.raw()?,
                resumed: r.bool()?,
                last_seq: r.u64()?,
                keepalive_ms: r.u32()?,
                server: r.str()?,
            },
            2 => Reject { code: ErrorCode::from_u16(r.u16()?), message: r.str()? },
            3 => Pong { nonce: r.u64()? },
            4 => Ack,
            10 => Error { req: r.u32()?, code: ErrorCode::from_u16(r.u16()?), message: r.str()? },
            11 => Ok { req: r.u32()? },
            12 => {
                let req = r.u32()?;
                let n = r.count()?;
                let mut sessions = Vec::with_capacity(n);
                for _ in 0..n {
                    sessions.push(get_desc(r)?);
                }
                SessionList { req, sessions }
            }
            13 => {
                let req = r.u32()?;
                let desc = get_desc(r)?;
                let host = r.u32()?;
                let n = r.count()?;
                let mut players = Vec::with_capacity(n);
                for _ in 0..n {
                    players.push(get_player(r)?);
                }
                Joined { req, desc, host, players }
            }
            14 => PlayerCreated { req: r.u32()?, dpid: r.u32()? },
            20 => PlayerAdded { player: get_player(r)? },
            21 => PlayerRemoved { dpid: r.u32()? },
            22 => PlayerData { dpid: r.u32()?, data: r.bytes()? },
            23 => PlayerName { dpid: r.u32()?, short_name: r.str()?, long_name: r.str()? },
            24 => SessionDescChanged { desc: get_desc(r)? },
            25 => BecameHost,
            26 => SessionLost { code: ErrorCode::from_u16(r.u16()?) },
            30 => Deliver { from: r.u32()?, to: r.u32()?, flags: r.u32()?, payload: r.bytes()? },
            40 => Desync { client_id: r.u32()?, step: r.u32()?, checksum: r.u32()?, detail: r.str()? },
            t => return Err(DecodeError::BadTag { what: "server message", tag: t as u64 }),
        })
    }
}

/// Parse a GUID in registry form (`XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX`, braces optional) into Windows memory order.
pub const fn guid(s: &str) -> Guid {
    const fn hex(c: u8) -> u8 {
        match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => panic!("bad hex digit in GUID"),
        }
    }
    let b = s.as_bytes();
    let mut digits = [0u8; 32];
    let mut n = 0;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c != b'-' && c != b'{' && c != b'}' {
            digits[n] = hex(c);
            n += 1;
        }
        i += 1;
    }
    assert!(n == 32, "GUID needs 32 hex digits");
    let mut v = [0u8; 16];
    let mut k = 0;
    while k < 16 {
        v[k] = digits[2 * k] << 4 | digits[2 * k + 1];
        k += 1;
    }
    // Data1 (4 bytes), Data2 (2), Data3 (2) are little-endian in memory; Data4 is a byte array.
    [v[3], v[2], v[1], v[0], v[5], v[4], v[7], v[6], v[8], v[9], v[10], v[11], v[12], v[13], v[14], v[15]]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desc() -> SessionDesc {
        SessionDesc {
            flags: 0x44,
            instance: [7; 16],
            application: crate::S3_APP_GUID,
            max_players: 8,
            current_players: 2,
            name: "Römer gegen Ägypter".into(),
            password: "pw".into(),
            user: [1, 2, 0xa0, 1],
        }
    }

    fn player() -> PlayerInfo {
        PlayerInfo { dpid: 0x1234_5678, owner: 3, flags: 0, short_name: "Ali".into(), long_name: "".into(), data: vec![1, 2, 3] }
    }

    fn rt_client(m: ClientMsg) {
        let seq = if m.is_control() { 0 } else { 42 };
        let f = Frame { seq, ack: 7, msg: m };
        let enc = f.encode();
        assert_eq!(Frame::<ClientMsg>::decode(&enc).unwrap(), f);
        for cut in 0..enc.len() {
            assert!(Frame::<ClientMsg>::decode(&enc[..cut]).is_err(), "prefix {cut} of {:?}", f.msg);
        }
    }

    fn rt_server(m: ServerMsg) {
        let seq = if m.is_control() { 0 } else { 9 };
        let f = Frame { seq, ack: 1 << 40, msg: m };
        let enc = f.encode();
        assert_eq!(Frame::<ServerMsg>::decode(&enc).unwrap(), f);
        for cut in 0..enc.len() {
            assert!(Frame::<ServerMsg>::decode(&enc[..cut]).is_err(), "prefix {cut} of {:?}", f.msg);
        }
    }

    #[test]
    fn client_messages_roundtrip() {
        use ClientMsg::*;
        rt_client(Hello { proto: 1, exe_sha256: crate::GOG_EXE_SHA256, name: "x".into(), build: "b".into(), resume: None });
        rt_client(Hello {
            proto: 1,
            exe_sha256: [0; 32],
            name: "".into(),
            build: "".into(),
            resume: Some(Resume { token: [9; 16], last_seq: 77 }),
        });
        rt_client(Ping { nonce: 5 });
        rt_client(Ack);
        rt_client(Bye);
        rt_client(ListSessions { req: 1, application: crate::S3_APP_GUID });
        rt_client(CreateSession { req: 2, desc: desc() });
        rt_client(JoinSession { req: 3, instance: [1; 16], password: "".into() });
        rt_client(LeaveSession { req: 4 });
        rt_client(SetSessionDesc { req: 5, desc: desc() });
        rt_client(CreatePlayer { req: 6, flags: 0, short_name: "a".into(), long_name: "b".into(), data: vec![] });
        rt_client(DestroyPlayer { req: 7, dpid: 3 });
        rt_client(SetPlayerData { req: 8, dpid: 3, data: vec![0; 300] });
        rt_client(SetPlayerName { req: 9, dpid: 3, short_name: "s".into(), long_name: "l".into() });
        rt_client(Send { from: 1, to: 0, flags: 1, payload: vec![0x20, 0, 8, 1, 0, 0, 0, 0] });
        rt_client(DesyncReport { step: 100, checksum: 0xdead_beef, detail: "Desync Settler!".into() });
    }

    #[test]
    fn server_messages_roundtrip() {
        use ServerMsg::*;
        rt_server(Welcome {
            proto: 1,
            client_id: 5,
            token: [3; 16],
            resumed: true,
            last_seq: 12,
            keepalive_ms: 2000,
            server: "s3-relay".into(),
        });
        rt_server(Reject { code: ErrorCode::ExeMismatch, message: "nope".into() });
        rt_server(Pong { nonce: 1 });
        rt_server(Ack);
        rt_server(Error { req: 1, code: ErrorCode::SessionFull, message: "full".into() });
        rt_server(Ok { req: 2 });
        rt_server(SessionList { req: 3, sessions: vec![desc(), SessionDesc::default()] });
        rt_server(Joined { req: 4, desc: desc(), host: 1, players: vec![player(), PlayerInfo::default()] });
        rt_server(PlayerCreated { req: 5, dpid: 99 });
        rt_server(PlayerAdded { player: player() });
        rt_server(PlayerRemoved { dpid: 99 });
        rt_server(PlayerData { dpid: 99, data: vec![5; 10] });
        rt_server(PlayerName { dpid: 99, short_name: "n".into(), long_name: "".into() });
        rt_server(SessionDescChanged { desc: desc() });
        rt_server(BecameHost);
        rt_server(SessionLost { code: ErrorCode::Disconnected });
        rt_server(Deliver { from: 1, to: 2, flags: 0, payload: vec![1; 1000] });
        rt_server(Desync { client_id: 1, step: 2, checksum: 3, detail: "d".into() });
    }

    #[test]
    fn rejects_unknown_tag_and_trailing() {
        assert!(matches!(Frame::<ClientMsg>::decode(&[0, 0, 200]), Err(DecodeError::BadTag { .. })));
        let mut enc = Frame { seq: 0, ack: 0, msg: ClientMsg::Ack }.encode();
        enc.push(0);
        assert_eq!(Frame::<ClientMsg>::decode(&enc), Err(DecodeError::TrailingBytes(1)));
    }

    #[test]
    fn guid_parsing_matches_exe_bytes() {
        // Bytes read from S3.EXE at 0x6970f8 (application GUID in the DPSESSIONDESC2 it opens).
        let exe = [0x00, 0x84, 0x9f, 0x0e, 0xa5, 0xd9, 0xcf, 0x11, 0x8f, 0x4d, 0x00, 0x80, 0x48, 0x80, 0xb1, 0x89];
        assert_eq!(guid("{0E9F8400-D9A5-11CF-8F4D-00804880B189}"), exe);
    }
}
