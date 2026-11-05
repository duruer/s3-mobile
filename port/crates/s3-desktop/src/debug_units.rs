//! Read-only debug inject commands for game-state reverse engineering (re-units, 2026-11-05).
//! See docs/reverse-engineering/game-state-units.md.
//!
//! Filtered out of the `--inject` spec before the other parsers see it:
//! - `t:xdump EXPR LEN FILE` write `LEN` bytes at the address `EXPR` to `FILE` (raw).
//! - `t:xpeek EXPR LEN`      print `LEN` bytes at `EXPR` as hex dwords to stderr.
//! - `t:scan VALUE [MAX]`    print the addresses of up to `MAX` (default 32) aligned dwords equal to `VALUE` in all
//!   committed guest memory (find heap objects by vtable, pointers to an object, ...).
//!
//! `EXPR` is a start address followed by `@OFF` steps; each step reads the dword at the current address and adds
//! `OFF` (pointer chasing): `0x700ffee0@0x3a0@0x67444+0x4800` = `*( *(0x700ffee0) + 0x3a0 ) + 0x67444 + 0x4800`.
//! A part may be a `+` sum (added without deref). All numbers are decimal or `0x` hex. Nothing here writes guest memory.

use s3_engine::Engine;
use std::collections::VecDeque;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub enum UnitsCmd {
    Dump { expr: Vec<u32>, len: usize, file: PathBuf },
    Peek { expr: Vec<u32>, len: usize },
    Scan { value: u32, max: usize },
}

fn num(s: &str) -> Option<u64> {
    match s.strip_prefix("0x") {
        Some(h) => u64::from_str_radix(h, 16).ok(),
        None => s.parse().ok(),
    }
}

/// Parse `ADDR@OFF@OFF...` into `[ADDR, OFF, OFF, ...]`. Each part may be a sum `A+B+...` (no deref inside a part).
fn parse_expr(s: &str) -> Option<Vec<u32>> {
    s.split('@')
        .map(|p| p.split('+').try_fold(0u32, |acc, t| num(t).map(|v| acc.wrapping_add(v as u32))))
        .collect()
}

/// Split our commands out of an inject spec. Returns (remaining spec, commands sorted by time).
pub fn split(spec: &str) -> Result<(String, Vec<(u64, UnitsCmd)>), String> {
    let mut rest = Vec::new();
    let mut cmds = Vec::new();
    for item in spec.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        let Some((t, cmd)) = item.split_once(':') else {
            rest.push(item);
            continue;
        };
        let w: Vec<&str> = cmd.split_whitespace().collect();
        let expr = || w.get(1).and_then(|s| parse_expr(s)).ok_or(format!("bad address in '{item}'"));
        let arg = |i: usize| w.get(i).and_then(|s| num(s)).ok_or(format!("bad argument {i} in '{item}'"));
        let c = match w.first().copied() {
            Some("xdump") => UnitsCmd::Dump {
                expr: expr()?,
                len: arg(2)? as usize,
                file: PathBuf::from(w.get(3).ok_or(format!("missing file in '{item}'"))?),
            },
            Some("xpeek") => UnitsCmd::Peek { expr: expr()?, len: arg(2)? as usize },
            Some("scan") => UnitsCmd::Scan { value: arg(1)? as u32, max: if w.len() > 2 { arg(2)? as usize } else { 32 } },
            _ => {
                rest.push(item);
                continue;
            }
        };
        let t: u64 = t.trim().parse().map_err(|_| format!("bad time in '{item}'"))?;
        cmds.push((t, c));
    }
    cmds.sort_by_key(|(t, _)| *t);
    Ok((rest.join(";"), cmds))
}

fn committed(engine: &Engine, addr: u32, len: usize) -> bool {
    let end = addr as u64 + len.max(1) as u64;
    let mut a = addr as u64;
    while a < end {
        let info = engine.machine.mem.regions.query(a as u32);
        if info.state != s3_runtime::mem::MEM_COMMIT || info.region_size == 0 {
            return false;
        }
        a = info.base_address as u64 + info.region_size as u64;
    }
    true
}

fn eval(engine: &Engine, expr: &[u32]) -> Option<u32> {
    let mut a = *expr.first()?;
    for off in &expr[1..] {
        if !committed(engine, a, 4) {
            return None;
        }
        let b = engine.machine.mem.read_bytes(a, 4);
        a = u32::from_le_bytes([b[0], b[1], b[2], b[3]]).wrapping_add(*off);
    }
    Some(a)
}

/// Run due commands.
pub fn run_due(engine: &mut Engine, cmds: &mut VecDeque<(u64, UnitsCmd)>, now_ms: u64) {
    while cmds.front().map(|(t, _)| *t <= now_ms).unwrap_or(false) {
        let (_, c) = cmds.pop_front().unwrap();
        match c {
            UnitsCmd::Dump { expr, len, file } => {
                let Some(addr) = eval(engine, &expr).filter(|a| committed(engine, *a, len)) else {
                    eprintln!("s3run: xdump {expr:x?}: not mapped");
                    continue;
                };
                let b = engine.machine.mem.read_bytes(addr, len);
                match std::fs::write(&file, b) {
                    Ok(()) => eprintln!("s3run: xdump {addr:#x}+{len:#x} -> {} at {now_ms} ms", file.display()),
                    Err(e) => eprintln!("s3run: xdump {}: {e}", file.display()),
                }
            }
            UnitsCmd::Peek { expr, len } => {
                let Some(addr) = eval(engine, &expr).filter(|a| committed(engine, *a, len)) else {
                    eprintln!("s3run: xpeek {expr:x?}: not mapped");
                    continue;
                };
                let b = engine.machine.mem.read_bytes(addr, len);
                for (i, ch) in b.chunks(16).enumerate() {
                    let words: Vec<String> = ch
                        .chunks(4)
                        .map(|w| w.iter().rev().map(|x| format!("{x:02x}")).collect::<String>())
                        .collect();
                    eprintln!("s3run: xpeek {:#010x}: {}", addr as usize + i * 16, words.join(" "));
                }
            }
            UnitsCmd::Scan { value, max } => {
                let mut found = Vec::new();
                let mut a: u64 = 0x10000;
                while a < 0xFFFF_0000 && found.len() < max {
                    let info = engine.machine.mem.regions.query(a as u32);
                    let size = (info.region_size as u64).max(0x1000);
                    if info.state == s3_runtime::mem::MEM_COMMIT {
                        let b = engine.machine.mem.read_bytes(info.base_address, info.region_size as usize);
                        for (i, w) in b.chunks_exact(4).enumerate() {
                            if u32::from_le_bytes([w[0], w[1], w[2], w[3]]) == value {
                                found.push(info.base_address + 4 * i as u32);
                                if found.len() >= max {
                                    break;
                                }
                            }
                        }
                    }
                    a = info.base_address as u64 + size;
                }
                eprintln!("s3run: scan {value:#x} at {now_ms} ms: {found:x?}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_units_commands() {
        let (rest, c) = split("100:click 1 2;50:xdump 0x7e9174@0x90@0x3a0 16 /tmp/a.bin;60:scan 0x658c18;70:xpeek 0x10 4;80:dump 0x1 2 f").unwrap();
        assert_eq!(rest, "100:click 1 2;80:dump 0x1 2 f");
        assert_eq!(c[0], (50, UnitsCmd::Dump { expr: vec![0x7e9174, 0x90, 0x3a0], len: 16, file: "/tmp/a.bin".into() }));
        assert_eq!(c[1], (60, UnitsCmd::Scan { value: 0x658c18, max: 32 }));
        assert_eq!(c[2], (70, UnitsCmd::Peek { expr: vec![0x10], len: 4 }));
        assert!(split("1:xpeek 0x10@zz 4").is_err());
        assert_eq!(parse_expr("0x10+0x20@4+1"), Some(vec![0x30, 5]));
    }
}
