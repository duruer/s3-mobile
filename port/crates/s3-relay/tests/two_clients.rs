//! Integration tests: real relay on 127.0.0.1 + real `s3_net::client::NetClient`s (each with its own thread),
//! exercising the DirectPlay-like flow S3.EXE performs (see docs/porting/multiplayer.md).

use std::time::{Duration, Instant};

use s3_net::client::{ClientConfig, Event, LinkState, NetClient, NetError};
use s3_net::dplay::{DPSEND_GUARANTEED, DPSESSION_KEEPALIVE, DPSESSION_MIGRATEHOST};
use s3_net::protocol::{ErrorCode, SessionDesc};
use s3_net::S3_APP_GUID;
use s3_relay::{RelayConfig, RelayHandle};

struct Relay {
    rt: tokio::runtime::Runtime,
    handle: Option<RelayHandle>,
}

impl Relay {
    fn start(cfg: RelayConfig) -> Relay {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let handle = rt.block_on(s3_relay::start(cfg)).unwrap();
        Relay { rt, handle: Some(handle) }
    }
    fn url(&self) -> String {
        self.handle.as_ref().unwrap().url()
    }
    fn stats(&self) -> s3_relay::Stats {
        self.rt.block_on(self.handle.as_ref().unwrap().stats())
    }
    fn kick(&self, cid: u32) {
        self.handle.as_ref().unwrap().kick_connection(cid);
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            h.shutdown();
        }
    }
}

fn client(relay: &Relay, name: &str) -> NetClient {
    NetClient::connect(ClientConfig::new(relay.url(), name)).unwrap()
}

/// The session description S3.EXE builds at 0x46ff80 (flags MIGRATEHOST|KEEPALIVE, app GUID, user dwords).
fn s3_desc(name: &str) -> SessionDesc {
    SessionDesc {
        flags: DPSESSION_MIGRATEHOST | DPSESSION_KEEPALIVE,
        application: S3_APP_GUID,
        max_players: 8,
        name: name.into(),
        user: [0, 0, 0xa0, 0],
        ..Default::default()
    }
}

/// Wait for an event matching `pred`, skipping others (returned so tests can inspect them).
fn wait_for(c: &NetClient, what: &str, mut pred: impl FnMut(&Event) -> bool) -> Event {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Some(e) = c.wait_event(Duration::from_millis(100)) {
            if pred(&e) {
                return e;
            }
        }
    }
    panic!("timed out waiting for {what}");
}

#[test]
fn host_join_exchange_and_leave() {
    let relay = Relay::start(RelayConfig::default());
    let host = client(&relay, "host");
    let guest = client(&relay, "guest");
    assert_ne!(host.client_id(), guest.client_id());

    // Host: DPOPEN_CREATE, then CreatePlayer.
    let joined = host.create_session(s3_desc("Römer 2v2")).unwrap();
    assert_eq!(joined.host, host.client_id());
    assert_ne!(joined.desc.instance, [0; 16]);
    let hp = host.create_player("Hans", "", b"host-data", 0).unwrap();
    assert!(hp > 1);
    assert!(host.snapshot().is_host());

    // Guest: EnumSessions finds it (password hidden), joins, sees the host's player.
    let list = guest.list_sessions(S3_APP_GUID).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "Römer 2v2");
    assert_eq!(list[0].current_players, 1);
    assert!(guest.list_sessions([9; 16]).unwrap().is_empty(), "other application GUIDs are filtered");
    let gj = guest.join_session(list[0].instance, "").unwrap();
    assert_eq!(gj.host, host.client_id());
    assert_eq!(gj.players.len(), 1);
    assert_eq!(gj.players[0].dpid, hp);
    assert_eq!(gj.players[0].data, b"host-data");
    let gp = guest.create_player("Grete", "", &[], 0).unwrap();
    assert_ne!(gp, hp);

    // Host gets DPSYS_CREATEPLAYERORGROUP.
    match wait_for(&host, "PlayerAdded", |e| matches!(e, Event::PlayerAdded(_))) {
        Event::PlayerAdded(p) => {
            assert_eq!(p.dpid, gp);
            assert_eq!(p.short_name, "Grete");
            assert_eq!(p.owner, guest.client_id());
        }
        _ => unreachable!(),
    }

    // Ordered data both ways, broadcast and directed, guaranteed and not.
    for i in 0..200u32 {
        let flags = if i % 2 == 0 { DPSEND_GUARANTEED } else { 0 };
        guest.send(gp, if i % 3 == 0 { 0 } else { hp }, flags, &i.to_le_bytes()).unwrap();
    }
    for i in 0..200u32 {
        match wait_for(&host, "data", |e| matches!(e, Event::Message { .. })) {
            Event::Message { from, to, payload, .. } => {
                assert_eq!(from, gp);
                assert_eq!(to, hp, "broadcast is delivered addressed to the receiving player");
                assert_eq!(payload, i.to_le_bytes(), "order preserved");
            }
            _ => unreachable!(),
        }
    }
    host.send(hp, gp, DPSEND_GUARANTEED, &[0x19; 315]).unwrap(); // GameSetup-sized message
    match wait_for(&guest, "data", |e| matches!(e, Event::Message { .. })) {
        Event::Message { from, payload, .. } => {
            assert_eq!(from, hp);
            assert_eq!(payload.len(), 315);
        }
        _ => unreachable!(),
    }

    // Spoofing another player's DPID is ignored: the next message the host sees is the sentinel.
    guest.send(hp, hp, DPSEND_GUARANTEED, b"spoof").unwrap();
    guest.send(gp, hp, DPSEND_GUARANTEED, b"sentinel").unwrap();
    assert!(matches!(wait_for(&host, "sentinel", |e| matches!(e, Event::Message { .. })),
        Event::Message { payload, .. } if payload == b"sentinel"));
    // Player data / name / session desc changes reach the other side.
    guest.set_player_data(gp, b"slot=2").unwrap();
    assert!(matches!(wait_for(&host, "PlayerData", |e| matches!(e, Event::PlayerData { .. })),
        Event::PlayerData { dpid, data } if dpid == gp && data == b"slot=2"));
    assert_eq!(guest.set_player_data(hp, b"x").unwrap_err().code(), ErrorCode::NotOwner);
    let mut d = joined.desc.clone();
    d.user[3] = 7;
    assert_eq!(guest.set_session_desc(d.clone()).unwrap_err().code(), ErrorCode::NotOwner);
    host.set_session_desc(d).unwrap();
    assert!(matches!(wait_for(&guest, "SessionDesc", |e| matches!(e, Event::SessionDesc(_))),
        Event::SessionDesc(sd) if sd.user[3] == 7));

    // Desync report is relayed to the other members.
    guest.report_desync(1234, 0xdead_beef, "Desync Settler!").unwrap();
    assert!(matches!(wait_for(&host, "Desync", |e| matches!(e, Event::Desync { .. })),
        Event::Desync { step: 1234, checksum: 0xdead_beef, .. }));

    // Guest leaves: host sees DPSYS_DESTROYPLAYERORGROUP.
    guest.leave_session().unwrap();
    assert!(guest.snapshot().session.is_none());
    assert!(matches!(wait_for(&host, "PlayerRemoved", |e| matches!(e, Event::PlayerRemoved(_))),
        Event::PlayerRemoved(p) if p.dpid == gp && p.short_name == "Grete"));
    let st = relay.stats();
    assert_eq!(st.desync_reports, 1);
    assert!(st.delivered >= 201);
    guest.close();
    host.close();
}

#[test]
fn host_migration_when_host_leaves() {
    let relay = Relay::start(RelayConfig::default());
    let a = client(&relay, "a");
    let b = client(&relay, "b");
    let c = client(&relay, "c");
    let s = a.create_session(s3_desc("mig")).unwrap();
    let pa = a.create_player("A", "", &[], 0).unwrap();
    b.join_session(s.desc.instance, "").unwrap();
    let pb = b.create_player("B", "", &[], 0).unwrap();
    c.join_session(s.desc.instance, "").unwrap();
    let _pc = c.create_player("C", "", &[], 0).unwrap();
    a.close(); // Bye: immediate removal
    // b joined first -> new host (DPSYS_HOST), c only sees the player leave.
    wait_for(&b, "BecameHost", |e| matches!(e, Event::BecameHost));
    assert!(b.snapshot().is_host());
    assert!(matches!(wait_for(&c, "PlayerRemoved", |e| matches!(e, Event::PlayerRemoved(_))), Event::PlayerRemoved(p) if p.dpid == pa));
    assert!(!c.snapshot().is_host());
    assert_eq!(relay.stats().host_migrations, 1);
    // Without MIGRATEHOST the session is lost for everyone when the host goes.
    let mut d = s3_desc("nomig");
    d.flags = DPSESSION_KEEPALIVE;
    b.leave_session().unwrap();
    let _ = pb;
    let s2 = b.create_session(d).unwrap();
    c.leave_session().unwrap();
    c.join_session(s2.desc.instance, "").unwrap();
    b.close();
    assert!(matches!(wait_for(&c, "SessionLost", |e| matches!(e, Event::SessionLost(_))), Event::SessionLost(ErrorCode::Disconnected)));
    assert!(c.snapshot().session.is_none());
}

#[test]
fn join_rules() {
    let relay = Relay::start(RelayConfig::default());
    let a = client(&relay, "a");
    let b = client(&relay, "b");
    let mut d = s3_desc("locked");
    d.password = "geheim".into();
    d.max_players = 1;
    let s = a.create_session(d).unwrap();
    let list = b.list_sessions(S3_APP_GUID).unwrap();
    assert_eq!(list[0].password, "", "password never leaves the relay");
    assert_ne!(list[0].flags & s3_net::dplay::DPSESSION_PASSWORDREQUIRED, 0);
    assert_eq!(b.join_session(s.desc.instance, "falsch").unwrap_err().code(), ErrorCode::WrongPassword);
    assert_eq!(b.join_session(s.desc.instance, "geheim").unwrap_err().code(), ErrorCode::SessionFull);
    assert_eq!(b.join_session([1; 16], "").unwrap_err().code(), ErrorCode::NoSuchSession);
    assert_eq!(a.create_session(s3_desc("x")).unwrap_err().code(), ErrorCode::AlreadyInSession);
    assert_eq!(b.leave_session().unwrap_err().code(), ErrorCode::NotInSession);

    // A different S3.EXE neither sees nor joins the session.
    let mut cfg = ClientConfig::new(relay.url(), "other-exe");
    cfg.exe_sha256 = [0x11; 32];
    let other = NetClient::connect(cfg).unwrap();
    assert!(other.list_sessions(S3_APP_GUID).unwrap().is_empty());
    assert_eq!(other.join_session(s.desc.instance, "geheim").unwrap_err().code(), ErrorCode::ExeMismatch);
}

#[test]
fn relay_rejects_unknown_exe_when_restricted() {
    let relay = Relay::start(RelayConfig { allowed_exes: vec![s3_net::GOG_EXE_SHA256], ..Default::default() });
    let _ok = client(&relay, "gog");
    let mut cfg = ClientConfig::new(relay.url(), "bad");
    cfg.exe_sha256 = [0x22; 32];
    match NetClient::connect(cfg) {
        Err(NetError::Rejected(ErrorCode::ExeMismatch, _)) => {}
        Err(e) => panic!("unexpected error {e:?}"),
        Ok(_) => panic!("should be rejected"),
    }
}

#[test]
fn resume_after_connection_drop_loses_nothing() {
    let relay = Relay::start(RelayConfig::default());
    let host = client(&relay, "host");
    let guest = client(&relay, "guest");
    let s = host.create_session(s3_desc("resume")).unwrap();
    let hp = host.create_player("H", "", &[], 0).unwrap();
    guest.join_session(s.desc.instance, "").unwrap();
    let gp = guest.create_player("G", "", &[], 0).unwrap();
    wait_for(&host, "PlayerAdded", |e| matches!(e, Event::PlayerAdded(_)));

    // Cut the guest's TCP connection; meanwhile the host keeps sending.
    relay.kick(guest.client_id());
    for i in 0..50u32 {
        host.send(hp, gp, DPSEND_GUARANTEED, &i.to_le_bytes()).unwrap();
    }
    wait_for(&guest, "Resumed", |e| matches!(e, Event::Link(LinkState::Resumed)));
    for i in 0..50u32 {
        match wait_for(&guest, "data", |e| matches!(e, Event::Message { .. })) {
            Event::Message { payload, .. } => assert_eq!(payload, i.to_le_bytes()),
            _ => unreachable!(),
        }
    }
    // The guest still owns its player and can talk; the host never saw it leave.
    guest.send(gp, hp, DPSEND_GUARANTEED, b"back").unwrap();
    match wait_for(&host, "data", |e| matches!(e, Event::Message { .. } | Event::PlayerRemoved(_))) {
        Event::Message { payload, .. } => assert_eq!(payload, b"back"),
        e => panic!("unexpected {e:?}"),
    }
    assert_eq!(relay.stats().resumes, 1);
}

#[test]
fn slot_expires_after_grace() {
    let relay = Relay::start(RelayConfig { resume_grace: Duration::from_millis(300), ..Default::default() });
    let host = client(&relay, "host");
    let mut cfg = ClientConfig::new(relay.url(), "guest");
    cfg.resume_window = Duration::ZERO; // never resume
    let guest = NetClient::connect(cfg).unwrap();
    let s = host.create_session(s3_desc("grace")).unwrap();
    host.create_player("H", "", &[], 0).unwrap();
    guest.join_session(s.desc.instance, "").unwrap();
    let gp = guest.create_player("G", "", &[], 0).unwrap();
    relay.kick(guest.client_id());
    assert!(matches!(wait_for(&guest, "SessionLost", |e| matches!(e, Event::SessionLost(_))), Event::SessionLost(_)));
    // After the grace period the relay destroys the guest's player for everyone else.
    assert!(matches!(wait_for(&host, "PlayerRemoved", |e| matches!(e, Event::PlayerRemoved(_))), Event::PlayerRemoved(p) if p.dpid == gp));
}
