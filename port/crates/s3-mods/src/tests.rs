//! Machine-level tests: hooks on hand-assembled guest code, post-call trampolines, guest calls, deferred
//! work, touch pipeline, overlay compose. No game bytes.

use std::sync::{Arc, Mutex};

use iced_x86::code_asm::*;
use s3_cpu::{EBX, ECX, EDI, ESI};
use s3_mem::layout;
use s3_runtime::*;

use crate::*;

const CODE: u32 = 0x0040_1000;
const ADD2: u32 = 0x0040_2000; // stdcall add2(a, b)
const DBL: u32 = 0x0040_2100; // cdecl dbl(a)
const METHOD: u32 = 0x0040_2200; // thiscall: [ecx] + arg, ret 4
const OBJ: u32 = 0x0050_0000;

#[derive(Default)]
struct Frames(Vec<FrameBuffer>);

struct TestHost {
    data: std::path::PathBuf,
    frames: Arc<Mutex<Frames>>,
    start: std::time::Instant,
}

impl Host for TestHost {
    fn present(&mut self, fb: &FrameBuffer) {
        self.frames.lock().unwrap().0.push(fb.clone());
    }
    fn poll_events(&mut self) -> Vec<HostEvent> {
        Vec::new()
    }
    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }
    fn game_dir(&self) -> std::path::PathBuf {
        self.data.clone()
    }
    fn data_dir(&self) -> std::path::PathBuf {
        self.data.clone()
    }
    fn log(&self, _l: LogLevel, _m: &str) {}
}

fn asm(m: &mut Machine, at: u32, build: impl FnOnce(&mut CodeAssembler)) {
    let mut a = CodeAssembler::new(32).unwrap();
    build(&mut a);
    let bytes = a.assemble(at as u64).unwrap();
    m.mem.write_bytes(at, &bytes);
}

fn h_exit(m: &mut Machine) -> ThunkResult {
    ThunkResult::Exit(m.arg_u32(0))
}

/// Machine with test functions and a main program; returns (machine, frames, address of `mov edi, eax`).
fn machine(tag: &str) -> (Machine, Arc<Mutex<Frames>>) {
    let d = std::env::temp_dir().join(format!("s3mods-test-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let frames = Arc::new(Mutex::new(Frames::default()));
    let host = TestHost { data: d, frames: frames.clone(), start: std::time::Instant::now() };
    let mut m = Machine::new(Box::new(host)).unwrap();
    m.mem.alloc_region(Some(layout::IMAGE_BASE), 0x20_0000, s3_mem::RegionKind::Image, (0, 0)).unwrap();
    m.setup_process(layout::IMAGE_BASE, 0x20_0000, CODE, 0x10_0000, "TEST.EXE");
    let exit = m.registry.register("test.dll", "exit", h_exit);
    asm(&mut m, ADD2, |a| {
        a.mov(eax, dword_ptr(esp + 4)).unwrap();
        a.add(eax, dword_ptr(esp + 8)).unwrap();
        a.ret_1(8).unwrap();
    });
    asm(&mut m, DBL, |a| {
        a.mov(eax, dword_ptr(esp + 4)).unwrap();
        a.add(eax, eax).unwrap();
        a.ret().unwrap();
    });
    asm(&mut m, METHOD, |a| {
        a.mov(eax, dword_ptr(ecx)).unwrap();
        a.add(eax, dword_ptr(esp + 4)).unwrap();
        a.ret_1(4).unwrap();
    });
    m.mem.write_u32(OBJ, 1000);
    asm(&mut m, CODE, |a| {
        a.push(7).unwrap();
        a.push(5).unwrap();
        a.mov(eax, ADD2).unwrap();
        a.call(eax).unwrap();
        a.mov(ebx, eax).unwrap();
        a.push(3).unwrap();
        a.mov(eax, DBL).unwrap();
        a.call(eax).unwrap();
        a.add(esp, 4).unwrap();
        a.mov(esi, eax).unwrap(); // CODE + 0x1c (checked below)
        a.mov(ecx, OBJ).unwrap();
        a.push(10).unwrap();
        a.mov(eax, METHOD).unwrap();
        a.call(eax).unwrap();
        a.mov(edi, eax).unwrap();
        a.push(0x42).unwrap();
        a.mov(eax, exit).unwrap();
        a.call(eax).unwrap();
    });
    m.create_thread(CODE, 0, 0x10_000, false).unwrap();
    install(&mut m, b"not the real exe");
    (m, frames)
}

fn run(m: &mut Machine) {
    for _ in 0..100 {
        if m.run_slice(1_000_000) == SliceResult::Stopped {
            assert_eq!(m.stop_reason(), Some(&StopReason::Exited(0x42)), "{:?}", m.stop_reason());
            return;
        }
    }
    panic!("did not stop");
}

/// Address of the `mov esi, eax` instruction in the main program.
fn mov_esi_addr(m: &Machine) -> u32 {
    let bytes = m.mem.read_bytes(CODE, 0x40);
    let mut dec = iced_x86::Decoder::with_ip(32, &bytes, CODE as u64, 0);
    for ins in &mut dec {
        if ins.code() == iced_x86::Code::Mov_rm32_r32 || ins.code() == iced_x86::Code::Mov_r32_rm32 {
            if ins.op0_register() == iced_x86::Register::ESI {
                return ins.ip32();
            }
        }
    }
    panic!("mov esi not found");
}

#[test]
fn unhooked_baseline() {
    let (mut m, _) = machine("base");
    assert!(m.hook_addrs().is_none(), "unknown exe: no hooks");
    run(&mut m);
    let r = m.threads[0].cpu.regs;
    assert_eq!((r[EBX], r[ESI], r[EDI]), (12, 6, 1010));
}

#[test]
fn skip_function_post_call_and_mid_block_hook() {
    let (mut m, _) = machine("hooks");
    add_hook(&mut m, ADD2, "skip", |c| {
        assert_eq!((c.arg(0), c.arg(1)), (5, 7));
        c.return_from_function(100, 2)
    });
    add_hook(&mut m, DBL, "post", |c| {
        c.on_return(|p| {
            assert_eq!(p.eax, 6);
            let v = p.eax + 1000;
            p.set_eax(v);
        });
        HookFlow::Continue
    });
    let seen = Arc::new(Mutex::new(0u32));
    let s2 = seen.clone();
    let at = mov_esi_addr(&m);
    add_hook(&mut m, at, "mid", move |c| {
        *s2.lock().unwrap() = c.reg(s3_cpu::EAX);
        HookFlow::Continue
    });
    hooks::commit(&mut m);
    run(&mut m);
    let r = m.threads[0].cpu.regs;
    assert_eq!(r[EBX], 100, "skipped add2");
    assert_eq!(r[ESI], 1006, "post-call changed eax");
    assert_eq!(r[EDI], 1010);
    assert_eq!(*seen.lock().unwrap(), 1006, "mid-block hook ran before mov esi");
    let stats = hooks::hook_stats(&mut m);
    assert!(stats.iter().all(|(_, _, hits)| *hits == 1), "{stats:?}");
    assert!(m.stats.hooks >= 3);
}

#[test]
fn guest_calls_from_hook_and_deferred_safe_point() {
    let (mut m, _) = machine("calls");
    let got = Arc::new(Mutex::new(Vec::new()));
    let g2 = got.clone();
    let inside = Arc::new(std::sync::atomic::AtomicBool::new(false));
    add_hook(&mut m, DBL, "call", move |c| {
        // Hooks fire again for guest calls made from the hook itself: guard against recursion.
        if inside.swap(true, std::sync::atomic::Ordering::SeqCst) {
            g2.lock().unwrap().push((0, 0, 0, 0));
            return HookFlow::Continue;
        }
        let ecx_before = c.reg(ECX);
        let a = call::call_stdcall(c.m, ADD2, &[2, 3]);
        let b = call::call_cdecl(c.m, DBL, &[21]); // nested: the hook itself fires again (inner call)
        let t = call::call_thiscall(c.m, METHOD, OBJ, &[5]);
        let v = call::call_virtual(c.m, OBJ + 0x10, 0, &[1]);
        assert_eq!(c.reg(ECX), ecx_before, "thiscall must not leak ECX");
        g2.lock().unwrap().push((a, b, t, v));
        inside.store(false, std::sync::atomic::Ordering::SeqCst);
        HookFlow::Continue
    });
    // Virtual table for call_virtual: object at OBJ+0x10 -> vtbl at OBJ+0x20 -> METHOD; [obj] is the vtbl
    // pointer, so METHOD reads [ecx] = vtbl address.
    m.mem.write_u32(OBJ + 0x10, OBJ + 0x20);
    m.mem.write_u32(OBJ + 0x20, METHOD);
    let ran = Arc::new(Mutex::new(0u32));
    let r2 = ran.clone();
    let at = mov_esi_addr(&m);
    add_hook(&mut m, at, "safe", |_| HookFlow::Continue);
    add_safe_point(&mut m, at);
    defer(&mut m, move |m| {
        *r2.lock().unwrap() = call::call_stdcall(m, ADD2, &[40, 2]);
    });
    hooks::commit(&mut m);
    run(&mut m);
    let v = got.lock().unwrap().clone();
    // Outer hook call: (5, 42, 1005, OBJ+0x20+1); the inner DBL call re-entered the hook once (guarded).
    assert_eq!(v, vec![(0, 0, 0, 0), (5, 42, 1005, OBJ + 0x20 + 1)]);
    assert!(v.contains(&(5, 42, 1005, OBJ + 0x20 + 1)), "{v:?}");
    assert_eq!(*ran.lock().unwrap(), 42);
    let r = m.threads[0].cpu.regs;
    assert_eq!((r[EBX], r[ESI], r[EDI]), (12, 6, 1010), "guest results unchanged");
}

fn pointer(m: &mut Machine, id: u32, phase: PointerPhase, x: i32, y: i32) -> Vec<HostEvent> {
    let f = m.ext.event_filter.unwrap();
    let mut out = Vec::new();
    f(m, HostEvent::Pointer { id, phase, x, y, wx: -1, wy: -1 }, &mut out);
    out
}

struct Button {
    hits: Arc<Mutex<Vec<String>>>,
}

impl Mod for Button {
    fn name(&self) -> &str {
        "button"
    }
    fn on_touch(&mut self, _m: &mut Machine, ev: &TouchEvent) -> ModEvent {
        match *ev {
            TouchEvent::Down { x, .. } if x < 50.0 => {
                self.hits.lock().unwrap().push("down".into());
                ModEvent::Consumed
            }
            TouchEvent::Up { .. } | TouchEvent::Move { .. } => {
                self.hits.lock().unwrap().push(format!("{ev:?}"));
                ModEvent::Pass
            }
            _ => ModEvent::Pass,
        }
    }
    fn wants_draw_frame(&self) -> bool {
        true
    }
    fn draw(&mut self, _m: &mut Machine, c: &mut Canvas) {
        c.fill_rect(0, 0, 4, 4, Rgba::rgb(255, 0, 0));
    }
}

#[test]
fn touch_pipeline_fallback_and_capture() {
    let (mut m, frames) = machine("touch");
    // Tap on the "map" -> fallback left click.
    let out1 = pointer(&mut m, 1, PointerPhase::Down, 300, 200);
    assert!(out1.is_empty(), "{out1:?}");
    let out = pointer(&mut m, 1, PointerPhase::Up, 301, 200);
    assert_eq!(out, vec![HostEvent::MouseMove { x: 300, y: 200 }, HostEvent::MouseButton { button: MouseButton::Left, down: true }]);
    std::thread::sleep(std::time::Duration::from_millis(70));
    m.run_slice(10); // service drains the delayed button-up into m.input
    assert!(!m.input.buttons[0]);
    // A mod button captures pointers that start on it.
    let hits = Arc::new(Mutex::new(Vec::new()));
    add_mod(&mut m, Box::new(Button { hits: hits.clone() }));
    assert!(pointer(&mut m, 2, PointerPhase::Down, 10, 10).is_empty());
    assert!(pointer(&mut m, 2, PointerPhase::Move, 200, 10).is_empty(), "captured: no drag");
    assert!(pointer(&mut m, 2, PointerPhase::Up, 200, 10).is_empty(), "captured: no tap");
    let h = hits.lock().unwrap().clone();
    assert_eq!(h.len(), 3, "{h:?}");
    // Overlay: the mod draws on presented frames.
    let fb = FrameBuffer::new(8, 8);
    m.present(&fb);
    let f = frames.lock().unwrap().0.last().cloned().unwrap();
    assert_eq!(f.pixels[0], 0xF800);
    assert_eq!(f.pixels[7 * 8 + 7], 0);
    assert_eq!(m.last_frame_size(), (8, 8));
    // Keys pass through untouched.
    let f = m.ext.event_filter.unwrap();
    let mut out = Vec::new();
    f(&mut m, HostEvent::Key { vk: 0x1B, down: true }, &mut out);
    assert_eq!(out, vec![HostEvent::Key { vk: 0x1B, down: true }]);
}
