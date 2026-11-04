//! Minimal COM support: GUIDs, vtables made of thunks, object allocation, and a CLSID factory table used by
//! `ole32!CoCreateInstance`.
//!
//! An HLE COM object lives in guest memory: `[vtable ptr][refcount][impl data...]`. The vtable is an array of
//! thunk addresses; method `Iface::Method` is a normal registry entry in the implementing DLL (e.g.
//! `ddraw.dll!IDirectDrawSurface::Flip`), so the usual UNIMPLEMENTED reporting and tracing apply. `this` is
//! the first stdcall argument (`m.arg_u32(0)`).

use std::collections::HashMap;
use std::fmt;

use crate::Machine;

/// A GUID as stored in memory (little-endian Data1..3 then 8 bytes).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Guid(pub [u8; 16]);

impl Guid {
    /// Parse `"D7B70EE0-4340-11CF-B063-0020AFC2CD35"` (braces optional).
    pub const fn parse(s: &str) -> Guid {
        let b = s.as_bytes();
        let mut hex = [0u8; 32];
        let mut n = 0;
        let mut i = 0;
        while i < b.len() {
            let c = b[i];
            let v = match c {
                b'0'..=b'9' => Some(c - b'0'),
                b'a'..=b'f' => Some(c - b'a' + 10),
                b'A'..=b'F' => Some(c - b'A' + 10),
                _ => None,
            };
            if let Some(v) = v {
                hex[n] = v;
                n += 1;
            }
            i += 1;
        }
        assert!(n == 32, "bad GUID string");
        let mut raw = [0u8; 16];
        let mut k = 0;
        while k < 16 {
            raw[k] = hex[2 * k] << 4 | hex[2 * k + 1];
            k += 1;
        }
        // Data1 (4), Data2 (2), Data3 (2) are little-endian in memory.
        let mut out = [0u8; 16];
        out[0] = raw[3];
        out[1] = raw[2];
        out[2] = raw[1];
        out[3] = raw[0];
        out[4] = raw[5];
        out[5] = raw[4];
        out[6] = raw[7];
        out[7] = raw[6];
        let mut j = 8;
        while j < 16 {
            out[j] = raw[j];
            j += 1;
        }
        Guid(out)
    }

    /// Read a GUID from guest memory.
    pub fn read(m: &Machine, addr: u32) -> Guid {
        let mut g = [0u8; 16];
        g.copy_from_slice(m.mem.slice(addr, 16));
        Guid(g)
    }

    /// Write a GUID to guest memory.
    pub fn write(&self, m: &mut Machine, addr: u32) {
        m.mem.write_bytes(addr, &self.0);
    }
}

impl fmt::Display for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let g = &self.0;
        write!(
            f,
            "{{{:02X}{:02X}{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
            g[3], g[2], g[1], g[0], g[5], g[4], g[7], g[6], g[8], g[9], g[10], g[11], g[12], g[13], g[14], g[15]
        )
    }
}

impl fmt::Debug for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// Creates an object for `CoCreateInstance(clsid, ..., iid)`. Returns the interface pointer or an HRESULT.
pub type ComFactory = fn(&mut Machine, iid: Guid) -> Result<u32, u32>;

/// HRESULTs.
pub const S_OK: u32 = 0;
pub const E_NOINTERFACE: u32 = 0x8000_4002;
pub const E_FAIL: u32 = 0x8000_4005;
pub const E_OUTOFMEMORY: u32 = 0x8007_000E;
pub const CLASS_E_CLASSNOTAVAILABLE: u32 = 0x8004_0111;
pub const REGDB_E_CLASSNOTREG: u32 = 0x8004_0154;

/// CLSID factories and built vtables.
#[derive(Default)]
pub struct ComRegistry {
    pub factories: HashMap<Guid, (String, ComFactory)>,
    vtables: HashMap<String, u32>,
}

impl ComRegistry {
    /// Register a class factory for `CoCreateInstance`.
    pub fn register_class(&mut self, clsid: Guid, name: &str, f: ComFactory) {
        self.factories.insert(clsid, (name.to_string(), f));
    }
}

impl Machine {
    /// Vtable for `iface` implemented by `dll`: one thunk per method name (in vtable order), declared in the
    /// registry as `dll!iface::method`. Built once and cached by `dll!iface`.
    pub fn com_vtable(&mut self, dll: &str, iface: &str, methods: &[&str]) -> u32 {
        let key = format!("{dll}!{iface}");
        if let Some(&v) = self.com.vtables.get(&key) {
            return v;
        }
        let vt = self.mem.alloc(methods.len() as u32 * 4);
        for (i, meth) in methods.iter().enumerate() {
            let a = self.registry.declare(dll, &format!("{iface}::{meth}"));
            self.mem.write_u32(vt + i as u32 * 4, a);
        }
        self.com.vtables.insert(key, vt);
        vt
    }

    /// Allocate a COM object: `size` bytes (>= 8), `[0]` = vtable, `[4]` = refcount 1. Returns the pointer.
    pub fn com_new_object(&mut self, vtable: u32, size: u32) -> u32 {
        let p = self.mem.alloc(size.max(8));
        self.mem.write_u32(p, vtable);
        self.mem.write_u32(p + 4, 1);
        p
    }

    /// Generic `IUnknown::AddRef` on the standard object layout. Use as a handler.
    pub fn com_addref(m: &mut Machine) -> crate::ThunkResult {
        let this = m.arg_u32(0);
        let n = m.mem.read_u32(this + 4).wrapping_add(1);
        m.mem.write_u32(this + 4, n);
        crate::ThunkResult::stdcall(n, 1)
    }

    /// Generic `IUnknown::Release` on the standard object layout (memory is not freed: objects are few and
    /// the guest may still hold aliases). Use as a handler.
    pub fn com_release(m: &mut Machine) -> crate::ThunkResult {
        let this = m.arg_u32(0);
        let n = m.mem.read_u32(this + 4).saturating_sub(1);
        m.mem.write_u32(this + 4, n);
        crate::ThunkResult::stdcall(n, 1)
    }
}
