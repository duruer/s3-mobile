// SPDX-License-Identifier: GPL-3.0-or-later
//! Recursive-descent instruction census for a 32-bit PE (built for S3.EXE).
//! Reads the user's local exe at runtime and prints statistics only (no game bytes are written).
//!
//! usage: isa-census <exe> [--data-seeds] [--list-rare]
use iced_x86::{
    Code, CpuidFeature, Decoder, DecoderOptions, FlowControl, Formatter, Instruction, IntelFormatter, Mnemonic, OpKind,
    Register,
};
use std::collections::{BTreeMap, HashMap, HashSet};

struct Img {
    base: u32,
    mem: Vec<u8>,
    text: (u32, u32), // VA range [start, end)
    data_ranges: Vec<(u32, u32)>,
    entry: u32,
    imports: HashMap<u32, String>, // IAT slot VA -> "DLL!name"
}

fn rd16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn rd32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn load(path: &str) -> Img {
    let f = std::fs::read(path).expect("read exe");
    let pe = rd32(&f, 0x3c) as usize;
    let nsec = rd16(&f, pe + 6) as usize;
    let optsz = rd16(&f, pe + 20) as usize;
    let opt = pe + 24;
    let entry_rva = rd32(&f, opt + 16);
    let base = rd32(&f, opt + 28);
    let size = rd32(&f, opt + 56) as usize;
    let import_rva = rd32(&f, opt + 96 + 8) as usize;
    let mut mem = vec![0u8; size];
    mem[..0x1000.min(f.len())].copy_from_slice(&f[..0x1000.min(f.len())]);
    let mut text = (0, 0);
    let mut data_ranges = vec![];
    for i in 0..nsec {
        let s = opt + optsz + i * 40;
        let name = String::from_utf8_lossy(&f[s..s + 8]).trim_end_matches('\0').to_string();
        let vsz = rd32(&f, s + 8) as usize;
        let va = rd32(&f, s + 12) as usize;
        let rsz = rd32(&f, s + 16) as usize;
        let rptr = rd32(&f, s + 20) as usize;
        let n = rsz.min(vsz.max(rsz)).min(f.len() - rptr).min(size - va);
        mem[va..va + n].copy_from_slice(&f[rptr..rptr + n]);
        let r = (base + va as u32, base + (va + vsz) as u32);
        if name == ".text" {
            text = r;
        } else if name == ".rdata" || name == ".data" {
            data_ranges.push(r);
        }
    }
    let mut imports = HashMap::new();
    let mut d = import_rva;
    loop {
        let oft = rd32(&mem, d) as usize;
        let name_rva = rd32(&mem, d + 12) as usize;
        let ft = rd32(&mem, d + 16) as usize;
        if name_rva == 0 {
            break;
        }
        let dll = cstr(&mem, name_rva);
        let mut k = 0;
        loop {
            let thunk = rd32(&mem, if oft != 0 { oft } else { ft } + k * 4);
            if thunk == 0 {
                break;
            }
            let nm = if thunk & 0x8000_0000 != 0 { format!("#{}", thunk & 0xffff) } else { cstr(&mem, thunk as usize + 2) };
            imports.insert(base + (ft + k * 4) as u32, format!("{dll}!{nm}"));
            k += 1;
        }
        d += 20;
    }
    Img { base, mem, text, data_ranges, entry: base + entry_rva, imports }
}

fn cstr(m: &[u8], o: usize) -> String {
    let e = m[o..].iter().position(|&c| c == 0).unwrap_or(0);
    String::from_utf8_lossy(&m[o..o + e]).to_string()
}

impl Img {
    fn in_text(&self, va: u32) -> bool {
        va >= self.text.0 && va < self.text.1
    }
    fn u32_at(&self, va: u32) -> Option<u32> {
        let o = va.checked_sub(self.base)? as usize;
        if o + 4 <= self.mem.len() {
            Some(rd32(&self.mem, o))
        } else {
            None
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let img = load(&args[1]);
    let data_seeds = args.iter().any(|a| a == "--data-seeds");
    let list_rare = args.iter().any(|a| a == "--list-rare");

    let mut work: Vec<u32> = vec![img.entry];
    if data_seeds {
        for &(s, e) in &img.data_ranges {
            let mut a = s;
            while a + 4 <= e {
                let v = img.u32_at(a).unwrap();
                if img.in_text(v) {
                    work.push(v);
                }
                a += 4;
            }
        }
    }
    let mut seen: HashSet<u32> = HashSet::new(); // instruction starts
    let mut owner: HashMap<u32, u32> = HashMap::new(); // byte -> insn start (for overlap detection)
    let mut mn: BTreeMap<String, u64> = BTreeMap::new();
    let mut feat: BTreeMap<String, u64> = BTreeMap::new();
    let mut rare: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    let mut misc: BTreeMap<&str, u64> = BTreeMap::new();
    let mut misc_addr: BTreeMap<&str, Vec<u32>> = BTreeMap::new();
    let mut import_calls: BTreeMap<String, u64> = BTreeMap::new();
    let mut overlaps = 0u64;
    let mut invalid = 0u64;
    let mut jump_tables = 0u64;
    let mut fmt = IntelFormatter::new();
    let base_feats = [
        CpuidFeature::INTEL8086,
        CpuidFeature::INTEL186,
        CpuidFeature::INTEL286,
        CpuidFeature::INTEL386,
        CpuidFeature::INTEL486,
        CpuidFeature::FPU,
        CpuidFeature::FPU287,
        CpuidFeature::FPU387,
    ];

    while let Some(start) = work.pop() {
        let mut ip = start;
        loop {
            if !img.in_text(ip) || seen.contains(&ip) {
                break;
            }
            if let Some(&o) = owner.get(&ip) {
                if o != ip {
                    overlaps += 1;
                    break;
                }
            }
            let off = (ip - img.base) as usize;
            let end = (img.text.1 - img.base) as usize;
            let mut dec = Decoder::with_ip(32, &img.mem[off..end], ip as u64, DecoderOptions::NONE);
            let ins: Instruction = dec.decode();
            if ins.is_invalid() {
                invalid += 1;
                break;
            }
            seen.insert(ip);
            for b in 0..ins.len() as u32 {
                owner.insert(ip + b, ip);
            }
            let mut name = format!("{:?}", ins.mnemonic()).to_lowercase();
            if ins.has_rep_prefix() && ins.mnemonic() != Mnemonic::Ret {
                name = format!("rep {name}");
            }
            if ins.has_repne_prefix() {
                name = format!("repne {name}");
            }
            if ins.has_lock_prefix() {
                name = format!("lock {name}");
            }
            *mn.entry(name.clone()).or_default() += 1;
            for f in ins.cpuid_features() {
                *feat.entry(format!("{f:?}")).or_default() += 1;
                if !base_feats.contains(f) {
                    rare.entry(format!("{f:?} {name}")).or_default().push(ip);
                }
            }
            // segment overrides
            match ins.segment_prefix() {
                Register::FS => {
                    *misc.entry("fs: override").or_default() += 1;
                }
                Register::GS => {
                    *misc.entry("gs: override").or_default() += 1;
                    misc_addr.entry("gs: override").or_default().push(ip);
                }
                Register::None | Register::DS | Register::SS => {}
                _ => {
                    *misc.entry("es/cs override").or_default() += 1;
                    misc_addr.entry("es/cs override").or_default().push(ip);
                }
            }
            if matches!(ins.code(), Code::Rdtsc) {
                misc_addr.entry("rdtsc").or_default().push(ip);
            }
            if ins.op_count() > 0 && ins.op0_kind() == OpKind::Memory && ins.memory_base() == Register::None
                && ins.memory_index() == Register::None && img.in_text(ins.memory_displacement32())
                && ins.mnemonic() != Mnemonic::Call && ins.mnemonic() != Mnemonic::Jmp
            {
                // store to an absolute .text address = possible self-modifying code
                let info_write = matches!(ins.mnemonic(), Mnemonic::Mov | Mnemonic::Add | Mnemonic::Sub | Mnemonic::Xor | Mnemonic::Or | Mnemonic::And | Mnemonic::Inc | Mnemonic::Dec);
                if info_write {
                    *misc.entry("store to .text (SMC?)").or_default() += 1;
                    misc_addr.entry("store to .text (SMC?)").or_default().push(ip);
                }
            }
            // immediates pointing into .text = callbacks / function pointers
            for i in 0..ins.op_count() {
                if ins.op_kind(i) == OpKind::Immediate32 {
                    let v = ins.immediate32();
                    if img.in_text(v) {
                        work.push(v);
                        *misc.entry("code-address immediates").or_default() += 1;
                    }
                }
            }
            let next = ip + ins.len() as u32;
            match ins.flow_control() {
                FlowControl::Next => {}
                FlowControl::ConditionalBranch | FlowControl::UnconditionalBranch => {
                    if ins.op0_kind() == OpKind::NearBranch32 {
                        work.push(ins.near_branch32());
                    }
                    if ins.flow_control() == FlowControl::UnconditionalBranch {
                        break;
                    }
                }
                FlowControl::Call => {
                    if ins.op0_kind() == OpKind::NearBranch32 {
                        work.push(ins.near_branch32());
                    }
                }
                FlowControl::IndirectCall => {
                    if ins.memory_base() == Register::None && ins.memory_index() == Register::None {
                        if let Some(n) = img.imports.get(&ins.memory_displacement32()) {
                            *import_calls.entry(n.clone()).or_default() += 1;
                            if n.ends_with("!ExitProcess") || n.ends_with("!ExitThread") || n.ends_with("!RaiseException") && false {
                                break;
                            }
                        }
                    } else {
                        *misc.entry("indirect call via reg/mem (vtable etc.)").or_default() += 1;
                    }
                }
                FlowControl::IndirectBranch => {
                    if ins.op0_kind() == OpKind::Memory && ins.memory_index() != Register::None && ins.memory_index_scale() == 4 {
                        let t = ins.memory_displacement32();
                        let mut k = 0;
                        jump_tables += 1;
                        while k < 1024 {
                            match img.u32_at(t + k * 4) {
                                Some(v) if img.in_text(v) => work.push(v),
                                _ => break,
                            }
                            k += 1;
                        }
                    } else if ins.op0_kind() == OpKind::Memory && ins.memory_base() == Register::None && ins.memory_index() == Register::None {
                        if img.imports.contains_key(&ins.memory_displacement32()) {
                            *misc.entry("import thunks (jmp [iat])").or_default() += 1;
                        }
                    } else {
                        *misc.entry("indirect jmp other").or_default() += 1;
                    }
                    break;
                }
                FlowControl::Return | FlowControl::Interrupt | FlowControl::Exception => break,
                FlowControl::XbeginXabortXend => break,
            }
            ip = next;
            let _ = &mut fmt;
        }
    }
    let total: u64 = mn.values().sum();
    let text_bytes: usize = owner.len();
    println!("mode: {}", if data_seeds { "entry + code immediates + data pointers" } else { "entry + code immediates" });
    println!("instructions decoded: {total}, bytes covered: {text_bytes} of {} .text bytes", img.text.1 - img.text.0);
    println!("overlap conflicts: {overlaps}, invalid stops: {invalid}, jump tables followed: {jump_tables}");
    println!("\n## CPUID feature classes");
    let mut fv: Vec<_> = feat.iter().collect();
    fv.sort_by(|a, b| b.1.cmp(a.1));
    for (k, v) in fv {
        println!("{v:>8} {k}");
    }
    println!("\n## mnemonics");
    let mut v: Vec<_> = mn.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1));
    for (k, c) in v {
        println!("{c:>8} {k}");
    }
    println!("\n## misc");
    for (k, c) in &misc {
        println!("{c:>8} {k}");
    }
    for (k, a) in &misc_addr {
        let s: Vec<String> = a.iter().take(20).map(|x| format!("{x:#x}")).collect();
        println!("  {k}: {}", s.join(" "));
    }
    if list_rare {
        println!("\n## non-386/x87 instructions (addresses, first 12)");
        for (k, a) in &rare {
            let s: Vec<String> = a.iter().take(12).map(|x| format!("{x:#x}")).collect();
            println!("{:>6} {k}: {}", a.len(), s.join(" "));
        }
    }
    println!("\n## direct import call sites (call [IAT])");
    let mut iv: Vec<_> = import_calls.iter().collect();
    iv.sort_by(|a, b| b.1.cmp(a.1));
    for (k, c) in iv {
        println!("{c:>6} {k}");
    }
}
