use iced_x86::code_asm::*;

use s3_cpu::{EAX, EBX, ESP};
use s3_mem::layout;

use crate::*;

const CODE: u32 = 0x0040_1000;

fn tmpdir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("s3rt-test-{}-{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn machine() -> Machine {
    let d = tmpdir("m");
    let mut host = NullHost::new(d.join("game"), d.join("data"));
    host.quiet = true;
    let mut m = Machine::new(Box::new(host)).unwrap();
    m.mem.alloc_region(Some(layout::IMAGE_BASE), 0x10_0000, s3_mem::RegionKind::Image, (0, 0)).unwrap();
    m.setup_process(layout::IMAGE_BASE, 0x10_0000, CODE, 0x10_0000, "TEST.EXE");
    m
}

fn load(m: &mut Machine, at: u32, build: impl FnOnce(&mut CodeAssembler)) {
    let mut a = CodeAssembler::new(32).unwrap();
    build(&mut a);
    let bytes = a.assemble(at as u64).unwrap();
    m.mem.write_bytes(at, &bytes);
}

fn run_to_stop(m: &mut Machine) -> StopReason {
    for _ in 0..1000 {
        if m.run_slice(1_000_000) == SliceResult::Stopped {
            return m.stop_reason().unwrap().clone();
        }
    }
    panic!("did not stop");
}

fn h_add2(m: &mut Machine) -> ThunkResult {
    let s = m.arg_u32(0) + m.arg_u32(1);
    ThunkResult::stdcall(s, 2)
}

fn h_exit(m: &mut Machine) -> ThunkResult {
    ThunkResult::Exit(m.arg_u32(0))
}

#[test]
fn registry_basics() {
    let mut r = Registry::new();
    r.alias("_inmm.dll", "winmm.dll");
    let a = r.declare("KERNEL32", "Sleep");
    assert_eq!(r.lookup("kernel32.dll", "Sleep"), Some(a));
    let b = r.register("kernel32.dll", "Sleep", h_add2);
    assert_eq!(a, b, "register keeps the declared address");
    assert!(r.entry(a).unwrap().handler.is_some());
    let t = r.declare("winmm.dll", "timeGetTime");
    assert_eq!(r.resolve_import("_INMM.dll", "timeGetTime"), t);
    let auto = r.resolve_import("foo.dll", "Bar");
    assert!(r.entry(auto).unwrap().auto);
    assert_eq!(r.describe(auto), "foo.dll!Bar");
    let o = r.resolve_import_ordinal("wsock32.dll", 23);
    assert_eq!(r.lookup_ordinal("WSOCK32", 23), Some(o));
    let hm = r.module_handle("KERNEL32.DLL").unwrap();
    assert_eq!(r.module_name(hm), Some("kernel32.dll"));
    assert_eq!(r.module_handle("_inmm.dll"), r.module_handle("winmm.dll"));
    assert!(r.module_handle("nothere.dll").is_none());
}

#[test]
fn vfs_case_insensitive_and_overlay() {
    let d = tmpdir("vfs");
    let game = d.join("game");
    std::fs::create_dir_all(game.join("Gfx")).unwrap();
    std::fs::write(game.join("Gfx").join("Siedler3_00.f8007e01f.DAT"), b"x").unwrap();
    std::fs::write(game.join("S3.EXE"), b"mz").unwrap();
    let mut v = Vfs::new(game.clone(), d.join("data"));
    assert_eq!(v.components("C:\\S3\\GFX\\a.dat"), vec!["GFX", "a.dat"]);
    assert_eq!(v.components("d:\\gfx\\..\\snd\\x"), vec!["snd", "x"]);
    assert_eq!(v.components("gfx/x.dat"), vec!["gfx", "x.dat"]);
    let p = v.resolve_read("C:\\S3\\gfx\\siedler3_00.F8007E01F.dat").unwrap();
    assert!(p.ends_with("Gfx/Siedler3_00.f8007e01f.DAT"));
    assert!(v.resolve_read("gfx\\missing.dat").is_none());
    // Writes go to the overlay and shadow the game dir.
    let w = v.resolve_write("Save\\game1.sav").unwrap();
    std::fs::write(&w, b"save").unwrap();
    assert!(w.starts_with(d.join("data").join("overlay")));
    assert_eq!(v.resolve_read("SAVE\\GAME1.SAV").unwrap(), w);
    let up = v.copy_up("s3.exe").unwrap();
    assert_eq!(std::fs::read(&up).unwrap(), b"mz");
    assert_eq!(v.resolve_read("S3.exe").unwrap(), up);
    let names: Vec<String> = v.list_dir("C:\\S3").into_iter().map(|e| e.name).collect();
    assert_eq!(names.iter().filter(|n| n.eq_ignore_ascii_case("s3.exe")).count(), 1);
    assert!(names.iter().any(|n| n == "Gfx"));
    assert!(v.set_cwd("gfx"));
    assert_eq!(v.cwd, "C:\\S3\\gfx");
    assert!(!v.set_cwd("nope"));
    assert!(vfs::wildcard_match("*.dat", "A.DAT"));
    assert!(vfs::wildcard_match("s?.exe", "S3.EXE"));
    assert!(!vfs::wildcard_match("*.dat", "a.dax"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn thunk_call_stdcall_and_exit() {
    let mut m = machine();
    let add = m.registry.register("test.dll", "Add2", h_add2);
    let exit = m.registry.register("test.dll", "Exit", h_exit);
    load(&mut m, CODE, |a| {
        a.push(30).unwrap();
        a.push(12).unwrap();
        a.mov(ecx, add).unwrap();
        a.call(ecx).unwrap();
        a.push(eax).unwrap();
        a.mov(ecx, exit).unwrap();
        a.call(ecx).unwrap();
    });
    m.create_thread(CODE, 0, 0x1_0000, false).unwrap();
    assert_eq!(run_to_stop(&mut m), StopReason::Exited(42));
    assert_eq!(m.registry.entry(add).unwrap().calls, 1);
}

#[test]
fn unimplemented_stops_with_name() {
    let mut m = machine();
    let f = m.registry.resolve_import("KERNEL32.dll", "GetVersion");
    load(&mut m, CODE, |a| {
        a.mov(ecx, f).unwrap();
        a.call(ecx).unwrap();
    });
    m.create_thread(CODE, 0, 0x1_0000, false).unwrap();
    match run_to_stop(&mut m) {
        StopReason::Unimplemented(s) => assert!(s.contains("UNIMPLEMENTED kernel32.dll!GetVersion"), "{s}"),
        r => panic!("{r:?}"),
    }
}

fn h_callback(m: &mut Machine) -> ThunkResult {
    // Calls the guest function passed as arg 0 with (arg1) and returns its result + 1.
    let f = m.arg_u32(0);
    let x = m.arg_u32(1);
    let r = m.call_guest(f, &[x]);
    ThunkResult::stdcall(r + 1, 2)
}

#[test]
fn call_guest_nested() {
    let mut m = machine();
    let cb = m.registry.register("test.dll", "Callback", h_callback);
    let exit = m.registry.register("test.dll", "Exit", h_exit);
    let func = CODE + 0x100;
    // func(x) (stdcall): returns x*2, clobbers ebx (must be restored by call_guest).
    load(&mut m, func, |a| {
        a.mov(eax, dword_ptr(esp + 4)).unwrap();
        a.add(eax, eax).unwrap();
        a.mov(ebx, 0xBAD).unwrap();
        a.ret_1(4).unwrap();
    });
    load(&mut m, CODE, |a| {
        a.mov(ebx, 7).unwrap();
        a.push(20).unwrap();
        a.push(func).unwrap();
        a.mov(ecx, cb).unwrap();
        a.call(ecx).unwrap(); // 41
        a.add(eax, ebx).unwrap(); // + 7 = 48
        a.push(eax).unwrap();
        a.mov(ecx, exit).unwrap();
        a.call(ecx).unwrap();
    });
    m.create_thread(CODE, 0, 0x1_0000, false).unwrap();
    assert_eq!(run_to_stop(&mut m), StopReason::Exited(48));
}

// Blocking wait on an event that another thread sets.
fn h_wait(m: &mut Machine) -> ThunkResult {
    let h = m.arg_u32(0);
    if m.try_acquire(h) {
        ThunkResult::stdcall(0, 1)
    } else {
        ThunkResult::Block
    }
}

fn h_set(m: &mut Machine) -> ThunkResult {
    let h = m.arg_u32(0);
    if let Some(KernelObject::Event { signaled, .. }) = m.handles.get_mut(h) {
        *signaled = true;
    }
    ThunkResult::stdcall(1, 1)
}

#[test]
fn threads_block_and_wake() {
    let mut m = machine();
    let wait = m.registry.register("test.dll", "Wait", h_wait);
    let set = m.registry.register("test.dll", "Set", h_set);
    let exit = m.registry.register("test.dll", "Exit", h_exit);
    let ev = m.handles.insert(KernelObject::Event { manual_reset: false, signaled: false });
    let data = 0x0048_0000;
    // Thread A: wait(ev); exit([data])
    load(&mut m, CODE, |a| {
        a.push(ev).unwrap();
        a.mov(ecx, wait).unwrap();
        a.call(ecx).unwrap();
        a.push(dword_ptr(data)).unwrap();
        a.mov(ecx, exit).unwrap();
        a.call(ecx).unwrap();
    });
    // Thread B (param = value): spin a while, store value, set(ev), return 0 (thread exit)
    let b = CODE + 0x200;
    load(&mut m, b, |a| {
        let mut l = a.create_label();
        a.mov(ecx, 300_000).unwrap();
        a.set_label(&mut l).unwrap();
        a.dec(ecx).unwrap();
        a.jnz(l).unwrap();
        a.mov(eax, dword_ptr(esp + 4)).unwrap();
        a.mov(dword_ptr(data), eax).unwrap();
        a.push(ev).unwrap();
        a.mov(ecx, set).unwrap();
        a.call(ecx).unwrap();
        a.xor(eax, eax).unwrap();
        a.ret_1(4).unwrap();
    });
    m.create_thread(CODE, 0, 0x1_0000, false).unwrap();
    m.create_thread(b, 77, 0x1_0000, false).unwrap();
    m.quantum = 10_000;
    assert_eq!(run_to_stop(&mut m), StopReason::Exited(77));
    assert!(matches!(m.threads[1].state, ThreadState::Exited(0)));
    assert_eq!(m.is_signaled(m.threads[1].handle), Some(true));
}

#[test]
fn seh_handler_resumes_after_divide_error() {
    let mut m = machine();
    let exit = m.registry.register("test.dll", "Exit", h_exit);
    let handler = CODE + 0x300;
    // handler(rec, frame, ctx, disp): ctx->Eip += 2; ctx->Ecx = 5; return ExceptionContinueExecution
    load(&mut m, handler, |a| {
        a.mov(eax, dword_ptr(esp + 12)).unwrap();
        a.add(dword_ptr(eax + 0xB8), 2).unwrap();
        a.mov(dword_ptr(eax + 0xAC), 5).unwrap();
        a.xor(eax, eax).unwrap();
        a.ret().unwrap();
    });
    load(&mut m, CODE, |a| {
        a.push(handler).unwrap();
        a.push(dword_ptr(0).fs()).unwrap();
        a.mov(dword_ptr(0).fs(), esp).unwrap();
        a.xor(ecx, ecx).unwrap();
        a.div(ecx).unwrap(); // 2 bytes: f7 f1
        a.pop(dword_ptr(0).fs()).unwrap();
        a.add(esp, 4).unwrap();
        a.push(ecx).unwrap();
        a.mov(ecx, exit).unwrap();
        a.call(ecx).unwrap();
    });
    m.create_thread(CODE, 0, 0x1_0000, false).unwrap();
    assert_eq!(run_to_stop(&mut m), StopReason::Exited(5));
    assert_eq!(m.stats.exceptions, 1);
}

#[test]
fn unhandled_exception_stops() {
    let mut m = machine();
    load(&mut m, CODE, |a| {
        a.xor(eax, eax).unwrap();
        a.mov(dword_ptr(eax), 1).unwrap();
    });
    m.create_thread(CODE, 0, 0x1_0000, false).unwrap();
    match run_to_stop(&mut m) {
        StopReason::Fault(s) => assert!(s.contains("0xc0000005"), "{s}"),
        r => panic!("{r:?}"),
    }
}

#[test]
fn teb_and_args_and_state() {
    let mut m = machine();
    load(&mut m, CODE, |a| {
        a.nop().unwrap();
    });
    let slot = m.create_thread(CODE, 0x55, 0x1_0000, false).unwrap();
    m.cur = slot;
    let t = &m.threads[slot];
    assert_eq!(m.mem.read_u32(t.teb + 0x18), t.teb);
    assert_eq!(m.mem.read_u32(t.teb + 0x30), layout::PEB_ADDR);
    assert_eq!(m.mem.read_u32(t.teb), 0xFFFF_FFFF);
    assert_eq!(t.cpu.fs_base, t.teb);
    // At entry: [esp] = thread_exit thunk, [esp+4] = param.
    assert_eq!(m.arg_u32(0), 0x55);
    m.set_last_error(123);
    assert_eq!(m.last_error(), 123);
    assert_eq!(m.mem.read_cstr(m.process.cmdline_a), "TEST.EXE");
    assert_eq!(m.process.module_path, "C:\\S3\\TEST.EXE");
    #[derive(Default)]
    struct Counter(u32);
    m.state::<Counter>().borrow_mut().0 += 2;
    assert_eq!(m.state::<Counter>().borrow_mut().0, 2);
    let _ = (EAX, EBX, ESP);
}

// ---------------------------------------------------------------------------------------------- multi-core mode

/// Set when a present ran on the draw worker (multi-core test).
static MT_PRESENT_OFFLOADED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn h_wait1(m: &mut Machine) -> ThunkResult {
    let h = m.arg_u32(0);
    match m.poll_wait(&[h], false, 0xFFFF_FFFF) {
        WaitPoll::Ready(_) => ThunkResult::stdcall(0, 1),
        WaitPoll::Pending => ThunkResult::Block,
        _ => ThunkResult::Error("wait failed".into()),
    }
}

fn h_set1(m: &mut Machine) -> ThunkResult {
    let h = m.arg_u32(0);
    if let Some(KernelObject::Event { signaled, .. }) = m.handles.get_mut(h) {
        *signaled = true;
    }
    ThunkResult::stdcall(1, 1)
}

fn h_present(m: &mut Machine) -> ThunkResult {
    if m.threads[m.cur].offloaded {
        MT_PRESENT_OFFLOADED.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    let fb = FrameBuffer { width: 4, height: 2, pixels: vec![m.mem.read_u32(0x48_0004) as u16; 8] };
    let _back = m.present_owned(fb);
    ThunkResult::stdcall(0, 0)
}

/// The game's draw/logic pipeline in miniature: thread 1 ("draw") waits for event A, does some work, presents,
/// counts, sets B; thread 0 ("logic") waits for B, counts, sets A. Returns (draw count, logic count, presents,
/// draw work sum).
fn pipeline(mt: bool, frames: u32) -> (u32, u32, u64, u32) {
    let mut m = machine();
    let wait = m.registry.register("test.dll", "Wait", h_wait1);
    let set = m.registry.register("test.dll", "Set", h_set1);
    let present = m.registry.register("test.dll", "Present", h_present);
    let exit = m.registry.register("test.dll", "Exit", h_exit);
    let ev_a = m.handles.insert(KernelObject::Event { manual_reset: false, signaled: true });
    let ev_b = m.handles.insert(KernelObject::Event { manual_reset: false, signaled: false });
    // logic (slot 0)
    load(&mut m, CODE, |a| {
        let mut top = a.create_label();
        a.set_label(&mut top).unwrap();
        a.push(ev_b).unwrap();
        a.mov(ecx, wait).unwrap();
        a.call(ecx).unwrap();
        a.inc(dword_ptr(0x48_0000)).unwrap();
        a.push(ev_a).unwrap();
        a.mov(ecx, set).unwrap();
        a.call(ecx).unwrap();
        a.jmp(top).unwrap();
    });
    // draw (slot 1): some guest work per frame (more than one quantum), then present
    load(&mut m, CODE + 0x200, |a| {
        let mut top = a.create_label();
        let mut work = a.create_label();
        a.mov(esi, frames).unwrap();
        a.set_label(&mut top).unwrap();
        a.push(ev_a).unwrap();
        a.mov(ecx, wait).unwrap();
        a.call(ecx).unwrap();
        a.mov(edx, 150_000u32).unwrap();
        a.set_label(&mut work).unwrap();
        a.add(dword_ptr(0x48_0008), edx).unwrap();
        a.dec(edx).unwrap();
        a.jnz(work).unwrap();
        a.mov(ecx, present).unwrap();
        a.call(ecx).unwrap();
        a.inc(dword_ptr(0x48_0004)).unwrap();
        a.push(ev_b).unwrap();
        a.mov(ecx, set).unwrap();
        a.call(ecx).unwrap();
        a.dec(esi).unwrap();
        a.jnz(top).unwrap();
        a.push(7).unwrap();
        a.mov(ecx, exit).unwrap();
        a.call(ecx).unwrap();
    });
    m.create_thread(CODE, 0, 0x1_0000, false).unwrap();
    m.create_thread(CODE + 0x200, 0, 0x1_0000, false).unwrap();
    if mt {
        m.enable_mt();
    }
    let start = std::time::Instant::now();
    loop {
        match m.run_slice(1_000_000) {
            SliceResult::Stopped => break,
            SliceResult::Idle => m.idle(1),
            SliceResult::Ran(_) => {}
        }
        assert!(start.elapsed().as_secs() < 120, "pipeline did not finish");
    }
    assert_eq!(m.stop_reason(), Some(&StopReason::Exited(7)));
    let r = (m.mem.read_u32(0x48_0004), m.mem.read_u32(0x48_0000), m.stats.presents, m.mem.read_u32(0x48_0008));
    drop(m); // joins the worker
    r
}

#[test]
fn mt_pipeline_matches_single_thread() {
    let st = pipeline(false, 120);
    assert_eq!(st.0, 120);
    assert!(st.1 >= 119, "{st:?}");
    MT_PRESENT_OFFLOADED.store(false, std::sync::atomic::Ordering::SeqCst);
    let mt = pipeline(true, 120);
    assert!(MT_PRESENT_OFFLOADED.load(std::sync::atomic::Ordering::SeqCst), "draw thread was never offloaded");
    assert_eq!(mt.0, 120, "{mt:?}");
    assert!(mt.1 >= 119, "{mt:?}");
    assert_eq!(mt.2, 120);
    assert_eq!(mt.3, st.3, "guest work of the offloaded thread differs");
}
