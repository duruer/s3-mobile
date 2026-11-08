//! Debug inject commands for runtime reverse engineering (re-ui, 2026-11-05).
//!
//! These are filtered out of the `--inject` spec before the normal parser sees it:
//! - `t:dump ADDR LEN FILE`  write `LEN` bytes of guest memory at `ADDR` to `FILE` (raw).
//! - `t:peek ADDR LEN`       print guest memory as hex dwords to stderr.
//! - `t:poke ADDR HEXBYTES`  write bytes (e.g. `poke 0x7e0000 0a000000`) — debug only, for verifying setters.
//! - `t:regs`                print every thread's EIP/registers to stderr.
//! - `t:watch ADDR LEN SECS` single-step all guest threads (run_slice(1)) for up to SECS wall seconds until the
//!                           bytes at ADDR change; then print old/new value and, for every thread that ran in the
//!                           changing step, the EIP before/after (the writer is in the block starting at "before").
//!                           Repeat the command to catch the next write. Slow (~1-3 M guest instr/s).
//! - `t:dumpall DIR`         write every committed private (heap/VirtualAlloc) region plus the exe `.data`
//!                           section to `DIR/<addr>.bin` (memory-mapped files and code are skipped).
//! - `t:dumpall DIR mapped`  same, plus committed memory-mapped file views (e.g. the game's text tables; local only).
//!
//! Only addresses inside committed guest memory may be used (an unmapped read faults the host).
//! The S3.EXE image `0x400000..0x7ED000` is always mapped.

use s3_engine::Engine;
use std::collections::VecDeque;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub enum DebugCmd {
    Dump { addr: u32, len: usize, file: PathBuf },
    Peek { addr: u32, len: usize },
    Poke { addr: u32, bytes: Vec<u8> },
    Regs,
    DumpAll { dir: PathBuf, with_mapped: bool },
    Watch { addr: u32, len: usize, secs: f64 },
}

fn num(s: &str) -> Option<u64> {
    match s.strip_prefix("0x") {
        Some(h) => u64::from_str_radix(h, 16).ok(),
        None => s.parse().ok(),
    }
}

/// Split debug commands out of an inject spec. Returns (remaining spec, debug commands sorted by time).
pub fn split(spec: &str) -> Result<(String, Vec<(u64, DebugCmd)>), String> {
    let mut rest = Vec::new();
    let mut cmds = Vec::new();
    for item in spec.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        let Some((t, cmd)) = item.split_once(':') else {
            rest.push(item);
            continue;
        };
        let w: Vec<&str> = cmd.split_whitespace().collect();
        let t_ms = || t.trim().parse::<u64>().map_err(|_| format!("bad time in '{item}'"));
        let arg = |i: usize| w.get(i).and_then(|s| num(s)).ok_or(format!("bad argument {i} in '{item}'"));
        let c = match w.first().copied() {
            Some("dump") => DebugCmd::Dump {
                addr: arg(1)? as u32,
                len: arg(2)? as usize,
                file: PathBuf::from(w.get(3).ok_or(format!("missing file in '{item}'"))?),
            },
            Some("peek") => DebugCmd::Peek { addr: arg(1)? as u32, len: arg(2)? as usize },
            Some("poke") => {
                let h = w.get(2).ok_or(format!("missing bytes in '{item}'"))?;
                if h.len() % 2 != 0 {
                    return Err(format!("odd hex length in '{item}'"));
                }
                let bytes = (0..h.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&h[i..i + 2], 16).map_err(|_| format!("bad hex in '{item}'")))
                    .collect::<Result<Vec<u8>, String>>()?;
                DebugCmd::Poke { addr: arg(1)? as u32, bytes }
            }
            Some("regs") => DebugCmd::Regs,
            Some("watch") => DebugCmd::Watch {
                addr: arg(1)? as u32,
                len: arg(2)? as usize,
                secs: w.get(3).and_then(|s| s.parse().ok()).ok_or(format!("bad secs in '{item}'"))?,
            },
            Some("dumpall") => DebugCmd::DumpAll {
                dir: PathBuf::from(w.get(1).ok_or(format!("missing dir in '{item}'"))?),
                with_mapped: w.get(2) == Some(&"mapped"),
            },
            _ => {
                rest.push(item);
                continue;
            }
        };
        cmds.push((t_ms()?, c));
    }
    cmds.sort_by_key(|(t, _)| *t);
    Ok((rest.join(";"), cmds))
}

fn mapped(engine: &Engine, addr: u32, len: usize) -> bool {
    let end = addr as u64 + len as u64;
    let mut a = addr as u64;
    while a < end {
        let info = engine.machine.mem.regions.query(a as u32);
        if info.state != s3_runtime::mem::MEM_COMMIT {
            return false;
        }
        a = info.base_address as u64 + info.region_size as u64;
        if info.region_size == 0 {
            return false;
        }
    }
    true
}

/// Run due debug commands.
pub fn run_due(engine: &mut Engine, cmds: &mut VecDeque<(u64, DebugCmd)>, now_ms: u64) {
    while cmds.front().map(|(t, _)| *t <= now_ms).unwrap_or(false) {
        let (_, c) = cmds.pop_front().unwrap();
        match c {
            DebugCmd::Dump { addr, len, file } => {
                if !mapped(engine, addr, len) {
                    eprintln!("s3run: dump {addr:#x}+{len:#x}: not mapped");
                    continue;
                }
                let b = engine.machine.mem.read_bytes(addr, len);
                match std::fs::write(&file, b) {
                    Ok(()) => eprintln!("s3run: dump {addr:#x}+{len:#x} -> {} at {now_ms} ms", file.display()),
                    Err(e) => eprintln!("s3run: dump {}: {e}", file.display()),
                }
            }
            DebugCmd::Peek { addr, len } => {
                if !mapped(engine, addr, len) {
                    eprintln!("s3run: peek {addr:#x}+{len:#x}: not mapped");
                    continue;
                }
                let b = engine.machine.mem.read_bytes(addr, len);
                for (i, ch) in b.chunks(16).enumerate() {
                    let words: Vec<String> = ch
                        .chunks(4)
                        .map(|w| w.iter().rev().map(|x| format!("{x:02x}")).collect::<String>())
                        .collect();
                    eprintln!("s3run: peek {:#010x}: {}", addr as usize + i * 16, words.join(" "));
                }
            }
            DebugCmd::Poke { addr, bytes } => {
                if !mapped(engine, addr, bytes.len()) {
                    eprintln!("s3run: poke {addr:#x}: not mapped");
                    continue;
                }
                engine.machine.mem.write_bytes(addr, &bytes);
                // Pokes may patch code (RE experiments): drop decoded instructions for the range.
                engine.machine.invalidate_code(addr, bytes.len() as u32);
                eprintln!("s3run: poke {addr:#x} <- {} bytes at {now_ms} ms", bytes.len());
            }
            DebugCmd::DumpAll { dir, with_mapped } => {
                let _ = std::fs::create_dir_all(&dir);
                let mut a: u64 = 0x10000;
                let (mut n, mut total) = (0, 0usize);
                while a < 0x8000_0000 {
                    let info = engine.machine.mem.regions.query(a as u32);
                    if info.region_size == 0 {
                        break;
                    }
                    let keep = info.state == s3_runtime::mem::MEM_COMMIT
                        && (info.type_ == s3_runtime::mem::MEM_PRIVATE
                            || (with_mapped && info.type_ == s3_runtime::mem::MEM_MAPPED)
                            || (info.base_address >= 0x692000 && info.base_address < 0x7eb000));
                    if keep {
                        let b = engine.machine.mem.read_bytes(info.base_address, info.region_size as usize);
                        total += b.len();
                        n += 1;
                        let _ = std::fs::write(dir.join(format!("{:08x}.bin", info.base_address)), b);
                    }
                    a = info.base_address as u64 + info.region_size as u64;
                }
                eprintln!("s3run: dumpall {} regions {} KiB -> {} at {now_ms} ms", n, total / 1024, dir.display());
            }
            DebugCmd::Watch { addr, len, secs } => {
                if !mapped(engine, addr, len) {
                    eprintln!("s3run: watch {addr:#x}: not mapped");
                    continue;
                }
                let t0 = std::time::Instant::now();
                let old = engine.machine.mem.read_bytes(addr, len);
                let mut steps = 0u64;
                loop {
                    let before: Vec<(u32, u64)> =
                        engine.machine.threads.iter().map(|t| (t.cpu.eip, t.cpu.instructions)).collect();
                    let r = engine.machine.run_slice(1);
                    steps += 1;
                    let new = engine.machine.mem.read_bytes(addr, len);
                    if new != old {
                        eprintln!("s3run: watch {addr:#x}: {old:02x?} -> {new:02x?} after {steps} steps");
                        for (i, t) in engine.machine.threads.iter().enumerate() {
                            if let Some(&(eip0, n0)) = before.get(i) {
                                if t.cpu.instructions != n0 {
                                    eprintln!(
                                        "s3run: watch   t{i} eip {eip0:#x} -> {:#x} ({} instr) regs {:x?}",
                                        t.cpu.eip,
                                        t.cpu.instructions - n0,
                                        t.cpu.regs
                                    );
                                }
                            }
                        }
                        break;
                    }
                    if r == s3_runtime::SliceResult::Stopped || t0.elapsed().as_secs_f64() > secs {
                        eprintln!("s3run: watch {addr:#x}: no change after {steps} steps");
                        break;
                    }
                    if r == s3_runtime::SliceResult::Idle {
                        engine.machine.host.idle(1);
                    }
                }
            }
            DebugCmd::Regs => {
                for (i, t) in engine.machine.threads.iter().enumerate() {
                    eprintln!("s3run: regs t{i} eip={:#x} {:x?} {:?}", t.cpu.eip, t.cpu.regs, t.state);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_debug_commands() {
        let (rest, c) = split("100:click 1 2;50:dump 0x400000 16 /tmp/a.bin;60:poke 0x10 0aff;70:peek 0x10 4;80:regs").unwrap();
        assert_eq!(rest, "100:click 1 2");
        assert_eq!(c[0], (50, DebugCmd::Dump { addr: 0x400000, len: 16, file: "/tmp/a.bin".into() }));
        assert_eq!(c[1], (60, DebugCmd::Poke { addr: 0x10, bytes: vec![0x0a, 0xff] }));
        assert_eq!(c[2], (70, DebugCmd::Peek { addr: 0x10, len: 4 }));
        assert_eq!(c[3], (80, DebugCmd::Regs));
        assert!(split("1:poke 0x10 abc").is_err());
    }

    #[test]
    fn split_dumpall_mapped_flag() {
        let (_, c) = split("1:dumpall d;2:dumpall e mapped").unwrap();
        assert_eq!(c[0], (1, DebugCmd::DumpAll { dir: "d".into(), with_mapped: false }));
        assert_eq!(c[1], (2, DebugCmd::DumpAll { dir: "e".into(), with_mapped: true }));
    }
}
