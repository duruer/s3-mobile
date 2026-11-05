//! Native fast paths: three small pixel loops of the GOG S3.EXE renderer (see `fast.rs` for the contract).
//!
//! Each runs whole loop iterations from its loop head, stops at the loop head when the instruction budget is
//! used up (the state there is exactly the interpreter's) or at the loop exit, and declines when the loop would
//! not terminate normally (odd distance, wrap), could fault (NULL guard) or when its writes overlap what it
//! reads.
//!
//! | Region | Loop | Guest instructions per iteration |
//! | --- | --- | --- |
//! | `0x5fc93d..0x5fc952` (in `0x5fc860`) | 8-bit indexed -> 16-bit pixels through a 256-entry palette at EBP | 8 |
//! | `0x5fd0ea..0x5fd0ff` (in `0x5fd0c0`) | darken dwords in place: `[eax] = ([eax] >> 1) & [0x7e8dec]` | 8 |
//! | `0x5fd096..0x5fd0ae` (in `0x5fd080`) | checkerboard shadow: zero every pixel where `(phase + ebp)` is odd, phase = `(phase+1) & 31` | 8, +1 when it writes |

use s3_cpu::{layout, Cpu, GuestMemory, EAX, EBX, ECX, EDI, ESI};

/// Largest iteration count handled natively (a longer loop is surely a wrapped/garbage range: decline).
const MAX_ITERS: u32 = 1 << 22;

/// `[a, a+len)` lies inside the guest window above the NULL guard without wrapping.
#[inline]
fn range_ok(a: u32, len: u64) -> bool {
    a >= layout::NULL_GUARD_END && a as u64 + len <= u32::MAX as u64
}

#[inline]
fn disjoint(a: u32, alen: u64, b: u32, blen: u64) -> bool {
    let (a, b) = (a as u64, b as u64);
    a + alen <= b || b + blen <= a
}

// ------------------------------------------------------------------ palette loop 0x5fc93d

pub const PAL_START: u32 = 0x005f_c93d;
pub const PAL_EXIT: u32 = 0x005f_c952;
pub const PAL_HASH: u64 = 0x4c41_6f63_4446_80ce;

/// `xor ebx,ebx; add eax,2; mov bl,[ecx]; inc ecx; cmp eax,esi; mov di,[ebp+ebx*2]; mov [eax-2],di; jne`.
pub fn palette_row(cpu: &mut Cpu, mem: &mut GuestMemory, budget: u64) -> Option<u64> {
    const PER: u64 = 8;
    let [mut eax, mut ecx, _, mut ebx, _, ebp, esi, mut edi] = cpu.regs;
    let dist = esi.wrapping_sub(eax);
    if dist == 0 || dist & 1 != 0 || dist / 2 > MAX_ITERS {
        return None;
    }
    let n = (dist / 2).min((budget / PER).min(u32::MAX as u64) as u32);
    if n == 0 {
        return None;
    }
    let (dlen, slen) = (2 * n as u64, n as u64);
    if !range_ok(eax, dlen) || !range_ok(ecx, slen) || !range_ok(ebp, 512) {
        return None;
    }
    if !disjoint(eax, dlen, ecx, slen) || !disjoint(eax, dlen, ebp, 512) {
        return None;
    }
    for _ in 0..n {
        ebx = mem.read_u8(ecx) as u32;
        eax = eax.wrapping_add(2);
        ecx = ecx.wrapping_add(1);
        let px = mem.read_u16(ebp.wrapping_add(ebx * 2));
        edi = (edi & 0xFFFF_0000) | px as u32;
        mem.write_u16(eax.wrapping_sub(2), px);
    }
    cpu.regs[EAX] = eax;
    cpu.regs[ECX] = ecx;
    cpu.regs[EBX] = ebx;
    cpu.regs[EDI] = edi;
    cpu.alu_sub(eax, esi, 0, 4);
    cpu.eip = if eax == esi { PAL_EXIT } else { PAL_START };
    Some(n as u64 * PER)
}

// ------------------------------------------------------------------ darken loop 0x5fd0ea

pub const DARK_START: u32 = 0x005f_d0ea;
pub const DARK_EXIT: u32 = 0x005f_d0ff;
pub const DARK_HASH: u64 = 0x50e3_c0c6_7165_21a4;
/// The mask global read every iteration.
const DARK_MASK: u32 = 0x007e_8dec;

/// `mov edi,[eax]; mov ebx,[0x7e8dec]; shr edi,1; and edi,ebx; mov [eax],edi; add eax,4; cmp eax,esi; jne`.
pub fn darken(cpu: &mut Cpu, mem: &mut GuestMemory, budget: u64) -> Option<u64> {
    const PER: u64 = 8;
    let [mut eax, _, _, _, _, _, esi, _] = cpu.regs;
    let dist = esi.wrapping_sub(eax);
    if dist == 0 || dist & 3 != 0 || dist / 4 > MAX_ITERS {
        return None;
    }
    let n = (dist / 4).min((budget / PER).min(u32::MAX as u64) as u32);
    if n == 0 {
        return None;
    }
    let len = 4 * n as u64;
    if !range_ok(eax, len) || !disjoint(eax, len, DARK_MASK, 4) {
        return None;
    }
    let mask = mem.read_u32(DARK_MASK);
    let mut edi = 0;
    for _ in 0..n {
        edi = (mem.read_u32(eax) >> 1) & mask;
        mem.write_u32(eax, edi);
        eax = eax.wrapping_add(4);
    }
    cpu.regs[EAX] = eax;
    cpu.regs[EBX] = mask;
    cpu.regs[EDI] = edi;
    cpu.alu_sub(eax, esi, 0, 4);
    cpu.eip = if eax == esi { DARK_EXIT } else { DARK_START };
    Some(n as u64 * PER)
}

// ------------------------------------------------------------------ checkerboard shadow 0x5fd096

pub const DITHER_START: u32 = 0x005f_d096;
pub const DITHER_EXIT: u32 = 0x005f_d0ae;
pub const DITHER_HASH: u64 = 0xa260_2e8f_9197_8473;

/// `lea ebx,[esi+ebp]; test bl,1; jz +; mov word [eax],0; +: add eax,2; inc esi; and esi,1f; cmp eax,edx; jnz`.
pub fn dither(cpu: &mut Cpu, mem: &mut GuestMemory, budget: u64) -> Option<u64> {
    let [mut eax, _, edx, mut ebx, _, ebp, mut esi, _] = cpu.regs;
    let dist = edx.wrapping_sub(eax);
    if dist == 0 || dist & 1 != 0 || dist / 2 > MAX_ITERS || !range_ok(eax, dist as u64) {
        return None;
    }
    let mut done = 0u64;
    while eax != edx {
        let b = esi.wrapping_add(ebp);
        let odd = b & 1;
        let per = 8 + odd as u64;
        if done + per > budget {
            break;
        }
        ebx = b;
        if odd != 0 {
            mem.write_u16(eax, 0);
        }
        eax = eax.wrapping_add(2);
        esi = esi.wrapping_add(1) & 0x1f;
        done += per;
    }
    if done == 0 {
        return None;
    }
    cpu.regs[EAX] = eax;
    cpu.regs[EBX] = ebx;
    cpu.regs[ESI] = esi;
    cpu.alu_sub(eax, edx, 0, 4);
    cpu.eip = if eax == edx { DITHER_EXIT } else { DITHER_START };
    Some(done)
}
