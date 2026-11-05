//! Native fast path: opaque RLE sprite rows (GOG S3.EXE, inside the sprite blitter `0x5fc350`).
//!
//! Region `0x5fc4c0..0x5fc536` is the second row loop of the 16-bit sprite blitter: per row a list of runs
//! `(len_bytes, skip|last)`: byte 0 = pixel count (bit 0 = one extra 16-bit pixel, the rest copied as dwords),
//! byte 1 = `skip` pixels (bits 0-6) and "last run of the row" (bit 7); the pixels follow the 2-byte header. At
//! the row end the destination row start `[esp+0x10]` advances by the pitch `[esp+0x30]` (bytes, `shr 1` then
//! `*2`), the row counter EDI increments and the loop repeats while `EDI < [esp+0x98]` (signed).
//!
//! The native version reproduces the guest state exactly at row boundaries (EIP `0x5fc4c0`) or at the loop exit
//! (`0x5fc536`): all registers, EFLAGS (from the final `cmp edi, ecx`), the stack slots it writes
//! (`[esp+0x8c]` = source cursor, `[esp+0x10]` = row start) and every pixel, plus the exact number of guest
//! instructions. It runs whole rows only, within the instruction budget, and declines a row (falls back to the
//! interpreter) if any address could fault or source, destination and the stack slots overlap.
//! Verified by `tests::differential_sprite_rows` (needs the local game files) and by `s3det` hash runs
//! (docs/porting/performance.md).

use s3_cpu::{layout, Cpu, GuestMemory, ESP};

/// Start of the replaced region (row loop head).
pub const START: u32 = 0x005f_c4c0;
/// First address after the loop (`pop edi`).
pub const EXIT: u32 = 0x005f_c536;
/// `s3_cpu::code_hash` of the GOG bytes `START..EXIT` (tools/perf/code_hash.py).
pub const HASH: u64 = 0x2b90_3216_3848_d568;

/// Guest instructions per run header (`0x5fc4c0..0x5fc4eb`), per copied dword, for the odd-pixel test, for an odd
/// pixel, for the run tail (`0x5fc50c..0x5fc517`) and for the row end (`0x5fc519..0x5fc534`).
const N_HEAD: u64 = 15;
const N_DWORD: u64 = 6;
const N_ODD_TEST: u64 = 2;
const N_ODD: u64 = 4;
const N_TAIL: u64 = 4;
const N_ROW_END: u64 = 9;

/// `[lo, hi)` (u64, no wrap) contains `[a, a+len)`?
#[inline]
fn overlaps(lo: u64, hi: u64, a: u32, len: u64) -> bool {
    let a = a as u64;
    a < hi && a + len > lo
}

/// Scan one row from `src` / `dst` without writing: (instructions, src end, dst end) or `None` if the row
/// is unsafe to run natively (fault possible or wrap).
fn scan_row(mem: &GuestMemory, src: u32, dst: u32) -> Option<(u64, u64, u64)> {
    let g = layout::NULL_GUARD_END as u64;
    let (mut p, mut d) = (src as u64, dst as u64);
    if p < g || d < g {
        return None;
    }
    let mut n = 0u64;
    loop {
        if p + 2 > u32::MAX as u64 {
            return None;
        }
        let b0 = mem.read_u8(p as u32) as u64;
        let b1 = mem.read_u8(p as u32 + 1) as u64;
        p += 2;
        d += (b1 & 0x7f) * 2;
        let dwords = (b0 & 0xfe) / 2;
        let odd = b0 & 1;
        n += N_HEAD + N_DWORD * dwords + N_ODD_TEST + N_ODD * odd + N_TAIL;
        p += dwords * 4 + odd * 2;
        d += dwords * 4 + odd * 2;
        if p > u32::MAX as u64 || d > u32::MAX as u64 {
            return None;
        }
        if b1 & 0x80 != 0 {
            break;
        }
    }
    Some((n + N_ROW_END, p, d))
}

/// The fast path (see the module docs and `s3_cpu::NativeFn`).
pub fn sprite_rows(cpu: &mut Cpu, mem: &mut GuestMemory, budget: u64) -> Option<u64> {
    let esp = cpu.regs[ESP];
    let g = layout::NULL_GUARD_END;
    if esp < g || esp > 0xFFFF_FF00 {
        return None;
    }
    let (s10, s30, s8c, s98) = (esp + 0x10, esp + 0x30, esp + 0x8c, esp + 0x98);
    let [mut eax, _, _, mut ebx, _, mut ebp, mut esi, mut edi] = cpu.regs;
    let (mut ecx, mut edx);
    let mut done = 0u64;
    loop {
        // One row from the loop head (EIP 0x5fc4c0): check it, then run it.
        let Some((n, src_end, dst_end)) = scan_row(mem, eax, esi) else { break };
        if done + n > budget {
            break;
        }
        let (src_lo, dst_lo) = (eax as u64, esi as u64);
        if overlaps(src_lo, src_end, esi, dst_end - dst_lo)
            || [s10, s30, s8c, s98].iter().any(|&s| overlaps(dst_lo, dst_end, s, 4))
            || [s10, s8c].iter().any(|&s| overlaps(src_lo, src_end, s, 4))
        {
            break;
        }
        loop {
            let b0 = mem.read_u8(eax) as u32;
            let b1 = mem.read_u8(eax.wrapping_add(1)) as u32;
            // xor ecx,ecx / xor edx,edx / mov cl,[eax+1] / mov dl,[eax] / mov bl,cl / and ecx,7f / and bl,80
            ebx = (ebx & !0xFF) | (b1 & 0x80);
            edx = b0;
            eax = eax.wrapping_add(2);
            ecx = esi.wrapping_add((b1 & 0x7f) * 2);
            esi = ecx.wrapping_add((edx & 0xfe) * 2);
            mem.write_u32(s8c, eax);
            while ecx != esi {
                ebp = mem.read_u32(eax);
                eax = eax.wrapping_add(4);
                mem.write_u32(ecx, ebp);
                ecx = ecx.wrapping_add(4);
            }
            if edx & 1 != 0 {
                edx = mem.read_u16(eax) as u32;
                eax = eax.wrapping_add(2);
                mem.write_u16(ecx, edx as u16);
                ecx = ecx.wrapping_add(2);
            }
            esi = ecx;
            mem.write_u32(s8c, eax);
            if ebx & 0xFF != 0 {
                break;
            }
        }
        // Row end: next row start = [esp+0x10] + ([esp+0x30] >> 1) * 2; edi++; loop while edi < [esp+0x98].
        let pitch = mem.read_u32(s30) >> 1;
        edx = mem.read_u32(s10);
        edi = edi.wrapping_add(1);
        esi = edx.wrapping_add(pitch.wrapping_mul(2));
        ecx = mem.read_u32(s98);
        mem.write_u32(s10, esi);
        done += n;
        // Flags of `cmp edi, ecx` (the shr before it is overwritten).
        cpu.alu_sub(edi, ecx, 0, 4);
        cpu.regs = [eax, ecx, edx, ebx, esp, ebp, esi, edi];
        if (edi as i32) < (ecx as i32) {
            cpu.eip = START;
        } else {
            cpu.eip = EXIT;
            return Some(done);
        }
    }
    (done > 0).then_some(done)
}
