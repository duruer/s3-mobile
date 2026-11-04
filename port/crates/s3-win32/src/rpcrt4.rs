//! `rpcrt4.dll` HLE. UUID helpers (DirectPlay session GUIDs, CoCreateGuid strings).
//!
//! `NAMES` lists every function we know the game can reach; all are declared (calling one without a handler
//! stops the machine with `UNIMPLEMENTED rpcrt4.dll!Name`).

use s3_runtime::{LogLevel, Machine, Registry, ThunkResult};

/// DLL name as registered.
pub const DLL: &str = "rpcrt4.dll";

/// Known exports.
pub const NAMES: &[&str] = &["UuidToStringA", "RpcStringFreeA", "UuidCreate", "UuidFromStringA"];

/// Declare all names and register implemented handlers.
pub fn register(reg: &mut Registry) {
    reg.declare_all(DLL, NAMES);
    reg.register(DLL, "UuidToStringA", uuid_to_string_a);
    reg.register(DLL, "RpcStringFreeA", rpc_string_free_a);
    reg.register(DLL, "UuidCreate", uuid_create);
}

/// Lower-case "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx" (no braces, like rpcrt4).
pub fn uuid_string(g: &[u8; 16]) -> String {
    let d1 = u32::from_le_bytes([g[0], g[1], g[2], g[3]]);
    let d2 = u16::from_le_bytes([g[4], g[5]]);
    let d3 = u16::from_le_bytes([g[6], g[7]]);
    format!(
        "{d1:08x}-{d2:04x}-{d3:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        g[8], g[9], g[10], g[11], g[12], g[13], g[14], g[15]
    )
}

/// UuidToStringA(const UUID*, RPC_CSTR*): allocates the string on the process heap.
fn uuid_to_string_a(m: &mut Machine) -> ThunkResult {
    let (u, out) = (m.arg_u32(0), m.arg_u32(1));
    let mut g = [0u8; 16];
    if u != 0 {
        g.copy_from_slice(m.mem.slice(u, 16));
    }
    let s = uuid_string(&g);
    let p = m.mem.alloc(s.len() as u32 + 1);
    if p == 0 {
        return ThunkResult::stdcall(14, 2); // RPC_S_OUT_OF_MEMORY
    }
    m.mem.write_cstr(p, &s);
    m.mem.write_u32(out, p);
    ThunkResult::stdcall(0, 2)
}

fn rpc_string_free_a(m: &mut Machine) -> ThunkResult {
    let pp = m.arg_u32(0);
    if pp != 0 {
        let p = m.mem.read_u32(pp);
        if p != 0 {
            m.mem.free(p);
        }
        m.mem.write_u32(pp, 0);
    }
    ThunkResult::stdcall(0, 1)
}

/// Random version-4 GUID bytes.
pub fn random_guid(m: &mut Machine) -> [u8; 16] {
    #[derive(Default)]
    struct Rng(u64);
    let st = m.state::<Rng>();
    let mut r = st.borrow_mut();
    if r.0 == 0 {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
        r.0 = t ^ 0x9E37_79B9_7F4A_7C15;
    }
    let mut g = [0u8; 16];
    for chunk in g.chunks_mut(8) {
        // xorshift64*
        r.0 ^= r.0 >> 12;
        r.0 ^= r.0 << 25;
        r.0 ^= r.0 >> 27;
        chunk.copy_from_slice(&r.0.wrapping_mul(0x2545_F491_4F6C_DD1D).to_le_bytes());
    }
    g[7] = (g[7] & 0x0F) | 0x40;
    g[8] = (g[8] & 0x3F) | 0x80;
    g
}

fn uuid_create(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    let g = random_guid(m);
    m.mem.write_bytes(p, &g);
    m.log(LogLevel::Trace, "UuidCreate");
    ThunkResult::stdcall(0, 1)
}

#[cfg(test)]
mod tests {
    #[test]
    fn uuid_fmt() {
        let g = s3_runtime::Guid::parse("D7B70EE0-4340-11CF-B063-0020AFC2CD35");
        assert_eq!(super::uuid_string(&g.0), "d7b70ee0-4340-11cf-b063-0020afc2cd35");
    }
}
