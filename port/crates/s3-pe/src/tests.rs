use super::*;
use s3_runtime::NullHost;

fn machine() -> Machine {
    let d = std::env::temp_dir().join(format!("s3pe-test-{}", std::process::id()));
    let mut h = NullHost::new(d.join("game"), d.join("data"));
    h.quiet = true;
    Machine::new(Box::new(h)).unwrap()
}

fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn put16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}

/// A tiny hand-made PE: one section at RVA 0x1000 holding an import table for
/// `KERNEL32.dll!ExitProcess` (by name) and `FOO.dll!#7` (by ordinal).
fn tiny_pe() -> Vec<u8> {
    let mut b = vec![0u8; 0x400];
    b[0..2].copy_from_slice(b"MZ");
    put32(&mut b, 0x3C, 0x80);
    b[0x80..0x84].copy_from_slice(b"PE\0\0");
    let coff = 0x84;
    put16(&mut b, coff, 0x14C);
    put16(&mut b, coff + 2, 1);
    put16(&mut b, coff + 16, 0xE0);
    put16(&mut b, coff + 18, 0x0103); // relocs stripped, exe, 32-bit
    let opt = coff + 20;
    put16(&mut b, opt, 0x10B);
    put32(&mut b, opt + 16, 0x1000); // entry
    put32(&mut b, opt + 28, 0x40_0000);
    put32(&mut b, opt + 32, 0x1000);
    put32(&mut b, opt + 36, 0x200);
    put32(&mut b, opt + 56, 0x2000); // size of image
    put32(&mut b, opt + 60, 0x200); // size of headers
    put16(&mut b, opt + 68, 2);
    put32(&mut b, opt + 72, 0x10_0000);
    put32(&mut b, opt + 92, 16);
    put32(&mut b, opt + 96 + 8, 0x1100); // import dir
    put32(&mut b, opt + 100 + 8, 40);
    let sh = opt + 0xE0;
    b[sh..sh + 5].copy_from_slice(b".text");
    put32(&mut b, sh + 8, 0x200);
    put32(&mut b, sh + 12, 0x1000);
    put32(&mut b, sh + 16, 0x200);
    put32(&mut b, sh + 20, 0x200);
    put32(&mut b, sh + 36, 0xE000_0020);
    // Section data at file 0x200 == RVA 0x1000.
    let s = 0x200usize;
    let rva = |r: u32| s + (r - 0x1000) as usize;
    b[rva(0x1000)] = 0xC3; // ret
    // Descriptors at 0x1100: kernel32, foo, terminator.
    put32(&mut b, rva(0x1100), 0x1140); // OFT
    put32(&mut b, rva(0x1100) + 12, 0x1180); // name
    put32(&mut b, rva(0x1100) + 16, 0x1160); // FT
    put32(&mut b, rva(0x1114), 0);
    put32(&mut b, rva(0x1114) + 12, 0x1190);
    put32(&mut b, rva(0x1114) + 16, 0x1170);
    put32(&mut b, rva(0x1140), 0x11A0); // hint/name
    put32(&mut b, rva(0x1160), 0x11A0);
    put32(&mut b, rva(0x1170), 0x8000_0007);
    b[rva(0x1180)..rva(0x1180) + 12].copy_from_slice(b"KERNEL32.dll");
    b[rva(0x1190)..rva(0x1190) + 7].copy_from_slice(b"FOO.dll");
    b[rva(0x11A2)..rva(0x11A2) + 11].copy_from_slice(b"ExitProcess");
    b
}

#[test]
fn parse_and_load_tiny() {
    let pe = tiny_pe();
    let h = parse(&pe).unwrap();
    assert_eq!(h.image_base, 0x40_0000);
    assert_eq!(h.sections.len(), 1);
    assert!(!h.has_relocs);
    let mut m = machine();
    let known = m.registry.declare("kernel32.dll", "ExitProcess");
    let img = load(&mut m, &pe, "TINY.EXE").unwrap();
    assert_eq!(img.entry, 0x40_1000);
    assert_eq!(img.imports.len(), 2);
    assert_eq!(m.mem.read_u32(0x40_1160), known);
    let foo = m.mem.read_u32(0x40_1170);
    assert_eq!(m.registry.describe(foo), "foo.dll!#7");
    assert_eq!(img.auto_stubbed(&m).count(), 1);
    assert_eq!(m.mem.read_u8(0x40_1000), 0xC3);
    assert_eq!(m.process.image_base, 0x40_0000);
    assert!(parse(b"MZ").is_err());
}

const S3_EXE: &str = "~/Projects/settlers-3/game-files/gog-s3-ultimate/S3.EXE";

/// Loads the user's local S3.EXE if present (never committed). Skips otherwise.
#[test]
fn load_real_s3_exe_if_present() {
    let path = std::env::var("S3_EXE").unwrap_or_else(|_| S3_EXE.to_string());
    let Ok(bytes) = std::fs::read(&path) else {
        eprintln!("skipping: {path} not found");
        return;
    };
    let mut m = machine();
    let img = load(&mut m, &bytes, "S3.EXE").unwrap();
    assert_eq!(img.entry, 0x0062_1BC6);
    assert_eq!(img.imports.len(), 219);
    assert!(!img.headers.has_relocs);
    // .rsrc is readable through find_resource (S3.EXE has at least one icon group).
    assert!(find_resource(&m, img.base, &ResId::Id(rt::GROUP_ICON), &ResId::Id(1), None).is_some()
        || img.headers.dirs[DIR_RESOURCE].1 > 0);
    let mut dlls: Vec<&str> = img.imports.iter().map(|i| i.dll.as_str()).collect();
    dlls.dedup();
    eprintln!("dlls: {dlls:?}");
}
