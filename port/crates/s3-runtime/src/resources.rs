//! PE resources (`.rsrc`) of images mapped in guest memory: `FindResourceA` / `LoadResource` /
//! `SizeofResource` (kernel32) and `LoadStringA` / `LoadIconA` / `LoadCursorA` (user32).
//!
//! A resource handle (HRSRC) is the guest address of its `IMAGE_RESOURCE_DATA_ENTRY`
//! (`find_resource_entry`); `LoadResource(HRSRC)` = `module_base + entry.OffsetToData`, size = `entry.Size`.

use crate::Machine;

/// Resource identifier: integer id (`MAKEINTRESOURCE`) or name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResId {
    Id(u16),
    Name(String),
}

impl ResId {
    /// From a guest `LPCSTR` argument: values < 0x10000 are integer ids, otherwise an ANSI string (`#123`
    /// strings are converted to ids).
    pub fn from_guest_a(m: &Machine, p: u32) -> ResId {
        if p < 0x1_0000 {
            return ResId::Id(p as u16);
        }
        let s = m.mem.read_cstr(p);
        if let Some(n) = s.strip_prefix('#').and_then(|n| n.parse().ok()) {
            return ResId::Id(n);
        }
        ResId::Name(s.to_ascii_uppercase())
    }
}

/// Standard resource types.
pub mod rt {
    pub const CURSOR: u16 = 1;
    pub const BITMAP: u16 = 2;
    pub const ICON: u16 = 3;
    pub const MENU: u16 = 4;
    pub const DIALOG: u16 = 5;
    pub const STRING: u16 = 6;
    pub const RCDATA: u16 = 10;
    pub const GROUP_CURSOR: u16 = 12;
    pub const GROUP_ICON: u16 = 14;
    pub const VERSION: u16 = 16;
}

fn res_dir_find(m: &Machine, rsrc: u32, dir: u32, id: Option<&ResId>) -> Option<u32> {
    let named = m.mem.read_u16(dir + 12) as u32;
    let ids = m.mem.read_u16(dir + 14) as u32;
    for i in 0..named + ids {
        let e = dir + 16 + i * 8;
        let name = m.mem.read_u32(e);
        let off = m.mem.read_u32(e + 4);
        let matches = match id {
            None => true,
            Some(ResId::Id(want)) => name & 0x8000_0000 == 0 && name as u16 == *want,
            Some(ResId::Name(want)) => {
                if name & 0x8000_0000 == 0 {
                    false
                } else {
                    let s = rsrc + (name & 0x7FFF_FFFF);
                    let len = m.mem.read_u16(s) as usize;
                    let units: Vec<u16> = (0..len).map(|k| m.mem.read_u16(s + 2 + 2 * k as u32)).collect();
                    String::from_utf16_lossy(&units).eq_ignore_ascii_case(want)
                }
            }
        };
        if matches {
            return Some(off);
        }
    }
    None
}

/// Find a resource in the module mapped at `module_base`. Returns (guest address, size) of the data.
/// `lang = None` takes the first language.
pub fn find_resource(m: &Machine, module_base: u32, rtype: &ResId, name: &ResId, lang: Option<u16>) -> Option<(u32, u32)> {
    let data = find_resource_entry(m, module_base, rtype, name, lang)?;
    Some((module_base + m.mem.read_u32(data), m.mem.read_u32(data + 4)))
}

/// Like [`find_resource`] but returns the guest address of the `IMAGE_RESOURCE_DATA_ENTRY` (used as HRSRC).
pub fn find_resource_entry(m: &Machine, module_base: u32, rtype: &ResId, name: &ResId, lang: Option<u16>) -> Option<u32> {
    let pe = module_base + m.mem.read_u32(module_base + 0x3C);
    let opt = pe + 24;
    let rva = m.mem.read_u32(opt + 96 + 2 * 8);
    if rva == 0 {
        return None;
    }
    let rsrc = module_base + rva;
    let l1 = res_dir_find(m, rsrc, rsrc, Some(rtype))?;
    if l1 & 0x8000_0000 == 0 {
        return None;
    }
    let l2 = res_dir_find(m, rsrc, rsrc + (l1 & 0x7FFF_FFFF), Some(name))?;
    if l2 & 0x8000_0000 == 0 {
        return None;
    }
    let lang_id = lang.map(ResId::Id);
    let l3_dir = rsrc + (l2 & 0x7FFF_FFFF);
    let l3 = res_dir_find(m, rsrc, l3_dir, lang_id.as_ref()).or_else(|| res_dir_find(m, rsrc, l3_dir, None))?;
    if l3 & 0x8000_0000 != 0 {
        return None;
    }
    Some(rsrc + l3)
}

/// `LoadString` helper: string `id` from the RT_STRING table of `module_base`.
pub fn load_string(m: &Machine, module_base: u32, id: u32) -> Option<String> {
    let block = (id >> 4) + 1;
    let (addr, _) = find_resource(m, module_base, &ResId::Id(rt::STRING), &ResId::Id(block as u16), None)?;
    let mut p = addr;
    for i in 0..16 {
        let len = m.mem.read_u16(p) as u32;
        if i == id & 15 {
            let units: Vec<u16> = (0..len).map(|k| m.mem.read_u16(p + 2 + 2 * k)).collect();
            return Some(String::from_utf16_lossy(&units));
        }
        p += 2 + 2 * len;
    }
    None
}

