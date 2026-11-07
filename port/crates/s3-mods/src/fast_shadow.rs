//! Native fast paths: the run/row loops of the two shadow sprite functions of the GOG S3.EXE (perf-shadow, see
//! `fast.rs` for the contract, docs/porting/performance.md).
//!
//! Both functions walk an RLE stream of `(count, skip|last)` byte pairs per row and call a per-run function
//! through a pointer global for every run; the loops (caller part + call + callee) are replaced as a whole.
//!
//! **`shadow_zero`** (`0x5fc2cd..0x5fc346`, second row loop of `0x5fc160`): per run `b0 = count`, `b1 = skip|last`;
//! `dst = cur + 2*(b1 & 0x7f)`, then `call [0x7e8c74]` (= `0x5f48e0`, hash-checked): zero `b0 >> 1` dwords and,
//! when `b0 & 1`, one word; the source cursor `[esp+0x90]` advances by 2 + the pixel bytes (the pixels are
//! skipped, not read). Row end: row start `[esp+0x10]` += pitch `[esp+0x34]` (`shr 1`, `*2`), EDI = row counter
//! vs `[esp+0x14]` (signed). Guest instructions: 38 + 5/dword + 3 (odd word) per run, 9 per row end, 1 row head.
//!
//! **`shadow_rows`** (`0x5fd24f..0x5fd301`, row loop of `0x5fd120`): per run `dst = cur + 2*skip`, phase
//! `[esp+0x90] = (phase + skip) & 31`, `call [0x7e8e20]` with `ecx = &cur`, `edx = dst + 2*count`, args
//! `(&phase, row phase ESI)`. The callee is `0x5fd080` (checkerboard: zero pixels where `phase + row phase` is
//! odd) or `0x5fd0c0` (darken: `(px >> 1) & mask`, word head/tail with `[0x7e8dd8]`, dwords with `[0x7e8dec]`),
//! both hash-checked. Row end (`0x5fd2ba`): row start `[esp+0x8c]` += pitch `[esp+0x30]`, ESI = (ESI + 1) mod 32
//! (C remainder), `[esp+0x90] = EDI` (start phase), counter `[esp+0xa4]` vs `[esp+0x18]` (signed).
//!
//! Every guest write is reproduced in the original order (stack slots of the call/pushes included, so the memory
//! below ESP is identical too); the natives stop at a run head, a row head or the loop exit, run whole runs
//! within the budget and stop (or decline) before a run that could fault, would not terminate (odd distance,
//! wrap), writes into the stack frame / the darken mask, or calls an unknown function.

use s3_cpu::{code_hash, layout, Cpu, GuestMemory, EAX, EBP, EBX, ECX, EDI, EDX, ESI, ESP};

/// `[a, a+len)` inside the guest window above the NULL guard, no wrap.
#[inline]
fn range_ok(a: u32, len: u64) -> bool {
    a >= layout::NULL_GUARD_END && a as u64 + len <= u32::MAX as u64
}

#[inline]
fn disjoint(a: u32, alen: u64, b: u32, blen: u64) -> bool {
    let (a, b) = (a as u64, b as u64);
    a + alen <= b || b + blen <= a
}

/// Largest run length handled (bytes); a longer one is garbage: stop.
const MAX_RUN: u64 = 1 << 20;

// ================================================================== 0x5fc160 second loop (zero runs)

/// Row head `mov [esp+0x98],eax` (jl target and first entry).
pub const ZERO_ROW: u32 = 0x005f_c2cd;
/// Run head `mov ecx,[esp+0x90]` (je target).
pub const ZERO_RUN: u32 = 0x005f_c2d4;
/// Loop exit `pop edi`.
pub const ZERO_EXIT: u32 = 0x005f_c346;
pub const ZERO_ROW_HASH: u64 = 0x4c3d_6969_605d_5f17;
pub const ZERO_RUN_HASH: u64 = 0x8911_9add_0620_64f6;
/// Hash of the row head instruction alone (`ZERO_ROW..ZERO_RUN`), checked when entered at the run head.
const ZERO_HEAD_HASH: u64 = 0x29fa_16c2_5452_31b4;
/// Return address of the run call.
const ZERO_RET: u32 = 0x005f_c327;
/// Function pointer global and the only callee handled: zero `[ecx]..edx` dwords + optional word.
const ZERO_PTR: u32 = 0x007e_8c74;
const ZERO_FN: u32 = 0x005f_48e0;
const ZERO_FN_LEN: usize = 0x3a;
const ZERO_FN_HASH: u64 = 0xb0f4_8b04_d105_8220;

pub fn shadow_zero(cpu: &mut Cpu, mem: &mut GuestMemory, budget: u64) -> Option<u64> {
    let s = cpu.regs[ESP];
    // frame: callee pushes down to s-0x14, locals up to s+0x9c
    let (flo, fhi) = (s.wrapping_sub(0x14), 0x14u64 + 0xa0);
    if s < 0x1000 || !range_ok(flo, fhi) || !disjoint(flo, fhi, ZERO_PTR, 4) {
        return None;
    }
    let mut pc = cpu.eip;
    let mut head_ok = pc == ZERO_ROW;
    let mut fn_ok = false;
    let mut done = 0u64;
    loop {
        match pc {
            ZERO_ROW => {
                if done + 1 > budget {
                    break;
                }
                if !head_ok {
                    if code_hash(mem.slice(ZERO_ROW, 7)) != ZERO_HEAD_HASH {
                        break;
                    }
                    head_ok = true;
                }
                mem.write_u32(s.wrapping_add(0x98), cpu.regs[EAX]);
                done += 1;
                pc = ZERO_RUN;
            }
            ZERO_RUN => {
                let src = mem.read_u32(s.wrapping_add(0x90));
                if !range_ok(src, 2) || !disjoint(src, 2, flo, fhi) {
                    break;
                }
                let b0 = mem.read_u8(src) as u32;
                let b1 = mem.read_u8(src.wrapping_add(1)) as u32;
                let cur = mem.read_u32(s.wrapping_add(0x98));
                let dst = cur.wrapping_add(2 * (b1 & 0x7f));
                let n = b0 >> 1;
                let odd = b0 & 1;
                let len = 4 * n as u64 + 2 * odd as u64;
                if !range_ok(dst, len) || !disjoint(dst, len, flo, fhi) || !disjoint(dst, len, ZERO_PTR, 4) {
                    break;
                }
                if mem.read_u32(ZERO_PTR) != ZERO_FN {
                    break;
                }
                if !fn_ok {
                    if code_hash(mem.slice(ZERO_FN, ZERO_FN_LEN)) != ZERO_FN_HASH {
                        break;
                    }
                    fn_ok = true;
                }
                let cost = 38 + 5 * n as u64 + 3 * odd as u64;
                if done + cost > budget {
                    break;
                }
                // caller 0x5fc2d4..0x5fc321
                mem.write_u32(s.wrapping_add(0x90), src.wrapping_add(2));
                mem.write_u32(s.wrapping_sub(4), odd); // push ecx (= b0 & 1)
                mem.write_u32(s.wrapping_sub(8), s.wrapping_add(0x90)); // push edx
                mem.write_u32(s.wrapping_add(0x98), dst);
                mem.write_u32(s.wrapping_sub(0xc), ZERO_RET);
                // callee 0x5f48e0
                let edi = cpu.regs[EDI];
                mem.write_u32(s.wrapping_sub(0x10), dst); // push esi (= dst)
                mem.write_u32(s.wrapping_sub(0x14), edi); // push edi
                let dend = dst.wrapping_add(4 * n);
                if n > 0 {
                    mem.fill(dst, 4 * n as usize, 0);
                }
                let mut eax = dend;
                if odd != 0 {
                    mem.write_u16(eax, 0);
                    eax = eax.wrapping_add(2);
                }
                mem.write_u32(s.wrapping_add(0x98), eax); // [ecx] = eax
                mem.write_u32(s.wrapping_add(0x90), src.wrapping_add(2).wrapping_add(len as u32)); // [edi] = esi
                cpu.regs[EAX] = eax;
                cpu.regs[EBX] = b1;
                cpu.regs[ECX] = s.wrapping_add(0x98);
                cpu.regs[EDX] = (dend & !0xff) | odd;
                cpu.regs[ESI] = dst;
                // test bl,0x80
                cpu.alu_logic(b1 & 0x80, 1);
                done += cost;
                pc = if b1 & 0x80 == 0 { ZERO_RUN } else { 0x005f_c32c };
            }
            0x005f_c32c => {
                if done + 9 > budget {
                    break;
                }
                let pitch = mem.read_u32(s.wrapping_add(0x34)) >> 1;
                let row = mem.read_u32(s.wrapping_add(0x10));
                let edi = cpu.regs[EDI].wrapping_add(1);
                let eax = row.wrapping_add(pitch.wrapping_mul(2));
                let rows = mem.read_u32(s.wrapping_add(0x14));
                cpu.alu_sub(edi, rows, 0, 4);
                mem.write_u32(s.wrapping_add(0x10), eax);
                cpu.regs[EAX] = eax;
                cpu.regs[ECX] = rows;
                cpu.regs[EDI] = edi;
                done += 9;
                pc = if (edi as i32) < (rows as i32) { ZERO_ROW } else { ZERO_EXIT };
            }
            _ => break,
        }
    }
    if done == 0 {
        return None;
    }
    cpu.eip = pc;
    Some(done)
}

// ================================================================== 0x5fd120 row loop (dither / darken runs)

/// First entry (`mov eax,[esp+0x8c]`, fall-through after the `jle` that skips the loop).
pub const ROWS_FIRST: u32 = 0x005f_d24f;
/// Row head `mov [esp+0x94],eax` (jl target).
pub const ROWS_ROW: u32 = 0x005f_d256;
/// Run head `mov ecx,[esp+0x10]` (je target).
pub const ROWS_RUN: u32 = 0x005f_d25d;
/// Row end (fall-through after the `je`).
const ROWS_END: u32 = 0x005f_d2ba;
/// Loop exit `pop edi`.
pub const ROWS_EXIT: u32 = 0x005f_d301;
pub const ROWS_FIRST_HASH: u64 = 0x98cc_f1dd_5cea_9327;
pub const ROWS_ROW_HASH: u64 = 0x999a_6335_cb88_679e;
pub const ROWS_RUN_HASH: u64 = 0xecbd_f380_9897_f81b;
/// Hash of the row head instruction alone (`ROWS_ROW..ROWS_RUN`), checked when entered at the run head.
const ROWS_HEAD_HASH: u64 = 0xa9e4_c6e3_ad29_23f8;
const ROWS_RET: u32 = 0x005f_d2b6;
const ROWS_PTR: u32 = 0x007e_8e20;
const DITHER_FN: u32 = 0x005f_d080;
const DITHER_FN_LEN: usize = 0x39;
const DITHER_FN_HASH: u64 = 0xe8fc_6c04_049b_befb;
const DARKEN_FN: u32 = 0x005f_d0c0;
const DARKEN_FN_LEN: usize = 0x5f;
const DARKEN_FN_HASH: u64 = 0x871d_d4d2_7f59_cc9a;
/// Darken masks: word head/tail, dwords.
const MASK16: u32 = 0x007e_8dd8;
const MASK32: u32 = 0x007e_8dec;

/// Guest instructions of `0x5fd080` from entry to `ret` for a run `[dst, end)`, phase `ph`, row phase `rp`, or
/// `None` if it would not terminate normally / could fault.
fn dither_cost(dst: u32, end: u32, ph: u32, rp: u32) -> Option<u64> {
    if dst >= end {
        return Some(11);
    }
    let dist = (end - dst) as u64;
    if dist & 1 != 0 || dist > MAX_RUN || !range_ok(dst, dist) {
        return None;
    }
    let c = dist / 2;
    let odd = if ph.wrapping_add(rp) & 1 != 0 { c.div_ceil(2) } else { c / 2 };
    Some(18 + 8 * c + odd)
}

/// Guest instructions of `0x5fd0c0` for a run `[dst, end)`, or `None` (fault / no termination / the dword
/// loop would write its own mask).
fn darken_cost(dst: u32, end: u32) -> Option<u64> {
    if dst >= end {
        return Some(4);
    }
    let dist = (end - dst) as u64;
    if (dst | end) & 1 != 0 || dist > MAX_RUN || !range_ok(dst, dist) {
        return None;
    }
    let h = (dst & 2 != 0) as u64;
    let a = dst.wrapping_add(2 * h as u32);
    let al = end & !3;
    let t = (al != end) as u64;
    let n = ((al - a) / 4) as u64;
    if n > 0 && !disjoint(a, 4 * n, MASK32, 4) {
        return None;
    }
    Some(17 + 6 * h + 6 * t + if n > 0 { 2 + 8 * n } else { 0 })
}

/// Run `0x5fd080` from its entry (EIP at the function, ESP at the return address) to after its `ret 8`, in the
/// original order of reads and writes. The caller has checked `dither_cost` for the same state.
fn dither_exec(cpu: &mut Cpu, mem: &mut GuestMemory) {
    let e = cpu.regs[ESP];
    let [_, ecx0, edx, ebx, _, ebp, esi, edi] = cpu.regs;
    mem.write_u32(e.wrapping_sub(4), esi);
    mem.write_u32(e.wrapping_sub(8), edi);
    let dptr = ecx0;
    let pptr = mem.read_u32(e.wrapping_add(4));
    let mut eax = mem.read_u32(dptr);
    let mut ph = mem.read_u32(pptr);
    if eax >= edx {
        cpu.alu_sub(eax, edx, 0, 4);
    } else {
        mem.write_u32(e.wrapping_sub(0xc), ebx);
        mem.write_u32(e.wrapping_sub(0x10), ebp);
        let rp = mem.read_u32(e.wrapping_add(8));
        // pixel i is cleared when (ph + i + rp) is odd; ph's parity alternates (the & 31 keeps it)
        let mut p = eax;
        let mut odd = ph.wrapping_add(rp) & 1;
        while p != edx {
            if odd != 0 {
                mem.write_u16(p, 0);
            }
            odd ^= 1;
            p = p.wrapping_add(2);
        }
        let c = (edx - eax) / 2;
        ph = ph.wrapping_add(c) & 0x1f;
        eax = edx;
        mem.write_u32(dptr, eax);
        mem.write_u32(pptr, ph);
        cpu.alu_sub(eax, edx, 0, 4);
    }
    cpu.regs[EAX] = eax;
    cpu.regs[ECX] = pptr;
    cpu.eip = mem.read_u32(e);
    cpu.regs[ESP] = e.wrapping_add(0xc);
}

/// Run `0x5fd0c0` from its entry to after its `ret 8` (original order). The caller has checked `darken_cost`.
fn darken_exec(cpu: &mut Cpu, mem: &mut GuestMemory) {
    let e = cpu.regs[ESP];
    let [_, ecx, mut edx, _, _, _, esi0, edi0] = cpu.regs;
    let mut eax = mem.read_u32(ecx);
    if eax >= edx {
        cpu.alu_sub(eax, edx, 0, 4);
    } else {
        mem.write_u32(e.wrapping_sub(4), esi0);
        mem.write_u32(e.wrapping_sub(8), edi0);
        if eax & 2 != 0 {
            let m = mem.read_u32(MASK16);
            let v = ((mem.read_u16(eax) >> 1) as u32 | (esi0 & 0xffff_0000)) & m;
            mem.write_u16(eax, v as u16);
            eax = eax.wrapping_add(2);
        }
        let al = edx & !3;
        if eax != al {
            mem.write_u32(e.wrapping_sub(0xc), cpu.regs[EBX]);
            let m = mem.read_u32(MASK32);
            while eax != al {
                let v = (mem.read_u32(eax) >> 1) & m;
                mem.write_u32(eax, v);
                eax = eax.wrapping_add(4);
            }
        }
        if al != edx {
            let m = mem.read_u32(MASK16);
            edx = ((mem.read_u16(eax) >> 1) as u32 | (edx & 0xffff_0000)) & m;
            mem.write_u16(eax, edx as u16);
            cpu.alu_add(eax, 2, 0, 4);
            eax = eax.wrapping_add(2);
        } else {
            cpu.alu_sub(al, edx, 0, 4);
        }
        mem.write_u32(ecx, eax);
    }
    cpu.regs[EAX] = eax;
    cpu.regs[EDX] = edx;
    cpu.eip = mem.read_u32(e);
    cpu.regs[ESP] = e.wrapping_add(0xc);
}

pub fn shadow_rows(cpu: &mut Cpu, mem: &mut GuestMemory, budget: u64) -> Option<u64> {
    let s = cpu.regs[ESP];
    // frame: call + callee pushes down to s-0x1c, locals/arguments up to s+0xb8
    let (flo, fhi) = (s.wrapping_sub(0x1c), 0x1cu64 + 0xb8);
    if s < 0x1000 || !range_ok(flo, fhi) || !disjoint(flo, fhi, ROWS_PTR, 4) {
        return None;
    }
    if !disjoint(flo, fhi, MASK16, 4) || !disjoint(flo, fhi, MASK32, 4) {
        return None;
    }
    let mut pc = cpu.eip;
    let mut head_ok = pc != ROWS_RUN;
    let (mut dither_ok, mut darken_ok) = (false, false);
    let mut done = 0u64;
    loop {
        match pc {
            ROWS_FIRST => {
                if done + 1 > budget {
                    break;
                }
                cpu.regs[EAX] = mem.read_u32(s.wrapping_add(0x8c));
                done += 1;
                pc = ROWS_ROW;
            }
            ROWS_ROW => {
                if done + 1 > budget {
                    break;
                }
                if !head_ok {
                    if code_hash(mem.slice(ROWS_ROW, 7)) != ROWS_HEAD_HASH {
                        break;
                    }
                    head_ok = true;
                }
                mem.write_u32(s.wrapping_add(0x94), cpu.regs[EAX]);
                done += 1;
                pc = ROWS_RUN;
            }
            ROWS_RUN => {
                let src = mem.read_u32(s.wrapping_add(0x10));
                if !range_ok(src, 2) {
                    break;
                }
                let cnt = mem.read_u8(src) as u32;
                let b1 = mem.read_u8(src.wrapping_add(1)) as u32;
                let ph0 = mem.read_u32(s.wrapping_add(0x90));
                let cur = mem.read_u32(s.wrapping_add(0x94));
                let skip = b1 & 0x7f;
                let dst = cur.wrapping_add(2 * skip);
                let ph = skip.wrapping_add(ph0) & 0x1f;
                let end = dst.wrapping_add(2 * cnt);
                let rp = cpu.regs[ESI];
                let target = mem.read_u32(ROWS_PTR);
                let cost = match target {
                    DITHER_FN => {
                        if !dither_ok {
                            if code_hash(mem.slice(DITHER_FN, DITHER_FN_LEN)) != DITHER_FN_HASH {
                                break;
                            }
                            dither_ok = true;
                        }
                        dither_cost(dst, end, ph, rp)
                    }
                    DARKEN_FN => {
                        if !darken_ok {
                            if code_hash(mem.slice(DARKEN_FN, DARKEN_FN_LEN)) != DARKEN_FN_HASH {
                                break;
                            }
                            darken_ok = true;
                        }
                        darken_cost(dst, end)
                    }
                    _ => None,
                };
                let Some(cost) = cost else { break };
                if dst < end {
                    let len = (end - dst) as u64;
                    if !disjoint(dst, len, flo, fhi) || !disjoint(dst, len, ROWS_PTR, 4) {
                        break;
                    }
                }
                let cost = 25 + cost;
                if done + cost > budget {
                    break;
                }
                // caller 0x5fd25d..0x5fd2b0
                let esi = cpu.regs[ESI];
                let ebx = (cpu.regs[EBX] & !0xff) | (b1 & 0x80);
                mem.write_u32(s.wrapping_add(0x10), src.wrapping_add(2));
                mem.write_u32(s.wrapping_sub(4), esi); // push esi
                mem.write_u32(s.wrapping_add(0x94), dst);
                mem.write_u32(s.wrapping_add(0x90), ph);
                mem.write_u32(s.wrapping_sub(8), s.wrapping_add(0x90)); // push eax
                mem.write_u32(s.wrapping_sub(0xc), ROWS_RET); // call
                cpu.regs[EAX] = s.wrapping_add(0x90);
                cpu.regs[ECX] = s.wrapping_add(0x94);
                cpu.regs[EDX] = end;
                cpu.regs[EBX] = ebx;
                cpu.regs[EBP] = ph0;
                cpu.regs[ESP] = s.wrapping_sub(0xc);
                if target == DITHER_FN {
                    dither_exec(cpu, mem);
                } else {
                    darken_exec(cpu, mem);
                }
                debug_assert_eq!(cpu.regs[ESP], s);
                // test bl,bl; je
                cpu.alu_logic(b1 & 0x80, 1);
                done += cost;
                pc = if b1 & 0x80 == 0 { ROWS_RUN } else { ROWS_END };
            }
            ROWS_END => {
                let mut esi = cpu.regs[ESI].wrapping_add(1) & 0x8000_001f;
                let neg = esi & 0x8000_0000 != 0;
                let cost = if neg { 18 } else { 15 };
                if done + cost > budget {
                    break;
                }
                let pitch = mem.read_u32(s.wrapping_add(0x30)) >> 1;
                let row = mem.read_u32(s.wrapping_add(0x8c));
                if neg {
                    esi = (esi.wrapping_sub(1) | 0xffff_ffe0).wrapping_add(1);
                }
                let eax = row.wrapping_add(pitch.wrapping_mul(2));
                mem.write_u32(s.wrapping_add(0x8c), eax);
                let ecx = mem.read_u32(s.wrapping_add(0xa4)).wrapping_add(1);
                let edx = mem.read_u32(s.wrapping_add(0x18));
                mem.write_u32(s.wrapping_add(0x90), cpu.regs[EDI]);
                cpu.alu_sub(ecx, edx, 0, 4);
                mem.write_u32(s.wrapping_add(0xa4), ecx);
                cpu.regs[EAX] = eax;
                cpu.regs[ECX] = ecx;
                cpu.regs[EDX] = edx;
                cpu.regs[ESI] = esi;
                done += cost;
                pc = if (ecx as i32) < (edx as i32) { ROWS_ROW } else { ROWS_EXIT };
            }
            _ => break,
        }
    }
    if done == 0 {
        return None;
    }
    cpu.eip = pc;
    Some(done)
}
