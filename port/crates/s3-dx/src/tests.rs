use s3_mem::layout;
use s3_runtime::{HostEvent, KernelObject, Machine, MouseButton, NullHost, Registry};

use crate::blit::{self, BltOpts, ColorKey, Rect, Surf};
use crate::*;

// ------------------------------------------------------------------------------------------ vtable layout

fn slot(list: &[&str], name: &str) -> usize {
    list.iter().position(|m| *m == name).unwrap_or_else(|| panic!("{name} missing"))
}

#[test]
fn guids_parse_to_memory_layout() {
    let g = ddraw::CLSID_DIRECTDRAW;
    assert_eq!(&g.0[..8], &[0xE0, 0x0E, 0xB7, 0xD7, 0x40, 0x43, 0xCF, 0x11]);
    assert_eq!(&g.0[8..], &[0xB0, 0x63, 0x00, 0x20, 0xAF, 0xC2, 0xCD, 0x35]);
    assert_eq!(g.to_string(), "{D7B70EE0-4340-11CF-B063-0020AFC2CD35}");
}

#[test]
fn vtable_offsets_match_s3_call_sites() {
    use ddraw::*;
    // Offsets observed in S3.EXE (docs/reverse-engineering/directx-surface.md).
    assert_eq!(IDIRECTDRAW2.len(), 24);
    assert_eq!(IDIRECTDRAW.len(), 23);
    assert_eq!(slot(IDIRECTDRAW2, "CreateSurface") * 4, 0x18);
    assert_eq!(slot(IDIRECTDRAW2, "Initialize") * 4, 0x48);
    assert_eq!(slot(IDIRECTDRAW2, "SetCooperativeLevel") * 4, 0x50);
    assert_eq!(slot(IDIRECTDRAW2, "SetDisplayMode") * 4, 0x54);
    assert_eq!(slot(IDIRECTDRAW2, "WaitForVerticalBlank") * 4, 0x58);
    assert_eq!(IDIRECTDRAWSURFACE3.len(), 40);
    assert_eq!(slot(IDIRECTDRAWSURFACE3, "Blt") * 4, 0x14);
    assert_eq!(slot(IDIRECTDRAWSURFACE3, "BltFast") * 4, 0x1C);
    assert_eq!(slot(IDIRECTDRAWSURFACE3, "Flip") * 4, 0x2C);
    assert_eq!(slot(IDIRECTDRAWSURFACE3, "GetAttachedSurface") * 4, 0x30);
    assert_eq!(slot(IDIRECTDRAWSURFACE3, "GetDC") * 4, 0x44);
    assert_eq!(slot(IDIRECTDRAWSURFACE3, "Lock") * 4, 0x64);
    assert_eq!(slot(IDIRECTDRAWSURFACE3, "Unlock") * 4, 0x80);
    assert_eq!(slot(IDIRECTDRAWSURFACE3, "GetDDInterface"), 36, "v2 additions start after 36 v1 methods");
    use dsound::*;
    assert_eq!(slot(IDIRECTSOUND, "DuplicateSoundBuffer") * 4, 0x14);
    assert_eq!(slot(IDIRECTSOUND, "SetCooperativeLevel") * 4, 0x18);
    assert_eq!(slot(IDIRECTSOUNDBUFFER, "SetFormat") * 4, 0x38);
    assert_eq!(slot(IDIRECTSOUNDBUFFER, "Lock"), 11);
    assert_eq!(IDIRECTSOUNDBUFFER.len(), 21);
    assert_eq!(slot(dinput::IDIRECTINPUTA, "CreateDevice") * 4, 0x0C);
    assert_eq!(slot(dinput::IDIRECTINPUTDEVICEA, "GetDeviceData"), 10);
    assert_eq!(dinput::IDIRECTINPUTDEVICEA.len(), 18);
    assert_eq!(dplay::IDIRECTPLAY4A.len(), dplay::IDIRECTPLAY4A_NARGS.len());
    assert_eq!(dplay::IDIRECTPLAYLOBBY3A.len(), dplay::IDIRECTPLAYLOBBY3A_NARGS.len());
    assert_eq!(slot(dplay::IDIRECTPLAY4A, "EnumConnections"), 35);
    assert_eq!(slot(dplay::IDIRECTPLAYLOBBY3A, "CreateCompoundAddress"), 14);
}

#[test]
fn register_implements_every_method() {
    let mut reg = Registry::new();
    register_all(&mut reg);
    let ifaces: &[(&str, &str, &[&str])] = &[
        (ddraw::DLL, "IDirectDraw", ddraw::IDIRECTDRAW),
        (ddraw::DLL, "IDirectDraw2", ddraw::IDIRECTDRAW2),
        (ddraw::DLL, "IDirectDrawSurface", ddraw::IDIRECTDRAWSURFACE3),
        (ddraw::DLL, "IDirectDrawPalette", ddraw::IDIRECTDRAWPALETTE),
        (ddraw::DLL, "IDirectDrawClipper", ddraw::IDIRECTDRAWCLIPPER),
        (dsound::DLL, "IDirectSound", dsound::IDIRECTSOUND),
        (dsound::DLL, "IDirectSoundBuffer", dsound::IDIRECTSOUNDBUFFER),
        (dinput::DLL, "IDirectInputA", dinput::IDIRECTINPUTA),
        (dinput::DLL, "IDirectInputDeviceA", dinput::IDIRECTINPUTDEVICEA),
        (dplay::DLL, "IDirectPlay4A", dplay::IDIRECTPLAY4A),
        (dplay::DLL, "IDirectPlayLobby3A", dplay::IDIRECTPLAYLOBBY3A),
    ];
    for (dll, iface, methods) in ifaces {
        for m in *methods {
            let a = reg.lookup(dll, &format!("{iface}::{m}")).unwrap();
            assert!(reg.entry(a).unwrap().handler.is_some(), "{dll}!{iface}::{m} has no handler");
        }
    }
    for (dll, ex) in [(ddraw::DLL, ddraw::EXPORTS), (dsound::DLL, dsound::EXPORTS), (dinput::DLL, dinput::EXPORTS), (dplay::DLL, dplay::EXPORTS)] {
        for e in ex {
            let a = reg.lookup(dll, e).unwrap();
            assert!(reg.entry(a).unwrap().handler.is_some(), "{dll}!{e}");
        }
    }
}

// ------------------------------------------------------------------------------------------ blit

fn mem() -> s3_mem::GuestMemory {
    s3_mem::GuestMemory::new().unwrap()
}

const A: u32 = 0x1000_0000;
const B: u32 = 0x1010_0000;

fn s16(bits: u32, w: u32, h: u32) -> Surf {
    Surf { bits, width: w, height: h, pitch: w * 2, bpp: 16 }
}

fn px(m: &s3_mem::GuestMemory, s: &Surf, x: u32, y: u32) -> u16 {
    m.read_u16(s.bits + y * s.pitch + x * 2)
}

#[test]
fn fill_clips() {
    let mut m = mem();
    let s = s16(A, 8, 4);
    blit::fill(&mut m, &s, Rect::new(-2, 1, 3, 10), 0xF800);
    assert_eq!(px(&m, &s, 0, 0), 0);
    assert_eq!(px(&m, &s, 0, 1), 0xF800);
    assert_eq!(px(&m, &s, 2, 3), 0xF800);
    assert_eq!(px(&m, &s, 3, 1), 0);
}

#[test]
fn blt_copy_clip_and_key() {
    let mut m = mem();
    let src = s16(A, 4, 4);
    let dst = s16(B, 4, 4);
    for y in 0..4 {
        for x in 0..4 {
            m.write_u16(A + y * 8 + x * 2, (y * 4 + x) as u16);
        }
    }
    // Copy the full 4x4 to (2,2): only the top-left 2x2 of the source lands.
    blit::blt(&mut m, &dst, Rect::new(2, 2, 6, 6), &src, src.full(), &BltOpts::default());
    assert_eq!(px(&m, &dst, 2, 2), 0);
    assert_eq!(px(&m, &dst, 3, 3), 5);
    assert_eq!(px(&m, &dst, 1, 1), 0);
    // Source colour key: pixel value 5 is transparent.
    blit::fill(&mut m, &dst, dst.full(), 0xAAAA);
    let o = BltOpts { src_key: Some(ColorKey { low: 5, high: 5 }), ..Default::default() };
    blit::blt(&mut m, &dst, dst.full(), &src, src.full(), &o);
    assert_eq!(px(&m, &dst, 1, 1), 0xAAAA);
    assert_eq!(px(&m, &dst, 2, 1), 6);
}

#[test]
fn blt_stretch_mirror_overlap() {
    let mut m = mem();
    let src = s16(A, 2, 2);
    m.write_u16(A, 1);
    m.write_u16(A + 2, 2);
    m.write_u16(A + 4, 3);
    m.write_u16(A + 6, 4);
    let dst = s16(B, 4, 4);
    blit::blt(&mut m, &dst, dst.full(), &src, src.full(), &BltOpts::default());
    assert_eq!([px(&m, &dst, 0, 0), px(&m, &dst, 1, 1), px(&m, &dst, 2, 1), px(&m, &dst, 3, 3)], [1, 1, 2, 4]);
    let o = BltOpts { mirror_lr: true, ..Default::default() };
    blit::blt(&mut m, &dst, Rect::new(0, 0, 2, 2), &src, src.full(), &o);
    assert_eq!([px(&m, &dst, 0, 0), px(&m, &dst, 1, 0), px(&m, &dst, 0, 1)], [2, 1, 4]);
    // Overlapping scroll inside one surface (down by one row).
    let s = s16(A, 4, 4);
    for y in 0..4 {
        blit::fill(&mut m, &s, Rect::new(0, y, 4, y + 1), y as u32 + 10);
    }
    blit::blt(&mut m, &s, Rect::new(0, 1, 4, 4), &s, Rect::new(0, 0, 4, 3), &BltOpts::default());
    assert_eq!([px(&m, &s, 0, 0), px(&m, &s, 0, 1), px(&m, &s, 0, 2), px(&m, &s, 0, 3)], [10, 10, 11, 12]);
}

#[test]
fn framebuffer_conversion() {
    let mut m = mem();
    let s = Surf { bits: A, width: 2, height: 1, pitch: 4, bpp: 8 };
    m.write_u8(A, 1);
    m.write_u8(A + 1, 0);
    let pal = vec![0u32; 256].into_iter().enumerate().map(|(i, _)| if i == 1 { 0x0000_00FF } else { 0 }).collect::<Vec<_>>();
    let fb = blit::to_framebuffer(&m, &s, false, Some(&pal));
    assert_eq!(fb.pixels, vec![0xF800, 0]);
    let s = s16(A, 1, 1);
    m.write_u16(A, 0x7C00); // 555 red
    assert_eq!(blit::to_framebuffer(&m, &s, true, None).pixels, vec![0xF800]);
}

// ------------------------------------------------------------------------------------------ mixer

#[test]
fn mixer_gain_and_voice() {
    use crate::mixer::*;
    assert_eq!(db100_to_gain(0), 1.0);
    assert!((db100_to_gain(-2000) - 0.1).abs() < 1e-6);
    assert_eq!(db100_to_gain(-10000), 0.0);
    assert_eq!(pan_gains(0), (1.0, 1.0));
    let (l, r) = pan_gains(10000);
    assert_eq!((l, r), (0.0, 1.0));
    // Mono 16-bit, 4 frames of 1000, played at the output rate.
    let data: Vec<u8> = [1000i16; 4].iter().flat_map(|v| v.to_le_bytes()).collect();
    let fmt = PcmFormat { rate: OUT_RATE, channels: 1, bits: 16 };
    let mut pos = 0.0;
    let mut out = vec![0i32; 12];
    let more = mix_voice(&data, &fmt, &mut pos, 1.0, (1.0, 0.5), false, &mut out);
    assert!(!more, "non-looping voice ends");
    assert_eq!(&out[..8], &[1000, 500, 1000, 500, 1000, 500, 1000, 500]);
    assert_eq!(&out[8..], &[0, 0, 0, 0]);
    // Looping 8-bit voice keeps going.
    let fmt8 = PcmFormat { rate: OUT_RATE, channels: 1, bits: 8 };
    let mut pos = 0.0;
    let mut out = vec![0i32; 20];
    assert!(mix_voice(&[128 + 64, 128 - 64], &fmt8, &mut pos, 1.0, (1.0, 1.0), true, &mut out));
    assert_eq!(out[0], 64 << 8);
    assert_eq!(out[2], -(64 << 8));
    assert_eq!(out[4], 64 << 8);
    let mut v = Vec::new();
    finish(&[40000, -40000, 100], 1.0, &mut v);
    assert_eq!(v, vec![32767, -32768, 100]);
}

// ------------------------------------------------------------------------------------------ COM objects through the machine

const CODE: u32 = 0x0040_1000;

fn machine() -> Machine {
    let d = std::env::temp_dir().join(format!("s3dx-test-{}-{:?}", std::process::id(), std::thread::current().id()));
    let _ = std::fs::create_dir_all(&d);
    let mut host = NullHost::new(d.join("game"), d.join("data"));
    host.quiet = true;
    let mut m = Machine::new(Box::new(host)).unwrap();
    m.mem.alloc_region(Some(layout::IMAGE_BASE), 0x10_0000, s3_mem::RegionKind::Image, (0, 0)).unwrap();
    m.setup_process(layout::IMAGE_BASE, 0x10_0000, CODE, 0x10_0000, "TEST.EXE");
    register_all(&mut m.registry);
    register_factories(&mut m);
    let slot = m.create_thread(CODE, 0, 0x10_0000, false).unwrap();
    m.cur = slot;
    m
}

/// Call method `name` of the COM object `this` through its vtable.
fn call(m: &mut Machine, this: u32, list: &[&str], name: &str, args: &[u32]) -> u32 {
    let vt = m.mem.read_u32(this);
    let f = m.mem.read_u32(vt + 4 * slot(list, name) as u32);
    let mut a = vec![this];
    a.extend_from_slice(args);
    let r = m.call_guest(f, &a);
    assert!(!m.is_stopped(), "machine stopped: {:?}", m.stop_reason());
    r
}

fn export(m: &mut Machine, dll: &str, name: &str, args: &[u32]) -> u32 {
    let f = m.registry.lookup(dll, name).unwrap();
    let r = m.call_guest(f, args);
    assert!(!m.is_stopped(), "machine stopped: {:?}", m.stop_reason());
    r
}

#[test]
fn ddraw_menu_flow_presents() {
    use ddraw::*;
    let mut m = machine();
    let (_, f) = m.com.factories[&CLSID_DIRECTDRAW];
    let dd = f(&mut m, IID_IDIRECTDRAW2).unwrap();
    let scratch = m.mem.alloc(0x200);
    assert_eq!(call(&mut m, dd, IDIRECTDRAW2, "Initialize", &[0]), 0);
    assert_eq!(call(&mut m, dd, IDIRECTDRAW2, "SetCooperativeLevel", &[0x1234, 0x13]), 0);
    assert_eq!(call(&mut m, dd, IDIRECTDRAW2, "SetDisplayMode", &[640, 480, 16, 0, 0]), 0);
    // GetDisplayMode
    m.mem.write_u32(scratch, DDSD_SIZE);
    assert_eq!(call(&mut m, dd, IDIRECTDRAW2, "GetDisplayMode", &[scratch]), 0);
    assert_eq!((m.mem.read_u32(scratch + 0xC), m.mem.read_u32(scratch + 8)), (640, 480));
    assert_eq!(m.mem.read_u32(scratch + 0x48 + 0x10), 0xF800);
    // Primary surface (caps only).
    let ddsd = m.mem.alloc(DDSD_SIZE);
    let out = m.mem.alloc(4);
    m.mem.write_u32(ddsd, DDSD_SIZE);
    m.mem.write_u32(ddsd + 4, DDSD_CAPS);
    m.mem.write_u32(ddsd + 0x68, DDSCAPS_PRIMARYSURFACE);
    assert_eq!(call(&mut m, dd, IDIRECTDRAW2, "CreateSurface", &[ddsd, out, 0]), 0);
    let prim = m.mem.read_u32(out);
    // QueryInterface to IDirectDrawSurface3 returns the same object.
    let iid = m.mem.alloc(16);
    IID_IDIRECTDRAWSURFACE3.write(&mut m, iid);
    assert_eq!(call(&mut m, prim, IDIRECTDRAWSURFACE3, "QueryInterface", &[iid, out]), 0);
    assert_eq!(m.mem.read_u32(out), prim);
    // Offscreen system-memory back buffer.
    m.mem.fill(ddsd, DDSD_SIZE as usize, 0);
    m.mem.write_u32(ddsd, DDSD_SIZE);
    m.mem.write_u32(ddsd + 4, DDSD_CAPS | DDSD_WIDTH | DDSD_HEIGHT);
    m.mem.write_u32(ddsd + 8, 480);
    m.mem.write_u32(ddsd + 0xC, 640);
    m.mem.write_u32(ddsd + 0x68, DDSCAPS_OFFSCREENPLAIN | DDSCAPS_SYSTEMMEMORY);
    assert_eq!(call(&mut m, dd, IDIRECTDRAW2, "CreateSurface", &[ddsd, out, 0]), 0);
    let back = m.mem.read_u32(out);
    // Lock, draw a pixel, unlock.
    m.mem.write_u32(ddsd, DDSD_SIZE);
    assert_eq!(call(&mut m, back, IDIRECTDRAWSURFACE3, "Lock", &[0, ddsd, 1, 0]), 0);
    let bits = m.mem.read_u32(ddsd + 0x24);
    let pitch = m.mem.read_u32(ddsd + 0x10);
    assert_eq!(pitch, 1280);
    assert_eq!(m.mem.read_u32(ddsd + 0x68) & DDSCAPS_SYSTEMMEMORY, DDSCAPS_SYSTEMMEMORY);
    m.mem.write_u16(bits + 10 * pitch + 20, 0x07E0);
    assert_eq!(call(&mut m, back, IDIRECTDRAWSURFACE3, "Unlock", &[0]), 0);
    let presents0 = m.stats.presents;
    // Blt back -> primary with the game's flags (WAIT|ROP|ASYNC, SRCCOPY).
    let rect = m.mem.alloc(16);
    m.mem.write_bytes(rect, &[0u32, 0, 640, 480].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>());
    let fx = m.mem.alloc(0x64);
    m.mem.write_u32(fx, 0x64);
    m.mem.write_u32(fx + 8, 0xCC0020);
    assert_eq!(call(&mut m, prim, IDIRECTDRAWSURFACE3, "Blt", &[rect, back, rect, 0x1020200, fx]), 0);
    assert_eq!(m.stats.presents, presents0 + 1, "Blt to primary presents");
    let st = m.state::<DdState>();
    let s = st.borrow_mut();
    assert_eq!((s.fb.width, s.fb.height), (640, 480));
    assert_eq!(s.fb.pixels[10 * 640 + 10], 0x07E0);
    drop(s);
    // Colour fill on the primary.
    m.mem.write_u32(fx + 0x50, 0x001F);
    assert_eq!(call(&mut m, prim, IDIRECTDRAWSURFACE3, "Blt", &[0, 0, 0, DDBLT_COLORFILL | DDBLT_WAIT, fx]), 0);
    assert_eq!(st.borrow_mut().fb.pixels[0], 0x001F);
    // BltFast back -> primary.
    assert_eq!(call(&mut m, prim, IDIRECTDRAWSURFACE3, "BltFast", &[0, 0, back, rect, 0x10]), 0);
    assert_eq!(st.borrow_mut().fb.pixels[0], 0);
    // WaitForVerticalBlank blocks until the next 60 Hz boundary (at most ~17 ms).
    let t0 = std::time::Instant::now();
    assert_eq!(call(&mut m, dd, IDIRECTDRAW2, "WaitForVerticalBlank", &[1, 0]), 0);
    assert!(t0.elapsed().as_millis() <= 40);
    // Release frees the offscreen surface.
    assert_eq!(call(&mut m, back, IDIRECTDRAWSURFACE3, "Release", &[]), 0);
    assert!(!st.borrow_mut().surfaces.contains_key(&back));
}

#[test]
fn ddraw_flip_chain_and_enum() {
    use ddraw::*;
    let mut m = machine();
    let out = m.mem.alloc(4);
    assert_eq!(export(&mut m, DLL, "DirectDrawCreate", &[0, out, 0]), 0);
    let dd = m.mem.read_u32(out);
    // IDirectDraw v1 SetDisplayMode has 3 args.
    assert_eq!(call(&mut m, dd, IDIRECTDRAW, "SetDisplayMode", &[800, 600, 16]), 0);
    let ddsd = m.mem.alloc(DDSD_SIZE);
    m.mem.write_u32(ddsd, DDSD_SIZE);
    m.mem.write_u32(ddsd + 4, DDSD_CAPS | DDSD_BACKBUFFERCOUNT);
    m.mem.write_u32(ddsd + 0x14, 1);
    m.mem.write_u32(ddsd + 0x68, DDSCAPS_PRIMARYSURFACE | DDSCAPS_FLIP | DDSCAPS_COMPLEX);
    assert_eq!(call(&mut m, dd, IDIRECTDRAW, "CreateSurface", &[ddsd, out, 0]), 0);
    let front = m.mem.read_u32(out);
    let caps = m.mem.alloc(4);
    m.mem.write_u32(caps, DDSCAPS_BACKBUFFER);
    assert_eq!(call(&mut m, front, IDIRECTDRAWSURFACE3, "GetAttachedSurface", &[caps, out]), 0);
    let back = m.mem.read_u32(out);
    assert_ne!(back, front);
    m.mem.write_u32(ddsd, DDSD_SIZE);
    call(&mut m, back, IDIRECTDRAWSURFACE3, "Lock", &[0, ddsd, 1, 0]);
    let bits = m.mem.read_u32(ddsd + 0x24);
    m.mem.write_u16(bits, 0x1234);
    call(&mut m, back, IDIRECTDRAWSURFACE3, "Unlock", &[0]);
    let p0 = m.stats.presents;
    assert_eq!(call(&mut m, front, IDIRECTDRAWSURFACE3, "Flip", &[0, 1]), 0);
    assert_eq!(m.stats.presents, p0 + 1);
    let st = m.state::<DdState>();
    assert_eq!(st.borrow_mut().fb.pixels[0], 0x1234);
    // EnumDisplayModes: a guest callback that counts calls into a global and returns 1.
    let counter = m.mem.alloc(4);
    let cb = CODE + 0x100;
    // inc dword [counter]; mov eax,1; ret 8
    let mut code = vec![0xFF, 0x05];
    code.extend_from_slice(&counter.to_le_bytes());
    code.extend_from_slice(&[0xB8, 1, 0, 0, 0, 0xC2, 8, 0]);
    m.mem.write_bytes(cb, &code);
    assert_eq!(call(&mut m, dd, IDIRECTDRAW, "EnumDisplayModes", &[0, 0, 0, cb]), 0);
    assert_eq!(m.mem.read_u32(counter), (MODES.len() * MODE_BPPS.len()) as u32);
}

#[test]
fn dsound_buffer_plays_in_real_time() {
    use dsound::*;
    let mut m = machine();
    let out = m.mem.alloc(4);
    assert_eq!(export(&mut m, DLL, "DirectSoundCreate", &[0, out, 0]), 0);
    let ds = m.mem.read_u32(out);
    assert_eq!(call(&mut m, ds, IDIRECTSOUND, "SetCooperativeLevel", &[0x1234, 2]), 0);
    // Primary buffer (flags 0x81, no format).
    let desc = m.mem.alloc(0x14);
    m.mem.write_u32(desc, 0x14);
    m.mem.write_u32(desc + 4, 0x81);
    assert_eq!(call(&mut m, ds, IDIRECTSOUND, "CreateSoundBuffer", &[desc, out, 0]), 0);
    let primary = m.mem.read_u32(out);
    assert_eq!(call(&mut m, primary, IDIRECTSOUNDBUFFER, "SetVolume", &[0]), 0);
    // Secondary: mono 22050 16-bit, 1 s.
    let wfx = m.mem.alloc(18);
    m.mem.write_u16(wfx, 1);
    m.mem.write_u16(wfx + 2, 1);
    m.mem.write_u32(wfx + 4, 22050);
    m.mem.write_u32(wfx + 8, 44100);
    m.mem.write_u16(wfx + 12, 2);
    m.mem.write_u16(wfx + 14, 16);
    m.mem.write_u32(desc + 4, 0xE0);
    m.mem.write_u32(desc + 8, 44100);
    m.mem.write_u32(desc + 0x10, wfx);
    assert_eq!(call(&mut m, ds, IDIRECTSOUND, "CreateSoundBuffer", &[desc, out, 0]), 0);
    let buf = m.mem.read_u32(out);
    let ptrs = m.mem.alloc(16);
    assert_eq!(call(&mut m, buf, IDIRECTSOUNDBUFFER, "Lock", &[44000, 200, ptrs, ptrs + 4, ptrs + 8, ptrs + 12, 0]), 0);
    assert_eq!(m.mem.read_u32(ptrs + 4), 100);
    assert_eq!(m.mem.read_u32(ptrs + 12), 100, "lock wraps around");
    assert_eq!(call(&mut m, buf, IDIRECTSOUNDBUFFER, "Unlock", &[0, 0, 0, 0]), 0);
    assert_eq!(call(&mut m, ds, IDIRECTSOUND, "DuplicateSoundBuffer", &[buf, out]), 0);
    let dup = m.mem.read_u32(out);
    assert_ne!(dup, buf);
    assert_eq!(call(&mut m, buf, IDIRECTSOUNDBUFFER, "SetPan", &[(-500i32) as u32]), 0);
    assert_eq!(call(&mut m, buf, IDIRECTSOUNDBUFFER, "Play", &[0, 0, 0]), 0);
    call(&mut m, buf, IDIRECTSOUNDBUFFER, "GetStatus", &[out]);
    assert_eq!(m.mem.read_u32(out), DSBSTATUS_PLAYING);
    mixer_service(&mut m);
    std::thread::sleep(std::time::Duration::from_millis(50));
    mixer_service(&mut m);
    call(&mut m, buf, IDIRECTSOUNDBUFFER, "GetCurrentPosition", &[out, 0]);
    let play = m.mem.read_u32(out);
    // >= 50 ms + latency at 22050 Hz * 2 bytes.
    assert!(play >= 2 * 22050 * 50 / 1000, "play cursor advanced: {play}");
    assert!(play < 44100);
    assert_eq!(call(&mut m, dup, IDIRECTSOUNDBUFFER, "Release", &[]), 0);
    assert_eq!(call(&mut m, buf, IDIRECTSOUNDBUFFER, "Stop", &[]), 0);
    call(&mut m, buf, IDIRECTSOUNDBUFFER, "GetStatus", &[out]);
    assert_eq!(m.mem.read_u32(out), 0);
}

#[test]
fn dinput_mouse_buffered_with_notification() {
    use dinput::*;
    let mut m = machine();
    let out = m.mem.alloc(4);
    assert_eq!(export(&mut m, DLL, "DirectInputCreateA", &[0x400000, 0x300, out, 0]), 0);
    let di = m.mem.read_u32(out);
    let g = m.mem.alloc(16);
    GUID_SYSMOUSE.write(&mut m, g);
    assert_eq!(call(&mut m, di, IDIRECTINPUTA, "CreateDevice", &[g, out, 0]), 0);
    let dev = m.mem.read_u32(out);
    let prop = m.mem.alloc(0x14);
    m.mem.write_bytes(prop, &[0x14u32, 0x10, 0, 0, 128].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>());
    assert_eq!(call(&mut m, dev, IDIRECTINPUTDEVICEA, "SetProperty", &[DIPROP_BUFFERSIZE, prop]), 0);
    let ev = m.handles.insert(KernelObject::Event { manual_reset: false, signaled: false });
    assert_eq!(call(&mut m, dev, IDIRECTINPUTDEVICEA, "SetEventNotification", &[ev]), 0);
    let data = m.mem.alloc(16 * 8);
    m.mem.write_u32(out, 8);
    assert_eq!(call(&mut m, dev, IDIRECTINPUTDEVICEA, "GetDeviceData", &[16, data, out, 0]), DIERR_NOTACQUIRED);
    assert_eq!(call(&mut m, dev, IDIRECTINPUTDEVICEA, "Acquire", &[]), 0);
    m.input.push(HostEvent::MouseMove { x: 10, y: 5 });
    m.input.push(HostEvent::MouseButton { button: MouseButton::Left, down: true });
    pump_service(&mut m);
    assert_eq!(m.is_signaled(ev), Some(true));
    // Peek first (the game does), then read.
    m.mem.write_u32(out, 8);
    assert_eq!(call(&mut m, dev, IDIRECTINPUTDEVICEA, "GetDeviceData", &[16, data, out, DIGDD_PEEK]), 0);
    assert_eq!(m.mem.read_u32(out), 3);
    m.mem.write_u32(out, 2);
    assert_eq!(call(&mut m, dev, IDIRECTINPUTDEVICEA, "GetDeviceData", &[16, data, out, 0]), 0);
    assert_eq!(m.mem.read_u32(out), 2);
    assert_eq!((m.mem.read_u32(data), m.mem.read_u32(data + 4)), (DIMOFS_X, 10));
    assert_eq!((m.mem.read_u32(data + 16), m.mem.read_u32(data + 20)), (DIMOFS_Y, 5));
    m.mem.write_u32(out, 8);
    assert_eq!(call(&mut m, dev, IDIRECTINPUTDEVICEA, "GetDeviceData", &[16, data, out, 0]), 0);
    assert_eq!(m.mem.read_u32(out), 1);
    assert_eq!((m.mem.read_u32(data), m.mem.read_u32(data + 4)), (DIMOFS_BUTTON0, 0x80));
    // Immediate state.
    let stt = m.mem.alloc(16);
    assert_eq!(call(&mut m, dev, IDIRECTINPUTDEVICEA, "GetDeviceState", &[16, stt]), 0);
    assert_eq!(m.mem.read_u8(stt + 12), 0x80);
    assert_eq!(vk_to_dik(0x41), 0x1E);
    assert_eq!(vk_to_dik(0x1B), 0x01);
}

#[test]
fn dplay_factories_and_stubs() {
    use dplay::*;
    let mut m = machine();
    let (_, f) = m.com.factories[&CLSID_DIRECTPLAY];
    let dp = f(&mut m, IID_IDIRECTPLAY4A).unwrap();
    let (_, f) = m.com.factories[&CLSID_DIRECTPLAYLOBBY];
    let lobby = f(&mut m, IID_IDIRECTPLAYLOBBY2A).unwrap();
    let out = m.mem.alloc(4);
    assert_eq!(call(&mut m, dp, IDIRECTPLAY4A, "EnumConnections", &[0, 0, 0, 0]), DP_OK);
    assert_eq!(call(&mut m, dp, IDIRECTPLAY4A, "InitializeConnection", &[0, 0]), DPERR_UNAVAILABLE);
    assert_eq!(call(&mut m, lobby, IDIRECTPLAYLOBBY3A, "GetConnectionSettings", &[0, 0, out]), DPERR_NOTLOBBIED);
    assert_eq!(call(&mut m, dp, IDIRECTPLAY4A, "Release", &[]), 0);
    assert_eq!(call(&mut m, lobby, IDIRECTPLAYLOBBY3A, "Release", &[]), 0);
}
