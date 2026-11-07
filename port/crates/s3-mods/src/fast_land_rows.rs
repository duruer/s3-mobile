//! Native fast path: the row loop of the dirty-triangle rasteriser in the landscape pass `0x54cbd0` (draw thread),
//! see `fast.rs` for the contract and docs/porting/performance.md.
//!
//! A triangle that needs a redraw (dirty corner, or every triangle after a size switch: full landscape redraw) is set
//! up per triangle (`0x54d015..0x54e4bc`: edge slopes, texture choice), then drawn row by row from the row head
//! `0x54e4bc` (`while [ebp-0x14] > 0`) until `0x54e6e8` (back to the triangle loop). Per row: step both edges and
//! the texture/light coordinates (`[ebp-0x40]`/`[ebp-0xe0]` = right/left x in 24.8, `[ebp-0x94]`/`[ebp-0x84]` u,
//! `[ebp-0xbc]`/`[ebp-0xcc]` v, `[ebp-0x70]`/`[ebp-0xa0]` light), span width `(xr >> 8) - (xl >> 8)`; for a span of
//! `n >= 1` pixels three `idiv` give the per-pixel steps, then per pixel
//! `dst[i] = shade[tex[(u/256)*16 + v/256] * 128 + light/256]` (`tex` = `[ebp-0x24]`, 16-bit `shade` = `[obj+0xa4]`,
//! `obj` = `[ebp-0x88]`, C division by 256). Row end: destination row += `edx` (the pitch), screen row
//! `[ebp-0x28]` += `[ebp-0xb0]` with a wrap at `[obj+0x54] - 0x28` / -1 (the landscape buffer is a vertical ring),
//! `[ebp-0x54]` counts the rows of the first triangle half; at 0 `[ebp-0x38]` = 2 / 3 switches one edge's steps.
//!
//! Instructions: 3 (head) + 32 (edges) + either 12 (empty span) or 22 + 44/pixel + 13, + 2 or 10 or 12 (wrap
//! test / wrap), + 4, + 2 (or 3 + 12 / 3 + 2 + 10 / 3 + 2 + 2 at the half switch); exit 3. During a full redraw
//! this loop is ~90 % of the landscape pass (~70 % of all guest instructions while pinching out).
//!
//! The pixels of a row are computed into a buffer first and written at the row's commit; a row whose texel or
//! shade reads overlap its destination span or the stack frame, whose destination overlaps the frame or the two
//! object fields, that could fault (NULL guard), is wider than `MAX_SPAN` or exceeds the budget ends the native run
//! at the row head before it (the interpreter runs it).

use s3_cpu::{layout, Cpu, GuestMemory, EAX, EBP, EBX, ECX, EDI, EDX, ESI};

/// Row head (entry, and stop point after a budget end).
pub const START: u32 = 0x0054_e4bc;
/// Loop exit (`[ebp-0x14] <= 0`), back to the triangle loop.
pub const EXIT: u32 = 0x0054_e6e8;
pub const HASH: u64 = 0x5ad9_73cb_c5ee_4879;

const MAX_SPAN: usize = 2048;

/// Branch coverage for the differential test: 0 empty span, 1 pixel span, 2/3 wrap (row == height-0x28 / -1),
/// 4/5/6 half switch 2 / 3 / other, 7 aliasing/fault/width stop, 8 budget stop, 9 exit, 10 no half switch.
#[cfg(test)]
pub static COV: [std::sync::atomic::AtomicU32; 11] = [const { std::sync::atomic::AtomicU32::new(0) }; 11];

#[inline(always)]
fn cov(_i: usize) {
    #[cfg(test)]
    COV[_i].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

#[inline]
fn ok(a: u32, len: u32) -> bool {
    a >= layout::NULL_GUARD_END && a as u64 + len as u64 <= u32::MAX as u64
}

#[inline]
fn disjoint(a: u32, alen: u32, b: u32, blen: u32) -> bool {
    let (a, b) = (a as u64, b as u64);
    a + alen as u64 <= b || b + blen as u64 <= a
}

/// `cdq; and edx,0xff; add eax,edx; sar eax,8`: signed division by 256 rounding toward zero.
#[inline(always)]
fn div256(x: u32) -> u32 {
    let t = x.wrapping_add(((x as i32) >> 31) as u32 & 0xff);
    ((t as i32) >> 8) as u32
}

/// `cdq; idiv d` quotient for `d >= 1` (no overflow possible).
#[inline(always)]
fn idiv(a: u32, d: u32) -> u32 {
    ((a as i32) / (d as i32)) as u32
}

pub fn land_rows(cpu: &mut Cpu, mem: &mut GuestMemory, budget: u64) -> Option<u64> {
    let ebp = cpu.regs[EBP];
    if ebp < 0x100 || !ok(ebp - 0x100, 0x100) {
        return None;
    }
    let fr = ebp - 0x100; // frame slots used: [ebp-0xfc, ebp-0x14]
    let rd = |mem: &GuestMemory, o: u32| mem.read_u32(ebp.wrapping_sub(o));
    let obj = rd(mem, 0x88);
    let (o54, oa4) = (obj.wrapping_add(0x54), obj.wrapping_add(0xa4));
    if !ok(o54, 4) || !ok(oa4, 4) || !disjoint(o54, 4, fr, 0x100) || !disjoint(oa4, 4, fr, 0x100) {
        return None;
    }
    // Loop invariants: no frame write of the loop touches these slots, and pixel writes never hit the frame or
    // the two object fields (checked per row).
    let tex = rd(mem, 0x24);
    let shade = mem.read_u32(oa4);
    let fc8 = rd(mem, 0xc8);
    let ff8 = rd(mem, 0xf8);
    let [mut eax, mut ecx, mut edx, mut ebx, _esp, _ebp, mut esi, mut edi] = cpu.regs;
    let mut done = 0u64;
    // flags of the last finished step: Some((a, b, dec)) = `cmp a,b` (CF source) then `dec dec`; exit: `test`
    let mut flags: Option<(u32, u32, u32)> = None;
    let mut exited = false;
    let mut buf = [0u16; MAX_SPAN];
    loop {
        let f14 = rd(mem, 0x14);
        if (f14 as i32) <= 0 {
            if done + 3 <= budget {
                eax = f14;
                done += 3;
                exited = true;
                cov(9);
            }
            break;
        }
        // ---- 0x54e4c7: edges and coordinates (frame values, written at the commit)
        let f40 = rd(mem, 0x40).wrapping_add(rd(mem, 0xa8));
        let fe0 = rd(mem, 0xe0).wrapping_add(rd(mem, 0xb8));
        let fbc = rd(mem, 0xbc).wrapping_add(rd(mem, 0xd8));
        let f94 = rd(mem, 0x94).wrapping_add(rd(mem, 0x2c));
        let fcc = rd(mem, 0xcc).wrapping_add(rd(mem, 0xac));
        let f84 = rd(mem, 0x84).wrapping_add(rd(mem, 0x34));
        let f70 = rd(mem, 0x70).wrapping_add(rd(mem, 0xc0));
        let fa0 = rd(mem, 0xa0).wrapping_add(rd(mem, 0xa4));
        let xl = ((fe0 as i32) >> 8) as u32;
        let w = (((f40 as i32) >> 8) as u32).wrapping_sub(xl);
        let mut n = 3 + 32u64;
        // EBX and EDX after the span (the row end overwrites the other registers)
        let (mut r_ebx, mut r_edx) = (fbc, edx);
        let f20_in = rd(mem, 0x20);
        // span (0x54e554 .. 0x54e620): pixels into `buf`, frame values (f18, f1c, fec, ffc, ff0)
        let mut span: Option<(u32, usize, [u32; 5])> = None;
        if (w as i32) >= 1 {
            let cnt = w;
            if cnt as usize > MAX_SPAN {
                cov(7);
                break;
            }
            n += 22 + 44 * cnt as u64 + 13;
            cov(1);
            if done + n > budget {
                cov(8);
                break; // the rest of the row costs more; stop here already
            }
            let fec = idiv(fcc.wrapping_sub(fbc), cnt);
            let ffc = idiv(f84.wrapping_sub(f94), cnt);
            let ff0 = idiv(fa0.wrapping_sub(f70), cnt);
            let d0 = f20_in.wrapping_add(xl.wrapping_mul(2));
            let dlen = 2 * cnt;
            if !ok(d0, dlen)
                || !disjoint(d0, dlen, fr, 0x100)
                || !disjoint(d0, dlen, o54, 4)
                || !disjoint(d0, dlen, oa4, 4)
            {
                cov(7);
                break;
            }
            let (mut u, mut v, mut l) = (f94, fbc, f70);
            let mut bad = false;
            for px in buf.iter_mut().take(cnt as usize) {
                let ta = div256(v).wrapping_add(tex.wrapping_add(div256(u) << 4));
                if !ok(ta, 1) || !disjoint(ta, 1, d0, dlen) || !disjoint(ta, 1, fr, 0x100) {
                    bad = true;
                    break;
                }
                let t = mem.read_u8(ta) as u32;
                let sa = shade.wrapping_add(((t << 7).wrapping_add(div256(l))).wrapping_mul(2));
                if !ok(sa, 2) || !disjoint(sa, 2, d0, dlen) || !disjoint(sa, 2, fr, 0x100) {
                    bad = true;
                    break;
                }
                *px = mem.read_u16(sa);
                v = v.wrapping_add(fec);
                u = u.wrapping_add(ffc);
                l = l.wrapping_add(ff0);
            }
            if bad {
                cov(7);
                break;
            }
            r_ebx = v;
            r_edx = ff8; // 0x54e620
            span = Some((d0, cnt as usize, [u, l, fec, ffc, ff0])); // [ebp-0x18] ends as u, [ebp-0x1c] as l
        } else {
            n += 12;
            cov(0);
        }
        // ---- 0x54e626: row end (the object field is read after the pixel writes; they are disjoint)
        let h = mem.read_u32(o54);
        let mut f20 = f20_in.wrapping_add(r_edx);
        let fb0 = rd(mem, 0xb0);
        let mut f28 = rd(mem, 0x28).wrapping_add(fb0);
        let (mut r_ecx, mut r_esi, mut r_edi) = (fb0, f28, h.wrapping_sub(0x28));
        let (cf_a, cf_b);
        if f28 == h.wrapping_sub(0x28) || f28 == u32::MAX {
            n += if f28 == h.wrapping_sub(0x28) { 10 } else { 2 + 10 };
            cov(if f28 == h.wrapping_sub(0x28) { 2 } else { 3 });
            let a = h.wrapping_sub(0x28);
            let prod = a.wrapping_mul(fc8);
            let e = a.wrapping_mul(fb0);
            r_edi = e;
            r_ecx = f20;
            f28 = f28.wrapping_sub(e);
            r_esi = f28;
            f20 = f20.wrapping_add(0u32.wrapping_sub(prod).wrapping_mul(2));
            (cf_a, cf_b) = (0, prod); // neg eax
        } else {
            n += 2;
            (cf_a, cf_b) = (f28, u32::MAX); // cmp esi,-1
        }
        // 0x54e670
        n += 4;
        let f54 = rd(mem, 0x54).wrapping_sub(1);
        let mut r_eax = f54;
        let f38 = rd(mem, 0x38);
        // half switch writes: (slot, value)
        let mut sw: [(u32, u32); 4] = [(0, 0); 4];
        let mut nsw = 0;
        let fl;
        if f54 != 0 {
            n += 2;
            cov(10);
            fl = (cf_a, cf_b, f14);
        } else {
            n += 3;
            if f38 == 2 {
                n += 12;
                cov(4);
                let (f90, f8c, f3c, f6c) = (rd(mem, 0x90), rd(mem, 0x8c), rd(mem, 0x3c), rd(mem, 0x6c));
                sw = [(0xd8, f90), (0xc0, f3c), (0xb8, f8c), (0x2c, f6c)];
                nsw = 4;
                r_eax = f14.wrapping_sub(1);
                r_ecx = f6c;
                fl = (2, 2, f14);
            } else {
                n += 2;
                if f38 == 3 {
                    n += 10;
                    cov(5);
                    let (f90, f8c, f3c, f6c) = (rd(mem, 0x90), rd(mem, 0x8c), rd(mem, 0x3c), rd(mem, 0x6c));
                    sw = [(0xa8, f8c), (0xac, f90), (0x34, f6c), (0xa4, f3c)];
                    nsw = 4;
                    r_eax = f3c;
                    r_ecx = f6c;
                } else {
                    n += 2;
                    cov(6);
                    r_eax = f38;
                }
                fl = (f38, 3, f14);
            }
        }
        if done + n > budget {
            cov(8);
            break;
        }
        // ---- commit
        let wr = |mem: &mut GuestMemory, o: u32, v: u32| mem.write_u32(ebp.wrapping_sub(o), v);
        wr(mem, 0x40, f40);
        wr(mem, 0xe0, fe0);
        wr(mem, 0xbc, fbc);
        wr(mem, 0x94, f94);
        wr(mem, 0xcc, fcc);
        wr(mem, 0x84, f84);
        wr(mem, 0x70, f70);
        wr(mem, 0xa0, fa0);
        if let Some((d0, cnt, [u, l, fec, ffc, ff0])) = span {
            wr(mem, 0x18, u);
            wr(mem, 0x1c, l);
            wr(mem, 0xec, fec);
            wr(mem, 0xfc, ffc);
            wr(mem, 0xf0, ff0);
            let dst = mem.slice_mut(d0, cnt * 2);
            for (i, &px) in buf[..cnt].iter().enumerate() {
                dst[2 * i..2 * i + 2].copy_from_slice(&px.to_le_bytes());
            }
        }
        wr(mem, 0x20, f20);
        wr(mem, 0x28, f28);
        wr(mem, 0x54, f54);
        for &(o, v) in &sw[..nsw] {
            wr(mem, o, v);
        }
        wr(mem, 0x14, f14.wrapping_sub(1));
        eax = r_eax;
        ebx = r_ebx;
        ecx = r_ecx;
        edx = r_edx;
        esi = r_esi;
        edi = r_edi;
        done += n;
        flags = Some(fl);
    }
    if done == 0 {
        return None;
    }
    cpu.regs[EAX] = eax;
    cpu.regs[ECX] = ecx;
    cpu.regs[EDX] = edx;
    cpu.regs[EBX] = ebx;
    cpu.regs[ESI] = esi;
    cpu.regs[EDI] = edi;
    if exited {
        cpu.alu_logic(eax, 4); // test eax,eax
        cpu.eip = EXIT;
    } else {
        let (a, b, d) = flags?;
        cpu.alu_sub(a, b, 0, 4);
        cpu.alu_dec(d, 4);
        cpu.eip = START;
    }
    Some(done)
}
