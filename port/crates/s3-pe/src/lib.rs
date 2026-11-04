//! PE32 loader for S3.EXE.
//!
//! [`parse`] reads headers; [`load`] maps the image at its preferred base (S3.EXE has no relocations),
//! resolves every import through the runtime [`Registry`](s3_runtime::Registry) (unknown imports become
//! auto stubs that stop the machine with `UNIMPLEMENTED dll!name` when called) and creates the PEB/process
//! parameters. Resources are looked up in guest memory with [`find_resource`] / [`load_string`].

use std::fmt;

use s3_mem::{RegionKind, PAGE_EXECUTE_READ, PAGE_READONLY, PAGE_READWRITE};
use s3_runtime::Machine;

/// Loader errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeError {
    Truncated(&'static str),
    BadMagic(&'static str),
    Unsupported(String),
    Map(String),
}

impl fmt::Display for PeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PeError::Truncated(w) => write!(f, "PE truncated ({w})"),
            PeError::BadMagic(w) => write!(f, "not a PE32 image ({w})"),
            PeError::Unsupported(s) => write!(f, "unsupported PE: {s}"),
            PeError::Map(s) => write!(f, "cannot map image: {s}"),
        }
    }
}

impl std::error::Error for PeError {}

/// A section header.
#[derive(Debug, Clone)]
pub struct Section {
    pub name: String,
    pub virtual_address: u32,
    pub virtual_size: u32,
    pub raw_offset: u32,
    pub raw_size: u32,
    pub characteristics: u32,
}

/// Parsed headers.
#[derive(Debug, Clone)]
pub struct PeHeaders {
    pub image_base: u32,
    pub size_of_image: u32,
    pub size_of_headers: u32,
    pub entry_rva: u32,
    pub stack_reserve: u32,
    pub stack_commit: u32,
    pub heap_reserve: u32,
    pub subsystem: u16,
    pub has_relocs: bool,
    /// Data directories: (rva, size), index as in the PE spec (1 = imports, 2 = resources, 9 = TLS).
    pub dirs: Vec<(u32, u32)>,
    pub sections: Vec<Section>,
}

pub const DIR_IMPORT: usize = 1;
pub const DIR_RESOURCE: usize = 2;
pub const DIR_BASERELOC: usize = 5;
pub const DIR_TLS: usize = 9;

fn u16_at(b: &[u8], o: usize) -> Result<u16, PeError> {
    b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]])).ok_or(PeError::Truncated("u16"))
}

fn u32_at(b: &[u8], o: usize) -> Result<u32, PeError> {
    b.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]])).ok_or(PeError::Truncated("u32"))
}

/// Parse PE32 headers.
pub fn parse(b: &[u8]) -> Result<PeHeaders, PeError> {
    if b.get(0..2) != Some(b"MZ") {
        return Err(PeError::BadMagic("MZ"));
    }
    let pe = u32_at(b, 0x3C)? as usize;
    if b.get(pe..pe + 4) != Some(b"PE\0\0") {
        return Err(PeError::BadMagic("PE"));
    }
    let coff = pe + 4;
    let machine = u16_at(b, coff)?;
    if machine != 0x14C {
        return Err(PeError::Unsupported(format!("machine {machine:#x}")));
    }
    let nsec = u16_at(b, coff + 2)? as usize;
    let opt_size = u16_at(b, coff + 16)? as usize;
    let characteristics = u16_at(b, coff + 18)?;
    let opt = coff + 20;
    if u16_at(b, opt)? != 0x10B {
        return Err(PeError::BadMagic("PE32 optional header"));
    }
    let entry_rva = u32_at(b, opt + 16)?;
    let image_base = u32_at(b, opt + 28)?;
    let size_of_image = u32_at(b, opt + 56)?;
    let size_of_headers = u32_at(b, opt + 60)?;
    let subsystem = u16_at(b, opt + 68)?;
    let stack_reserve = u32_at(b, opt + 72)?;
    let stack_commit = u32_at(b, opt + 76)?;
    let heap_reserve = u32_at(b, opt + 80)?;
    let ndirs = (u32_at(b, opt + 92)? as usize).min(16);
    let mut dirs = Vec::new();
    for i in 0..ndirs {
        dirs.push((u32_at(b, opt + 96 + i * 8)?, u32_at(b, opt + 100 + i * 8)?));
    }
    dirs.resize(16, (0, 0));
    let has_relocs = characteristics & 1 == 0 && dirs[DIR_BASERELOC].1 != 0;
    let mut sections = Vec::new();
    let sh = opt + opt_size;
    for i in 0..nsec {
        let o = sh + i * 40;
        let raw_name = b.get(o..o + 8).ok_or(PeError::Truncated("section"))?;
        let name = String::from_utf8_lossy(raw_name).trim_end_matches('\0').to_string();
        sections.push(Section {
            name,
            virtual_size: u32_at(b, o + 8)?,
            virtual_address: u32_at(b, o + 12)?,
            raw_size: u32_at(b, o + 16)?,
            raw_offset: u32_at(b, o + 20)?,
            characteristics: u32_at(b, o + 36)?,
        });
    }
    Ok(PeHeaders {
        image_base,
        size_of_image,
        size_of_headers,
        entry_rva,
        stack_reserve,
        stack_commit,
        heap_reserve,
        subsystem,
        has_relocs,
        dirs,
        sections,
    })
}

/// One resolved import.
#[derive(Debug, Clone)]
pub struct ImportRef {
    pub dll: String,
    /// Name, or `#ordinal`.
    pub name: String,
    pub ordinal: Option<u16>,
    /// Guest address of the IAT slot.
    pub iat: u32,
    /// Thunk address written into the slot.
    pub thunk: u32,
}

/// Result of [`load`].
#[derive(Debug, Clone)]
pub struct LoadedImage {
    pub headers: PeHeaders,
    pub base: u32,
    pub entry: u32,
    pub imports: Vec<ImportRef>,
}

impl LoadedImage {
    /// Imports that no HLE module declared (auto stubs).
    pub fn auto_stubbed<'a>(&'a self, m: &'a Machine) -> impl Iterator<Item = &'a ImportRef> + 'a {
        self.imports.iter().filter(move |i| m.registry.entry(i.thunk).map(|e| e.auto).unwrap_or(false))
    }
}

/// Map `bytes` into the machine at its image base, resolve imports, set up the process (PEB, command line
/// `exe_name`). Does not create the main thread.
pub fn load(m: &mut Machine, bytes: &[u8], exe_name: &str) -> Result<LoadedImage, PeError> {
    let h = parse(bytes)?;
    if h.dirs[DIR_TLS].1 != 0 {
        // S3.EXE has no TLS directory; implement TLS callbacks/index when another binary needs it.
        m.log(s3_runtime::LogLevel::Warn, "PE has a TLS directory; static TLS is not supported yet");
    }
    let base = h.image_base;
    m.mem
        .alloc_region(Some(base), h.size_of_image, RegionKind::Image, (0, 0))
        .map_err(|e| PeError::Map(e.to_string()))?;
    let hdr_len = (h.size_of_headers as usize).min(bytes.len());
    m.mem.write_bytes(base, &bytes[..hdr_len]);
    let _ = m.mem.virtual_protect(base, h.size_of_headers.max(1), PAGE_READONLY);
    for s in &h.sections {
        let raw = (s.raw_size.min(if s.virtual_size == 0 { s.raw_size } else { s.virtual_size })) as usize;
        let off = s.raw_offset as usize;
        if raw > 0 {
            let data = bytes.get(off..off + raw).ok_or(PeError::Truncated("section data"))?;
            m.mem.write_bytes(base + s.virtual_address, data);
        }
        let prot = if s.characteristics & 0x8000_0000 != 0 {
            PAGE_READWRITE
        } else if s.characteristics & 0x2000_0000 != 0 {
            PAGE_EXECUTE_READ
        } else {
            PAGE_READONLY
        };
        let vsize = s.virtual_size.max(s.raw_size).max(1);
        let _ = m.mem.virtual_protect(base + s.virtual_address, vsize, prot);
    }

    let mut imports = Vec::new();
    let (irva, _isize) = h.dirs[DIR_IMPORT];
    if irva != 0 {
        let mut d = base + irva;
        loop {
            let oft = m.mem.read_u32(d);
            let name_rva = m.mem.read_u32(d + 12);
            let ft = m.mem.read_u32(d + 16);
            if name_rva == 0 && ft == 0 {
                break;
            }
            let dll = m.mem.read_cstr(base + name_rva);
            let lookup = if oft != 0 { oft } else { ft };
            let mut i = 0u32;
            loop {
                let v = m.mem.read_u32(base + lookup + i * 4);
                if v == 0 {
                    break;
                }
                let iat = base + ft + i * 4;
                let (name, ordinal, thunk) = if v & 0x8000_0000 != 0 {
                    let ord = (v & 0xFFFF) as u16;
                    let t = m.registry.resolve_import_ordinal(&dll, ord);
                    (format!("#{ord}"), Some(ord), t)
                } else {
                    let n = m.mem.read_cstr(base + v + 2);
                    let t = m.registry.resolve_import(&dll, &n);
                    (n, None, t)
                };
                m.mem.write_u32(iat, thunk);
                imports.push(ImportRef { dll: dll.clone(), name, ordinal, iat, thunk });
                i += 1;
            }
            d += 20;
        }
    }
    let entry = base + h.entry_rva;
    m.setup_process(base, h.size_of_image, entry, h.stack_reserve.max(0x10_0000), exe_name);
    Ok(LoadedImage { headers: h, base, entry, imports })
}

// Resources live in s3-runtime so HLE modules (kernel32 FindResourceA, user32 LoadStringA/LoadIconA/...) can
// use them without depending on the loader.
pub use s3_runtime::resources::{find_resource, find_resource_entry, load_string, rt, ResId};

#[cfg(test)]
mod tests;
