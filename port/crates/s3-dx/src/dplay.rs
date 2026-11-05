//! DirectPlay 6 HLE (`dplayx.dll`): COM plumbing here, the relay-backed methods in `dplay_net.rs`. S3.EXE creates `CLSID_DirectPlay` (IID_IDirectPlay4A, `0x5c0808`) and
//! `CLSID_DirectPlayLobby` (IID_IDirectPlayLobby2A, `0x5c0847`) at startup and releases them again (static RE
//! + wine trace). IDirectPlay4A methods the game uses are implemented over our relay (see
//! docs/porting/multiplayer.md); groups and the lobby are stubs (not lobbied, no groups).
//! Method argument counts follow dplay.h / dplobby.h (from memory of the SDK headers; HYPOTHESIS until a game
//! path exercises them).

use s3_runtime::com::Guid;
use s3_runtime::{LogLevel, Machine, Registry, ThunkResult};

use crate::hr::*;
use crate::dplay_net as net;
use crate::{declare_interface, put_u32, reg_method, ret};

pub const DLL: &str = "dplayx.dll";

pub const CLSID_DIRECTPLAY: Guid = Guid::parse("D1EB6D20-8923-11D0-9D97-00A0C90A43CB");
pub const CLSID_DIRECTPLAYLOBBY: Guid = Guid::parse("2FE8F810-B2A5-11D0-A787-0000F803ABFC");
pub const IID_IDIRECTPLAY4A: Guid = Guid::parse("0AB1C531-4745-11D1-A7A1-0000F803ABFC");
pub const IID_IDIRECTPLAYLOBBY2A: Guid = Guid::parse("1BB4AF80-A303-11D0-9C4F-00A0C905425E");

pub const EXPORTS: &[&str] = &["DirectPlayCreate", "DirectPlayEnumerateA", "DirectPlayLobbyCreateA"];

pub const IDIRECTPLAY4A: &[&str] = &[
    "QueryInterface", "AddRef", "Release", "AddPlayerToGroup", "Close", "CreateGroup", "CreatePlayer",
    "DeletePlayerFromGroup", "DestroyGroup", "DestroyPlayer", "EnumGroupPlayers", "EnumGroups", "EnumPlayers",
    "EnumSessions", "GetCaps", "GetGroupData", "GetGroupName", "GetMessageCount", "GetPlayerAddress",
    "GetPlayerCaps", "GetPlayerData", "GetPlayerName", "GetSessionDesc", "Initialize", "Open", "Receive",
    "Send", "SetGroupData", "SetGroupName", "SetPlayerData", "SetPlayerName", "SetSessionDesc",
    "AddGroupToGroup", "CreateGroupInGroup", "DeleteGroupFromGroup", "EnumConnections", "EnumGroupsInGroup",
    "GetGroupConnectionSettings", "InitializeConnection", "SecureOpen", "SendChatMessage",
    "SetGroupConnectionSettings", "StartSession", "GetGroupFlags", "GetGroupParent", "GetPlayerAccount",
    "GetPlayerFlags", "GetGroupOwner", "SetGroupOwner", "SendEx", "GetMessageQueue", "CancelMessage",
    "CancelPriority",
];

/// Arguments (including `this`) of each IDirectPlay4A method, same order as [`IDIRECTPLAY4A`].
pub const IDIRECTPLAY4A_NARGS: &[u32] = &[
    3, 1, 1, 3, 1, 6, 7, 3, 2, 2, 6, 5, 5, 6, 3, 5, 4, 3, 4, 4, 5, 4, 3, 2, 3, 6, 6, 5, 4, 5, 4, 3, 3, 7, 3, 5,
    6, 5, 3, 5, 5, 4, 3, 3, 3, 5, 3, 3, 3, 10, 6, 3, 4,
];

pub const IDIRECTPLAYLOBBY3A: &[&str] = &[
    "QueryInterface", "AddRef", "Release", "Connect", "CreateAddress", "EnumAddress", "EnumAddressTypes",
    "EnumLocalApplications", "GetConnectionSettings", "ReceiveLobbyMessage", "RunApplication",
    "SendLobbyMessage", "SetConnectionSettings", "SetLobbyMessageEvent", "CreateCompoundAddress", "ConnectEx",
    "RegisterApplication", "UnregisterApplication", "WaitForConnectionSettings",
];

/// Arguments (including `this`) of each IDirectPlayLobby3A method.
pub const IDIRECTPLAYLOBBY3A_NARGS: &[u32] = &[3, 1, 1, 4, 7, 5, 5, 4, 4, 6, 5, 5, 4, 4, 5, 5, 3, 3, 2];

pub const DP_OK: u32 = 0;
const fn dphr(code: u32) -> u32 {
    0x8877_0000 | code
}
pub const DPERR_NOMESSAGES: u32 = dphr(190);
pub const DPERR_UNAVAILABLE: u32 = dphr(250);
pub const DPERR_NOSESSIONS: u32 = dphr(220);
/// HYPOTHESIS (MAKE_DPHRESULT(1070) from memory of dplay.h).
pub const DPERR_NOTLOBBIED: u32 = dphr(1070);

pub fn register(reg: &mut Registry) {
    reg.declare_all(DLL, EXPORTS);
    reg.alias("dplay.dll", DLL);
    declare_interface(reg, DLL, "IDirectPlay4A", IDIRECTPLAY4A);
    declare_interface(reg, DLL, "IDirectPlayLobby3A", IDIRECTPLAYLOBBY3A);
    reg.register(DLL, "DirectPlayCreate", direct_play_create);
    reg.register(DLL, "DirectPlayEnumerateA", ok2);
    reg.register(DLL, "DirectPlayLobbyCreateA", direct_play_lobby_create_a);

    for (i, name) in IDIRECTPLAY4A.iter().enumerate() {
        let h: s3_runtime::Handler = match *name {
            "QueryInterface" => qi_self,
            "AddRef" => Machine::com_addref,
            "Release" => Machine::com_release,
            // Relay-backed methods (dplay_net.rs).
            "EnumConnections" => net::enum_connections,
            "InitializeConnection" => net::initialize_connection,
            "GetCaps" => net::get_caps,
            "GetPlayerCaps" => net::get_player_caps,
            "EnumSessions" => net::enum_sessions,
            "Open" => net::open,
            "Close" => net::close,
            "EnumPlayers" => net::enum_players,
            "CreatePlayer" => net::create_player,
            "DestroyPlayer" => net::destroy_player,
            "Send" => net::send,
            "Receive" => net::receive,
            "GetMessageCount" => net::get_message_count,
            "GetSessionDesc" => net::get_session_desc,
            "SetSessionDesc" => net::set_session_desc,
            "GetPlayerData" => net::get_player_data,
            "SetPlayerData" => net::set_player_data,
            "GetPlayerName" => net::get_player_name,
            "SetPlayerName" => net::set_player_name,
            // No groups: succeed with no entries.
            "EnumGroups" | "EnumGroupPlayers" | "EnumGroupsInGroup" => by_nargs_ok(IDIRECTPLAY4A_NARGS[i]),
            "Initialize" => by_nargs_ok(IDIRECTPLAY4A_NARGS[i]),
            _ => by_nargs_unavailable(IDIRECTPLAY4A_NARGS[i]),
        };
        reg_method(reg, DLL, "IDirectPlay4A", name, h);
    }
    for (i, name) in IDIRECTPLAYLOBBY3A.iter().enumerate() {
        let n = IDIRECTPLAYLOBBY3A_NARGS[i];
        let h: s3_runtime::Handler = match *name {
            "QueryInterface" => qi_self,
            "AddRef" => Machine::com_addref,
            "Release" => Machine::com_release,
            "GetConnectionSettings" | "WaitForConnectionSettings" => by_nargs_notlobbied(n),
            "EnumAddress" | "EnumAddressTypes" | "EnumLocalApplications" => by_nargs_ok(n),
            _ => by_nargs_unavailable(n),
        };
        reg_method(reg, DLL, "IDirectPlayLobby3A", name, h);
    }
}

pub fn register_factories(m: &mut Machine) {
    m.add_service("dplay-pump", net::pump_service);
    m.com.register_class(CLSID_DIRECTPLAY, "DirectPlay", create_dplay);
    m.com.register_class(CLSID_DIRECTPLAYLOBBY, "DirectPlayLobby", create_lobby);
}

crate::stubs! {
    ok1 = (1, DP_OK); ok2 = (2, DP_OK); ok3 = (3, DP_OK); ok4 = (4, DP_OK); ok5 = (5, DP_OK);
    ok6 = (6, DP_OK); ok7 = (7, DP_OK);
    un1 = (1, DPERR_UNAVAILABLE); un2 = (2, DPERR_UNAVAILABLE); un3 = (3, DPERR_UNAVAILABLE);
    un4 = (4, DPERR_UNAVAILABLE); un5 = (5, DPERR_UNAVAILABLE); un6 = (6, DPERR_UNAVAILABLE);
    un7 = (7, DPERR_UNAVAILABLE); un10 = (10, DPERR_UNAVAILABLE);
    nl2 = (2, DPERR_NOTLOBBIED); nl4 = (4, DPERR_NOTLOBBIED);
}

fn by_nargs_ok(n: u32) -> s3_runtime::Handler {
    match n {
        1 => ok1,
        2 => ok2,
        3 => ok3,
        4 => ok4,
        5 => ok5,
        6 => ok6,
        7 => ok7,
        _ => panic!("dplay: no ok stub for {n} args"),
    }
}

fn by_nargs_unavailable(n: u32) -> s3_runtime::Handler {
    match n {
        1 => un1,
        2 => un2,
        3 => un3,
        4 => un4,
        5 => un5,
        6 => un6,
        7 => un7,
        10 => un10,
        _ => panic!("dplay: no stub for {n} args"),
    }
}

fn by_nargs_notlobbied(n: u32) -> s3_runtime::Handler {
    match n {
        2 => nl2,
        4 => nl4,
        _ => panic!("dplay: no notlobbied stub for {n} args"),
    }
}

fn qi_self(m: &mut Machine) -> ThunkResult {
    let (this, out) = (m.arg_u32(0), m.arg_u32(2));
    if out == 0 {
        return ret(E_POINTER, 3);
    }
    let n = m.mem.read_u32(this + 4) + 1;
    m.mem.write_u32(this + 4, n);
    m.mem.write_u32(out, this);
    ret(S_OK, 3)
}

fn new_dplay(m: &mut Machine) -> u32 {
    let vt = m.com_vtable(DLL, "IDirectPlay4A", IDIRECTPLAY4A);
    m.com_new_object(vt, 0x10)
}

fn new_lobby(m: &mut Machine) -> u32 {
    let vt = m.com_vtable(DLL, "IDirectPlayLobby3A", IDIRECTPLAYLOBBY3A);
    m.com_new_object(vt, 0x10)
}

/// Any IDirectPlay2/3/4 (A or W) IID gets the IDirectPlay4A-layout object (the vtables are prefixes of it).
fn create_dplay(m: &mut Machine, iid: Guid) -> Result<u32, u32> {
    m.log(LogLevel::Debug, &format!("dplay: CoCreateInstance(CLSID_DirectPlay, {iid})"));
    Ok(new_dplay(m))
}

fn create_lobby(m: &mut Machine, iid: Guid) -> Result<u32, u32> {
    m.log(LogLevel::Debug, &format!("dplay: CoCreateInstance(CLSID_DirectPlayLobby, {iid})"));
    Ok(new_lobby(m))
}

/// `DirectPlayCreate(lpGUIDSP, lplpDP, pUnk)`: no service providers -> DPERR_UNAVAILABLE.
fn direct_play_create(m: &mut Machine) -> ThunkResult {
    let out = m.arg_u32(1);
    put_u32(m, out, 0);
    ret(DPERR_UNAVAILABLE, 3)
}

/// `DirectPlayLobbyCreateA(lpGUIDDSP, lplpDPL, lpUnk, lpData, dwDataSize)`.
fn direct_play_lobby_create_a(m: &mut Machine) -> ThunkResult {
    let out = m.arg_u32(1);
    if out == 0 {
        return ret(E_INVALIDARG, 5);
    }
    let p = new_lobby(m);
    m.mem.write_u32(out, p);
    ret(DP_OK, 5)
}
