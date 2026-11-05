//! `--mp-hash FILE [--mp-hash-every N]`: per-simulation-tick hash of the game state, written from a hook at the
//! start of each simulation tick on the logic thread ([`SIM_HOOK`], GOG exe only), i.e. after the previous tick
//! was fully processed. Two lockstep peers must write identical lines for the same tick number;
//! `tools/mp_hash_compare.py` diffs two files. See docs/porting/multiplayer.md ("Desync check").
//!
//! Hashed (layouts from docs/reverse-engineering/game-state-units.md): settler table, building table, piles,
//! trade, economy sectors, player records, and (every 64 steps) the map tile array.

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use s3_runtime::Machine;

/// Logic message executor: `xor eax,eax; mov al,[esi]` before the class switch in `0x535990`.
pub const EXEC_HOOK: u32 = 0x0053_5a59;

/// Start of one simulation tick in the logic loop (`0x53e5a1: mov ecx,[ebp+0x3a0]` before the three game-data
/// tick calls `0x4e7810`, `0x4e05e0`, `0x4e9670` and the AI calls; ebp = logic). Only reached when the lockstep
/// layer lets the simulation run (`word [0x7ace62] == 0`).
pub const SIM_HOOK: u32 = 0x0053_e5a1;

fn fnv(h: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *h ^= *b as u64;
        *h = h.wrapping_mul(0x100_0000_01b3);
    }
}

fn hash_region(m: &Machine, addr: u32, len: usize) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    // Chunked so huge regions do not allocate much.
    let mut off = 0usize;
    while off < len {
        let n = (len - off).min(1 << 16);
        fnv(&mut h, m.mem.slice(addr + off as u32, n));
        off += n;
    }
    h
}

/// One hash line for game data `gd`.
pub fn state_line(m: &Machine, gd: u32, world: bool) -> String {
    let r = |o: u32| m.mem.read_u32(gd + o);
    let nsettler = r(0x12a5e5c).min(200_000) as usize;
    let nbuild = r(0x1499e60).min(100_000) as usize;
    let npile = r(0x1238858).min(100_000) as usize;
    let ntrade = r(0x110a578).min(10_000) as usize;
    let necon = r(0xe115dc).min(1_000) as usize;
    let parts = [
        ("settlers", hash_region(m, gd + 0x12a5e60, (nsettler + 1) * 0x40)),
        ("buildings", hash_region(m, gd + 0x1499e64, (nbuild + 1) * 0x3a)),
        ("piles", hash_region(m, gd + 0x123885c, (npile + 1) * 0xe)),
        ("trade", hash_region(m, gd + 0x10f45e8, (ntrade + 1) * 0x5a)),
        ("econ", hash_region(m, gd + 0xe145e8, (necon + 1) * 0x2e0)),
        ("players", hash_region(m, gd + 0x11943e0, 8 * 0x148)),
    ];
    let mut s = format!("n={nsettler}/{nbuild}/{npile}");
    for (k, v) in parts {
        s.push_str(&format!(" {k}={:016x}", v));
    }
    if world {
        let size = r(0x2d2d0).min(768) as usize;
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        // Per-peer tile bytes are masked (observed in two-instance runs; meanings HYPOTHESIS): +0x09/+0x0d differ
        // from the first tick in one region (local player's view / fog), +0x0b/+0x0c appear around the local
        // player's land while it is in build mode (placement markers). Simulation data stays identical.
        let mut buf = Vec::with_capacity(size * 0x18);
        for x in 0..size {
            buf.clear();
            buf.extend_from_slice(m.mem.slice(gd + 0x67444 + (x * 768 * 0x18) as u32, size * 0x18));
            for t in buf.chunks_mut(0x18) {
                t[0x09] = 0;
                t[0x0b] = 0;
                t[0x0c] = 0;
                t[0x0d] = 0;
            }
            fnv(&mut h, &buf);
        }
        s.push_str(&format!(" world={h:016x}"));
    }
    s
}

/// Install the hooks writing to `path`: one hash line every `every` simulation ticks, one `exec` line per
/// executed logic message.
pub fn install(m: &mut Machine, path: &Path, every: u32) {
    let f = match std::fs::File::create(path) {
        Ok(f) => Arc::new(Mutex::new(std::io::BufWriter::new(f))),
        Err(e) => {
            eprintln!("s3run: --mp-hash {}: {e}", path.display());
            return;
        }
    };
    let every = every.max(1) as u64;
    // Number of simulation ticks run so far (the game's frame counter gd+0x2d2d8 keeps running while the
    // lockstep layer pauses the simulation, so it differs between peers; this count does not).
    let ticks = Arc::new(AtomicU64::new(0));
    // Every message the logic executor dispatches (0x535a59: esi = message, ebx = logic), with the sim tick:
    // lockstep peers must execute the same messages at the same ticks.
    let (fx, tx) = (f.clone(), ticks.clone());
    s3_mods::add_hook(m, EXEC_HOOK, "mp-exec", move |c| {
        let msg = c.reg(s3_runtime::cpu::ESI);
        let logic = c.reg(s3_runtime::cpu::EBX);
        let gd = c.m.mem.read_u32(logic + 0x3a0);
        let frame = if gd != 0 { c.m.mem.read_u32(gd + 0x2d2d8) } else { 0 };
        let len = (c.m.mem.read_u8(msg + 2) as usize).clamp(4, 64);
        let hex: String = c.m.mem.slice(msg, len).iter().map(|b| format!("{b:02x}")).collect();
        let mut w = fx.lock().unwrap();
        let _ = writeln!(w, "exec at={} frame={frame} msg={hex}", tx.load(Ordering::Relaxed));
        s3_runtime::HookFlow::Continue
    });
    // S3_MP_DUMP_STEP=N[,M...]: also write the raw hashed tables before these sim ticks next to the hash file
    // (`<file>.<tick>.<table>.bin`, for diffing peers with tools/mp_dump_diff.py).
    let dump_steps: Vec<u64> = std::env::var("S3_MP_DUMP_STEP")
        .map(|v| v.split(',').filter_map(|x| x.trim().parse().ok()).collect())
        .unwrap_or_default();
    let dump_base = path.to_path_buf();
    let hdr = std::env::var_os("S3_MP_HDR").is_some();
    s3_mods::add_hook(m, SIM_HOOK, "mp-hash", move |c| {
        let logic = c.reg(s3_runtime::cpu::EBP);
        let gd = c.m.mem.read_u32(logic + 0x3a0);
        if gd == 0 {
            return s3_runtime::HookFlow::Continue;
        }
        let n = ticks.fetch_add(1, Ordering::Relaxed);
        if n % every == 0 {
            let frame = c.m.mem.read_u32(gd + 0x2d2d8);
            let local = c.m.mem.read_u32(gd + 0x2d2d4);
            let rng = c.m.mem.read_u32(gd + 0x2d2dc);
            let mut line = format!("step={n} rng={rng:08x} {}", state_line(c.m, gd, n % 64 == 0));
            if hdr {
                // Debug: game-data header dwords gd+0x2d240.. (to find counters).
                line.push_str(" hdr=");
                for i in 0..56u32 {
                    line.push_str(&format!("{:x},", c.m.mem.read_u32(gd + 0x2d240 + 4 * i)));
                }
            }
            let mut w = f.lock().unwrap();
            let _ = writeln!(w, "{line} frame={frame} local={local}");
            let _ = w.flush();
        }
        if dump_steps.contains(&n) {
            dump(c.m, gd, &dump_base.with_extension(format!("{n}")));
        }
        s3_runtime::HookFlow::Continue
    });
    s3_mods::hooks::commit(m);
}

/// Write each hashed table raw to `<hashfile>.<name>.bin`.
fn dump(m: &Machine, gd: u32, base: &Path) {
    let r = |o: u32| m.mem.read_u32(gd + o);
    let regions = [
        ("settlers", gd + 0x12a5e60, (r(0x12a5e5c).min(200_000) as usize + 1) * 0x40),
        ("buildings", gd + 0x1499e64, (r(0x1499e60).min(100_000) as usize + 1) * 0x3a),
        ("econ", gd + 0xe145e8, (r(0xe115dc).min(1_000) as usize + 1) * 0x2e0),
        ("players", gd + 0x11943e0, 8 * 0x148),
        ("world", gd + 0x67444, 768 * 768 * 0x18),
    ];
    for (name, a, len) in regions {
        let p = format!("{}.{name}.bin", base.display());
        let _ = std::fs::write(&p, m.mem.slice(a, len));
    }
}

/// Game debug log function (cdecl printf-like, `fmt` at [esp+4]); NET:/Desync messages go through it.
pub const GAME_LOG_FN: u32 = 0x0042_8720;

/// Minimal printf for the game's log lines (%d %i %u %x %X %s %c %%, flags/width ignored).
fn format_guest(m: &Machine, fmt: &str, mut arg: impl FnMut() -> u32) -> String {
    let mut out = String::new();
    let mut it = fmt.chars().peekable();
    while let Some(c) = it.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let mut spec = None;
        while let Some(&n) = it.peek() {
            it.next();
            if n.is_ascii_alphabetic() || n == '%' {
                if n == 'l' || n == 'h' {
                    continue;
                }
                spec = Some(n);
                break;
            }
        }
        match spec {
            Some('%') => out.push('%'),
            Some('d') | Some('i') => out.push_str(&(arg() as i32).to_string()),
            Some('u') => out.push_str(&arg().to_string()),
            Some('x') => out.push_str(&format!("{:x}", arg())),
            Some('X') => out.push_str(&format!("{:X}", arg())),
            Some('c') => out.push((arg() as u8) as char),
            Some('s') => {
                let p = arg();
                if p != 0 {
                    out.extend(m.mem.read_cstr_bytes(p).into_iter().take(200).map(|b| b as char));
                }
            }
            Some(o) => {
                arg();
                out.push_str(&format!("<%{o}>"));
            }
            None => {}
        }
    }
    out
}

/// `S3_GAME_LOG=1`: print the game's own debug log lines (`[game] ...`) to stderr.
pub fn install_game_log(m: &mut Machine) {
    s3_mods::add_hook(m, GAME_LOG_FN, "game-log", move |c| {
        let esp = c.esp();
        let fmt_p = c.m.mem.read_u32(esp + 4);
        if fmt_p == 0 {
            return s3_runtime::HookFlow::Continue;
        }
        let fmt: String = c.m.mem.read_cstr_bytes(fmt_p).into_iter().take(300).map(|b| b as char).collect();
        let mut i = 2;
        let mem = &c.m.mem;
        let line = format_guest(c.m, &fmt, || {
            let v = mem.read_u32(esp + 4 * i);
            i += 1;
            v
        });
        eprintln!("[game] {}", line.trim_end());
        s3_runtime::HookFlow::Continue
    });
    s3_mods::hooks::commit(m);
}

/// `S3_MP_NETDBG=1`: log the in-game network receive filter (`0x46ae35`: slot active/left flags) and bundle
/// checksum failures (`0x46ae95`), deduplicated. Debug aid for the lockstep start (multiplayer.md).
pub fn install_net_debug(m: &mut Machine) {
    let last = Arc::new(Mutex::new(String::new()));
    let l2 = last.clone();
    s3_mods::add_hook(m, 0x0046_ae35, "mp-netdbg", move |c| {
        let (esi, slot) = (c.reg(s3_runtime::cpu::ESI), c.reg(s3_runtime::cpu::EBP));
        let act = c.m.mem.read_u8(esi + 0x23d + slot);
        let left = c.m.mem.read_u8(esi + 0x251 + slot);
        let s = format!("netdbg: recv slot {slot} active={act} left={left} session={esi:#x}");
        let mut l = l2.lock().unwrap();
        if *l != s {
            eprintln!("{s}");
            *l = s;
        }
        s3_runtime::HookFlow::Continue
    });
    s3_mods::add_hook(m, 0x0046_ae91, "mp-netdbg", move |c| {
        let esp = c.esp();
        let want = c.m.mem.read_u32(esp + 0x18);
        let got = c.reg(s3_runtime::cpu::EAX);
        if want != got {
            eprintln!("netdbg: bundle checksum mismatch {want:#x} != {got:#x}");
        }
        s3_runtime::HookFlow::Continue
    });
    s3_mods::hooks::commit(m);
}
