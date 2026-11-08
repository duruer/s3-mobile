//! Native fast path: the triangle loop of the landscape pass `0x54cbd0` (draw thread) for triangles that need no
//! redraw (see `fast.rs` for the contract, docs/porting/performance.md).
//!
//! Loop head `0x54ce53`: `while [ebp-0x74] < [ebp-0xc4]`, alternating up/down triangles (`[ebp-0x5c]` = 1 / 0) over
//! the snapshot cells of one row (`cell = [ebp+8] + ([ebp-0x48] + [ebp-0x44]*130)*0x30`). Per triangle it reads three
//! corner heights, computes their screen y (`[ebp-0x98] - h`, +8 for some corners), keeps the minimum in
//! `[ebp-0x34]`, and draws only when a corner's dirty bit (`+5` bit 7) or one of the flags `[ebp-0x68]`,
//! `[ebp-0xe4]`, `[ebp-0xe8]` is set. In game almost all triangles are clean (the landscape is cached), so the
//! loop runs ~65 guest instructions per triangle, ~16 000 triangles per frame at 1536x600: 17-25 % of all guest
//! instructions. Only clean triangles are native; any other ends the native run at the loop head before it.

use s3_cpu::{layout, Cpu, GuestMemory, EAX, EBP, EBX, ECX, EDI, EDX, ESI};

use crate::res;

pub const START: u32 = 0x0054_ce53;
/// End of the code the native path stands for (the `jmp 0x54ce53` at 0x54e70d is its last instruction).
pub const END: u32 = 0x0054_e712;
/// FNV-1a of the code *after* the res patch (`res::code_patches`: row stride 258 in this loop).
pub const HASH: u64 = 0x6653_2149_bbd1_c4ee;
/// Cell offset of the same row in the next snapshot column (`0x500 + 0x1860` in the game's 130-row grid; the
/// res patch makes the stride [`res::SNAP_ROWS`]).
const NEXT: u32 = 0x500 + res::SNAP_COL_BYTES_NEW;
/// Bytes read from `cell + 0x500`: up to byte `+5` of the next column's next row.
const CELLS_LEN: u32 = NEXT + 0x36 - 0x500;

#[inline]
fn ok(a: u32, len: u32) -> bool {
    a >= layout::NULL_GUARD_END && a as u64 + len as u64 <= u32::MAX as u64
}

#[inline]
fn disjoint(a: u32, alen: u32, b: u32, blen: u32) -> bool {
    let (a, b) = (a as u64, b as u64);
    a + alen as u64 <= b || b + blen as u64 <= a
}

pub fn land_clean(cpu: &mut Cpu, mem: &mut GuestMemory, budget: u64) -> Option<u64> {
    let ebp = cpu.regs[EBP];
    if ebp < 0x1000 || !ok(ebp - 0xe8, 0xe8 + 0xc) {
        return None;
    }
    let fr = ebp - 0xe8; // frame range [ebp-0xe8, ebp+0xc)
    let rd = |mem: &GuestMemory, o: u32| mem.read_u32(ebp.wrapping_sub(o));
    // Loop-invariant flags (only the loop body below writes the frame, and not these).
    if rd(mem, 0x68) == 1 || rd(mem, 0xe4) == 1 || rd(mem, 0xe8) == 1 {
        return None;
    }
    let base = mem.read_u32(ebp.wrapping_add(8));
    let v48 = rd(mem, 0x48);
    let v98 = rd(mem, 0x98);
    let v9c = rd(mem, 0x9c);
    let vc4 = rd(mem, 0xc4);
    let mut ecx = cpu.regs[ECX];
    let mut done = 0u64;
    let mut out = None::<(u32, u32, u32, u32, u32, u32, u32)>; // eax ecx edx ebx esi edi, phase of the last triangle
    loop {
        let v74 = rd(mem, 0x74);
        if (v74 as i32) >= (vc4 as i32) {
            break; // loop exit: the original takes it
        }
        let phase = rd(mem, 0x5c);
        let v44 = rd(mem, 0x44);
        let off = v48.wrapping_add(v44.wrapping_mul(res::SNAP_ROWS)).wrapping_mul(48);
        let cell = off.wrapping_add(base);
        if !ok(cell.wrapping_add(0x500), CELLS_LEN) || !disjoint(cell.wrapping_add(0x500), CELLS_LEN, fr, 0xf4) {
            break;
        }
        let b = |o: u32| mem.read_u8(cell.wrapping_add(o)) as u32;
        // (eax, edi, ebx) entering the min/max block 0x54cf66, guest instructions so far
        let (eax, edi, ebx, mut n) = if phase == 1 {
            let (h1, h0, h2) = (b(NEXT + 0x30), b(0x500), b(NEXT));
            if b(0x505) & 0x80 != 0 || b(NEXT + 0x35) & 0x80 != 0 || b(NEXT + 5) & 0x80 != 0 {
                break;
            }
            (v98.wrapping_sub(h1).wrapping_add(8), v98.wrapping_sub(h0), v98.wrapping_sub(h2), 4 + 7 + 31u64)
        } else {
            let (h0, h1, h3) = (b(0x500), b(NEXT + 0x30), b(0x530));
            if b(0x505) & 0x80 != 0 || b(NEXT + 0x35) & 0x80 != 0 || b(0x535) & 0x80 != 0 {
                break;
            }
            (v98.wrapping_sub(h0), v98.wrapping_sub(h3).wrapping_add(8), v98.wrapping_sub(h1).wrapping_add(8), 4 + 7 + 33u64)
        };
        // 0x54cf66 min/max
        n += 5;
        let (mut edx, mut cx) = (eax, edi);
        if (edi as i32) < (eax as i32) {
            edx = edi;
            cx = eax;
            n += 3;
        }
        n += 2;
        if (ebx as i32) < (edx as i32) {
            edx = ebx;
            n += 2;
        }
        n += 2;
        if (ebx as i32) > (cx as i32) {
            n += 1;
        }
        n += 8; // 0x54cf87 / 0x54cfac / 0x54cfc5 / 0x54cfde (cmp [ebp-0x2c],1 -> 0, jne)
        n += 4 + 8 + if phase == 1 { 1 } else { 0 }; // 0x54e6e8, (0x54e6f3), 0x54e6f6
        if done + n > budget {
            break;
        }
        // ---- commit (frame writes only)
        let wr = |mem: &mut GuestMemory, o: u32, v: u32| mem.write_u32(ebp.wrapping_sub(o), v);
        ecx = ecx.wrapping_add(8);
        wr(mem, 0x30, ecx);
        wr(mem, 0x20, eax); // up: v98 - h1 + 8, down: v98 - h0
        wr(mem, 0x2c, 0);
        wr(mem, 0x34, edx);
        if phase == 1 {
            wr(mem, 0x44, v44.wrapping_add(1));
        }
        let nphase = 1u32.wrapping_sub(phase);
        wr(mem, 0x5c, nphase);
        wr(mem, 0x74, v74.wrapping_add(1));
        done += n;
        out = Some((v9c, ecx, nphase, ebx, cell, edi, phase));
    }
    let (eax, ecx, edx, ebx, esi, edi, phase) = out?;
    cpu.regs[EAX] = eax;
    cpu.regs[ECX] = ecx;
    cpu.regs[EDX] = edx;
    cpu.regs[EBX] = ebx;
    cpu.regs[ESI] = esi;
    cpu.regs[EDI] = edi;
    // flags: `sub edx,eax` (1 - phase) then `inc eax` ([ebp-0x74]+1; keeps CF)
    cpu.alu_sub(1, phase, 0, 4);
    let v74 = rd(mem, 0x74);
    cpu.alu_inc(v74.wrapping_sub(1), 4);
    cpu.eip = START;
    Some(done)
}
