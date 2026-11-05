//! Native fast path: the per-tile copy loop of the render snapshot fill (logic thread, `0x51f110`; see `fast.rs` for
//! the contract and docs/porting/performance.md).
//!
//! The loop at `0x522219..0x52303e` walks one diagonal of the visible map: per iteration one map tile
//! (`gd + 0x67444 + (row*768 + col)*24`, `row = [esp+0x28]`, `col = [esp+0x44]`) is copied into one snapshot cell
//! (`snap + 0x500 + ([esp+0x20] + k*130)*0x30`, `k = [esp+0x34]`), then `row` and `k` advance while `k <= [esp+0x3c]`.
//! It was 18-23 % of all guest instructions in game on the phone aspect (2316x904, 1536x600 render size).
//!
//! Only the common path is native (133 guest instructions): tile inside the map, `[gd+0x2d284] != 1`, not the
//! special row `[gd+0x2d2e4]`, tile byte `+0xb` != `[gd+0x2d300]`, light byte `+9` with equal low/high 3-bit
//! fields and bit 7 clear, no object (`+0` word 0), no flag 0x20 in `+0xa`, no `+2` word. Any other tile ends the
//! native run at the loop head *before* that tile, and the original code handles it.

use s3_cpu::{layout, Cpu, GuestMemory, EAX, EBP, EBX, ECX, EDI, EDX, ESI, ESP};

pub const START: u32 = 0x0052_2219;
/// First instruction after the loop (`jle` not taken at `0x523038`).
pub const EXIT: u32 = 0x0052_303e;
pub const HASH: u64 = 0xe6ca_74bf_1298_2e82;
/// Guest instructions per common-path iteration (blocks 0x522219 5, 0x52222b 5, 0x522240 2, 0x522248 2,
/// 0x522250 2, 0x5223b9 3, 0x5223f5 45, 0x5224f2 21, 0x52254e 13, 0x5225d0 10, 0x522b23 6, 0x522ed2 3, 0x522f33 7,
/// 0x523020 9).
pub const PER: u64 = 133;

#[inline]
fn ok(a: u32, len: u32) -> bool {
    a >= layout::NULL_GUARD_END && a as u64 + len as u64 <= u32::MAX as u64
}

#[inline]
fn disjoint(a: u32, alen: u32, b: u32, blen: u32) -> bool {
    let (a, b) = (a as u64, b as u64);
    a + alen as u64 <= b || b + blen as u64 <= a
}

pub fn snap_tiles(cpu: &mut Cpu, mem: &mut GuestMemory, budget: u64) -> Option<u64> {
    let ebp = cpu.regs[EBP];
    let s = cpu.regs[ESP];
    if !ok(s, 0x48) || !ok(ebp, 0x3d4) {
        return None;
    }
    let obj = (ebp.wrapping_add(0x3a0), 0x34u32);
    if !disjoint(s, 0x48, obj.0, obj.1) {
        return None;
    }
    let mut done = 0u64;
    let mut last = None::<(u32, u32, u32, u32)>; // eax, ecx, edx, ebx after the last native iteration
    loop {
        if done + PER > budget {
            break;
        }
        // ---- phase 1: reads and branch conditions (no side effects).
        let row = mem.read_u32(s.wrapping_add(0x28));
        let col = mem.read_u32(s.wrapping_add(0x44));
        if (row as i32) < 0 {
            break;
        }
        let gd = mem.read_u32(ebp.wrapping_add(0x3a0));
        if !ok(gd, 0x2d304) {
            break;
        }
        let max = mem.read_u32(gd.wrapping_add(0x2d2d0)).wrapping_sub(1);
        if (row as i32) > (max as i32) || (col as i32) < 0 || (col as i32) > (max as i32) {
            break;
        }
        if mem.read_u32(gd.wrapping_add(0x2d284)) == 1 {
            break;
        }
        if row == mem.read_u32(gd.wrapping_add(0x2d2e4)) {
            break;
        }
        let ebx = row.wrapping_mul(768).wrapping_add(col).wrapping_mul(24);
        let t = gd.wrapping_add(ebx).wrapping_add(0x67444);
        if !ok(t, 0x12) {
            break;
        }
        let k = mem.read_u32(s.wrapping_add(0x34));
        let base = mem.read_u32(s.wrapping_add(0x20));
        let idx = base.wrapping_add(k.wrapping_mul(130));
        let esi = idx.wrapping_mul(48);
        let snap = mem.read_u32(ebp.wrapping_add(0x3d0));
        let w = snap.wrapping_add(esi).wrapping_add(0x500);
        if !ok(w, 0x20)
            || !disjoint(w, 0x20, obj.0, obj.1)
            || !disjoint(w, 0x20, t, 0x12)
            || !disjoint(w, 0x20, gd.wrapping_add(0x2d284), 0x80)
            || !disjoint(w, 0x20, s, 0x48)
            || !disjoint(s, 0x48, t, 0x12)
            || !disjoint(s, 0x48, gd.wrapping_add(0x2d284), 0x80)
        {
            break;
        }
        let b = |o: u32| mem.read_u8(t.wrapping_add(o));
        if b(0xb) == mem.read_u8(gd.wrapping_add(0x2d300)) {
            break;
        }
        let light = b(9) as u32;
        let a = light & 7;
        if a != (light >> 3) & 7 {
            break;
        }
        // 0x52254e: cl = a*63/7 (via the magic multiply) + (light & 0xc0)
        let c = a * 63;
        let prod = c as u64 * 0x2492_4925u64;
        let mhi = (prod >> 32) as u32;
        let cl = ((((c.wrapping_sub(mhi)) >> 1).wrapping_add(mhi)) >> 2).wrapping_add(light & 0xc0);
        if cl & 0x80 != 0 {
            break;
        }
        let w0 = mem.read_u16(t);
        if w0 != 0 {
            break;
        }
        if b(0xa) & 0x20 != 0 {
            break;
        }
        if mem.read_u16(t.wrapping_add(2)) != 0 {
            break;
        }
        let kmax = mem.read_u32(s.wrapping_add(0x3c));
        // ---- phase 2: the writes, in the original order (no overlaps, checked above).
        let (b4, b5, b6, b7, b8, ba, b11) = (b(4), b(5), b(6), b(7), mem.read_u16(t.wrapping_add(0xe)), b(0xa), b(0x11));
        let b8c = b(8);
        mem.write_u32(s.wrapping_add(0x18), idx);
        mem.write_u8(w, b4); // +500 <- tile+4
        mem.write_u8(w + 2, b6); // +502 <- tile+6
        mem.write_u8(w + 1, b5); // +501 <- tile+5
        mem.write_u8(w + 3, b7); // +503 <- tile+7
        mem.write_u16(w + 8, b8); // +508 <- tile+0xe (word)
        mem.write_u8(w + 6, ba); // +506 <- tile+0xa
        mem.write_u8(w + 7, 0); // +507 <- 0
        mem.write_u8(w + 4, b8c); // +504 <- tile+8
        mem.write_u32(s.wrapping_add(0x14), t.wrapping_add(9));
        mem.write_u8(s.wrapping_add(0x38), light as u8);
        mem.write_u32(s.wrapping_add(0x10), a);
        mem.write_u8(w + 5, cl as u8); // +505
        mem.write_u32(s.wrapping_add(0x1c), 0);
        mem.write_u16(w + 0x1e, 0); // +51e <- tile+0 word (0)
        mem.write_u16(w + 0xa, 0); // +50a <- di = 0
        mem.write_u8(w + 0xc, b11); // +50c <- tile+0x11
        let esi2 = row.wrapping_add(1);
        let eax2 = k.wrapping_add(1);
        mem.write_u32(s.wrapping_add(0x28), esi2);
        mem.write_u32(s.wrapping_add(0x34), eax2);
        done += PER;
        last = Some((eax2, kmax, snap, ebx));
        cpu.regs[ESI] = esi2;
        if (eax2 as i32) > (kmax as i32) {
            break;
        }
    }
    let (eax, ecx, edx, ebx) = last?;
    cpu.regs[EAX] = eax;
    cpu.regs[ECX] = ecx;
    cpu.regs[EDX] = edx;
    cpu.regs[EBX] = ebx;
    cpu.regs[EDI] = 0;
    cpu.alu_sub(eax, ecx, 0, 4);
    cpu.eip = if (eax as i32) <= (ecx as i32) { START } else { EXIT };
    Some(done)
}
