//! PEB, process parameters, command line and environment block.

use s3_mem::layout;
use s3_mem::RegionKind;

use crate::vfs::GUEST_GAME_DIR;
use crate::Machine;

/// PEB field offsets (x86).
pub mod peb {
    pub const BEING_DEBUGGED: u32 = 0x02;
    pub const IMAGE_BASE: u32 = 0x08;
    pub const LDR: u32 = 0x0C;
    pub const PROCESS_PARAMETERS: u32 = 0x10;
    pub const PROCESS_HEAP: u32 = 0x18;
    pub const NUMBER_OF_PROCESSORS: u32 = 0x64;
    pub const OS_MAJOR: u32 = 0xA4;
    pub const OS_MINOR: u32 = 0xA8;
    pub const OS_BUILD: u32 = 0xAC;
    pub const OS_PLATFORM_ID: u32 = 0xB0;
}

/// Default environment variables the guest sees.
pub const DEFAULT_ENV: &[&str] = &["PATH=C:\\WINDOWS;C:\\WINDOWS\\SYSTEM;C:\\S3", "TEMP=C:\\TEMP", "TMP=C:\\TEMP", "windir=C:\\WINDOWS"];

pub(crate) fn setup_process(m: &mut Machine, image_base: u32, image_size: u32, entry: u32, stack_reserve: u32, exe_name: &str) {
    let peb_addr = layout::PEB_ADDR;
    let params = m
        .mem
        .alloc_region(None, 0x1_0000, RegionKind::Private, (layout::LOW_ARENA_START, layout::LOW_ARENA_END))
        .expect("low arena");
    m.mem.fill(peb_addr, 0x1000, 0);
    m.mem.write_u32(peb_addr + peb::IMAGE_BASE, image_base);
    m.mem.write_u32(peb_addr + peb::PROCESS_PARAMETERS, params);
    let heap = m.mem.process_heap();
    m.mem.write_u32(peb_addr + peb::PROCESS_HEAP, heap);
    m.mem.write_u32(peb_addr + peb::NUMBER_OF_PROCESSORS, 1);
    // Windows 98-like answers are chosen by kernel32!GetVersion*; the PEB says NT 5.1 (XP), build 2600.
    m.mem.write_u32(peb_addr + peb::OS_MAJOR, 5);
    m.mem.write_u32(peb_addr + peb::OS_MINOR, 1);
    m.mem.write_u16(peb_addr + peb::OS_BUILD, 2600);
    m.mem.write_u32(peb_addr + peb::OS_PLATFORM_ID, 2);

    let module_path = format!("{GUEST_GAME_DIR}\\{exe_name}");
    // Contract: the command line is just the exe name (kernel32 may append user arguments later).
    let cmdline = exe_name.to_string();
    // Layout inside the params block: 0x000 RTL_USER_PROCESS_PARAMETERS (0x2A0), strings from 0x400.
    let mut p = params + 0x400;
    let cmdline_a = p;
    p += m.mem.write_cstr(cmdline_a, &cmdline) as u32;
    p = (p + 3) & !3;
    let cmdline_w = p;
    p += 2 * m.mem.write_wstr(cmdline_w, &cmdline) as u32;
    p = (p + 3) & !3;
    let path_w = p;
    p += 2 * m.mem.write_wstr(path_w, &module_path) as u32;
    p = (p + 15) & !15;
    let env_a = p;
    for v in DEFAULT_ENV {
        p += m.mem.write_cstr(p, v) as u32;
    }
    m.mem.write_u8(p, 0);
    p += 1;
    p = (p + 15) & !15;
    let env_w = p;
    for v in DEFAULT_ENV {
        p += 2 * m.mem.write_wstr(p, v) as u32;
    }
    m.mem.write_u16(p, 0);

    // RTL_USER_PROCESS_PARAMETERS: ImagePathName UNICODE_STRING at 0x38, CommandLine at 0x40, Environment 0x48.
    let ustr = |m: &mut Machine, at: u32, s: &str, buf: u32| {
        let len = s.encode_utf16().count() as u32 * 2;
        m.mem.write_u16(at, len as u16);
        m.mem.write_u16(at + 2, (len + 2) as u16);
        m.mem.write_u32(at + 4, buf);
    };
    m.mem.write_u32(params, 0x2A0);
    m.mem.write_u32(params + 4, 0x2A0);
    m.mem.write_u32(params + 8, 1); // normalized
    ustr(m, params + 0x38, &module_path, path_w);
    ustr(m, params + 0x40, &cmdline, cmdline_w);
    m.mem.write_u32(params + 0x48, env_w);

    m.process = crate::ProcessInfo {
        image_base,
        image_size,
        entry,
        peb: peb_addr,
        params,
        cmdline_a,
        cmdline_w,
        module_path,
        env_a,
        env_w,
        stack_reserve,
    };
}
