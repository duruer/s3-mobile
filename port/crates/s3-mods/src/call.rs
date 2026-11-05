//! Calling guest functions from host code with the usual MSVC conventions. All wrap
//! [`Machine::call_guest`], which runs the call on the current guest thread and restores every register
//! (including ESP) afterwards, so stdcall and cdecl differ only in documentation here.
//!
//! Where to call from: inside a hook (the thread is stopped at a clean instruction boundary) or from a
//! deferred closure ([`crate::defer`]) that runs at a safe point of the main thread. Calling from input or
//! overlay callbacks runs on whatever thread is current and is not recommended.

use s3_cpu::{ECX, EDX};
use s3_runtime::Machine;

/// `__stdcall f(args...)` (callee pops the arguments).
pub fn call_stdcall(m: &mut Machine, addr: u32, args: &[u32]) -> u32 {
    m.call_guest(addr, args)
}

/// `__cdecl f(args...)` (caller pops; `call_guest` restores ESP).
pub fn call_cdecl(m: &mut Machine, addr: u32, args: &[u32]) -> u32 {
    m.call_guest(addr, args)
}

/// `__thiscall this->f(args...)` (MSVC: `this` in ECX, callee pops the arguments).
pub fn call_thiscall(m: &mut Machine, addr: u32, this: u32, args: &[u32]) -> u32 {
    with_regs(m, &[(ECX, this)], |m| m.call_guest(addr, args))
}

/// `__fastcall f(a, b, rest...)` (first two dword arguments in ECX, EDX).
pub fn call_fastcall(m: &mut Machine, addr: u32, args: &[u32]) -> u32 {
    let a = args.first().copied().unwrap_or(0);
    let b = args.get(1).copied().unwrap_or(0);
    let rest = if args.len() > 2 { &args[2..] } else { &[] };
    with_regs(m, &[(ECX, a), (EDX, b)], |m| m.call_guest(addr, rest))
}

/// Call a virtual method: `this->vtbl[index](args...)` with thiscall.
pub fn call_virtual(m: &mut Machine, this: u32, index: u32, args: &[u32]) -> u32 {
    let vtbl = m.mem.read_u32(this);
    let f = m.mem.read_u32(vtbl.wrapping_add(4 * index));
    call_thiscall(m, f, this, args)
}

/// Set registers for the duration of `f`, restoring them afterwards (the interrupted guest code must not
/// see our ECX/EDX).
fn with_regs(m: &mut Machine, regs: &[(usize, u32)], f: impl FnOnce(&mut Machine) -> u32) -> u32 {
    let saved: Vec<(usize, u32)> = regs.iter().map(|&(r, _)| (r, m.cpu().regs[r])).collect();
    for &(r, v) in regs {
        m.cpu().regs[r] = v;
    }
    let ret = f(m);
    for (r, v) in saved {
        m.cpu().regs[r] = v;
    }
    ret
}
