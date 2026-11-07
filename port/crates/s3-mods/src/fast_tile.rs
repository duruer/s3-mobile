//! Native fast paths: the tile loops of the frame renderer `0x4b26c0` (draw thread; see `fast.rs` for the contract,
//! docs/porting/performance.md).
//!
//! **`frame_tiles`** (`0x4b2e44..0x4b330e`): the main loop over the snapshot cells (rows `r = [ebp+0xc]` up to
//! `[ebp-0x44]`, columns `i = esi` from `[ebp-0x24]` to `[ebp-0x24] + [ebp-0x7c]`, cell
//! `snap + 0x500 + (r + i*130)*0x30`, `snap = [ebp+8]`, screen x `[ebp-0x18]` += 16 per cell, y `[ebp-0x14]` += 9
//! per row). Per cell the original computes the screen sector `x/40`, adds ambient-sound statistics for the cells of
//! the first 16 sectors (`[ebx+0x5f588/0x5f5cc + 4*(land>>4)]`), checks the 4x4 water-decoration grid, then calls
//! the per-tile object drawer `0x4d3050` (one of three call sites) and, for some cells, more drawers. It was
//! ~95 guest instructions per cell plus ~112 for the object call, ~16 000 cells per frame at 1944x760: 42 % of
//! all guest instructions in game (HUD). Native: cells whose object call is the empty case of `fast_obj` (done
//! inline with `fast_obj::plan`/`apply`), including the sound statistics, and the row advance. A cell that needs
//! anything else (water/shore sound weights `0x4b2f0e`, decoration `0x4b3046`, object type != 0, cell words
//! `+0x50a`/`+0x51e` != 0, `+0x508` kinds 0x800/0x1000/0x1800) ends the native run at the loop head before it.
//!
//! **`land_mark`** (`0x4b277e..0x4b27a7`): the inner loop of the pre-pass over one snapshot column (`eax` = cell
//! `+0x502`, stride 0x1860, `edi` cells): for cells with `(+0x505 & 0x3f)*4 <= 0x48` it sets the landscape byte to
//! 0x1c when bit 2 of `+0x509` is set, else turns 0x1c back into 0x10. 8-13 guest instructions per cell.

use s3_cpu::{layout, Cpu, GuestMemory, EAX, ECX, EDI, EDX, ESI, ESP};

use crate::fast_obj;

/// Loop head of the cell loop (`mov edx,[ebp-0x7c]`, target of the `jmp` at 0x4b32ca).
pub const START: u32 = 0x004b_2e44;
/// End of the code the native path stands for: the cell loop and the row advance (0x4b32cf..0x4b330e).
pub const END: u32 = 0x004b_330e;
pub const HASH: u64 = 0x4486_d940_c590_71e7;
/// Row loop head `0x4b2e30..0x4b2e44` (`cmp ecx,[ebp-0x44]; ja 0x4b330e`, four moves), run between rows; outside
/// the registered region, so its bytes are checked at each entry.
pub const HEAD: (u32, u32) = (0x004b_2e30, 0x004b_2e44);
const HEAD_HASH: u64 = 0x42e6_f725_ccdc_2517;
/// Exit of the row loop (the `ja` at 0x4b2e33 taken).
const ROWS_EXIT: u32 = 0x004b_330e;
/// Return addresses of the two `call 0x4d3050` sites handled natively.
const RET_H: u32 = 0x004b_30fa;
const RET_G: u32 = 0x004b_3207;
/// The instructions of `0x4d3050` that its empty case executes (`fast_obj`; traced, test
/// `obj_empty_path_hashed`), and the FNV-1a 64 hash of their bytes concatenated
/// (`tools/perf/code_hash.py --concat`). Checked at each entry of `frame_tiles`.
pub const OBJ_RANGES: [(u32, u32); 24] = [
    (0x004d_3050, 0x004d_30d8),
    (0x004d_3294, 0x004d_32a6),
    (0x004d_340d, 0x004d_3416),
    (0x004d_352e, 0x004d_3533),
    (0x004d_3595, 0x004d_359e),
    (0x004d_36de, 0x004d_36e7),
    (0x004d_3819, 0x004d_3822),
    (0x004d_3962, 0x004d_3967),
    (0x004d_39c9, 0x004d_39ce),
    (0x004d_3a2f, 0x004d_3a34),
    (0x004d_3a87, 0x004d_3a92),
    (0x004d_3bc0, 0x004d_3bcb),
    (0x004d_4119, 0x004d_4124),
    (0x004d_421e, 0x004d_4229),
    (0x004d_439a, 0x004d_43aa),
    (0x004d_4486, 0x004d_448f),
    (0x004d_456b, 0x004d_4574),
    (0x004d_465f, 0x004d_4668),
    (0x004d_4744, 0x004d_474d),
    (0x004d_4829, 0x004d_483e),
    (0x004d_4aea, 0x004d_4b18),
    (0x004d_52cc, 0x004d_52dd),
    (0x004d_53b2, 0x004d_53bc),
    (0x004d_5a42, 0x004d_5a4f),
];
const OBJ_HASH: u64 = 0x1db8_fad5_ad0a_7098;

pub const MARK_START: u32 = 0x004b_277e;
/// First instruction after the loop (`jne` at 0x4b27a5 not taken).
pub const MARK_EXIT: u32 = 0x004b_27a7;
pub const MARK_HASH: u64 = 0xdfba_d2b1_e6d2_c9d3;

#[inline]
fn ok(a: u32, len: u32) -> bool {
    a >= layout::NULL_GUARD_END && a as u64 + len as u64 <= u32::MAX as u64
}

#[inline]
fn disjoint(a: (u32, u32), b: (u32, u32)) -> bool {
    let (a0, b0) = (a.0 as u64, b.0 as u64);
    a0 + a.1 as u64 <= b0 || b0 + b.1 as u64 <= a0
}

/// Test coverage counters: row advances, row-loop exits, cells with statistics, call site G, call site H.
#[cfg(test)]
pub static COVER: [std::sync::atomic::AtomicU64; 5] = [const { std::sync::atomic::AtomicU64::new(0) }; 5];

#[inline]
fn cover(_i: usize) {
    #[cfg(test)]
    COVER[_i].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

fn ranges_hash(mem: &GuestMemory, ranges: &[(u32, u32)]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &(a, b) in ranges {
        for &x in mem.slice(a, (b - a) as usize) {
            h ^= x as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01B3);
        }
    }
    h
}

/// Cell bytes read by one iteration: `+0x500..+0x520` and the neighbours `+0x592`, `+0x4e22`, `+0x4eb2`.
const CELL_LO: u32 = 0x500;
const CELL_LEN: u32 = 0x4eb3 - 0x500;

/// What one cell iteration does (decided before anything is written).
struct Cell {
    /// Guest instructions from the loop head back to the loop head.
    n: u64,
    cell: u32,
    l: u32,
    /// Sound statistics: (index `land >> 4`, weight 1, weight 2).
    stats: Option<(u32, u32, u32)>,
    /// Call site G (`L < 0x49`): new `[ebp-0x28]` and `[ebp-0x60]`; else call site H.
    g: Option<(u32, u32)>,
    obj: fast_obj::Plan,
}

pub fn frame_tiles(cpu: &mut Cpu, mem: &mut GuestMemory, budget: u64) -> Option<u64> {
    let [mut eax, mut ecx, _, ebx, s, ebp, mut esi, edi] = cpu.regs;
    // Ranges the loop writes: frame [ebp-0x80, ebp+0x10), statistics, stack of the object call.
    let fr = (ebp.wrapping_sub(0x80), 0x90u32);
    let st = (ebx.wrapping_add(0x5f588), 0x84u32);
    let sk = (s.wrapping_sub(0x368), 0x368u32);
    if ebp < 0x1000 || !ok(fr.0, fr.1) || !ok(st.0, st.1) || s < 0x1000 || !ok(sk.0, sk.1) {
        return None;
    }
    if !disjoint(fr, st) || !disjoint(fr, sk) || !disjoint(st, sk) {
        return None;
    }
    let rd = |mem: &GuestMemory, o: i32| mem.read_u32(ebp.wrapping_add(o as u32));
    let snap = rd(mem, 8);
    if edi != snap || ecx != rd(mem, 0xc) {
        return None;
    }
    let hdr = (snap.wrapping_add(0x24), 0x40u32); // reads +0x24, +0x28, +0x60
    if !ok(hdr.0, hdr.1) || !disjoint(hdr, fr) || !disjoint(hdr, st) || !disjoint(hdr, sk) {
        return None;
    }
    let w = rd(mem, -0x7c);
    let rows = rd(mem, -0x44);
    let s24 = mem.read_u32(snap.wrapping_add(0x24));
    let s28 = mem.read_u32(snap.wrapping_add(0x28));
    let e = s - 0x1c; // ESP at the entry of 0x4d3050
    let mut done = 0u64;
    let mut moved = false;
    // The code outside the registered region (row head, the object call) is checked before the first change, so
    // a decline at the first cell (the interpreter runs it) costs no hashing.
    let code_ok = |mem: &GuestMemory| {
        ranges_hash(mem, &[HEAD]) == HEAD_HASH && ranges_hash(mem, &OBJ_RANGES) == OBJ_HASH && fast_obj::setter_ok(mem)
    };
    loop {
        let end = w.wrapping_add(eax);
        if esi > end {
            // ---- row advance: 0x4b2e44 (5), 0x4b32cf (7), odd 0x4b32e2 (9) / even 0x4b32fa (7), 0x4b2e30 (2),
            // 0x4b2e39 (4) unless the row loop ends.
            let r1 = ecx.wrapping_add(1);
            let odd = ecx & 1 == 1;
            let last = r1 > rows;
            let n = 5 + 7 + if odd { 9 } else { 7 } + 2 + if last { 0 } else { 4 };
            if done + n > budget || (!moved && !code_ok(mem)) {
                break;
            }
            mem.write_u32(ebp.wrapping_sub(0x28), end);
            let y = rd(mem, -0x14).wrapping_add(9);
            mem.write_u32(ebp.wrapping_sub(0x14), y);
            let x0 = if odd {
                eax = eax.wrapping_add(1);
                let x0 = rd(mem, -0x1c).wrapping_add(8);
                mem.write_u32(ebp.wrapping_sub(0x24), eax);
                x0
            } else {
                rd(mem, -0x1c).wrapping_sub(8)
            };
            mem.write_u32(ebp.wrapping_sub(0x1c), x0);
            mem.write_u32(ebp.wrapping_add(0xc), r1);
            ecx = r1;
            cpu.alu_sub(r1, rows, 0, 4); // cmp ecx,[ebp-0x44]
            done += n;
            moved = true;
            cpu.regs[EAX] = eax;
            cpu.regs[ECX] = ecx;
            cpu.regs[EDX] = x0;
            cover(0);
            if last {
                cover(1);
                cpu.regs[ESI] = 0;
                cpu.eip = ROWS_EXIT;
                return Some(done);
            }
            esi = eax;
            mem.write_u32(ebp.wrapping_sub(0x18), x0);
            mem.write_u32(ebp.wrapping_sub(0x4c), esi);
            cpu.regs[ESI] = esi;
            continue;
        }
        let Some(c) = plan_cell(mem, ebp, ecx, esi, snap, end, rows, s24, s28, e, (fr, st, sk)) else {
            break;
        };
        if done + c.n > budget || (!moved && !code_ok(mem)) {
            break;
        }
        // ---- commit, in the original order where it matters (no overlaps, checked above)
        let x = rd(mem, -0x18);
        let y = rd(mem, -0x14);
        mem.write_u32(ebp.wrapping_sub(0x28), end);
        if let Some((k, w1, w2)) = c.stats {
            cover(2);
            let a1 = ebx.wrapping_add(0x5f588 + 4 * k);
            mem.write_u32(a1, mem.read_u32(a1).wrapping_add(w1));
            let a2 = ebx.wrapping_add(0x5f5cc + 4 * k);
            mem.write_u32(a2, mem.read_u32(a2).wrapping_add(w2));
        }
        mem.write_u32(ebp.wrapping_sub(0x2c), c.l);
        mem.write_u32(ebp.wrapping_sub(0x20), ecx.wrapping_add(esi.wrapping_mul(130)));
        cover(if c.g.is_some() { 3 } else { 4 });
        let ret = if let Some((v28, v60)) = c.g {
            mem.write_u32(ebp.wrapping_sub(0x28), v28);
            mem.write_u32(ebp.wrapping_sub(0x60), v60);
            RET_G
        } else {
            RET_H
        };
        // push a6=0, y, x, r, i, snap; call
        for (o, v) in [(4u32, 0u32), (8, y), (0xc, x), (0x10, ecx), (0x14, esi), (0x18, snap), (0x1c, ret)] {
            mem.write_u32(s - o, v);
        }
        cpu.regs[ESP] = e;
        cpu.regs[ESI] = esi;
        cpu.regs[EDI] = c.cell;
        fast_obj::apply(cpu, mem, e, snap, &c.obj);
        // 0x4b32b4: next cell
        let x1 = x.wrapping_add(0x10);
        cpu.alu_add(x, 0x10, 0, 4);
        cpu.alu_inc(esi, 4);
        esi = esi.wrapping_add(1);
        mem.write_u32(ebp.wrapping_sub(0x18), x1);
        mem.write_u32(ebp.wrapping_sub(0x4c), esi);
        eax = rd(mem, -0x24);
        ecx = rd(mem, 0xc);
        cpu.regs[EAX] = eax;
        cpu.regs[ECX] = ecx;
        cpu.regs[ESI] = esi;
        cpu.regs[EDI] = snap;
        done += c.n;
        moved = true;
    }
    if !moved {
        return None;
    }
    cpu.eip = START;
    Some(done)
}

/// Decide one cell iteration from the loop head (pure: reads only). `None` = not the native case.
#[allow(clippy::too_many_arguments)]
#[inline]
fn plan_cell(
    mem: &GuestMemory,
    ebp: u32,
    r: u32,
    i: u32,
    snap: u32,
    end: u32,
    rows: u32,
    s24: u32,
    s28: u32,
    e: u32,
    wr: ((u32, u32), (u32, u32), (u32, u32)),
) -> Option<Cell> {
    let idx = r.wrapping_add(i.wrapping_mul(130));
    let cell = snap.wrapping_add(idx.wrapping_mul(48));
    let cr = (cell.wrapping_add(CELL_LO), CELL_LEN);
    if !ok(cr.0, cr.1) || !disjoint(cr, wr.0) || !disjoint(cr, wr.1) || !disjoint(cr, wr.2) {
        return None;
    }
    let b = |o: u32| mem.read_u8(cell.wrapping_add(o)) as u32;
    let x = mem.read_u32(ebp.wrapping_sub(0x18));
    let y = mem.read_u32(ebp.wrapping_sub(0x14));
    // 0x4b2e44 (5), 0x4b2e54 (8): q = x / 40 (the compiler's magic multiply)
    let mut n: u64 = 5 + 8;
    let hi = ((0x6666_6667i64 * x as i32 as i64) >> 32) as i32 >> 4;
    let q = hi.wrapping_add((hi as u32 >> 31) as i32);
    let mut stats = None;
    if q >= 0 {
        n += 2; // 0x4b2e6f
        if q <= 15 {
            n += 12; // 0x4b2e78
            let d = y.wrapping_sub(b(0x500)) as i32;
            if d >= 0 {
                n += 2; // 0x4b2e9b
                if d <= 0x258 {
                    n += 4; // 0x4b2ea3
                    let w1 = if q > 7 {
                        n += 2;
                        (15 - q) as u32
                    } else {
                        8
                    };
                    n += 3; // 0x4b2eb7
                    let w2 = if q < 8 {
                        n += 1;
                        q as u32
                    } else {
                        8
                    };
                    n += 12; // 0x4b2ec2
                    stats = Some((b(0x502) >> 4, w1, w2));
                }
            }
            n += 3; // 0x4b2ef8
            let al = b(0x503);
            if al >= 0xf8 {
                n += 2;
                if al <= 0xfd {
                    return None; // 0x4b2f0e: weighted water/shore statistics
                }
            }
        }
    }
    // 0x4b2f9e (18)
    n += 18;
    let l = (b(0x505) & 0x3f) << 2;
    let mut ea = l; // eax entering 0x4b30b4
    if i <= end.wrapping_sub(3) {
        n += 5; // 0x4b2fd8
        ea = r;
        if r <= rows.wrapping_sub(3) {
            n += 2;
            if b(0x502) < 0x10 {
                n += 2;
                if b(0x4eb2) < 0x10 {
                    n += 2;
                    if b(0x4e22) < 0x10 {
                        n += 2;
                        if b(0x592) < 0x10 {
                            n += 3; // 0x4b301d
                            ea = (r & !0xff) | b(0x503);
                            if b(0x503) == 0 {
                                n += 5; // 0x4b302b
                                ea = snap;
                                if s24.wrapping_add(i) & 3 == 0 {
                                    n += 5; // 0x4b3039
                                    ea = s28;
                                    if s28.wrapping_add(r) & 3 == 0 {
                                        return None; // 0x4b3046: water decoration
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    n += 2; // 0x4b30b4
    let g;
    let post;
    if (l as i32) < 0x49 {
        // 0x4b31ae (7): word +0x508 kinds
        n += 7;
        let v28 = (ea & 0xffff_0000) | mem.read_u16(cell.wrapping_add(0x508)) as u32;
        let v60 = v28 & 0xf800;
        if v60 == 0x1800 {
            return None;
        }
        n += 12; // 0x4b31ed, call
        if v60 == 0x1000 {
            return None;
        }
        if v60 == 0x800 {
            return None;
        }
        post = 3 + 2; // 0x4b3207, 0x4b3244
        g = Some((v28, v60));
    } else {
        n += 4; // 0x4b30be
        if b(0x503) != 0 {
            return None; // object type
        }
        n += 2;
        if b(0x505) & 0x40 != 0 {
            n += 2;
            if b(0x502) & 0xf0 != 0 {
                return None; // type 0xfe
            }
        }
        n += 12; // 0x4b30e1, call
        if mem.read_u16(cell.wrapping_add(0x50a)) != 0 || mem.read_u16(cell.wrapping_add(0x51e)) != 0 {
            return None;
        }
        post = 3 + 3 + 2; // 0x4b30fa, 0x4b3124, 0x4b314e
        g = None;
    }
    let obj = fast_obj::plan(mem, e, snap, i, r, 0)?;
    n += obj.n + post + 9; // + 0x4b32b4
    Some(Cell { n, cell, l, stats, g, obj })
}

pub fn land_mark(cpu: &mut Cpu, mem: &mut GuestMemory, budget: u64) -> Option<u64> {
    let mut a = cpu.regs[EAX];
    let mut edi = cpu.regs[EDI];
    let mut done = 0u64;
    let mut last = None::<(u32, u32, u32)>; // eax before `add`, edi before `dec`, edx
    while done + 13 <= budget {
        if !ok(a, 8) {
            break;
        }
        let edx = (mem.read_u8(a + 3) as u32 & 0x3f) << 2;
        let n = if edx > 0x48 {
            8
        } else if mem.read_u8(a + 7) & 4 != 0 {
            mem.write_u8(a, 0x1c);
            12
        } else if mem.read_u8(a) == 0x1c {
            mem.write_u8(a, 0x10);
            13
        } else {
            12
        };
        done += n;
        last = Some((a, edi, edx));
        a = a.wrapping_add(0x1860);
        edi = edi.wrapping_sub(1);
        if edi == 0 {
            break;
        }
    }
    let (a0, e0, edx) = last?;
    cpu.alu_add(a0, 0x1860, 0, 4);
    cpu.alu_dec(e0, 4);
    cpu.regs[EAX] = a;
    cpu.regs[EDI] = edi;
    cpu.regs[EDX] = edx;
    cpu.eip = if edi != 0 { MARK_START } else { MARK_EXIT };
    Some(done)
}
