//! `s3det --write-trace FROM-TO --write-trace-out FILE`: which guest memory each thread writes (perf-mt evidence
//! for docs/porting/multicore.md). Between virtual ms FROM and TO, a sample of the scheduling turns of every thread
//! (at most one per thread every `SAMPLE_VMS` virtual ms) is bracketed by a copy of all writable guest pages
//! before and a byte compare after; written bytes are accumulated per thread. The report lists, per thread, the
//! written ranges grouped by region (S3.EXE image, heap/VirtualAlloc block, stack, TEB) and the bytes it shares
//! with every other thread. Sampling only reads guest memory: the run (and its hashes) stays identical.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use s3_runtime::Machine;

/// Minimum virtual ms between two samples of the same thread.
const SAMPLE_VMS: u64 = 40;

struct Trace {
    from: u64,
    to: u64,
    now_vms: u64,
    last_sample: HashMap<usize, u64>,
    /// Snapshot of the writable pages taken before the sampled turn of `slot`.
    before: Option<(usize, Vec<(u32, Box<[u8; 4096]>)>)>,
    /// slot -> page -> bitmap of written bytes.
    written: BTreeMap<usize, BTreeMap<u32, Box<[u64; 64]>>>,
    samples: HashMap<usize, u32>,
}

static TRACE: Mutex<Option<Trace>> = Mutex::new(None);

/// Parse `FROM-TO` (virtual ms) and install the observer.
pub fn install(m: &mut Machine, spec: &str) -> Result<(), String> {
    let (a, b) = spec.split_once('-').ok_or("--write-trace FROM-TO")?;
    let from = a.parse().map_err(|_| "--write-trace FROM-TO")?;
    let to = b.parse().map_err(|_| "--write-trace FROM-TO")?;
    *TRACE.lock().unwrap() = Some(Trace {
        from,
        to,
        now_vms: 0,
        last_sample: HashMap::new(),
        before: None,
        written: BTreeMap::new(),
        samples: HashMap::new(),
    });
    m.run_observer = Some(observe);
    Ok(())
}

/// Current virtual ms (called by the s3det loop).
pub fn set_now(vms: u64) {
    if let Some(t) = TRACE.lock().unwrap().as_mut() {
        t.now_vms = vms;
    }
}

fn writable(protect: u32) -> bool {
    protect & (0x04 | 0x08 | 0x40 | 0x80) != 0
}

fn pages(m: &Machine) -> Vec<u32> {
    let mut v = Vec::new();
    for a in m.mem.regions.iter() {
        if matches!(a.kind, s3_runtime::mem::RegionKind::Mapped) {
            continue;
        }
        for (i, &prot) in a.pages.iter().enumerate() {
            if prot != 0 && writable(prot) {
                v.push(a.base + (i as u32) * 4096);
            }
        }
    }
    v
}

fn observe(m: &mut Machine, slot: usize, after: bool) {
    let mut g = TRACE.lock().unwrap();
    let Some(t) = g.as_mut() else { return };
    if !after {
        if t.now_vms < t.from || t.now_vms >= t.to {
            return;
        }
        if t.last_sample
            .get(&slot)
            .is_some_and(|&l| t.now_vms < l + SAMPLE_VMS)
        {
            return;
        }
        t.last_sample.insert(slot, t.now_vms);
        let snap = pages(m)
            .into_iter()
            .map(|a| {
                let mut b = Box::new([0u8; 4096]);
                b.copy_from_slice(m.mem.slice(a, 4096));
                (a, b)
            })
            .collect();
        t.before = Some((slot, snap));
        return;
    }
    let Some((s, snap)) = t.before.take() else {
        return;
    };
    if s != slot {
        return;
    }
    *t.samples.entry(slot).or_default() += 1;
    let w = t.written.entry(slot).or_default();
    for (a, old) in snap {
        let new = m.mem.slice(a, 4096);
        if new == &old[..] {
            continue;
        }
        let bits = w.entry(a).or_insert_with(|| Box::new([0u64; 64]));
        for (i, (x, y)) in old.iter().zip(new).enumerate() {
            if x != y {
                bits[i / 64] |= 1 << (i % 64);
            }
        }
    }
}

fn ranges(bits: &BTreeMap<u32, Box<[u64; 64]>>) -> Vec<(u32, u32)> {
    let mut out: Vec<(u32, u32)> = Vec::new();
    for (&page, b) in bits {
        for i in 0..4096u32 {
            if b[(i / 64) as usize] >> (i % 64) & 1 == 1 {
                let a = page + i;
                match out.last_mut() {
                    Some((s, l)) if *s + *l == a => *l += 1,
                    _ => out.push((a, 1)),
                }
            }
        }
    }
    out
}

/// Region label of a guest address.
fn region(m: &Machine, a: u32) -> String {
    if (0x0040_0000..0x007e_d000).contains(&a) {
        return "S3.EXE image".into();
    }
    if (0x7f00_0000..0x7fff_0000).contains(&a) {
        return "TEB/PEB".into();
    }
    for t in &m.threads {
        if a >= t.stack_bottom && a < t.stack_top {
            return format!("stack of thread tid {:#x}", t.tid);
        }
    }
    for r in m.mem.regions.iter() {
        if a >= r.base && (a as u64) < r.base as u64 + r.size as u64 {
            return format!("block {:#x}+{:#x} ({:?})", r.base, r.size, r.kind);
        }
    }
    "unallocated".into()
}

/// Write the report (call at the end of the run).
pub fn report(m: &Machine, path: &std::path::Path) -> Result<(), String> {
    let g = TRACE.lock().unwrap();
    let Some(t) = g.as_ref() else { return Ok(()) };
    let mut s = String::new();
    s.push_str(&format!(
        "# s3det write trace, vms {}..{}, one sampled scheduling turn per thread every {} vms\n",
        t.from, t.to, SAMPLE_VMS
    ));
    for (&slot, bits) in &t.written {
        let tid = m.threads.get(slot).map(|x| x.tid).unwrap_or(0);
        let total: u32 = bits
            .values()
            .map(|b| b.iter().map(|w| w.count_ones()).sum::<u32>())
            .sum();
        s.push_str(&format!(
            "\n## thread slot {slot} (tid {tid:#x}): {} samples, {} bytes written\n",
            t.samples.get(&slot).copied().unwrap_or(0),
            total
        ));
        // Group ranges by region label.
        let mut by_region: BTreeMap<String, (u32, Vec<(u32, u32)>)> = BTreeMap::new();
        for (a, l) in ranges(bits) {
            let e = by_region.entry(region(m, a)).or_default();
            e.0 += l;
            e.1.push((a, l));
        }
        for (r, (n, v)) in &by_region {
            s.push_str(&format!("- {r}: {n} bytes in {} ranges", v.len()));
            let show: Vec<String> = v
                .iter()
                .take(24)
                .map(|(a, l)| format!("{a:#x}+{l}"))
                .collect();
            s.push_str(&format!(
                ": {}{}\n",
                show.join(" "),
                if v.len() > 24 { " ..." } else { "" }
            ));
        }
        // Overlap with the other threads' writes.
        for (&o, obits) in &t.written {
            if o == slot {
                continue;
            }
            let mut shared = 0u32;
            let mut sample = Vec::new();
            for (page, b) in bits {
                if let Some(ob) = obits.get(page) {
                    for i in 0..64 {
                        let x = b[i] & ob[i];
                        if x != 0 {
                            shared += x.count_ones();
                            if sample.len() < 16 {
                                sample.push(format!(
                                    "{:#x}",
                                    page + (i as u32) * 64 + x.trailing_zeros()
                                ));
                            }
                        }
                    }
                }
            }
            s.push_str(&format!(
                "  - also written by slot {o}: {shared} bytes {}\n",
                sample.join(" ")
            ));
        }
    }
    std::fs::write(path, s).map_err(|e| e.to_string())
}
