//! `wsock32.dll` HLE. WinSock 1.1 stubs: networking is not available in the port. `WSAStartup` fails with
//! WSASYSNOTREADY so the LAN/Internet code stays off (HYPOTHESIS: single player is unaffected); every socket call
//! fails with WSAENETDOWN. Pure helpers (htons, inet_addr, ...) work.
//!
//! `NAMES` lists every function we know the game can reach; all are declared (calling one without a handler
//! stops the machine with `UNIMPLEMENTED wsock32.dll!Name`).

use s3_runtime::{LogLevel, Machine, Registry, ThunkResult};

/// DLL name as registered.
pub const DLL: &str = "wsock32.dll";

/// Known exports.
pub const NAMES: &[&str] = &[
    "WSAStartup", "WSACleanup", "WSAGetLastError", "socket", "closesocket", "bind", "connect",
    "listen", "accept", "send", "recv", "sendto", "recvfrom", "select", "gethostname",
    "gethostbyname", "inet_addr", "inet_ntoa", "htons", "ntohs", "htonl", "ntohl", "ioctlsocket",
    "setsockopt", "getsockopt", "getsockname", "shutdown", "WSASetLastError", "__WSAFDIsSet",
];

const WSASYSNOTREADY: u32 = 10091;
const WSAENETDOWN: u32 = 10050;
const SOCKET_ERROR: u32 = 0xFFFF_FFFF;

/// Declare all names and register implemented handlers.
pub fn register(reg: &mut Registry) {
    reg.declare_all(DLL, NAMES);
    let h: &[(&str, s3_runtime::Handler)] = &[
        ("WSAStartup", wsa_startup),
        ("WSACleanup", ret0_0),
        ("WSAGetLastError", wsa_get_last_error),
        ("WSASetLastError", ret0_1),
        ("socket", fail_3),
        ("closesocket", fail_1),
        ("bind", fail_3),
        ("connect", fail_3),
        ("listen", fail_2),
        ("accept", fail_3),
        ("send", fail_4),
        ("recv", fail_4),
        ("sendto", fail_6),
        ("recvfrom", fail_6),
        ("select", fail_5),
        ("gethostname", gethostname),
        ("gethostbyname", null_1),
        ("inet_addr", inet_addr),
        ("inet_ntoa", inet_ntoa),
        ("htons", htons),
        ("ntohs", htons),
        ("htonl", htonl),
        ("ntohl", htonl),
        ("ioctlsocket", fail_3),
        ("setsockopt", fail_5),
        ("getsockopt", fail_5),
        ("getsockname", fail_3),
        ("shutdown", fail_2),
        ("__WSAFDIsSet", ret0_2),
    ];
    for (n, f) in h {
        reg.register(DLL, n, *f);
    }
}

fn ret0_0(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(0, 0)
}
fn ret0_1(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(0, 1)
}
fn ret0_2(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(0, 2)
}
fn null_1(m: &mut Machine) -> ThunkResult {
    m.set_last_error(WSAENETDOWN);
    ThunkResult::stdcall(0, 1)
}

macro_rules! fail {
    ($name:ident, $n:expr) => {
        fn $name(m: &mut Machine) -> ThunkResult {
            m.set_last_error(WSAENETDOWN);
            ThunkResult::stdcall(SOCKET_ERROR, $n)
        }
    };
}
fail!(fail_1, 1);
fail!(fail_2, 2);
fail!(fail_3, 3);
fail!(fail_4, 4);
fail!(fail_5, 5);
fail!(fail_6, 6);

/// WSAStartup(wVersionRequested, lpWSAData): fails, networking is unavailable.
fn wsa_startup(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(1);
    if p != 0 {
        m.mem.fill(p, 400, 0);
        m.mem.write_u16(p, 0x0101);
        m.mem.write_u16(p + 2, 0x0101);
    }
    m.log(LogLevel::Info, "WSAStartup: networking not available in the port (WSASYSNOTREADY)");
    ThunkResult::stdcall(WSASYSNOTREADY, 2)
}

fn wsa_get_last_error(m: &mut Machine) -> ThunkResult {
    let e = m.last_error();
    ThunkResult::stdcall(if e == 0 { WSAENETDOWN } else { e }, 0)
}

fn gethostname(m: &mut Machine) -> ThunkResult {
    let (buf, len) = (m.arg_u32(0), m.arg_u32(1));
    if buf == 0 || len < 7 {
        return ThunkResult::stdcall(SOCKET_ERROR, 2);
    }
    m.mem.write_cstr(buf, "s3port");
    ThunkResult::stdcall(0, 2)
}

/// Parse a dotted IPv4 address into network byte order (as stored in memory).
pub fn parse_inet_addr(s: &str) -> Option<u32> {
    let parts: Vec<u8> = s.trim().split('.').map(|p| p.parse().ok()).collect::<Option<Vec<_>>>()?;
    if parts.len() != 4 {
        return None;
    }
    Some(u32::from_le_bytes([parts[0], parts[1], parts[2], parts[3]]))
}

fn inet_addr(m: &mut Machine) -> ThunkResult {
    let s = m.arg_str(0).unwrap_or_default();
    ThunkResult::stdcall(parse_inet_addr(&s).unwrap_or(0xFFFF_FFFF), 1)
}

#[derive(Default)]
struct NtoaBuf(u32);

fn inet_ntoa(m: &mut Machine) -> ThunkResult {
    let a = m.arg_u32(0).to_le_bytes();
    let st = m.state::<NtoaBuf>();
    let mut b = st.borrow_mut();
    if b.0 == 0 {
        b.0 = m.mem.alloc(16);
    }
    let p = b.0;
    drop(b);
    m.mem.write_cstr(p, &format!("{}.{}.{}.{}", a[0], a[1], a[2], a[3]));
    ThunkResult::stdcall(p, 1)
}

fn htons(m: &mut Machine) -> ThunkResult {
    let v = m.arg_u32(0) as u16;
    ThunkResult::stdcall(v.swap_bytes() as u32, 1)
}

fn htonl(m: &mut Machine) -> ThunkResult {
    let v = m.arg_u32(0);
    ThunkResult::stdcall(v.swap_bytes(), 1)
}

#[cfg(test)]
mod tests {
    #[test]
    fn inet() {
        assert_eq!(super::parse_inet_addr("127.0.0.1"), Some(0x0100_007F));
        assert_eq!(super::parse_inet_addr("x"), None);
    }
}
