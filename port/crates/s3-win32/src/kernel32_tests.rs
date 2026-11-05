//! kernel32 tests: hand-assembled guest snippets (iced-x86 code_asm) for threads/waits/SEH, and direct thunk
//! calls (`Machine::call_guest` on the thunk address) for the rest. No game bytes involved.

use iced_x86::code_asm::*;
use s3_mem::layout;
use s3_runtime::{Machine, NullHost, SliceResult, StopReason};

const CODE: u32 = 0x0040_1000;
const DATA: u32 = 0x0048_0000;

fn tmpdir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("s3k32-test-{}-{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("game")).unwrap();
    std::fs::create_dir_all(d.join("data")).unwrap();
    d
}

fn machine(tag: &str) -> (Machine, std::path::PathBuf) {
    let d = tmpdir(tag);
    let mut host = NullHost::new(d.join("game"), d.join("data"));
    host.quiet = true;
    let mut m = Machine::new(Box::new(host)).unwrap();
    super::register(&mut m.registry);
    m.mem.alloc_region(Some(layout::IMAGE_BASE), 0x10_0000, s3_mem::RegionKind::Image, (0, 0)).unwrap();
    m.setup_process(layout::IMAGE_BASE, 0x10_0000, CODE, 0x10_0000, "TEST.EXE");
    (m, d)
}

fn k(m: &Machine, name: &str) -> u32 {
    m.registry.lookup("kernel32.dll", name).unwrap_or_else(|| panic!("{name} not registered"))
}

fn load(m: &mut Machine, at: u32, build: impl FnOnce(&mut CodeAssembler)) {
    let mut a = CodeAssembler::new(32).unwrap();
    build(&mut a);
    let bytes = a.assemble(at as u64).unwrap();
    m.mem.write_bytes(at, &bytes);
}

/// `call kernel32!name` (via ecx; clobbers ecx).
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

/// A machine with one idle thread, for calling thunks directly with `api`.
fn direct(tag: &str) -> (Machine, std::path::PathBuf) {
    let (mut m, d) = machine(tag);
    load(&mut m, CODE, |a| {
        a.nop().unwrap();
    });
    let slot = m.create_thread(CODE, 0, 0x1_0000, false).unwrap();
    m.cur = slot;
    (m, d)
}

fn api(m: &mut Machine, name: &str, args: &[u32]) -> u32 {
    let t = k(m, name);
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

#[test]
fn thread_event_handoff() {
    let (mut m, _d) = machine("ev");
    let (ce, se, ws, ct, ex) =
        (k(&m, "CreateEventA"), k(&m, "SetEvent"), k(&m, "WaitForSingleObject"), k(&m, "CreateThread"), k(&m, "ExitProcess"));
    let tb = CODE + 0x400;
    // main: ev = CreateEvent(0,0,0,0) -> [DATA]; CreateThread(tb); Wait(ev); Wait(thread); Exit([DATA+4])
    load(&mut m, CODE, |a| {
        a.push(0).unwrap();
        a.push(0).unwrap();
        a.push(0).unwrap();
        a.push(0).unwrap();
        call(a, ce);
        a.mov(dword_ptr(DATA), eax).unwrap();
        a.push(0).unwrap();
        a.push(0).unwrap();
        a.push(5).unwrap();
        a.push(tb).unwrap();
        a.push(0).unwrap();
        a.push(0).unwrap();
        call(a, ct);
        a.mov(esi, eax).unwrap();
        a.push(0xFFFF_FFFFu32 as i32).unwrap();
        a.push(dword_ptr(DATA)).unwrap();
        call(a, ws);
        a.push(0xFFFF_FFFFu32 as i32).unwrap();
        a.push(esi).unwrap();
        call(a, ws);
        a.push(dword_ptr(DATA + 4)).unwrap();
        call(a, ex);
    });
    // thread: spin, [DATA+4] = param*10, SetEvent, return
    load(&mut m, tb, |a| {
        let mut l = a.create_label();
        a.mov(edx, 50_000).unwrap();
        a.set_label(&mut l).unwrap();
        a.dec(edx).unwrap();
        a.jnz(l).unwrap();
        a.mov(eax, dword_ptr(esp + 4)).unwrap();
        a.imul_3(eax, eax, 10).unwrap();
        a.mov(dword_ptr(DATA + 4), eax).unwrap();
        a.push(dword_ptr(DATA)).unwrap();
        call(a, se);
        a.xor(eax, eax).unwrap();
        a.ret_1(4).unwrap();
    });
    m.create_thread(CODE, 0, 0x1_0000, false).unwrap();
    m.quantum = 5_000;
    assert_eq!(run_to_stop(&mut m), StopReason::Exited(50));
}

#[test]
fn wait_timeout_and_multiple() {
    let (mut m, _d) = direct("wt");
    let ev = api(&mut m, "CreateEventA", &[0, 1, 0, 0]);
    let sem = api(&mut m, "CreateSemaphoreA", &[0, 0, 2, 0]);
    let t0 = m.now_ms();
    assert_eq!(api(&mut m, "WaitForSingleObject", &[ev, 30]), 0x102);
    assert!(m.now_ms() - t0 >= 30);
    assert_eq!(api(&mut m, "WaitForSingleObject", &[sem, 0]), 0x102);
    let arr = m.mem.alloc(8);
    m.mem.write_u32(arr, sem);
    m.mem.write_u32(arr + 4, ev);
    api(&mut m, "SetEvent", &[ev]);
    assert_eq!(api(&mut m, "WaitForMultipleObjects", &[2, arr, 0, 0]), 1);
    // manual-reset stays signaled
    assert_eq!(api(&mut m, "WaitForSingleObject", &[ev, 0]), 0);
    // wait-all needs the semaphore too
    assert_eq!(api(&mut m, "WaitForMultipleObjects", &[2, arr, 1, 0]), 0x102);
    assert_eq!(api(&mut m, "ReleaseSemaphore", &[sem, 1, 0]), 1);
    assert_eq!(api(&mut m, "WaitForMultipleObjects", &[2, arr, 1, 0]), 0);
    assert_eq!(api(&mut m, "ReleaseSemaphore", &[sem, 3, 0]), 0, "over max");
    assert_eq!(api(&mut m, "WaitForSingleObject", &[0x7777, 0]), 0xFFFF_FFFF);
    // named objects
    let name = cstr(&mut m, "S3Event");
    let e1 = api(&mut m, "CreateEventA", &[0, 0, 0, name]);
    let e2 = api(&mut m, "CreateEventA", &[0, 0, 0, name]);
    assert_ne!(e1, e2);
    assert_eq!(m.last_error(), 183);
    api(&mut m, "SetEvent", &[e1]);
    assert_eq!(api(&mut m, "WaitForSingleObject", &[e2, 0]), 0);
    assert_eq!(api(&mut m, "WaitForSingleObject", &[e1, 0]), 0x102, "auto-reset consumed through the alias");
    // mutex recursion
    let mx = api(&mut m, "CreateMutexA", &[0, 1, 0]);
    assert_eq!(api(&mut m, "WaitForSingleObject", &[mx, 0]), 0);
    assert_eq!(api(&mut m, "ReleaseMutex", &[mx]), 1);
    assert_eq!(api(&mut m, "ReleaseMutex", &[mx]), 1);
    assert_eq!(api(&mut m, "ReleaseMutex", &[mx]), 0);
    assert_eq!(api(&mut m, "CloseHandle", &[mx]), 1);
    assert_eq!(api(&mut m, "CloseHandle", &[mx]), 0);
}

#[test]
fn critical_section_mutual_exclusion() {
    let (mut m, _d) = machine("cs");
    let (ic, ec, lc, sl, ct, ws, ex) = (
        k(&m, "InitializeCriticalSection"),
        k(&m, "EnterCriticalSection"),
        k(&m, "LeaveCriticalSection"),
        k(&m, "Sleep"),
        k(&m, "CreateThread"),
        k(&m, "WaitForSingleObject"),
        k(&m, "ExitProcess"),
    );
    let csa = DATA + 0x100;
    let (counter, inside, errors) = (DATA, DATA + 4, DATA + 8);
    let worker = CODE + 0x400;
    // worker: 200 x { Enter; if [inside] errors++; inside=1; Sleep(0); counter++; inside=0; Leave }
    load(&mut m, worker, |a| {
        let mut top = a.create_label();
        let mut ok = a.create_label();
        a.push(ebx).unwrap();
        a.mov(ebx, 200).unwrap();
        a.set_label(&mut top).unwrap();
        a.push(csa).unwrap();
        call(a, ec);
        a.cmp(dword_ptr(inside), 0).unwrap();
        a.je(ok).unwrap();
        a.inc(dword_ptr(errors)).unwrap();
        a.set_label(&mut ok).unwrap();
        a.mov(dword_ptr(inside), 1).unwrap();
        a.push(0).unwrap();
        call(a, sl);
        a.inc(dword_ptr(counter)).unwrap();
        a.mov(dword_ptr(inside), 0).unwrap();
        a.push(csa).unwrap();
        call(a, lc);
        a.dec(ebx).unwrap();
        a.jnz(top).unwrap();
        a.pop(ebx).unwrap();
        a.xor(eax, eax).unwrap();
        a.ret_1(4).unwrap();
    });
    load(&mut m, CODE, |a| {
        a.push(csa).unwrap();
        call(a, ic);
        for reg in [esi, edi] {
            a.push(0).unwrap();
            a.push(0).unwrap();
            a.push(0).unwrap();
            a.push(worker).unwrap();
            a.push(0).unwrap();
            a.push(0).unwrap();
            call(a, ct);
            a.mov(reg, eax).unwrap();
        }
        for reg in [esi, edi] {
            a.push(0xFFFF_FFFFu32 as i32).unwrap();
            a.push(reg).unwrap();
            call(a, ws);
        }
        a.mov(eax, dword_ptr(counter)).unwrap();
        a.imul_3(edx, dword_ptr(errors), 1000).unwrap();
        a.add(eax, edx).unwrap();
        a.push(eax).unwrap();
        call(a, ex);
    });
    m.create_thread(CODE, 0, 0x1_0000, false).unwrap();
    m.quantum = 300;
    assert_eq!(run_to_stop(&mut m), StopReason::Exited(400));
    assert_eq!(m.mem.read_u32(csa + 12), 0, "released");
}

#[test]
fn seh_raise_unwind_and_continue() {
    let (mut m, _d) = machine("seh");
    let (re, ru, ex) = (k(&m, "RaiseException"), k(&m, "RtlUnwind"), k(&m, "ExitProcess"));
    let outer_h = CODE + 0x300;
    let inner_h = CODE + 0x380;
    let cont = CODE + 0x200;
    let counter = DATA;
    // main: outer frame; inner frame; RaiseException(0xE0001234, NONCONTINUABLE); never returns
    load(&mut m, CODE, |a| {
        a.push(outer_h).unwrap();
        a.push(dword_ptr(0).fs()).unwrap();
        a.mov(dword_ptr(0).fs(), esp).unwrap();
        a.push(inner_h).unwrap();
        a.push(dword_ptr(0).fs()).unwrap();
        a.mov(dword_ptr(0).fs(), esp).unwrap();
        a.push(0).unwrap();
        a.push(0).unwrap();
        a.push(1).unwrap();
        a.push(0xE000_1234u32 as i32).unwrap();
        call(a, re);
        a.push(99).unwrap();
        call(a, ex);
    });
    // continuation: esp = outer frame; pop it; exit(42 + counter*1 + [DATA+4])
    load(&mut m, cont, |a| {
        a.pop(dword_ptr(0).fs()).unwrap();
        a.add(esp, 4).unwrap();
        a.mov(eax, 42).unwrap();
        a.add(eax, dword_ptr(counter)).unwrap();
        a.add(eax, dword_ptr(DATA + 4)).unwrap();
        a.push(eax).unwrap();
        call(a, ex);
    });
    // outer handler: if unwinding -> return 1; else [DATA+4]=code==E0001234 ? 100 : 0; RtlUnwind(frame,0,rec,0);
    // esp = frame; jmp cont
    load(&mut m, outer_h, |a| {
        let mut unw = a.create_label();
        let mut no = a.create_label();
        a.mov(eax, dword_ptr(esp + 4)).unwrap();
        a.test(dword_ptr(eax + 4), 6).unwrap();
        a.jnz(unw).unwrap();
        a.cmp(dword_ptr(eax), 0xE000_1234u32 as i32).unwrap();
        a.jne(no).unwrap();
        a.mov(dword_ptr(DATA + 4), 100).unwrap();
        a.set_label(&mut no).unwrap();
        a.mov(ebx, dword_ptr(esp + 8)).unwrap();
        a.push(0).unwrap();
        a.push(eax).unwrap();
        a.push(0).unwrap();
        a.push(ebx).unwrap();
        call(a, ru);
        a.mov(esp, ebx).unwrap();
        a.mov(eax, cont).unwrap();
        a.jmp(eax).unwrap();
        a.set_label(&mut unw).unwrap();
        a.mov(eax, 1).unwrap();
        a.ret().unwrap();
    });
    // inner handler: count unwind calls, always continue search
    load(&mut m, inner_h, |a| {
        let mut skip = a.create_label();
        a.mov(eax, dword_ptr(esp + 4)).unwrap();
        a.test(dword_ptr(eax + 4), 2).unwrap();
        a.jz(skip).unwrap();
        a.inc(dword_ptr(counter)).unwrap();
        a.set_label(&mut skip).unwrap();
        a.mov(eax, 1).unwrap();
        a.ret().unwrap();
    });
    m.create_thread(CODE, 0, 0x1_0000, false).unwrap();
    assert_eq!(run_to_stop(&mut m), StopReason::Exited(143));
}

#[test]
fn raise_exception_continue_execution_returns() {
    let (mut m, _d) = machine("sehc");
    let (re, ex) = (k(&m, "RaiseException"), k(&m, "ExitProcess"));
    let h = CODE + 0x300;
    load(&mut m, CODE, |a| {
        a.push(h).unwrap();
        a.push(dword_ptr(0).fs()).unwrap();
        a.mov(dword_ptr(0).fs(), esp).unwrap();
        a.mov(esi, 7).unwrap();
        a.push(0).unwrap();
        a.push(0).unwrap();
        a.push(0).unwrap();
        a.push(0x4000_0001).unwrap();
        call(a, re);
        // returns here; esi preserved, handler set [DATA]
        a.pop(dword_ptr(0).fs()).unwrap();
        a.add(esp, 4).unwrap();
        a.add(esi, dword_ptr(DATA)).unwrap();
        a.push(esi).unwrap();
        call(a, ex);
    });
    load(&mut m, h, |a| {
        a.mov(dword_ptr(DATA), 30).unwrap();
        a.xor(eax, eax).unwrap();
        a.ret().unwrap();
    });
    m.create_thread(CODE, 0, 0x1_0000, false).unwrap();
    assert_eq!(run_to_stop(&mut m), StopReason::Exited(37));
}

#[test]
fn pulse_event_releases_waiter() {
    let (mut m, _d) = machine("pulse");
    let (ce, pe, ws, ct, sl, ex) = (
        k(&m, "CreateEventA"),
        k(&m, "PulseEvent"),
        k(&m, "WaitForSingleObject"),
        k(&m, "CreateThread"),
        k(&m, "Sleep"),
        k(&m, "ExitProcess"),
    );
    let tb = CODE + 0x400;
    load(&mut m, CODE, |a| {
        a.push(0).unwrap();
        a.push(0).unwrap();
        a.push(1).unwrap();
        a.push(0).unwrap();
        call(a, ce);
        a.mov(dword_ptr(DATA), eax).unwrap();
        for v in [0, 0, 0, tb as i32, 0, 0] {
            a.push(v).unwrap();
        }
        call(a, ct);
        a.mov(esi, eax).unwrap();
        a.push(20).unwrap();
        call(a, sl);
        a.push(dword_ptr(DATA)).unwrap();
        call(a, pe);
        a.push(2000).unwrap();
        a.push(esi).unwrap();
        call(a, ws); // 0 if the thread finished
        a.mov(edi, eax).unwrap();
        a.push(0).unwrap();
        a.push(dword_ptr(DATA)).unwrap();
        call(a, ws); // event not signaled after pulse -> 0x102
        a.add(eax, edi).unwrap();
        a.push(eax).unwrap();
        call(a, ex);
    });
    load(&mut m, tb, |a| {
        a.push(0xFFFF_FFFFu32 as i32).unwrap();
        a.push(dword_ptr(DATA)).unwrap();
        call(a, ws);
        a.ret_1(4).unwrap();
    });
    m.create_thread(CODE, 0, 0x1_0000, false).unwrap();
    assert_eq!(run_to_stop(&mut m), StopReason::Exited(0x102));
}

#[test]
fn sleep_tls_timer() {
    let (mut m, _d) = direct("tls");
    let t0 = api(&mut m, "GetTickCount", &[]);
    api(&mut m, "Sleep", &[40]);
    assert!(api(&mut m, "GetTickCount", &[]) - t0 >= 40);
    let i = api(&mut m, "TlsAlloc", &[]);
    assert!(i < 64);
    assert_eq!(api(&mut m, "TlsGetValue", &[i]), 0);
    api(&mut m, "TlsSetValue", &[i, 0x1234]);
    assert_eq!(api(&mut m, "TlsGetValue", &[i]), 0x1234);
    assert_ne!(api(&mut m, "TlsAlloc", &[]), i);
    // waitable timer: due -10 ms relative, period 15 ms
    let t = api(&mut m, "CreateWaitableTimerA", &[0, 0, 0]);
    let due = m.mem.alloc(8);
    m.mem.write_u64(due, (-100_000i64) as u64);
    assert_eq!(api(&mut m, "SetWaitableTimer", &[t, due, 15, 0, 0, 0]), 1);
    let start = m.now_ms();
    for _ in 0..3 {
        assert_eq!(api(&mut m, "WaitForSingleObject", &[t, 1000]), 0);
    }
    let el = m.now_ms() - start;
    assert!(el >= 10 + 2 * 15 - 2, "elapsed {el}");
    api(&mut m, "CancelWaitableTimer", &[t]);
    assert_eq!(api(&mut m, "WaitForSingleObject", &[t, 20]), 0x102);
    // QPC strictly increases
    let (a, b) = (m.mem.alloc(8), m.mem.alloc(8));
    api(&mut m, "QueryPerformanceCounter", &[a]);
    api(&mut m, "QueryPerformanceCounter", &[b]);
    assert!(m.mem.read_u64(b) > m.mem.read_u64(a));
}

#[test]
fn files_through_vfs() {
    let (mut m, d) = direct("files");
    let game = d.join("game");
    std::fs::create_dir_all(game.join("Map").join("User")).unwrap();
    std::fs::write(game.join("Map").join("User").join("Test.MAP"), b"hello world").unwrap();
    std::fs::create_dir_all(game.join("Gfx")).unwrap();
    std::fs::write(game.join("Gfx").join("Big.dat"), vec![0xAB; 200_000]).unwrap();

    // read a game file case-insensitively
    let p = cstr(&mut m, "c:\\s3\\map\\user\\test.map");
    let h = api(&mut m, "CreateFileA", &[p, 0x8000_0000, 1, 0, 3, 0x80, 0]);
    assert_ne!(h, 0xFFFF_FFFF);
    assert_eq!(api(&mut m, "GetFileSize", &[h, 0]), 11);
    let buf = m.mem.alloc(64);
    let nread = m.mem.alloc(4);
    assert_eq!(api(&mut m, "SetFilePointer", &[h, 6, 0, 0]), 6);
    assert_eq!(api(&mut m, "ReadFile", &[h, buf, 64, nread, 0]), 1);
    assert_eq!(m.mem.read_u32(nread), 5);
    assert_eq!(m.mem.read_cstr(buf), "world");
    assert_eq!(api(&mut m, "CloseHandle", &[h]), 1);

    // write a new file (goes to the overlay), relative to the current directory
    let sub = cstr(&mut m, "Save");
    assert_eq!(api(&mut m, "CreateDirectoryA", &[sub, 0]), 1);
    assert_eq!(api(&mut m, "SetCurrentDirectoryA", &[sub]), 1);
    let cur = m.mem.alloc(260);
    assert_eq!(api(&mut m, "GetCurrentDirectoryA", &[260, cur]), 10);
    assert_eq!(m.mem.read_cstr(cur), "C:\\S3\\Save");
    let f = cstr(&mut m, "game1.sav");
    let h = api(&mut m, "CreateFileA", &[f, 0x4000_0000, 0, 0, 2, 0x80, 0]);
    assert_ne!(h, 0xFFFF_FFFF);
    m.mem.write_cstr(buf, "SAVEDATA");
    assert_eq!(api(&mut m, "WriteFile", &[h, buf, 8, nread, 0]), 1);
    api(&mut m, "CloseHandle", &[h]);
    assert_eq!(std::fs::read(d.join("data/overlay/Save/game1.sav")).unwrap(), b"SAVEDATA");
    assert!(!game.join("Save").exists(), "game dir untouched");

    // find
    let pat = cstr(&mut m, "C:\\S3\\Map\\User\\*.map");
    let fd = m.mem.alloc(320);
    let fh = api(&mut m, "FindFirstFileA", &[pat, fd]);
    assert_ne!(fh, 0xFFFF_FFFF);
    assert_eq!(m.mem.read_cstr(fd + 44), "Test.MAP");
    assert_eq!(m.mem.read_u32(fd + 32), 11);
    assert_eq!(api(&mut m, "FindNextFileA", &[fh, fd]), 0);
    assert_eq!(m.last_error(), 18);
    assert_eq!(api(&mut m, "FindClose", &[fh]), 1);

    // move + delete of a game file use whiteouts
    let a = cstr(&mut m, "C:\\S3\\Map\\User\\Test.map");
    let b = cstr(&mut m, "C:\\S3\\Map\\User\\Renamed.map");
    assert_eq!(api(&mut m, "MoveFileA", &[a, b]), 1);
    assert_eq!(api(&mut m, "GetFileAttributesA", &[a]), 0xFFFF_FFFF);
    assert_eq!(api(&mut m, "GetFileAttributesA", &[b]), 0x20);
    assert!(game.join("Map/User/Test.MAP").exists(), "game dir untouched");
    assert_eq!(api(&mut m, "DeleteFileA", &[b]), 1);
    assert_eq!(api(&mut m, "GetFileAttributesA", &[b]), 0xFFFF_FFFF);
    assert_eq!(api(&mut m, "DeleteFileA", &[b]), 0);
    assert_eq!(m.last_error(), 2);

    // memory mapping
    let g = cstr(&mut m, "GFX\\big.DAT");
    let root = cstr(&mut m, "C:\\S3");
    assert_eq!(api(&mut m, "SetCurrentDirectoryA", &[root]), 1);
    let h = api(&mut m, "CreateFileA", &[g, 0x8000_0000, 1, 0, 3, 0x80, 0]);
    assert_ne!(h, 0xFFFF_FFFF);
    let mh = api(&mut m, "CreateFileMappingA", &[h, 0, 2, 0, 0, 0]);
    assert_ne!(mh, 0);
    let v = api(&mut m, "MapViewOfFile", &[mh, 4, 0, 0, 0]);
    assert_ne!(v, 0);
    assert_eq!(m.mem.read_u8(v), 0xAB);
    assert_eq!(m.mem.read_u8(v + 199_999), 0xAB);
    assert_eq!(m.mem.read_u8(v + 200_000), 0);
    let v2 = api(&mut m, "MapViewOfFile", &[mh, 4, 0, 0x1_0000, 0x100]);
    assert_eq!(m.mem.read_u8(v2 + 0xFF), 0xAB);
    assert_eq!(api(&mut m, "UnmapViewOfFile", &[v]), 1);
    assert_eq!(m.mem.read_u8(v), 0, "unmapped range is zero again");
    assert_eq!(api(&mut m, "UnmapViewOfFile", &[v2]), 1);
    api(&mut m, "CloseHandle", &[mh]);
    api(&mut m, "CloseHandle", &[h]);
}

#[test]
fn process_and_strings() {
    let (mut m, _d) = direct("proc");
    let buf = m.mem.alloc(512);
    assert_eq!(api(&mut m, "GetModuleFileNameA", &[0, buf, 260]), 14);
    assert_eq!(m.mem.read_cstr(buf), "C:\\S3\\TEST.EXE");
    m.mem.write_u32(buf, 148);
    assert_eq!(api(&mut m, "GetVersionExA", &[buf]), 1);
    assert_eq!((m.mem.read_u32(buf + 4), m.mem.read_u32(buf + 8), m.mem.read_u32(buf + 16)), (4, 10, 1));
    // MultiByteToWideChar / WideCharToMultiByte (cp1252 umlaut + euro)
    let s = m.mem.alloc(16);
    m.mem.write_bytes(s, &[b'M', 0xFC, 0x80, 0]);
    assert_eq!(api(&mut m, "MultiByteToWideChar", &[0, 0, s, 0xFFFF_FFFF, 0, 0]), 4);
    assert_eq!(api(&mut m, "MultiByteToWideChar", &[0, 0, s, 0xFFFF_FFFF, buf, 16]), 4);
    assert_eq!(m.mem.read_u16(buf + 2), 0xFC);
    assert_eq!(m.mem.read_u16(buf + 4), 0x20AC);
    let out = m.mem.alloc(16);
    assert_eq!(api(&mut m, "WideCharToMultiByte", &[0, 0, buf, 0xFFFF_FFFF, out, 16, 0, 0]), 4);
    assert_eq!(m.mem.read_cstr_bytes(out), vec![b'M', 0xFC, 0x80]);
    // GetLocaleInfoA
    assert_eq!(api(&mut m, "GetLocaleInfoA", &[0x409, 0x1004, buf, 16]), 5);
    assert_eq!(m.mem.read_cstr(buf), "1252");
    // LCMapStringA upper
    let lo = cstr(&mut m, "abc");
    assert_eq!(api(&mut m, "LCMapStringA", &[0x409, 0x200, lo, 3, buf, 16]), 3);
    assert_eq!(&m.mem.read_bytes(buf, 3), b"ABC");
    // lstrcmpiA
    let up = cstr(&mut m, "ABC");
    assert_eq!(api(&mut m, "lstrcmpiA", &[lo, up]), 0);
    assert_eq!(api(&mut m, "lstrlenA", &[lo]), 3);
    // system time sanity
    api(&mut m, "GetSystemTime", &[buf]);
    let y = m.mem.read_u16(buf);
    assert!((2020..2100).contains(&y));
    // GetProcAddress of the waitable timer API through the module handle
    let kn = cstr(&mut m, "KERNEL32.dll");
    let hk = api(&mut m, "GetModuleHandleA", &[kn]);
    let fname = cstr(&mut m, "CreateWaitableTimerA");
    assert_eq!(api(&mut m, "GetProcAddress", &[hk, fname]), k(&m, "CreateWaitableTimerA"));
    // memory
    let la = api(&mut m, "LocalAlloc", &[0x40, 100]);
    assert_ne!(la, 0);
    assert_eq!(api(&mut m, "IsBadReadPtr", &[la, 100]), 0);
    assert_eq!(api(&mut m, "IsBadReadPtr", &[0x100, 4]), 1);
    assert_eq!(api(&mut m, "LocalFree", &[la]), 0);
}

/// Save/load round trip as S3.EXE does it (save-load task, 2026-11-05): `CreateDirectoryA("Save")` on a game dir
/// that already has `Save\Multi` (GOG install) -> ERROR_ALREADY_EXISTS; `slot N.sav` probes fail with
/// ERROR_FILE_NOT_FOUND; the save is created with GENERIC_READ|WRITE + CREATE_ALWAYS in the overlay; the Load Game
/// screen lists `Save\*.sav` (overlay) and `Save\Multi\*.mps` from the game dir stays visible.
#[test]
fn save_slots_overlay_and_game_dir() {
    let (mut m, d) = direct("saves");
    let game = d.join("game");
    std::fs::create_dir_all(game.join("Save").join("Multi")).unwrap();
    std::fs::write(game.join("Save").join("Multi").join("abc.146.mps"), b"mp").unwrap();

    let save = cstr(&mut m, "C:\\S3\\Save");
    assert_eq!(api(&mut m, "CreateDirectoryA", &[save, 0]), 0);
    assert_eq!(m.last_error(), 183);
    let multi = cstr(&mut m, "C:\\S3\\Save\\Multi");
    assert_eq!(api(&mut m, "CreateDirectoryA", &[multi, 0]), 0);
    assert_eq!(m.last_error(), 183);

    let slot = cstr(&mut m, "C:\\S3\\Save\\slot 1.sav");
    assert_eq!(api(&mut m, "CreateFileA", &[slot, 0x8000_0000, 1, 0, 3, 0x80, 0]), 0xFFFF_FFFF);
    assert_eq!(m.last_error(), 2);
    let h = api(&mut m, "CreateFileA", &[slot, 0xC000_0000, 0, 0, 2, 0x80, 0]);
    assert_ne!(h, 0xFFFF_FFFF);
    let buf = m.mem.alloc(16);
    let n = m.mem.alloc(4);
    m.mem.write_cstr(buf, "S3SAVE");
    assert_eq!(api(&mut m, "WriteFile", &[h, buf, 6, n, 0]), 1);
    // the game rewinds and patches the header (checksum) before closing
    assert_eq!(api(&mut m, "SetFilePointer", &[h, 0, 0, 0]), 0);
    m.mem.write_cstr(buf, "s");
    assert_eq!(api(&mut m, "WriteFile", &[h, buf, 1, n, 0]), 1);
    api(&mut m, "CloseHandle", &[h]);
    assert_eq!(std::fs::read(d.join("data/overlay/Save/slot 1.sav")).unwrap(), b"s3SAVE");

    // Load Game list: *.sav (case-insensitive pattern), the Multi folder from the game dir is still there.
    let fd = m.mem.alloc(320);
    let pat = cstr(&mut m, "C:\\S3\\SAVE\\*.SAV");
    let fh = api(&mut m, "FindFirstFileA", &[pat, fd]);
    assert_ne!(fh, 0xFFFF_FFFF);
    assert_eq!(m.mem.read_cstr(fd + 44), "slot 1.sav");
    assert_ne!(m.mem.read_u32(fd + 20) | m.mem.read_u32(fd + 24), 0, "last write time set");
    assert_eq!(api(&mut m, "FindNextFileA", &[fh, fd]), 0);
    api(&mut m, "FindClose", &[fh]);
    let pat = cstr(&mut m, "C:\\S3\\Save\\Multi\\*.mps");
    let fh = api(&mut m, "FindFirstFileA", &[pat, fd]);
    assert_ne!(fh, 0xFFFF_FFFF);
    assert_eq!(m.mem.read_cstr(fd + 44), "abc.146.mps");
    api(&mut m, "FindClose", &[fh]);
    let pat = cstr(&mut m, "C:\\S3\\Save\\*");
    let fh = api(&mut m, "FindFirstFileA", &[pat, fd]);
    let mut names = vec![m.mem.read_cstr(fd + 44)];
    while api(&mut m, "FindNextFileA", &[fh, fd]) == 1 {
        names.push(m.mem.read_cstr(fd + 44));
    }
    api(&mut m, "FindClose", &[fh]);
    assert_eq!(names, vec![".", "..", "Multi", "slot 1.sav"]);

    // reopen for reading (load) gets the overlay copy
    let h = api(&mut m, "CreateFileA", &[slot, 0x8000_0000, 1, 0, 3, 0x80, 0]);
    assert_ne!(h, 0xFFFF_FFFF);
    assert_eq!(api(&mut m, "GetFileSize", &[h, 0]), 6);
    api(&mut m, "CloseHandle", &[h]);
}

/// CreateThread's stack argument is the initial commit; the reserve stays the PE default (S3.EXE passes 0x1000
/// and later uses 256 KiB of locals on that thread).
#[test]
fn create_thread_stack_is_commit_not_reserve() {
    let (mut m, _d) = direct("stack");
    let tid = m.mem.alloc(4);
    let h = api(&mut m, "CreateThread", &[0, 0x1000, CODE, 0, 4, tid]);
    assert_ne!(h, 0);
    let t = m.threads.last().unwrap();
    assert_eq!(t.stack_top - t.stack_bottom, 0x10_0000, "PE reserve (1 MiB)");
    let h = api(&mut m, "CreateThread", &[0, 0x18_0000, CODE, 0, 4, tid]);
    assert_ne!(h, 0);
    let t = m.threads.last().unwrap();
    assert_eq!(t.stack_top - t.stack_bottom, 0x20_0000, "commit > reserve rounds up to 1 MiB");
}
