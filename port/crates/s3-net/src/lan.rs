//! LAN discovery of hosts running an embedded relay (docs/porting/multiplayer.md section 8).
//!
//! One background thread per process:
//! * the **listen socket** on [`LAN_PORT`] (SO_REUSEADDR, so several instances on one PC can share it) receives
//!   queries and beacons;
//! * one **interface socket** per local IPv4 address (ephemeral port, bound to that address, so a broadcast leaves
//!   through every interface, not only the default route) broadcasts a query every second to 255.255.255.255 and
//!   the interface's subnet broadcast; hosts answer with a unicast beacon to that socket. Unicast answers to an
//!   outgoing broadcast pass the Windows firewall by default, so a PC finds a phone host without an inbound rule;
//! * while hosting, beacons go out every second from every interface socket (the source address tells the
//!   receiver where the relay is) and to loopback (other instances on this machine).
//!
//! Packet: `"S3PLAN1"` kind:u8 (1 = query, 2 = beacon) nonce:u64le exe_tag:8 relay_port:u16le name_len:u8 name.
//! `nonce` identifies the sending process (own packets are ignored); `exe_tag` = first 8 bytes of the exe sha256
//! (hosts with another exe are ignored: lockstep needs identical code).

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// UDP port of the discovery protocol.
pub const LAN_PORT: u16 = 47777;
const MAGIC: &[u8; 7] = b"S3PLAN1";
const KIND_QUERY: u8 = 1;
const KIND_BEACON: u8 = 2;
/// A host not heard from for this long is dropped from the list.
const EXPIRE: Duration = Duration::from_secs(6);
const PERIOD: Duration = Duration::from_millis(1000);
/// Interfaces are re-read this often (Wi-Fi reconnects, new DHCP address).
const IFACE_REFRESH: Duration = Duration::from_secs(10);

/// A host found on the LAN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanHost {
    /// Candidate addresses of its relay (`ws://addr/`), best first (a host with several interfaces, e.g. Wi-Fi plus
    /// a virtual adapter, is heard on each of them).
    pub relays: Vec<SocketAddr>,
    /// Session name it announced.
    pub name: String,
    pub nonce: u64,
}

impl LanHost {
    pub fn url(addr: &SocketAddr) -> String {
        format!("ws://{addr}/")
    }
}

#[derive(Default)]
struct Inner {
    hosting: Option<(u16, String)>,
    /// nonce -> (name, address -> last heard)
    hosts: HashMap<u64, (String, HashMap<SocketAddr, Instant>)>,
}

/// Discovery service (one per process, see [`Discovery::global`]).
pub struct Discovery {
    nonce: u64,
    exe_tag: [u8; 8],
    inner: Mutex<Inner>,
    /// Local IPv4 addresses (for ranking candidate host addresses).
    local_ips: Mutex<Vec<Ipv4Addr>>,
}

fn encode(kind: u8, nonce: u64, exe_tag: &[u8; 8], port: u16, name: &str) -> Vec<u8> {
    let mut v = MAGIC.to_vec();
    v.push(kind);
    v.extend_from_slice(&nonce.to_le_bytes());
    v.extend_from_slice(exe_tag);
    v.extend_from_slice(&port.to_le_bytes());
    let n = &name.as_bytes()[..name.len().min(64)];
    v.push(n.len() as u8);
    v.extend_from_slice(n);
    v
}

/// (kind, nonce, exe_tag, port, name)
fn decode(b: &[u8]) -> Option<(u8, u64, [u8; 8], u16, String)> {
    if b.len() < 7 + 1 + 8 + 8 + 2 + 1 || &b[..7] != MAGIC {
        return None;
    }
    let kind = b[7];
    let nonce = u64::from_le_bytes(b[8..16].try_into().ok()?);
    let tag: [u8; 8] = b[16..24].try_into().ok()?;
    let port = u16::from_le_bytes([b[24], b[25]]);
    let n = b[26] as usize;
    let name = String::from_utf8_lossy(b.get(27..27 + n)?).into_owned();
    Some((kind, nonce, tag, port, name))
}

fn udp_socket(ip: Ipv4Addr, port: u16, reuse: bool) -> std::io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    if reuse {
        s.set_reuse_address(true)?;
    }
    s.set_broadcast(true)?;
    s.bind(&SocketAddr::V4(SocketAddrV4::new(ip, port)).into())?;
    let u: UdpSocket = s.into();
    u.set_nonblocking(true)?;
    Ok(u)
}

/// Local IPv4 interface addresses with their broadcast address (if known), loopback excluded.
#[cfg(unix)]
pub fn interfaces() -> Vec<(Ipv4Addr, Option<Ipv4Addr>)> {
    let mut out = Vec::new();
    unsafe {
        let mut ifa: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifa) != 0 {
            return out;
        }
        let mut p = ifa;
        while !p.is_null() {
            let i = &*p;
            let up = i.ifa_flags & libc::IFF_UP as libc::c_uint != 0;
            if up && !i.ifa_addr.is_null() && (*i.ifa_addr).sa_family as i32 == libc::AF_INET {
                let a = &*(i.ifa_addr as *const libc::sockaddr_in);
                let ip = Ipv4Addr::from(u32::from_be(a.sin_addr.s_addr));
                let mut bc = None;
                if !i.ifa_netmask.is_null() {
                    let nm = &*(i.ifa_netmask as *const libc::sockaddr_in);
                    let mask = u32::from_be(nm.sin_addr.s_addr);
                    bc = Some(Ipv4Addr::from(u32::from(ip) | !mask));
                }
                if !ip.is_loopback() {
                    out.push((ip, bc));
                }
            }
            p = i.ifa_next;
        }
        libc::freeifaddrs(ifa);
    }
    out
}

/// Local IPv4 interface addresses (Windows: the host name's addresses, std only); the broadcast address is guessed
/// as /24 (a wrong guess only sends one useless datagram: 255.255.255.255 from the bound address is the main path).
#[cfg(not(unix))]
pub fn interfaces() -> Vec<(Ipv4Addr, Option<Ipv4Addr>)> {
    use std::net::ToSocketAddrs;
    let mut out: Vec<(Ipv4Addr, Option<Ipv4Addr>)> = Vec::new();
    let host = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "localhost".into());
    let mut add = |ip: Ipv4Addr| {
        if !ip.is_loopback() && !ip.is_unspecified() && !ip.is_link_local() && !out.iter().any(|(a, _)| *a == ip) {
            let o = ip.octets();
            out.push((ip, Some(Ipv4Addr::new(o[0], o[1], o[2], 255))));
        }
    };
    if let Ok(it) = (host.as_str(), 0u16).to_socket_addrs() {
        for a in it {
            if let IpAddr::V4(ip) = a.ip() {
                add(ip);
            }
        }
    }
    if let Some(ip) = primary_ipv4() {
        add(ip);
    }
    out
}

/// Our IPv4 address on the default route (no packet is sent: UDP connect only picks the interface).
pub fn primary_ipv4() -> Option<Ipv4Addr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    match s.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Some(ip),
        _ => None,
    }
}

/// Number of leading bits `a` shares with the closest local address (ranks candidate host addresses).
fn affinity(a: &SocketAddr, local: &[Ipv4Addr]) -> u32 {
    match a.ip() {
        IpAddr::V4(ip) if ip.is_loopback() => 1,
        IpAddr::V4(ip) => 2 + local.iter().map(|l| (u32::from(ip) ^ u32::from(*l)).leading_zeros()).max().unwrap_or(0),
        _ => 0,
    }
}

struct Iface {
    ip: Ipv4Addr,
    bcast: Option<Ipv4Addr>,
    sock: UdpSocket,
}

static START_HOOK: OnceLock<fn()> = OnceLock::new();

/// Called once before discovery opens its sockets (Android: acquire the Wi-Fi multicast lock so broadcasts from
/// other devices are not filtered).
pub fn set_start_hook(f: fn()) {
    let _ = START_HOOK.set(f);
}

impl Discovery {
    /// The process-wide discovery service (started on first use).
    pub fn global(exe_sha256: &[u8; 32]) -> Arc<Discovery> {
        static G: Mutex<Option<Arc<Discovery>>> = Mutex::new(None);
        let mut g = G.lock().unwrap();
        if let Some(d) = g.as_ref() {
            return d.clone();
        }
        let d = Discovery::start(exe_sha256[..8].try_into().unwrap());
        *g = Some(d.clone());
        d
    }

    /// Start a discovery thread (tests may run several in one process).
    pub fn start(exe_tag: [u8; 8]) -> Arc<Discovery> {
        if let Some(h) = START_HOOK.get() {
            h();
        }
        let listen = match udp_socket(Ipv4Addr::UNSPECIFIED, LAN_PORT, true) {
            Ok(s) => Some(s),
            Err(e) => {
                log::warn!("lan: cannot listen on udp {LAN_PORT}: {e} (queries still work)");
                None
            }
        };
        let any = udp_socket(Ipv4Addr::UNSPECIFIED, 0, false).map_err(|e| log::warn!("lan: no query socket: {e}")).ok();
        let d = Arc::new(Discovery {
            nonce: u64::from_le_bytes(crate::random_bytes::<8>()),
            exe_tag,
            inner: Mutex::new(Inner::default()),
            local_ips: Mutex::new(Vec::new()),
        });
        let d2 = d.clone();
        let _ = std::thread::Builder::new().name("s3-lan".into()).spawn(move || d2.run(listen, any));
        log::info!("lan: discovery started (udp {LAN_PORT}, nonce {:016x})", d.nonce);
        d
    }

    /// Announce (`Some((relay_port, session_name))`) or stop announcing (`None`).
    pub fn set_hosting(&self, h: Option<(u16, String)>) {
        log::info!("lan: hosting {h:?}");
        self.inner.lock().unwrap().hosting = h;
    }

    /// Hosts heard from recently (not ourselves), candidate addresses best first.
    pub fn hosts(&self) -> Vec<LanHost> {
        let local = self.local_ips.lock().unwrap().clone();
        let mut g = self.inner.lock().unwrap();
        for (_, addrs) in g.hosts.values_mut() {
            addrs.retain(|_, t| t.elapsed() < EXPIRE);
        }
        g.hosts.retain(|_, (_, a)| !a.is_empty());
        let mut v: Vec<LanHost> = g
            .hosts
            .iter()
            .map(|(n, (name, addrs))| {
                let mut relays: Vec<SocketAddr> = addrs.keys().copied().collect();
                relays.sort_by_key(|a| (std::cmp::Reverse(affinity(a, &local)), *a));
                LanHost { relays, name: name.clone(), nonce: *n }
            })
            .collect();
        v.sort_by_key(|h| h.nonce);
        v
    }

    /// Wait up to `timeout` until at least one host is known; returns the list.
    pub fn wait_hosts(&self, timeout: Duration) -> Vec<LanHost> {
        let end = Instant::now() + timeout;
        loop {
            let h = self.hosts();
            if !h.is_empty() || Instant::now() >= end {
                return h;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn open_ifaces(&self, old: Vec<Iface>) -> Vec<Iface> {
        let now = interfaces();
        if now.len() == old.len() && now.iter().zip(&old).all(|((ip, bc), i)| *ip == i.ip && *bc == i.bcast) {
            return old;
        }
        let v: Vec<Iface> = now
            .into_iter()
            .filter_map(|(ip, bcast)| match udp_socket(ip, 0, false) {
                Ok(sock) => Some(Iface { ip, bcast, sock }),
                Err(e) => {
                    log::warn!("lan: cannot bind {ip}: {e}");
                    None
                }
            })
            .collect();
        log::info!("lan: interfaces {:?}", v.iter().map(|i| (i.ip, i.bcast)).collect::<Vec<_>>());
        *self.local_ips.lock().unwrap() = v.iter().map(|i| i.ip).collect();
        v
    }

    fn run(self: Arc<Self>, listen: Option<UdpSocket>, any: Option<UdpSocket>) {
        let mut last_tx: Option<Instant> = None;
        let mut last_if: Option<Instant> = None;
        let mut ifaces: Vec<Iface> = Vec::new();
        let mut buf = [0u8; 512];
        let loopback = SocketAddr::from((Ipv4Addr::LOCALHOST, LAN_PORT));
        let limited = SocketAddr::from((Ipv4Addr::BROADCAST, LAN_PORT));
        loop {
            if last_if.map_or(true, |t| t.elapsed() >= IFACE_REFRESH) {
                last_if = Some(Instant::now());
                ifaces = self.open_ifaces(std::mem::take(&mut ifaces));
            }
            if last_tx.map_or(true, |t| t.elapsed() >= PERIOD) {
                last_tx = Some(Instant::now());
                let hosting = self.inner.lock().unwrap().hosting.clone();
                let pkt = match &hosting {
                    Some((port, name)) => encode(KIND_BEACON, self.nonce, &self.exe_tag, *port, name),
                    None => encode(KIND_QUERY, self.nonce, &self.exe_tag, 0, ""),
                };
                for i in &ifaces {
                    let _ = i.sock.send_to(&pkt, limited);
                    if let Some(b) = i.bcast {
                        let _ = i.sock.send_to(&pkt, SocketAddr::from((b, LAN_PORT)));
                    }
                }
                if let Some(a) = &any {
                    let _ = a.send_to(&pkt, loopback);
                    if ifaces.is_empty() {
                        let _ = a.send_to(&pkt, limited);
                    }
                }
            }
            let mut got = false;
            let socks = listen.iter().chain(any.iter()).chain(ifaces.iter().map(|i| &i.sock));
            for s in socks {
                while let Ok((n, from)) = s.recv_from(&mut buf) {
                    got = true;
                    self.handle(s, &buf[..n], from);
                }
            }
            if !got {
                std::thread::sleep(Duration::from_millis(30));
            }
        }
    }

    fn handle(&self, sock: &UdpSocket, pkt: &[u8], from: SocketAddr) {
        let Some((kind, nonce, tag, port, name)) = decode(pkt) else { return };
        if nonce == self.nonce || tag != self.exe_tag {
            return;
        }
        match kind {
            KIND_QUERY => {
                let hosting = self.inner.lock().unwrap().hosting.clone();
                if let Some((p, n)) = hosting {
                    let _ = sock.send_to(&encode(KIND_BEACON, self.nonce, &self.exe_tag, p, &n), from);
                }
            }
            KIND_BEACON if port != 0 => {
                let addr = SocketAddr::new(from.ip(), port);
                let mut g = self.inner.lock().unwrap();
                let e = g.hosts.entry(nonce).or_insert_with(|| (name.clone(), HashMap::new()));
                if !e.1.contains_key(&addr) {
                    log::info!("lan: host '{name}' relay {addr}");
                }
                e.0 = name;
                e.1.insert(addr, Instant::now());
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_roundtrip() {
        let p = encode(KIND_BEACON, 0x1122, b"abcdefgh", 7777, "Player's game");
        assert_eq!(decode(&p), Some((KIND_BEACON, 0x1122, *b"abcdefgh", 7777, "Player's game".into())));
        assert_eq!(decode(&p[..10]), None);
    }

    #[test]
    fn affinity_prefers_same_subnet() {
        let local = [Ipv4Addr::new(192, 168, 1, 51)];
        let wifi: SocketAddr = "192.168.1.50:7777".parse().unwrap();
        let vnet: SocketAddr = "172.16.0.1:7777".parse().unwrap();
        let lo: SocketAddr = "127.0.0.1:7777".parse().unwrap();
        assert!(affinity(&wifi, &local) > affinity(&vnet, &local));
        assert!(affinity(&vnet, &local) > affinity(&lo, &local));
    }

    #[test]
    fn two_instances_find_each_other() {
        let a = Discovery::start(*b"testtag1");
        let b = Discovery::start(*b"testtag1");
        let other = Discovery::start(*b"othertag");
        other.set_hosting(Some((1, "other exe".into())));
        a.set_hosting(Some((12345, "game A".into())));
        let h = b.wait_hosts(Duration::from_secs(5));
        assert_eq!(h.len(), 1, "{h:?}");
        assert!(h[0].relays.iter().all(|r| r.port() == 12345));
        assert_eq!(h[0].name, "game A");
        assert!(a.hosts().is_empty());
        a.set_hosting(None);
    }
}
