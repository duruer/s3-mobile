//! Online multiplayer for the Settlers 3 port (workstream W5).
//!
//! * [`protocol`]: relay wire protocol v1 (frames, client/server messages), see docs/porting/multiplayer.md.
//! * [`link`]: sequence/ack bookkeeping that lets a dropped connection resume without loss.
//! * [`client`]: synchronous client handle for the DirectPlay HLE (a background thread runs the async socket).
//! * [`dplay`]: DirectPlay 6 constants (error table observed in S3.EXE) and system-message layouts.
//!
//! The server lives in the `s3-relay` crate.

pub mod client;
pub mod dplay;
pub mod lan;
pub mod link;
pub mod protocol;
pub mod wire;

/// sha256 of the GOG S3.EXE (the build all our hooks are keyed to). Lockstep play requires every peer to run
/// the same exe; the relay only lets clients with the same hash into one session.
pub const GOG_EXE_SHA256: protocol::Sha256 = hex32("00a925cfdcfc3349b5d0f3b8a4c312f01b813e363b4464f6f3d6de17804d950b");

/// Application GUID S3.EXE puts into its DPSESSIONDESC2 (OBSERVED, `0x6970f8`).
pub const S3_APP_GUID: protocol::Guid = protocol::guid("{0E9F8400-D9A5-11CF-8F4D-00804880B189}");

/// Parse 64 hex digits at compile time.
pub const fn hex32(s: &str) -> [u8; 32] {
    const fn h(c: u8) -> u8 {
        match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => panic!("bad hex"),
        }
    }
    let b = s.as_bytes();
    assert!(b.len() == 64);
    let mut out = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        out[i] = h(b[2 * i]) << 4 | h(b[2 * i + 1]);
        i += 1;
    }
    out
}

/// Parse a 64-digit hex sha256 at runtime (CLI flags).
pub fn parse_sha256(s: &str) -> Option<protocol::Sha256> {
    let s = s.trim();
    if s.len() != 64 || !s.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(hex32(s))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Bytes from the OS-seeded std hasher (no extra dependency). Good enough for session GUIDs and resume
/// tokens on a hobby relay; HYPOTHESIS: switch to `getrandom` before a public deployment.
pub fn random_bytes<const N: usize>() -> [u8; N] {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static CTR: AtomicU64 = AtomicU64::new(0);
    let mut out = [0u8; N];
    for chunk in out.chunks_mut(8) {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(CTR.fetch_add(1, Ordering::Relaxed));
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        h.write_u128(t);
        let v = h.finish().to_le_bytes();
        chunk.copy_from_slice(&v[..chunk.len()]);
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn hashes() {
        assert_eq!(super::GOG_EXE_SHA256[0], 0x00);
        assert_eq!(super::GOG_EXE_SHA256[1], 0xa9);
        assert_eq!(super::hex(&super::GOG_EXE_SHA256[..4]), "00a925cf");
        assert_eq!(super::parse_sha256(&super::hex(&super::GOG_EXE_SHA256)), Some(super::GOG_EXE_SHA256));
        assert_eq!(super::parse_sha256("zz"), None);
        let a: [u8; 16] = super::random_bytes();
        let b: [u8; 16] = super::random_bytes();
        assert_ne!(a, b);
    }
}
