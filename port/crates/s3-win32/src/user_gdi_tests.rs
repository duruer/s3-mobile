//! Tests for user32 / gdi32 / advapi32 / winmm through the thunk layer: hand-assembled guest snippets
//! (iced-x86 code_asm) and direct thunk calls. No game bytes involved (the optional music test reads the local
//! GOG install at runtime only and is skipped when it is absent).

use iced_x86::code_asm::*;
use s3_mem::layout;
use s3_runtime::{HostEvent, Machine, MouseButton, NullHost, SliceResult, StopReason};

const CODE: u32 = 0x0040_1000;
const DATA: u32 = 0x0048_0000;

fn machine(tag: &str) -> (Machine, std::path::PathBuf) {
    let d = std::env::temp_dir().join(format!("s3ugdi-test-{}-{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("game")).unwrap();
    std::fs::create_dir_all(d.join("data")).unwrap();
    let mut host = NullHost::new(d.join("game"), d.join("data"));
    host.quiet = true;
    let mut m = Machine::new(Box::new(host)).unwrap();
    crate::register_all(&mut m.registry);
    m.mem.alloc_region(Some(layout::IMAGE_BASE), 0x10_0000, s3_mem::RegionKind::Image, (0, 0)).unwrap();
    m.setup_process(layout::IMAGE_BASE, 0x10_0000, CODE, 0x10_0000, "TEST.EXE");
    (m, d)
}

fn f(m: &Machine, dll: &str, name: &str) -> u32 {
    m.registry.lookup(dll, name).unwrap_or_else(|| panic!("{dll}!{name} not registered"))
}

fn load(m: &mut Machine, at: u32, build: impl FnOnce(&mut CodeAssembler)) {
    let mut a = CodeAssembler::new(32).unwrap();
    build(&mut a);
    let bytes = a.assemble(at as u64).unwrap();
    m.mem.write_bytes(at, &bytes);
}

fn call(a: &mut CodeAssembler, addr: u32) {
    a.mov(ecx, addr).unwrap();
    a.call(ecx).unwrap();
}

fn run_to_stop(m: &mut Machine) -> StopReason {
    let t0 = std::time::Instant::now();
    while t0.elapsed().as_secs() < 20 {
        match m.run_slice(1_000_000) {
            SliceResult::Stopped => return m.stop_reason().unwrap().clone(),
            SliceResult::Idle => std::thread::sleep(std::time::Duration::from_millis(1)),
            SliceResult::Ran(_) => {}
        }
    }
    panic!("did not stop");
}

fn direct(tag: &str) -> (Machine, std::path::PathBuf) {
    let (mut m, d) = machine(tag);
    load(&mut m, CODE, |a| a.nop().unwrap());
    let slot = m.create_thread(CODE, 0, 0x1_0000, false).unwrap();
    m.cur = slot;
    (m, d)
}

fn api(m: &mut Machine, dll: &str, name: &str, args: &[u32]) -> u32 {
    let t = f(m, dll, name);
    m.cur = 0;
    let r = m.call_guest(t, args);
    assert!(!m.is_stopped(), "{name}: {:?}", m.stop_reason());
    r
}

fn cstr(m: &mut Machine, s: &str) -> u32 {
    let p = m.mem.alloc(s.len() as u32 + 1);
    m.mem.write_cstr(p, s);
    p
}

/// Register a window class whose WndProc is `wndproc` (0 = DefWindowProc) and create a window.
fn make_window(m: &mut Machine, wndproc: u32) -> u32 {
    let wc = m.mem.alloc(40);
    let name = cstr(m, "TestClass");
    m.mem.write_u32(wc + 4, wndproc);
    m.mem.write_u32(wc + 12, 8); // cbWndExtra
    m.mem.write_u32(wc + 36, name);
    assert_ne!(api(m, "user32.dll", "RegisterClassA", &[wc]), 0);
    let title = cstr(m, "S3");
    api(m, "user32.dll", "CreateWindowExA", &[8, name, title, 0x90C8_0000, 0, 0, 800, 600, 0, 0, 0x40_0000, 0])
}

#[test]
fn message_loop_with_guest_wndproc() {
    let (mut m, _d) = machine("msgloop");
    let u = |m: &Machine, n: &str| f(m, "user32.dll", n);
    let (rc, cw, pm, pq, gm, tm, dm, dwp) =
        (u(&m, "RegisterClassA"), u(&m, "CreateWindowExA"), u(&m, "PostMessageA"), u(&m, "PostQuitMessage"), u(&m, "GetMessageA"), u(&m, "TranslateMessage"), u(&m, "DispatchMessageA"), u(&m, "DefWindowProcA"));
    let ex = f(&m, "kernel32.dll", "ExitProcess");
    let wndproc = CODE + 0x800;
    let wc = DATA + 0x100;
    let cls = DATA + 0x200;
    let msg = DATA + 0x300;
    m.mem.write_cstr(cls, "CWindow");
    m.mem.write_u32(wc + 4, wndproc);
    m.mem.write_u32(wc + 36, cls);
    // WndProc(hwnd, msg, wp, lp): WM_CREATE -> [DATA+12]=1, ret 0; 0x400 -> [DATA+8] += wp, ret 0;
    // WM_ACTIVATEAPP -> [DATA+16] = wp; else tail-jump to DefWindowProcA.
    load(&mut m, wndproc, |a| {
        let mut not_create = a.create_label();
        let mut not_user = a.create_label();
        let mut not_act = a.create_label();
        a.mov(eax, dword_ptr(esp + 8)).unwrap();
        a.cmp(eax, 1).unwrap();
        a.jne(not_create).unwrap();
        a.mov(dword_ptr(DATA + 12), 1).unwrap();
        a.xor(eax, eax).unwrap();
        a.ret_1(16).unwrap();
        a.set_label(&mut not_create).unwrap();
        a.cmp(eax, 0x400).unwrap();
        a.jne(not_user).unwrap();
        a.mov(eax, dword_ptr(esp + 12)).unwrap();
        a.add(dword_ptr(DATA + 8), eax).unwrap();
        a.xor(eax, eax).unwrap();
        a.ret_1(16).unwrap();
        a.set_label(&mut not_user).unwrap();
        a.cmp(eax, 0x1C).unwrap();
        a.jne(not_act).unwrap();
        a.mov(eax, dword_ptr(esp + 12)).unwrap();
        a.mov(dword_ptr(DATA + 16), eax).unwrap();
        a.xor(eax, eax).unwrap();
        a.ret_1(16).unwrap();
        a.set_label(&mut not_act).unwrap();
        a.mov(eax, dwp).unwrap();
        a.jmp(eax).unwrap();
    });
    load(&mut m, CODE, |a| {
        let mut lp = a.create_label();
        let mut done = a.create_label();
        a.push(wc).unwrap();
        call(a, rc);
        for v in [0u32, 0x40_0000, 0, 0, 600, 800, 0, 0, 0x90C8_0000, 0, cls, 8] {
            a.push(v).unwrap();
        }
        call(a, cw);
        a.mov(esi, eax).unwrap();
        for wp in [5u32, 7] {
            a.push(0).unwrap();
            a.push(wp).unwrap();
            a.push(0x400).unwrap();
            a.push(esi).unwrap();
            call(a, pm);
        }
        a.push(3).unwrap();
        call(a, pq);
        a.set_label(&mut lp).unwrap();
        a.push(0).unwrap();
        a.push(0).unwrap();
        a.push(0).unwrap();
        a.push(msg).unwrap();
        call(a, gm);
        a.test(eax, eax).unwrap();
        a.je(done).unwrap();
        a.push(msg).unwrap();
        call(a, tm);
        a.push(msg).unwrap();
        call(a, dm);
        a.jmp(lp).unwrap();
        a.set_label(&mut done).unwrap();
        a.push(dword_ptr(msg + 8)).unwrap(); // WM_QUIT wParam
        call(a, ex);
    });
    m.create_thread(CODE, 0, 0x1_0000, false).unwrap();
    assert_eq!(run_to_stop(&mut m), StopReason::Exited(3));
    assert_eq!(m.mem.read_u32(DATA + 8), 12, "posted WM_USER sum");
    assert_eq!(m.mem.read_u32(DATA + 12), 1, "WM_CREATE seen");
    assert_eq!(m.mem.read_u32(DATA + 16), 1, "WM_ACTIVATEAPP(TRUE) seen");
}

#[test]
fn input_becomes_messages() {
    let (mut m, _d) = direct("input");
    let hwnd = make_window(&mut m, 0);
    assert_ne!(hwnd, 0);
    let msg = m.mem.alloc(28);
    // Drain startup messages (WM_PAINT etc.).
    while api(&mut m, "user32.dll", "PeekMessageA", &[msg, 0, 0, 0, 1]) != 0 {}
    m.input.push(HostEvent::MouseMove { x: 10, y: 20 });
    m.input.push(HostEvent::MouseMove { x: 30, y: 40 });
    m.input.push(HostEvent::MouseButton { button: MouseButton::Left, down: true });
    m.input.push(HostEvent::Key { vk: 0x41, down: true });
    let mut got = Vec::new();
    while api(&mut m, "user32.dll", "PeekMessageA", &[msg, 0, 0, 0, 1]) != 0 {
        got.push((m.mem.read_u32(msg + 4), m.mem.read_u32(msg + 8), m.mem.read_u32(msg + 12)));
        if m.mem.read_u32(msg + 4) == 0x100 {
            api(&mut m, "user32.dll", "TranslateMessage", &[msg]);
        }
    }
    assert_eq!(got[0], (0x200, 0, 40 << 16 | 30), "coalesced WM_MOUSEMOVE");
    assert_eq!(got[1], (0x201, 1, 40 << 16 | 30), "WM_LBUTTONDOWN with MK_LBUTTON");
    assert_eq!(got[2].0, 0x100);
    assert_eq!(got[2].1, 0x41);
    assert_eq!(got[2].2, 1 | 0x1E << 16);
    assert_eq!(got[3], (0x102, b'a' as u32, 1 | 0x1E << 16), "WM_CHAR from TranslateMessage");
    assert_eq!(api(&mut m, "user32.dll", "GetAsyncKeyState", &[1]) & 0x8000, 0x8000);
    let pos = api(&mut m, "user32.dll", "GetMessagePos", &[]);
    assert_eq!(pos, 40 << 16 | 30);
    // Window longs and extra bytes.
    assert_eq!(api(&mut m, "user32.dll", "SetWindowLongA", &[hwnd, 4, 0x1234]), 0);
    assert_eq!(api(&mut m, "user32.dll", "GetWindowLongA", &[hwnd, 4]), 0x1234);
    assert_eq!(api(&mut m, "user32.dll", "GetWindowLongA", &[hwnd, (-16i32) as u32]) & 0x1000_0000, 0x1000_0000);
    // Timers -> WM_TIMER.
    assert_eq!(api(&mut m, "user32.dll", "SetTimer", &[hwnd, 9, 10, 0]), 9);
    std::thread::sleep(std::time::Duration::from_millis(25));
    assert_eq!(api(&mut m, "user32.dll", "PeekMessageA", &[msg, 0, 0, 0, 1]), 1);
    assert_eq!((m.mem.read_u32(msg + 4), m.mem.read_u32(msg + 8)), (0x113, 9));
    // Thread messages and PostQuitMessage ordering.
    let tid = m.current_tid();
    api(&mut m, "user32.dll", "KillTimer", &[hwnd, 9]);
    api(&mut m, "user32.dll", "PostQuitMessage", &[4]);
    api(&mut m, "user32.dll", "PostThreadMessageA", &[tid, 0x8001, 1, 2]);
    assert_eq!(api(&mut m, "user32.dll", "GetMessageA", &[msg, 0, 0, 0]), 1);
    assert_eq!(m.mem.read_u32(msg + 4), 0x8001);
    assert_eq!(api(&mut m, "user32.dll", "GetMessageA", &[msg, 0, 0, 0]), 0);
    assert_eq!(m.mem.read_u32(msg + 8), 4);
}

#[test]
fn rect_helpers_and_wsprintf() {
    let (mut m, _d) = direct("rect");
    let (a, b, c) = (m.mem.alloc(16), m.mem.alloc(16), m.mem.alloc(16));
    api(&mut m, "user32.dll", "SetRect", &[a, 0, 0, 10, 10]);
    api(&mut m, "user32.dll", "SetRect", &[b, 5, 5, 20, 20]);
    assert_eq!(api(&mut m, "user32.dll", "IntersectRect", &[c, a, b]), 1);
    assert_eq!((m.mem.read_u32(c), m.mem.read_u32(c + 8)), (5, 10));
    assert_eq!(api(&mut m, "user32.dll", "PtInRect", &[a, 9, 9]), 1);
    assert_eq!(api(&mut m, "user32.dll", "PtInRect", &[a, 10, 9]), 0);
    api(&mut m, "user32.dll", "SetRect", &[b, 50, 50, 60, 60]);
    assert_eq!(api(&mut m, "user32.dll", "IntersectRect", &[c, a, b]), 0);
    assert_eq!(api(&mut m, "user32.dll", "IsRectEmpty", &[c]), 1);
    let buf = m.mem.alloc(64);
    let fmt = cstr(&mut m, "%s=%03d");
    let s = cstr(&mut m, "x");
    // wsprintfA is cdecl: call_guest restores ESP anyway.
    assert_eq!(api(&mut m, "user32.dll", "wsprintfA", &[buf, fmt, s, 7]), 5);
    assert_eq!(m.mem.read_cstr(buf), "x=007");
}

#[test]
fn registry_api() {
    let (mut m, d) = direct("reg");
    let hk = m.mem.alloc(4);
    let key = cstr(&mut m, r"Software\BlueByte\Siedler3\1.0\General");
    assert_eq!(api(&mut m, "advapi32.dll", "RegOpenKeyExA", &[0x8000_0002, key, 0, 0xF003F, hk]), 0);
    let h = m.mem.read_u32(hk);
    let (ty, data, cb) = (m.mem.alloc(4), m.mem.alloc(64), m.mem.alloc(4));
    let name = cstr(&mut m, "FontSize");
    m.mem.write_u32(cb, 0);
    assert_eq!(api(&mut m, "advapi32.dll", "RegQueryValueExA", &[h, name, 0, ty, 0, cb]), 0);
    assert_eq!(m.mem.read_u32(cb), 4);
    assert_eq!(api(&mut m, "advapi32.dll", "RegQueryValueExA", &[h, name, 0, ty, data, cb]), 0);
    assert_eq!((m.mem.read_u32(ty), m.mem.read_u32(data)), (4, 15));
    let font = cstr(&mut m, "Font");
    m.mem.write_u32(cb, 3);
    assert_eq!(api(&mut m, "advapi32.dll", "RegQueryValueExA", &[h, font, 0, ty, data, cb]), 234, "ERROR_MORE_DATA");
    assert_eq!(m.mem.read_u32(cb), 14);
    let missing = cstr(&mut m, "NoFlip");
    assert_eq!(api(&mut m, "advapi32.dll", "RegQueryValueExA", &[h, missing, 0, ty, data, cb]), 2);
    // Writes persist to data_dir/registry.json.
    let res = cstr(&mut m, "Resolution");
    m.mem.write_u32(data, 1);
    assert_eq!(api(&mut m, "advapi32.dll", "RegSetValueExA", &[h, res, 0, 4, data, 4]), 0);
    let json = std::fs::read_to_string(d.join("data/registry.json")).unwrap();
    assert!(json.contains("\"Resolution\""), "{json}");
    // SingleNumber rewrite is ignored once a value exists.
    let sn = cstr(&mut m, "SingleNumber");
    let plain = cstr(&mut m, "PLAIN");
    assert_eq!(api(&mut m, "advapi32.dll", "RegSetValueExA", &[h, sn, 0, 1, plain, 6]), 0);
    let enc = cstr(&mut m, "ENCRYPTED");
    assert_eq!(api(&mut m, "advapi32.dll", "RegSetValueExA", &[h, sn, 0, 1, enc, 10]), 0);
    m.mem.write_u32(cb, 64);
    assert_eq!(api(&mut m, "advapi32.dll", "RegQueryValueExA", &[h, sn, 0, ty, data, cb]), 0);
    assert_eq!(m.mem.read_cstr(data), "PLAIN");
    assert_eq!(api(&mut m, "advapi32.dll", "RegCloseKey", &[h]), 0);
    // HKCR seed key exists (no serial prompt).
    let seed = cstr(&mut m, r"CLSID\{0e9f8400-d9a5-11cf-8f4d00804880b189}");
    assert_eq!(api(&mut m, "advapi32.dll", "RegOpenKeyExA", &[0x8000_0000, seed, 0, 0xF003F, hk]), 0);
    // Create + disposition.
    let newk = cstr(&mut m, r"Software\BlueByte\Siedler3\1.0\Patches");
    let disp = m.mem.alloc(4);
    assert_eq!(api(&mut m, "advapi32.dll", "RegCreateKeyExA", &[0x8000_0002, newk, 0, 0, 0, 0xF003F, 0, hk, disp]), 0);
    assert_eq!(m.mem.read_u32(disp), 2, "Patches exists by default");
    let user = m.mem.alloc(32);
    m.mem.write_u32(cb, 32);
    assert_eq!(api(&mut m, "advapi32.dll", "GetUserNameA", &[user, cb]), 1);
    assert_eq!(m.mem.read_cstr(user), "Player");
}

#[test]
fn gdi_text_on_surface_dc() {
    let (mut m, _d) = direct("gdi");
    let (w, h) = (120u32, 30u32);
    let pix = m.mem.alloc(w * h * 2);
    let hdc = s3_runtime::gdi::create_dc(&mut m, s3_runtime::DcTarget { addr: pix, pitch: w * 2, width: w, height: h, bpp: 16, rgb565: true });
    assert_ne!(hdc, 0);
    // LOGFONT "MS Sans Serif" height 15.
    let lf = m.mem.alloc(60);
    m.mem.write_u32(lf, 15);
    m.mem.write_u32(lf + 16, 400);
    m.mem.write_cstr(lf + 28, "MS Sans Serif");
    let font = api(&mut m, "gdi32.dll", "CreateFontIndirectA", &[lf]);
    assert_ne!(font, 0);
    api(&mut m, "gdi32.dll", "SelectObject", &[hdc, font]);
    assert_eq!(api(&mut m, "gdi32.dll", "SetBkMode", &[hdc, 1]), 2);
    api(&mut m, "gdi32.dll", "SetTextColor", &[hdc, 0x00FF_FFFF]);
    let tm = m.mem.alloc(56);
    assert_eq!(api(&mut m, "gdi32.dll", "GetTextMetricsA", &[hdc, tm]), 1);
    let th = m.mem.read_u32(tm) as i32;
    assert!((14..=16).contains(&th), "tmHeight {th}");
    let s = cstr(&mut m, "Settlers");
    let size = m.mem.alloc(8);
    assert_eq!(api(&mut m, "gdi32.dll", "GetTextExtentPoint32A", &[hdc, s, 8, size]), 1);
    let tw = m.mem.read_u32(size);
    assert!(tw > 30 && tw < 80, "width {tw}");
    // Clip to the left half: nothing may be drawn right of x=20.
    let rgn = api(&mut m, "gdi32.dll", "CreateRectRgn", &[0, 0, 20, 30]);
    api(&mut m, "gdi32.dll", "SelectClipRgn", &[hdc, rgn]);
    api(&mut m, "gdi32.dll", "TextOutA", &[hdc, 2, 2, s, 8]);
    let px = |m: &Machine, x: u32, y: u32| m.mem.read_u16(pix + (y * w + x) * 2);
    let lit_left = (0..20).flat_map(|x| (0..h).map(move |y| (x, y))).filter(|&(x, y)| px(&m, x, y) != 0).count();
    let lit_right = (20..w).flat_map(|x| (0..h).map(move |y| (x, y))).filter(|&(x, y)| px(&m, x, y) != 0).count();
    assert!(lit_left > 10, "text drawn");
    assert_eq!(lit_right, 0, "clip respected");
    api(&mut m, "gdi32.dll", "SelectClipRgn", &[hdc, 0]);
    api(&mut m, "gdi32.dll", "DeleteObject", &[rgn]);
    // DrawTextA DT_CENTER|DT_SINGLELINE|DT_VCENTER inside a rect + DT_CALCRECT.
    let rc = m.mem.alloc(16);
    for (i, v) in [60u32, 0, 120, 30].iter().enumerate() {
        m.mem.write_u32(rc + 4 * i as u32, *v);
    }
    let hgt = api(&mut m, "user32.dll", "DrawTextA", &[hdc, s, 0xFFFF_FFFF, rc, 0x25]);
    assert_eq!(hgt as i32, th);
    let lit = (60..w).flat_map(|x| (0..h).map(move |y| (x, y))).filter(|&(x, y)| px(&m, x, y) != 0).count();
    assert!(lit > 10);
    assert_eq!(api(&mut m, "user32.dll", "DrawTextA", &[hdc, s, 8, rc, 0x400 | 0x20]) as i32, th);
    assert_eq!(m.mem.read_u32(rc + 8) - m.mem.read_u32(rc), tw);
    // Opaque background fill via ExtTextOutA ETO_OPAQUE.
    api(&mut m, "gdi32.dll", "SetBkColor", &[hdc, 0x0000_00FF]);
    for (i, v) in [0u32, 25, 10, 30].iter().enumerate() {
        m.mem.write_u32(rc + 4 * i as u32, *v);
    }
    api(&mut m, "gdi32.dll", "ExtTextOutA", &[hdc, 0, 25, 2, rc, 0, 0, 0]);
    assert_eq!(px(&m, 5, 27), 0xF800, "red fill in RGB565");
    // Lines.
    let pen = api(&mut m, "gdi32.dll", "CreatePen", &[0, 1, 0x0000_FF00]);
    api(&mut m, "gdi32.dll", "SelectObject", &[hdc, pen]);
    api(&mut m, "gdi32.dll", "MoveToEx", &[hdc, 0, 0, 0]);
    api(&mut m, "gdi32.dll", "LineTo", &[hdc, 10, 0]);
    assert_eq!(px(&m, 9, 0), 0x07E0);
    s3_runtime::gdi::release_dc(&mut m, hdc);
    assert_eq!(api(&mut m, "gdi32.dll", "SetBkMode", &[hdc, 1]), 0, "DC gone after release");
}

#[test]
fn mci_cdaudio_and_avi() {
    let (mut m, d) = direct("mci");
    std::fs::create_dir_all(d.join("game/MUSIC")).unwrap();
    std::fs::write(d.join("game/MUSIC/Track05.ogg"), b"not really ogg").unwrap();
    let ret = m.mem.alloc(64);
    let send = |m: &mut Machine, s: &str| {
        let p = cstr(m, s);
        api(m, "winmm.dll", "mciSendStringA", &[p, ret, 64, 0])
    };
    assert_eq!(send(&mut m, r"open AVIVideo!.\S3\Add\001\01_00_00.dat alias test"), 0, "fake video opens");
    assert_eq!(send(&mut m, "status test window handle"), 0);
    assert_eq!(send(&mut m, "window test handle 65552"), 0);
    assert_eq!(send(&mut m, "play test window notify"), 0);
    assert_eq!(send(&mut m, "close test wait"), 0);
    assert_eq!(send(&mut m, "play nosuch"), 263);
    assert_eq!(send(&mut m, "open cdaudio alias cd"), 0);
    assert_eq!(send(&mut m, "status cd number of tracks"), 0);
    assert_eq!(m.mem.read_cstr(ret), "5");
    assert_eq!(send(&mut m, "set cd time format tmsf"), 0);
    assert_eq!(send(&mut m, "status cd mode"), 0);
    assert_eq!(m.mem.read_cstr(ret), "stopped");
    assert_eq!(send(&mut m, "play cd from 9"), 282, "out of range");
    assert_eq!(send(&mut m, "play cd from 5 to 6"), 262, "bad ogg -> hardware error");
    assert_eq!(send(&mut m, "close cd"), 0);
    // Command interface like S3.EXE 0x615740..0x615fe6.
    let p = m.mem.alloc(32);
    let ty = cstr(&mut m, "cdaudio");
    m.mem.write_u32(p + 8, ty);
    assert_eq!(api(&mut m, "_inmm.dll", "mciSendCommandA", &[0, 0x803, 0x2100, p]), 0);
    let dev = m.mem.read_u32(p + 4);
    assert_ne!(dev, 0);
    m.mem.write_u32(p + 4, 10);
    assert_eq!(api(&mut m, "_inmm.dll", "mciSendCommandA", &[dev, 0x80D, 0x400, p]), 0);
    m.mem.write_u32(p + 8, 3);
    assert_eq!(api(&mut m, "_inmm.dll", "mciSendCommandA", &[dev, 0x814, 0x100, p]), 0);
    assert_eq!(m.mem.read_u32(p + 4), 5);
    assert_eq!(api(&mut m, "_inmm.dll", "mciSendCommandA", &[dev, 0x808, 0, 0]), 0);
    assert_eq!(api(&mut m, "_inmm.dll", "mciSendCommandA", &[dev, 0x804, 0, 0]), 0);
    let buf = m.mem.alloc(128);
    assert_eq!(api(&mut m, "winmm.dll", "mciGetErrorStringA", &[266, buf, 128]), 1);
    assert!(m.mem.read_cstr(buf).contains("driver"));
}

#[test]
fn time_set_event_runs_callback_on_timer_thread() {
    let (mut m, _d) = machine("mmtimer");
    let (tse, ex) = (f(&m, "winmm.dll", "timeSetEvent"), f(&m, "kernel32.dll", "ExitProcess"));
    let cb = CODE + 0x400;
    // TimeProc(id, msg, user, dw1, dw2): [DATA] += 1; [DATA+4] = user
    load(&mut m, cb, |a| {
        a.inc(dword_ptr(DATA)).unwrap();
        a.mov(eax, dword_ptr(esp + 12)).unwrap();
        a.mov(dword_ptr(DATA + 4), eax).unwrap();
        a.ret_1(20).unwrap();
    });
    load(&mut m, CODE, |a| {
        let mut spin = a.create_label();
        a.push(1).unwrap(); // TIME_PERIODIC
        a.push(0x77).unwrap();
        a.push(cb).unwrap();
        a.push(0).unwrap();
        a.push(5).unwrap();
        call(a, tse);
        a.set_label(&mut spin).unwrap();
        a.cmp(dword_ptr(DATA), 3).unwrap();
        a.jb(spin).unwrap();
        a.push(dword_ptr(DATA + 4)).unwrap();
        call(a, ex);
    });
    m.create_thread(CODE, 0, 0x1_0000, false).unwrap();
    m.quantum = 20_000;
    assert_eq!(run_to_stop(&mut m), StopReason::Exited(0x77));
}

/// Decodes a real track of the local GOG install if present (runtime only, nothing is copied).
#[test]
fn music_decodes_local_gog_track() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../game-files/gog-s3-ultimate/MUSIC/Track02.ogg");
    if !dir.exists() {
        eprintln!("skipping: no local GOG install");
        return;
    }
    let ms = crate::audio_music::ogg_duration_ms(&dir).unwrap();
    assert!(ms > 30_000, "duration {ms}");
    let mut p = crate::audio_music::MusicPlayer::default();
    let path = dir.clone();
    p.set_resolver(Box::new(move |n| if n == 2 { Some(path.clone()) } else { None }));
    p.play(2, 3, 0).unwrap();
    let mut buf = vec![0i16; 22050 * 2];
    assert_eq!(p.mix(&mut buf, 2, 22050, 0), None);
    assert!(buf.iter().any(|&s| s != 0), "decoded audio is silent");
}

/// Integration r1: S3.EXE (0x4a8233) checks `SelectObject(CreateCompatibleDC(0), bitmap) != 0` and throws a C++
/// exception otherwise (-> "abnormal program termination" when opening the build menu).
#[test]
fn select_bitmap_into_memory_dc_returns_previous() {
    let (mut m, _d) = direct("gdi-memdc");
    let mdc = api(&mut m, "gdi32.dll", "CreateCompatibleDC", &[0]);
    assert_ne!(mdc, 0);
    let bmp = api(&mut m, "gdi32.dll", "CreateCompatibleBitmap", &[mdc, 16, 16]);
    assert_ne!(bmp, 0);
    let old = api(&mut m, "gdi32.dll", "SelectObject", &[mdc, bmp]);
    assert_ne!(old, 0, "first SelectObject(bitmap) must return the default bitmap");
    assert_eq!(api(&mut m, "gdi32.dll", "SelectObject", &[mdc, old]), bmp);
    assert_eq!(api(&mut m, "gdi32.dll", "SelectObject", &[mdc, bmp]), old);
}

/// A tap (s3run `--inject click`, Android touch) arrives as MouseMove + down + up in one batch. S3's UI needs exactly
/// WM_MOUSEMOVE, WM_LBUTTONDOWN, WM_LBUTTONUP at the tap position, in that order, for every tap: one tap = one
/// button press (the "first OK on the players screen is ignored" report was a miscount, see user32-gdi32.md).
#[test]
fn tap_gives_one_press_per_tap() {
    let (mut m, _d) = direct("tap");
    let hwnd = make_window(&mut m, 0);
    let msg = m.mem.alloc(28);
    while api(&mut m, "user32.dll", "PeekMessageA", &[msg, 0, 0, 0, 1]) != 0 {}
    let lp = 567 << 16 | 730;
    for round in 0..2 {
        m.input.push(HostEvent::MouseMove { x: 730, y: 567 });
        m.input.push(HostEvent::MouseButton { button: MouseButton::Left, down: true });
        m.input.push(HostEvent::MouseButton { button: MouseButton::Left, down: false });
        let mut got = Vec::new();
        while api(&mut m, "user32.dll", "PeekMessageA", &[msg, 0, 0, 0, 1]) != 0 {
            got.push((m.mem.read_u32(msg), m.mem.read_u32(msg + 4), m.mem.read_u32(msg + 8), m.mem.read_u32(msg + 12)));
        }
        // The second tap repeats at the same spot (as when OK buttons of consecutive screens share a position).
        assert_eq!(got, vec![(hwnd, 0x200, 0, lp), (hwnd, 0x201, 1, lp), (hwnd, 0x202, 0, lp)], "tap {round}");
    }
    // CRT lookups through GetProcAddress (were missing: logged "not declared").
    let un = cstr(&mut m, "USER32.dll");
    let hu = api(&mut m, "kernel32.dll", "LoadLibraryA", &[un]);
    let pn = cstr(&mut m, "GetLastActivePopup");
    let p = api(&mut m, "kernel32.dll", "GetProcAddress", &[hu, pn]);
    assert_ne!(p, 0);
    assert_eq!(m.call_guest(p, &[hwnd]), hwnd);
    let kn = cstr(&mut m, "KERNEL32.dll");
    let hk = api(&mut m, "kernel32.dll", "LoadLibraryA", &[kn]);
    let fname = cstr(&mut m, "IsProcessorFeaturePresent");
    let ipf = api(&mut m, "kernel32.dll", "GetProcAddress", &[hk, fname]);
    assert_ne!(ipf, 0);
    assert_eq!(m.call_guest(ipf, &[0]), 0, "PF_FLOATING_POINT_PRECISION_ERRATA");
    assert_eq!(m.call_guest(ipf, &[1]), 0, "PF_FLOATING_POINT_EMULATED");
    assert_eq!(m.call_guest(ipf, &[2]), 1, "PF_COMPARE_EXCHANGE_DOUBLE (CPUID CX8)");
    assert_eq!(m.call_guest(ipf, &[3]), 0, "no MMX (CPUID has no MMX bit)");
    assert_eq!(m.call_guest(ipf, &[8]), 1, "PF_RDTSC_INSTRUCTION_AVAILABLE");
}

/// S3.EXE renders in-game panel text (Save Game / Quit Game, tooltips) with DrawTextA into a 640x480x1 top-down
/// DIB section (white text on black, OPAQUE) and reads the bits as a mask. 1bpp pixels must be packed MSB first.
#[test]
fn draw_text_into_monochrome_dib() {
    let (mut m, _d) = direct("gdi-1bpp");
    let bmi = m.mem.alloc(48);
    m.mem.write_u32(bmi, 40);
    m.mem.write_u32(bmi + 4, 64);
    m.mem.write_u32(bmi + 8, (-16i32) as u32);
    m.mem.write_u16(bmi + 12, 1);
    m.mem.write_u16(bmi + 14, 1);
    m.mem.write_u32(bmi + 40, 0); // colour 0 black
    m.mem.write_u32(bmi + 44, 0xFF_FFFF); // colour 1 white
    let ppv = m.mem.alloc(4);
    let hb = api(&mut m, "gdi32.dll", "CreateDIBSection", &[0, bmi, 0, ppv, 0, 0]);
    assert_ne!(hb, 0);
    let bits = m.mem.read_u32(ppv);
    let mdc = api(&mut m, "gdi32.dll", "CreateCompatibleDC", &[0]);
    api(&mut m, "gdi32.dll", "SelectObject", &[mdc, hb]);
    api(&mut m, "gdi32.dll", "SetBkMode", &[mdc, 2]);
    api(&mut m, "gdi32.dll", "SetBkColor", &[mdc, 0]);
    api(&mut m, "gdi32.dll", "SetTextColor", &[mdc, 0xFF_FFFF]);
    let rc = m.mem.alloc(16);
    for (i, v) in [0u32, 0, 64, 16].iter().enumerate() {
        m.mem.write_u32(rc + 4 * i as u32, *v);
    }
    let s = cstr(&mut m, "Save");
    assert!(api(&mut m, "user32.dll", "DrawTextA", &[mdc, s, 4, rc, 0x15]) > 0);
    // stride = 8 bytes; some bits set, nothing written past the 16 rows.
    let set: u32 = (0..8 * 16).map(|i| m.mem.read_u8(bits + i).count_ones()).sum();
    assert!(set > 20, "text pixels in the mask: {set}");
    assert!(set < 64 * 16 / 2, "mostly background: {set}");
    let last_row: u32 = (0..8).map(|i| m.mem.read_u8(bits + 15 * 8 + i).count_ones()).sum();
    assert_eq!(last_row, 0);
}
