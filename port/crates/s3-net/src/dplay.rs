//! DirectPlay 6 constants and system-message layouts, for the future s3-dx DirectPlay HLE that sits on top of
//! [`crate::client::NetClient`]. Nothing here touches guest memory directly: [`SystemMessage::layout`] produces
//! the bytes the HLE copies into the game's Receive buffer.
//!
//! Sources: error codes are **OBSERVED** in S3.EXE's own HRESULT->name table at `0x7a03b4`; flag/struct values
//! are from dplay.h (memory) and are cross-checked against S3.EXE where the game uses them (see
//! docs/porting/multiplayer.md). Struct layouts are HYPOTHESIS until a runtime test feeds them to the game.

use crate::protocol::{ErrorCode, PlayerInfo, SessionDesc};

pub const DP_OK: u32 = 0;

/// HRESULTs exactly as listed in S3.EXE's table at `0x7a03b4` (OBSERVED).
pub mod dperr {
    pub const ALREADYINITIALIZED: u32 = 0x8877_0005;
    pub const ACCESSDENIED: u32 = 0x8877_000a;
    pub const ACTIVEPLAYERS: u32 = 0x8877_0014;
    pub const BUFFERTOOSMALL: u32 = 0x8877_001e;
    pub const CANTADDPLAYER: u32 = 0x8877_0028;
    pub const CANTCREATEGROUP: u32 = 0x8877_0032;
    pub const CANTCREATEPLAYER: u32 = 0x8877_003c;
    pub const CANTCREATESESSION: u32 = 0x8877_0046;
    pub const CAPSNOTAVAILABLEYET: u32 = 0x8877_0050;
    pub const EXCEPTION: u32 = 0x8877_005a;
    pub const GENERIC: u32 = 0x8000_4005;
    pub const INVALIDFLAGS: u32 = 0x8877_0078;
    pub const INVALIDOBJECT: u32 = 0x8877_0082;
    pub const INVALIDPARAMS: u32 = 0x8007_0057;
    pub const INVALIDPLAYER: u32 = 0x8877_0096;
    pub const INVALIDGROUP: u32 = 0x8877_009b;
    pub const NOCAPS: u32 = 0x8877_00a0;
    pub const NOCONNECTION: u32 = 0x8877_00aa;
    pub const OUTOFMEMORY: u32 = 0x8007_000e;
    pub const NOMESSAGES: u32 = 0x8877_00be;
    pub const NONAMESERVERFOUND: u32 = 0x8877_00c8;
    pub const NOPLAYERS: u32 = 0x8877_00d2;
    pub const NOSESSIONS: u32 = 0x8877_00dc;
    pub const PENDING: u32 = 0x8000_000a;
    pub const SENDTOOBIG: u32 = 0x8877_00e6;
    pub const TIMEOUT: u32 = 0x8877_00f0;
    pub const UNAVAILABLE: u32 = 0x8877_00fa;
    pub const UNSUPPORTED: u32 = 0x8000_4001;
    pub const BUSY: u32 = 0x8877_010e;
    pub const USERCANCEL: u32 = 0x8877_0118;
    pub const NOINTERFACE: u32 = 0x8000_4002;
    pub const CANNOTCREATESERVER: u32 = 0x8877_0122;
    pub const PLAYERLOST: u32 = 0x8877_012c;
    pub const SESSIONLOST: u32 = 0x8877_0136;
    pub const UNINITIALIZED: u32 = 0x8877_0140;
    pub const NONEWPLAYERS: u32 = 0x8877_014a;
    pub const INVALIDPASSWORD: u32 = 0x8877_0154;
    pub const CONNECTING: u32 = 0x8877_015e;
    pub const CONNECTIONLOST: u32 = 0x8877_0168;
    pub const UNKNOWNAPPLICATION: u32 = 0x8877_041a;
    pub const NOTLOBBIED: u32 = 0x8877_042e;
}

/// `DPSYS_*` message types. S3.EXE handles CREATEPLAYERORGROUP, DESTROYPLAYERORGROUP, SESSIONLOST, HOST,
/// SETPLAYERORGROUPDATA and SETSESSIONDESC (switch at `0x5bcde1`); anything else logs "Unsupported DPSYS Message".
pub mod dpsys {
    pub const CREATEPLAYERORGROUP: u32 = 0x0003;
    pub const DESTROYPLAYERORGROUP: u32 = 0x0005;
    pub const ADDPLAYERTOGROUP: u32 = 0x0007;
    pub const DELETEPLAYERFROMGROUP: u32 = 0x0021;
    pub const SESSIONLOST: u32 = 0x0031;
    pub const HOST: u32 = 0x0101;
    pub const SETPLAYERORGROUPDATA: u32 = 0x0102;
    pub const SETPLAYERORGROUPNAME: u32 = 0x0103;
    pub const SETSESSIONDESC: u32 = 0x0104;
    pub const ADDGROUPTOGROUP: u32 = 0x0105;
    pub const DELETEGROUPFROMGROUP: u32 = 0x0106;
    pub const SECUREMESSAGE: u32 = 0x0107;
    pub const STARTSESSION: u32 = 0x0108;
    pub const CHAT: u32 = 0x0109;
    pub const SETGROUPOWNER: u32 = 0x010a;
    pub const SENDCOMPLETE: u32 = 0x010d;
}

pub const DPPLAYERTYPE_GROUP: u32 = 0;
pub const DPPLAYERTYPE_PLAYER: u32 = 1;

pub const DPID_SYSMSG: u32 = 0;
pub const DPID_ALLPLAYERS: u32 = 0;
pub const DPID_SERVERPLAYER: u32 = 1;
pub const DPID_UNKNOWN: u32 = 0xffff_ffff;

/// `DPSEND_GUARANTEED`. S3 uses it for setup/control messages, not for the in-game lockstep bundles.
pub const DPSEND_GUARANTEED: u32 = 0x1;
pub const DPRECEIVE_ALL: u32 = 0x1;
pub const DPRECEIVE_TOPLAYER: u32 = 0x2;
pub const DPRECEIVE_FROMPLAYER: u32 = 0x4;
pub const DPRECEIVE_PEEK: u32 = 0x8;

pub const DPOPEN_JOIN: u32 = 0x1;
pub const DPOPEN_CREATE: u32 = 0x2;

pub const DPENUMSESSIONS_AVAILABLE: u32 = 0x1;
pub const DPENUMSESSIONS_ALL: u32 = 0x2;
pub const DPENUMSESSIONS_ASYNC: u32 = 0x10;
pub const DPENUMSESSIONS_STOPASYNC: u32 = 0x20;
pub const DPENUMSESSIONS_PASSWORDREQUIRED: u32 = 0x40;

pub const DPESC_TIMEDOUT: u32 = 0x1;

pub const DPSESSION_NEWPLAYERSDISABLED: u32 = 0x1;
pub const DPSESSION_MIGRATEHOST: u32 = 0x4;
pub const DPSESSION_NOMESSAGEID: u32 = 0x8;
pub const DPSESSION_JOINDISABLED: u32 = 0x20;
pub const DPSESSION_KEEPALIVE: u32 = 0x40;
pub const DPSESSION_PRIVATE: u32 = 0x200;
pub const DPSESSION_PASSWORDREQUIRED: u32 = 0x400;
pub const DPSESSION_DIRECTPLAYPROTOCOL: u32 = 0x2000;

pub const DPCONNECTION_DIRECTPLAY: u32 = 0x1;
pub const DPCONNECTION_DIRECTPLAYLOBBY: u32 = 0x2;

pub const DPCAPS_GUARANTEEDOPTIMIZED: u32 = 0x40;
pub const DPCAPS_GUARANTEEDSUPPORTED: u32 = 0x80;

/// `DPSESSIONDESC2` size (0x50, OBSERVED: S3 sets `dwSize = 0x50` at `0x46ff80`).
pub const DPSESSIONDESC2_SIZE: usize = 0x50;
/// `DPNAME` size.
pub const DPNAME_SIZE: usize = 16;

/// Service provider GUID our HLE reports from EnumConnections ("Internet (Settlers 3 Port)").
/// Random GUID owned by this project (not a Microsoft SP); the game only displays the name and passes the
/// connection blob back to InitializeConnection (HYPOTHESIS to confirm in the build phase).
pub const SP_GUID_S3PORT: crate::protocol::Guid = crate::protocol::guid("{6A2F4C1E-3B5D-4E8F-9A10-53335056F4E1}");
/// Microsoft DirectPlay TCP/IP SP GUID (for reference / future PC bridge).
pub const DPSPGUID_TCPIP: crate::protocol::Guid = crate::protocol::guid("{36E95EE0-8577-11CF-960C-0080C7534E82}");

/// Map relay error codes to the HRESULTs the game expects from the matching DirectPlay call.
pub fn hresult_for(code: ErrorCode) -> u32 {
    use ErrorCode::*;
    match code {
        NoSuchSession => dperr::NOSESSIONS,
        SessionFull | JoinDisabled => dperr::NONEWPLAYERS,
        WrongPassword => dperr::INVALIDPASSWORD,
        NotInSession => dperr::UNINITIALIZED,
        AlreadyInSession => dperr::ALREADYINITIALIZED,
        NoSuchPlayer => dperr::INVALIDPLAYER,
        NotOwner => dperr::ACCESSDENIED,
        Timeout => dperr::TIMEOUT,
        Disconnected => dperr::CONNECTIONLOST,
        VersionMismatch | ExeMismatch => dperr::UNKNOWNAPPLICATION,
        ServerFull => dperr::CANNOTCREATESERVER,
        BadRequest => dperr::INVALIDPARAMS,
        ResumeFailed => dperr::SESSIONLOST,
        Unknown => dperr::GENERIC,
    }
}

/// UTF-8 -> Windows-1252-ish ANSI (chars above U+00FF become `?`). S3 is a German/Latin-1 game.
pub fn to_ansi(s: &str) -> Vec<u8> {
    s.chars().map(|c| if (c as u32) < 0x100 { c as u32 as u8 } else { b'?' }).collect()
}

/// A DirectPlay system message (`idFrom == DPID_SYSMSG`) as the HLE must hand it to `Receive`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemMessage {
    CreatePlayer { player: PlayerInfo, current_players: u32 },
    DestroyPlayer { player: PlayerInfo },
    SetPlayerData { dpid: u32, data: Vec<u8> },
    SetSessionDesc { desc: SessionDesc },
    Host,
    SessionLost,
}

fn put(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}

/// Write a DPSESSIONDESC2 whose string pointers point into `tail` (appended after the struct by the caller).
fn put_session_desc(out: &mut Vec<u8>, tail: &mut Vec<u8>, base: u32, fixed_len: usize, d: &SessionDesc) {
    let tail_ptr = |tail: &Vec<u8>| base + fixed_len as u32 + tail.len() as u32;
    put(out, DPSESSIONDESC2_SIZE as u32);
    put(out, d.flags);
    out.extend_from_slice(&d.instance);
    out.extend_from_slice(&d.application);
    put(out, d.max_players);
    put(out, d.current_players);
    let name_ptr = tail_ptr(tail);
    tail.extend(to_ansi(&d.name));
    tail.push(0);
    put(out, name_ptr);
    put(out, 0); // lpszPassword: never exposed
    put(out, 0); // dwReserved1
    put(out, 0); // dwReserved2
    for u in d.user {
        put(out, u);
    }
}

fn put_name(out: &mut Vec<u8>, tail: &mut Vec<u8>, base: u32, fixed_len: usize, short: &str, long: &str) {
    put(out, DPNAME_SIZE as u32);
    put(out, 0);
    for s in [short, long] {
        if s.is_empty() {
            put(out, 0);
        } else {
            put(out, base + fixed_len as u32 + tail.len() as u32);
            tail.extend(to_ansi(s));
            tail.push(0);
        }
    }
}

impl SystemMessage {
    /// Size of the fixed `DPMSG_*` struct (strings/data follow it in the same buffer).
    pub fn fixed_len(&self) -> usize {
        match self {
            // dwType dwPlayerType dpId dwCurrentPlayers lpData dwDataSize DPNAME dpIdParent dwFlags
            SystemMessage::CreatePlayer { .. } => 6 * 4 + DPNAME_SIZE + 2 * 4,
            // dwType dwPlayerType dpId lpLocalData dwLocalDataSize lpRemoteData dwRemoteDataSize DPNAME dpIdParent dwFlags
            SystemMessage::DestroyPlayer { .. } => 7 * 4 + DPNAME_SIZE + 2 * 4,
            // dwType dwPlayerType dpId lpData dwDataSize
            SystemMessage::SetPlayerData { .. } => 5 * 4,
            SystemMessage::SetSessionDesc { .. } => 4 + DPSESSIONDESC2_SIZE,
            SystemMessage::Host | SystemMessage::SessionLost => 4,
        }
    }

    /// Serialize as it must appear at guest address `base`: fixed struct, then strings/data it points to.
    /// The returned length is what `Receive` reports in `*lpdwDataSize` (DirectPlay also counts the tail).
    pub fn layout(&self, base: u32) -> Vec<u8> {
        let fixed = self.fixed_len();
        let mut out = Vec::with_capacity(fixed + 64);
        let mut tail = Vec::new();
        match self {
            SystemMessage::CreatePlayer { player, current_players } => {
                put(&mut out, dpsys::CREATEPLAYERORGROUP);
                put(&mut out, DPPLAYERTYPE_PLAYER);
                put(&mut out, player.dpid);
                put(&mut out, *current_players);
                let data_ptr = if player.data.is_empty() { 0 } else { base + fixed as u32 };
                tail.extend_from_slice(&player.data);
                put(&mut out, data_ptr);
                put(&mut out, player.data.len() as u32);
                put_name(&mut out, &mut tail, base, fixed, &player.short_name, &player.long_name);
                put(&mut out, 0); // dpIdParent
                put(&mut out, player.flags);
            }
            SystemMessage::DestroyPlayer { player } => {
                put(&mut out, dpsys::DESTROYPLAYERORGROUP);
                put(&mut out, DPPLAYERTYPE_PLAYER);
                put(&mut out, player.dpid);
                put(&mut out, 0); // lpLocalData
                put(&mut out, 0);
                let data_ptr = if player.data.is_empty() { 0 } else { base + fixed as u32 };
                tail.extend_from_slice(&player.data);
                put(&mut out, data_ptr);
                put(&mut out, player.data.len() as u32);
                put_name(&mut out, &mut tail, base, fixed, &player.short_name, &player.long_name);
                put(&mut out, 0);
                put(&mut out, player.flags);
            }
            SystemMessage::SetPlayerData { dpid, data } => {
                put(&mut out, dpsys::SETPLAYERORGROUPDATA);
                put(&mut out, DPPLAYERTYPE_PLAYER);
                put(&mut out, *dpid);
                put(&mut out, if data.is_empty() { 0 } else { base + fixed as u32 });
                put(&mut out, data.len() as u32);
                tail.extend_from_slice(data);
            }
            SystemMessage::SetSessionDesc { desc } => {
                put(&mut out, dpsys::SETSESSIONDESC);
                put_session_desc(&mut out, &mut tail, base, fixed, desc);
            }
            SystemMessage::Host => put(&mut out, dpsys::HOST),
            SystemMessage::SessionLost => put(&mut out, dpsys::SESSIONLOST),
        }
        debug_assert_eq!(out.len(), fixed);
        out.extend_from_slice(&tail);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rd(b: &[u8], off: usize) -> u32 {
        u32::from_le_bytes(b[off..off + 4].try_into().unwrap())
    }

    #[test]
    fn hresults_match_dplay_formula() {
        // MAKE_DPHRESULT(code) = 0x88770000 | code
        assert_eq!(dperr::NOTLOBBIED, 0x8877_0000 | 1070);
        assert_eq!(dperr::NOMESSAGES, 0x8877_0000 | 190);
        assert_eq!(dperr::NOMESSAGES as i32, -0x7788_ff42); // the constant S3 compares Receive against
        assert_eq!(hresult_for(ErrorCode::SessionFull), dperr::NONEWPLAYERS);
    }

    #[test]
    fn create_player_layout() {
        let p = PlayerInfo { dpid: 0x55, owner: 1, flags: 0, short_name: "Jörg".into(), long_name: "".into(), data: vec![9, 8] };
        let m = SystemMessage::CreatePlayer { player: p, current_players: 2 };
        let base = 0x0100_0000;
        let b = m.layout(base);
        assert_eq!(m.fixed_len(), 48);
        assert_eq!(rd(&b, 0), dpsys::CREATEPLAYERORGROUP);
        assert_eq!(rd(&b, 4), DPPLAYERTYPE_PLAYER);
        assert_eq!(rd(&b, 8), 0x55);
        assert_eq!(rd(&b, 12), 2);
        assert_eq!(rd(&b, 16), base + 48); // data right after the struct
        assert_eq!(rd(&b, 20), 2);
        assert_eq!(rd(&b, 24), 16); // DPNAME.dwSize
        let short = rd(&b, 32);
        assert_eq!(short, base + 50);
        assert_eq!(&b[50..55], &[b'J', 0xf6, b'r', b'g', 0]);
        assert_eq!(rd(&b, 36), 0); // empty long name -> NULL
    }

    #[test]
    fn set_player_data_type_is_player() {
        // S3 checks dwPlayerType == 1 at 0x5bd013 before dispatching SETPLAYERORGROUPDATA.
        let b = SystemMessage::SetPlayerData { dpid: 7, data: vec![1, 2, 3] }.layout(0x1000);
        assert_eq!(rd(&b, 4), 1);
        assert_eq!(b.len(), 23);
    }

    #[test]
    fn session_desc_layout() {
        let d = SessionDesc { flags: 0x44, max_players: 6, current_players: 3, name: "abc".into(), user: [1, 2, 3, 4], ..Default::default() };
        let b = SystemMessage::SetSessionDesc { desc: d }.layout(0x2000);
        assert_eq!(rd(&b, 4), 0x50);
        assert_eq!(rd(&b, 8), 0x44);
        assert_eq!(rd(&b, 4 + 0x28), 6);
        assert_eq!(rd(&b, 4 + 0x30), 0x2000 + 0x54);
        assert_eq!(rd(&b, 4 + 0x4c), 4);
        assert_eq!(&b[0x54..], b"abc\0");
    }
}
