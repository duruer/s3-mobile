//! Native fast path: the per-tile object drawer `0x4d3050` (draw thread, `thiscall`, 6 stack arguments, `ret 0x18`)
//! for the common case "nothing to draw" (see `fast.rs` for the contract, docs/porting/performance.md).
//!
//! The frame renderer `0x4b26c0` calls it once per snapshot tile and frame (~10 000 times per frame at 1536x600);
//! it was 17-22 % of all guest instructions in game. Arguments: `a1` = snapshot, `a2`/`a3` = cell column/row
//! (cell `a1 + (a3 + a2*130)*0x30`), `a6` = object type of the tile. The native path handles only `a6 == 0` with
//! cell byte `+0x502` (landscape type) != 0x50 (for types 2..7 only when word `+0x50a` != 0 or
//! `(byte +0x505 & 0x3f)*4 <= 0x48`), and `[a1+0x60]` not 1 or 4: then the original runs its prologue (four
//! `0x5fc540` setter calls on locals), a chain of type compares and returns without drawing (110-116 guest
//! instructions). Everything else (and a budget too small for the whole call) declines,
//! so the original code runs.

use s3_cpu::{code_hash, layout, Cpu, GuestMemory, EAX, ECX, EDX, ESP};

pub const START: u32 = 0x004d_3050;
/// End of the function's code (`ret 0x18` at 0x4d5a4c is its last instruction).
pub const END: u32 = 0x004d_5a4f;
pub const HASH: u64 = 0x57ce_2335_d031_1e1d;
/// The 4-instruction setter `mov eax,ecx; mov ecx,[esp+4]; mov [eax],ecx; ret 4` it calls four times.
const SETTER: u32 = 0x005f_c540;
const SETTER_LEN: u32 = 11;
const SETTER_HASH: u64 = 0xbd7e_00ef_0d5a_a4ab;

#[inline]
fn ok(a: u32, len: u32) -> bool {
    a >= layout::NULL_GUARD_END && a as u64 + len as u64 <= u32::MAX as u64
}

#[inline]
fn disjoint(a: u32, alen: u32, b: u32, blen: u32) -> bool {
    let (a, b) = (a as u64, b as u64);
    a + alen as u64 <= b || b + blen as u64 <= a
}

pub fn obj_empty(cpu: &mut Cpu, mem: &mut GuestMemory, budget: u64) -> Option<u64> {
    let e = cpu.regs[ESP];
    // stack: writes [e-0x34c, e), reads [e, e+0x1c)
    if e < 0x1000 || !ok(e - 0x34c, 0x34c + 0x1c) {
        return None;
    }
    if mem.read_u32(e.wrapping_add(0x18)) != 0 {
        return None; // a6: object type
    }
    let a1 = mem.read_u32(e.wrapping_add(4));
    let a2 = mem.read_u32(e.wrapping_add(8));
    let a3 = mem.read_u32(e.wrapping_add(0xc));
    let cell = a1.wrapping_add(a3.wrapping_add(a2.wrapping_mul(130)).wrapping_mul(48));
    if !ok(cell.wrapping_add(0x502), 0xa) || !ok(a1.wrapping_add(0x60), 4) {
        return None;
    }
    let lo = e - 0x34c;
    if !disjoint(lo, 0x34c, cell.wrapping_add(0x502), 0xa) || !disjoint(lo, 0x34c, a1.wrapping_add(0x60), 4) {
        return None;
    }
    let land = mem.read_u8(cell.wrapping_add(0x502));
    if land == 0x50 {
        return None;
    }
    let b505 = mem.read_u8(cell.wrapping_add(0x505)) as u32;
    let n: u64 = match land {
        0 | 1 => 110,
        // 0x4d4af8: land 2..7 -> 0x4d4b00 `cmp word [cell+0x50a],0; jne` -> 0x4d4b0d `cmp [esp+0x20],0x48; jle`
        2..=7 if mem.read_u16(cell.wrapping_add(0x50a)) != 0 => 114,
        2..=7 if ((b505 & 0x3f) << 2) <= 0x48 => 116,
        2..=7 => return None, // draws decorations (0x4d4b18..)
        _ => 112,
    };
    let mode = mem.read_u32(a1.wrapping_add(0x60));
    if mode == 4 || mode == 1 {
        return None;
    }
    if n > budget {
        return None;
    }
    if code_hash(mem.slice(SETTER, SETTER_LEN as usize)) != SETTER_HASH {
        return None;
    }
    let x = e - 0x344; // esp after the prologue pushes
    let [_, _, _, ebx, _, ebp, esi, edi] = cpu.regs;
    // prologue
    mem.write_u32(e - 0x338, ebx);
    mem.write_u32(e - 0x33c, ebp);
    mem.write_u32(e - 0x340, esi);
    mem.write_u32(e - 0x344, edi);
    // four setter calls: push 0; call (return address); [local] = 0
    for (ret, local) in [(0x004d_306cu32, 0x2cu32), (0x004d_3076, 0x38), (0x004d_3080, 0x18), (0x004d_308a, 0x24)] {
        mem.write_u32(x - 4, 0);
        mem.write_u32(x - 8, ret);
        if ret == 0x004d_306c {
            mem.write_u32(x + 0x1c, 0); // mov [esp+0x20],ebx (before the first call)
        }
        mem.write_u32(x + local, 0);
    }
    mem.write_u32(x + 0x20, (b505 & 0x3f) << 2);
    let ret_addr = mem.read_u32(e);
    cpu.regs[EAX] = land as u32;
    cpu.regs[ECX] = a1;
    cpu.regs[EDX] = a1;
    // ebx, ebp, esi, edi are restored by the epilogue (unchanged)
    cpu.alu_add(e - 0x334, 0x334, 0, 4); // add esp,0x334 (after the last cmp)
    cpu.regs[ESP] = e.wrapping_add(4 + 0x18);
    cpu.eip = ret_addr;
    Some(n)
}
