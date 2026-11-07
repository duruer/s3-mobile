//! `s3det` — deterministic headless run of S3.EXE with per-interval game-state hashes (W5 determinism test).
//!
//! ```text
//! s3det --game-dir <dir> --data-dir <dir> [--exe <S3.EXE>] --vms <virtual ms to run>
//!       [--ipms N (150000 guest instructions per virtual ms)] [--hash-every MS (1000)] [--hash-out <file>]
//!       [--dump-at VMS --dump <file>]  (raw dump of the hashed memory: repeated [u32 addr][4096 bytes])
//!       [--inject "t_vms:cmd args;..."]   (click x y | rclick x y | move x y | key VK | shot file.png | quit)
//!       [--write-trace FROM-TO --write-trace-out <file>]  (which memory each thread writes, see write_trace.rs)
//! ```
//! Everything the guest can observe about time comes from a **virtual clock** derived from the number of guest
//! instructions executed (plus 1 ms per idle slice), never from the host clock:
//! `GetTickCount`/`timeGetTime`/waits/timers via `Host::now_ms`, and (overridden here) `QueryPerformanceCounter`,
//! `GetSystemTime`/`GetLocalTime`/`GetSystemTimeAsFileTime` (fixed epoch 2001-01-01 00:00 UTC + virtual time, no
//! time zone). Inject times are virtual ms. Two runs with the same game files, an identical fresh `--data-dir`
//! and the same arguments must print identical hash lines; compare them with `diff`.
//! See docs/porting/determinism.md.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[path = "../write_trace.rs"]
mod write_trace;

use s3_engine::Engine;
use s3_runtime::{FrameBuffer, Host, HostEvent, LogLevel, Machine, MouseButton, SliceResult, ThunkResult};

/// Virtual time in microseconds, shared by the host and the overridden time thunks.
static VCLOCK_US: AtomicU64 = AtomicU64::new(0);

/// 2001-01-01 00:00:00 UTC as FILETIME (100 ns ticks since 1601).
const EPOCH_FT: u64 = 126_227_808_000_000_000;

struct DetHost {
    game_dir: PathBuf,
    data_dir: PathBuf,
    inject: VecDeque<(u64, HostEvent)>,
    frame: Arc<Mutex<Option<FrameBuffer>>>,
    verbose: bool,
}

impl Host for DetHost {
    fn present(&mut self, fb: &FrameBuffer) {
        *self.frame.lock().unwrap() = Some(fb.clone());
    }
    fn poll_events(&mut self) -> Vec<HostEvent> {
        let now = self.now_ms();
        let mut out = Vec::new();
        while self.inject.front().map(|(t, _)| *t <= now).unwrap_or(false) {
            out.push(self.inject.pop_front().unwrap().1);
        }
        out
    }
    fn now_ms(&self) -> u64 {
        VCLOCK_US.load(Ordering::Relaxed) / 1000
    }
    fn game_dir(&self) -> PathBuf {
        self.game_dir.clone()
    }
    fn data_dir(&self) -> PathBuf {
        self.data_dir.clone()
    }
    fn log(&self, level: LogLevel, msg: &str) {
        if self.verbose || level <= LogLevel::Warn {
            eprintln!("[{level:?}] {msg}");
        }
    }
}

// ---------------------------------------------------------------------------------------- time overrides

fn qpc(m: &mut Machine) -> ThunkResult {
    #[derive(Default)]
    struct Last(u64);
    let p = m.arg_u32(0);
    let st = m.state::<Last>();
    let v = {
        let mut l = st.borrow_mut();
        let mut v = VCLOCK_US.load(Ordering::Relaxed);
        if v <= l.0 {
            v = l.0 + 1; // strictly increasing (the blit benchmark at 0x5c4280 waits for it to advance)
        }
        l.0 = v;
        v
    };
    if p == 0 {
        return ThunkResult::stdcall(0, 1);
    }
    m.mem.write_u64(p, v);
    ThunkResult::stdcall(1, 1)
}

fn now_ft() -> u64 {
    EPOCH_FT + VCLOCK_US.load(Ordering::Relaxed) * 10
}

/// FILETIME -> SYSTEMTIME (16 bytes) at `p`.
fn write_systemtime(m: &mut Machine, p: u32, ft: u64) {
    let ms_total = ft / 10_000 - 11_644_473_600_000; // ms since 1970
    let days = (ms_total / 86_400_000) as i64;
    let rem = ms_total % 86_400_000;
    // Civil date from days since 1970-01-01 (H. Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if mo <= 2 { 1 } else { 0 };
    let dow = (days + 4).rem_euclid(7); // 1970-01-01 was a Thursday
    let f = [
        y as u16,
        mo as u16,
        dow as u16,
        d as u16,
        (rem / 3_600_000) as u16,
        (rem / 60_000 % 60) as u16,
        (rem / 1000 % 60) as u16,
        (rem % 1000) as u16,
    ];
    for (i, v) in f.iter().enumerate() {
        m.mem.write_u16(p + 2 * i as u32, *v);
    }
}

fn get_system_time(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    if p != 0 {
        write_systemtime(m, p, now_ft());
    }
    ThunkResult::stdcall(0, 1)
}

fn get_system_time_as_file_time(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    if p != 0 {
        m.mem.write_u64(p, now_ft());
    }
    ThunkResult::stdcall(0, 1)
}

fn get_time_zone_information(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    if p != 0 {
        m.mem.fill(p, 172, 0); // UTC, no daylight rules
    }
    ThunkResult::stdcall(0, 1) // TIME_ZONE_ID_UNKNOWN
}

/// Deterministic v4 GUIDs (CoCreateGuid / UuidCreate): fixed-seed xorshift64* instead of the host clock.
fn det_guid(m: &mut Machine) -> [u8; 16] {
    #[derive(Default)]
    struct Rng(u64);
    let st = m.state::<Rng>();
    let mut r = st.borrow_mut();
    if r.0 == 0 {
        r.0 = 0x5333_4445_5445_5231;
    }
    let mut g = [0u8; 16];
    for chunk in g.chunks_mut(8) {
        r.0 ^= r.0 >> 12;
        r.0 ^= r.0 << 25;
        r.0 ^= r.0 >> 27;
        chunk.copy_from_slice(&r.0.wrapping_mul(0x2545_F491_4F6C_DD1D).to_le_bytes());
    }
    g[7] = (g[7] & 0x0F) | 0x40;
    g[8] = (g[8] & 0x3F) | 0x80;
    g
}

fn create_guid(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    if p == 0 {
        return ThunkResult::stdcall(0x8007_0057, 1); // E_INVALIDARG
    }
    let g = det_guid(m);
    m.mem.write_bytes(p, &g);
    ThunkResult::stdcall(0, 1)
}

fn install_time_overrides(m: &mut Machine) {
    let r = &mut m.registry;
    r.register("kernel32.dll", "QueryPerformanceCounter", qpc);
    r.register("kernel32.dll", "GetSystemTime", get_system_time);
    r.register("kernel32.dll", "GetLocalTime", get_system_time);
    r.register("kernel32.dll", "GetSystemTimeAsFileTime", get_system_time_as_file_time);
    r.register("kernel32.dll", "GetTimeZoneInformation", get_time_zone_information);
    // Host-random GUIDs (rpcrt4::random_guid seeds from the wall clock): the first divergence found by this tool
    // (a 16-byte GUID at 0x7a7ac8 written during the first menu click).
    r.register("ole32.dll", "CoCreateGuid", create_guid);
    r.register("rpcrt4.dll", "UuidCreate", create_guid);
}

// ---------------------------------------------------------------------------------------------- state hash

/// 64-bit hash over little-endian words (fast; not cryptographic).
struct Hasher(u64);
impl Hasher {
    fn word(&mut self, w: u64) {
        self.0 = (self.0 ^ w).wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(29);
    }
    fn bytes(&mut self, b: &[u8]) {
        let (words, rest) = b.as_chunks::<8>();
        for c in words {
            self.word(u64::from_le_bytes(*c));
        }
        for &x in rest {
            self.word(x as u64);
        }
    }
}

/// Is a page protection writable (PAGE_READWRITE / WRITECOPY / EXECUTE_READWRITE / EXECUTE_WRITECOPY)?
fn writable(protect: u32) -> bool {
    protect & (0x04 | 0x08 | 0x40 | 0x80) != 0
}

/// Visit every page included in the state hash.
fn for_each_page(m: &Machine, mut f: impl FnMut(u32, &[u8])) {
    for a in m.mem.regions.iter() {
        if matches!(a.kind, s3_runtime::mem::RegionKind::Mapped) {
            continue;
        }
        for (i, &prot) in a.pages.iter().enumerate() {
            if prot == 0 || !writable(prot) {
                continue;
            }
            let addr = a.base + (i as u32) * 4096;
            f(addr, m.mem.slice(addr, 4096));
        }
    }
}

/// Hash of all guest-writable committed memory (not file mappings) plus every thread's CPU + x87 state.
/// Returns (memory hash, cpu hash, bytes hashed).
fn state_hash(m: &Machine) -> (u64, u64, u64) {
    let mut h = Hasher(0x5333_4445_5445_524D);
    let mut n = 0u64;
    for_each_page(m, |addr, page| {
        h.word(addr as u64);
        h.bytes(page);
        n += 4096;
    });
    let mut c = Hasher(0x4350_5553_5441_5445);
    for t in &m.threads {
        c.word(t.tid as u64);
        for r in t.cpu.regs {
            c.word(r as u64);
        }
        c.word(t.cpu.eip as u64);
        for r in t.cpu.fpu.regs {
            c.word(r.to_bits());
        }
        c.word(t.cpu.fpu.status_word() as u64);
        c.word(t.cpu.fpu.control as u64);
        c.word(t.cpu.instructions);
    }
    (h.0, c.0, n)
}

// ---------------------------------------------------------------------------------------------------- main

fn usage() -> ! {
    eprintln!(
        "usage: s3det --game-dir <dir> --data-dir <dir> [--exe <path>] --vms <virtual ms> [--ipms N] \
         [--hash-every MS] [--hash-out <file>] [--inject \"t:cmd ...;...\"] [-v]"
    );
    std::process::exit(2)
}

fn parse_inject(spec: &str, shots: &mut Vec<(u64, PathBuf)>) -> Vec<(u64, HostEvent)> {
    let mut out = Vec::new();
    for item in spec.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        let (t, cmd) = item.split_once(':').unwrap_or_else(|| usage());
        let t: u64 = t.trim().parse().unwrap_or_else(|_| usage());
        let w: Vec<&str> = cmd.split_whitespace().collect();
        let num = |i: usize| -> i64 {
            let s = w.get(i).copied().unwrap_or_else(|| usage());
            match s.strip_prefix("0x") {
                Some(h) => i64::from_str_radix(h, 16).unwrap_or_else(|_| usage()),
                None => s.parse().unwrap_or_else(|_| usage()),
            }
        };
        match w.first().copied() {
            Some("move") => out.push((t, HostEvent::MouseMove { x: num(1) as i32, y: num(2) as i32 })),
            Some(c @ ("click" | "rclick")) => {
                let button = if c == "rclick" { MouseButton::Right } else { MouseButton::Left };
                out.push((t, HostEvent::MouseMove { x: num(1) as i32, y: num(2) as i32 }));
                out.push((t, HostEvent::MouseButton { button, down: true }));
                out.push((t + 50, HostEvent::MouseButton { button, down: false }));
            }
            Some("key") => {
                let vk = num(1) as u32;
                out.push((t, HostEvent::Key { vk, down: true }));
                out.push((t + 50, HostEvent::Key { vk, down: false }));
            }
            Some("shot") => shots.push((t, PathBuf::from(w.get(1).copied().unwrap_or_else(|| usage())))),
            Some("quit") => out.push((t, HostEvent::Quit)),
            _ => usage(),
        }
    }
    out.sort_by_key(|(t, _)| *t);
    shots.sort_by_key(|(t, _)| *t);
    out
}

fn write_png(path: &std::path::Path, fb: &FrameBuffer) -> Result<(), String> {
    let f = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), fb.width, fb.height);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    w.write_image_data(&fb.to_rgb888()).map_err(|e| e.to_string())
}

fn main() {
    let mut game_dir = None;
    let mut data_dir = None;
    let mut exe: Option<PathBuf> = None;
    let mut vms: u64 = 0;
    let mut ipms: u64 = 150_000;
    let mut every: u64 = 1000;
    let mut hash_out: Option<PathBuf> = None;
    let mut inject = String::new();
    let mut verbose = false;
    let mut dump_at: Option<u64> = None;
    let mut dump: Option<PathBuf> = None;
    let mut write_trace: Option<String> = None;
    let mut write_trace_out: Option<PathBuf> = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut val = || it.next().unwrap_or_else(|| usage());
        match a.as_str() {
            "--game-dir" => game_dir = Some(PathBuf::from(val())),
            "--data-dir" => data_dir = Some(PathBuf::from(val())),
            "--exe" => exe = Some(val().into()),
            "--vms" => vms = val().parse().unwrap_or_else(|_| usage()),
            "--ipms" => ipms = val().parse().unwrap_or_else(|_| usage()),
            "--hash-every" => every = val().parse().unwrap_or_else(|_| usage()),
            "--hash-out" => hash_out = Some(val().into()),
            "--inject" => inject = val(),
            "--dump-at" => dump_at = Some(val().parse().unwrap_or_else(|_| usage())),
            "--dump" => dump = Some(val().into()),
            "--write-trace" => write_trace = Some(val()),
            "--write-trace-out" => write_trace_out = Some(val().into()),
            "-v" => verbose = true,
            _ => usage(),
        }
    }
    let (Some(game_dir), Some(data_dir)) = (game_dir, data_dir) else { usage() };
    if vms == 0 || ipms == 0 || every == 0 {
        usage();
    }
    if let Err(e) = s3_runtime::cpu::fpu::math::host_float_env_check() {
        eprintln!("s3det: host floating-point environment is not deterministic: {e}");
        std::process::exit(1);
    }
    std::fs::create_dir_all(&data_dir).ok();
    let mut shots = Vec::new();
    let events = parse_inject(&inject, &mut shots);
    let mut shots: VecDeque<_> = shots.into();
    let frame = Arc::new(Mutex::new(None));
    let host = DetHost { game_dir, data_dir, inject: events.into(), frame: frame.clone(), verbose };
    let mut engine = match Engine::new(Box::new(host), exe.as_deref()) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("s3det: {e}");
            std::process::exit(1);
        }
    };
    install_time_overrides(&mut engine.machine);
    if let Some(w) = &write_trace {
        if let Err(e) = write_trace::install(&mut engine.machine, w) {
            eprintln!("s3det: {e}");
            std::process::exit(2);
        }
    }

    let mut out: Box<dyn std::io::Write> = match &hash_out {
        Some(p) => Box::new(std::fs::File::create(p).unwrap_or_else(|e| {
            eprintln!("s3det: {}: {e}", p.display());
            std::process::exit(1)
        })),
        None => Box::new(std::io::stdout()),
    };
    let wall = std::time::Instant::now();
    let mut idle_us: u64 = 0;
    let mut next_hash = every;
    // Slices of 1/4 virtual ms keep inject/timer latency small; the clock only moves between slices, so every
    // guest-visible time value is a function of the instruction count alone.
    let slice = (ipms / 4).max(1);
    loop {
        let now_us = idle_us + engine.machine.stats.instructions * 1000 / ipms;
        VCLOCK_US.store(now_us, Ordering::Relaxed);
        let now = now_us / 1000;
        if write_trace.is_some() {
            write_trace::set_now(now);
        }
        while shots.front().map(|(t, _)| *t <= now).unwrap_or(false) {
            let (_, p) = shots.pop_front().unwrap();
            match frame.lock().unwrap().as_ref().map(|f| write_png(&p, f)) {
                Some(Ok(())) => eprintln!("s3det: shot {} at {now} vms", p.display()),
                Some(Err(e)) => eprintln!("s3det: shot {} failed: {e}", p.display()),
                None => eprintln!("s3det: shot {}: no frame yet", p.display()),
            }
        }
        if now >= next_hash {
            let (mh, ch, n) = state_hash(&engine.machine);
            let s = &engine.machine.stats;
            writeln!(
                out,
                "vms={next_hash} insns={} presents={} mem={mh:016x} cpu={ch:016x} bytes={n}",
                s.instructions, s.presents
            )
            .ok();
            out.flush().ok();
            next_hash += every;
        }
        if let (Some(t), Some(p)) = (dump_at, &dump) {
            if now >= t {
                let mut buf = Vec::new();
                for_each_page(&engine.machine, |addr, page| {
                    buf.extend_from_slice(&addr.to_le_bytes());
                    buf.extend_from_slice(page);
                });
                match std::fs::write(p, &buf) {
                    Ok(()) => eprintln!("s3det: dumped {} bytes at {now} vms to {}", buf.len(), p.display()),
                    Err(e) => eprintln!("s3det: dump {}: {e}", p.display()),
                }
                dump_at = None;
            }
        }
        if now >= vms {
            break;
        }
        match engine.run_slice(slice) {
            SliceResult::Stopped => break,
            SliceResult::Idle => idle_us += 1000,
            SliceResult::Ran(_) => {}
        }
        if engine.machine.input.quit_requested {
            break;
        }
    }
    let (mh, ch, n) = state_hash(&engine.machine);
    writeln!(
        out,
        "final insns={} presents={} mem={mh:016x} cpu={ch:016x} bytes={n}",
        engine.machine.stats.instructions, engine.machine.stats.presents
    )
    .ok();
    if let Some(p) = &write_trace_out {
        if let Err(e) = write_trace::report(&engine.machine, p) {
            eprintln!("s3det: write trace report: {e}");
        }
    }
    eprintln!(
        "s3det: {} virtual ms in {:.1} s wall, {} instructions, stop: {:?}",
        VCLOCK_US.load(Ordering::Relaxed) / 1000,
        wall.elapsed().as_secs_f64(),
        engine.machine.stats.instructions,
        engine.machine.stop_reason()
    );
}
