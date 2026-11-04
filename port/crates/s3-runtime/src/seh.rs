//! Structured exception handling (x86 Win32 SEH), enough for MSVC6 `__try/__except/__finally` and C++ `throw`
//! (which goes through `RaiseException`).
//!
//! Dispatch runs *in the guest*, without nesting host frames: for each registration record on the FS:[0]
//! chain the handler is called as `handler(EXCEPTION_RECORD*, frame, CONTEXT*, dispatcher_ctx)` with return
//! address `s3port.dll!seh_handler_return`. That thunk reads the disposition and either resumes from the
//! (possibly modified) CONTEXT, or calls the next handler. MSVC handlers that decide to execute an `__except`
//! block never return (they `RtlUnwind` and jump into the function); stale dispatch records are dropped the
//! next time an exception is raised.

use s3_cpu::{Context, CpuExit, FaultKind, EAX, EBP, EBX, ECX, EDI, EDX, ESI, ESP};

use crate::machine::{teb, StopReason};
use crate::{Machine, ThunkResult};

pub const EXCEPTION_ACCESS_VIOLATION: u32 = 0xC000_0005;
pub const EXCEPTION_BREAKPOINT: u32 = 0x8000_0003;
pub const EXCEPTION_ILLEGAL_INSTRUCTION: u32 = 0xC000_001D;
pub const EXCEPTION_INT_DIVIDE_BY_ZERO: u32 = 0xC000_0094;
pub const EXCEPTION_INT_OVERFLOW: u32 = 0xC000_0095;
pub const EXCEPTION_PRIV_INSTRUCTION: u32 = 0xC000_0096;
pub const STATUS_UNWIND: u32 = 0xC000_0027;
/// MSVC C++ exception code ('msc' | 0xE0000000).
pub const CXX_EXCEPTION: u32 = 0xE06D_7363;

pub const EXCEPTION_NONCONTINUABLE: u32 = 1;
pub const EXCEPTION_UNWINDING: u32 = 2;
pub const EXCEPTION_EXIT_UNWIND: u32 = 4;

/// Size of the x86 `CONTEXT` structure.
pub const CONTEXT_SIZE: u32 = 0x2CC;
/// `CONTEXT_FULL | CONTEXT_FLOATING_POINT | CONTEXT_DEBUG_REGISTERS | CONTEXT_EXTENDED_REGISTERS` (i386).
pub const CONTEXT_ALL: u32 = 0x0001_003F;

/// CONTEXT field offsets.
pub mod ctx {
    pub const FLAGS: u32 = 0x00;
    pub const SEG_GS: u32 = 0x8C;
    pub const SEG_FS: u32 = 0x90;
    pub const SEG_ES: u32 = 0x94;
    pub const SEG_DS: u32 = 0x98;
    pub const EDI: u32 = 0x9C;
    pub const ESI: u32 = 0xA0;
    pub const EBX: u32 = 0xA4;
    pub const EDX: u32 = 0xA8;
    pub const ECX: u32 = 0xAC;
    pub const EAX: u32 = 0xB0;
    pub const EBP: u32 = 0xB4;
    pub const EIP: u32 = 0xB8;
    pub const SEG_CS: u32 = 0xBC;
    pub const EFLAGS: u32 = 0xC0;
    pub const ESP: u32 = 0xC4;
    pub const SEG_SS: u32 = 0xC8;
}

/// One exception dispatch in progress on a thread.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Dispatch {
    rec: u32,
    ctx: u32,
    frame: u32,
    call_esp: u32,
}

/// Write a CONTEXT structure.
pub fn write_context(m: &mut Machine, addr: u32, c: &Context) {
    m.mem.fill(addr, CONTEXT_SIZE as usize, 0);
    let w = |m: &mut Machine, off: u32, v: u32| m.mem.write_u32(addr + off, v);
    w(m, ctx::FLAGS, CONTEXT_ALL);
    w(m, ctx::SEG_GS, 0);
    w(m, ctx::SEG_FS, 0x3B);
    w(m, ctx::SEG_ES, 0x23);
    w(m, ctx::SEG_DS, 0x23);
    w(m, ctx::EDI, c.regs[EDI]);
    w(m, ctx::ESI, c.regs[ESI]);
    w(m, ctx::EBX, c.regs[EBX]);
    w(m, ctx::EDX, c.regs[EDX]);
    w(m, ctx::ECX, c.regs[ECX]);
    w(m, ctx::EAX, c.regs[EAX]);
    w(m, ctx::EBP, c.regs[EBP]);
    w(m, ctx::EIP, c.eip);
    w(m, ctx::SEG_CS, 0x1B);
    w(m, ctx::EFLAGS, c.eflags);
    w(m, ctx::ESP, c.regs[ESP]);
    w(m, ctx::SEG_SS, 0x23);
}

/// Read a CONTEXT structure.
pub fn read_context(m: &Machine, addr: u32) -> Context {
    let r = |off: u32| m.mem.read_u32(addr + off);
    let mut c = Context::default();
    c.regs[EDI] = r(ctx::EDI);
    c.regs[ESI] = r(ctx::ESI);
    c.regs[EBX] = r(ctx::EBX);
    c.regs[EDX] = r(ctx::EDX);
    c.regs[ECX] = r(ctx::ECX);
    c.regs[EAX] = r(ctx::EAX);
    c.regs[EBP] = r(ctx::EBP);
    c.regs[ESP] = r(ctx::ESP);
    c.eip = r(ctx::EIP);
    c.eflags = r(ctx::EFLAGS);
    c
}

/// Turn a CPU exit (fault / int / hlt) into a guest exception. Returns true if guest execution can continue
/// (an exception handler is being called), false if the machine stopped.
pub(crate) fn handle_cpu_exit(m: &mut Machine, exit: CpuExit) -> bool {
    let c = m.cpu().context();
    match exit {
        CpuExit::Fault { eip, kind, detail } => {
            let (code, params): (u32, Vec<u32>) = match kind {
                FaultKind::Unimplemented => {
                    m.stop(StopReason::Fault(format!("unimplemented instruction at {eip:#x}: {detail}")));
                    return false;
                }
                FaultKind::AccessViolation { addr, write } => (EXCEPTION_ACCESS_VIOLATION, vec![write as u32, addr]),
                FaultKind::DivideError => (EXCEPTION_INT_DIVIDE_BY_ZERO, vec![]),
                FaultKind::InvalidOpcode => (EXCEPTION_ILLEGAL_INSTRUCTION, vec![]),
                FaultKind::GeneralProtection => (EXCEPTION_PRIV_INSTRUCTION, vec![]),
            };
            m.log(crate::LogLevel::Warn, &format!("guest exception {code:#x} at {eip:#x} ({detail})"));
            raise_exception(m, code, 0, &params, c, eip)
        }
        CpuExit::Interrupt(3) => {
            let mut c = c;
            c.eip = c.eip.wrapping_sub(1);
            raise_exception(m, EXCEPTION_BREAKPOINT, 0, &[0], c, c.eip)
        }
        CpuExit::Interrupt(n) => {
            m.stop(StopReason::Fault(format!("int {n:#x} at {:#x}", c.eip)));
            false
        }
        CpuExit::Halt => {
            m.stop(StopReason::Fault(format!("hlt at {:#x}", c.eip)));
            false
        }
        other => {
            m.stop(StopReason::Error(format!("unexpected cpu exit {other:?}")));
            false
        }
    }
}

/// Raise a guest exception on the current thread: builds EXCEPTION_RECORD + CONTEXT (from `c`) on the
/// guest stack and starts calling handlers. `address` = ExceptionAddress. Returns false if nobody handles it
/// (the machine is stopped). After a `true` return the CPU is set up to run the first handler; a thunk that
/// calls this must return [`ThunkResult::Done`].
pub fn raise_exception(m: &mut Machine, code: u32, flags: u32, params: &[u32], c: Context, address: u32) -> bool {
    m.stats.exceptions += 1;
    let esp = c.regs[ESP];
    let slot = m.cur;
    // Drop dispatches whose frames were unwound (handler jumped away instead of returning).
    m.threads[slot].seh.retain(|d| d.call_esp > esp);
    if m.threads[slot].seh.len() > 16 {
        m.stop(StopReason::Fault(format!("exception {code:#x} at {address:#x}: nested too deep")));
        return false;
    }
    let ctx_addr = (esp.wrapping_sub(0x100 + CONTEXT_SIZE)) & !0xF;
    let rec_addr = ctx_addr - 0x50;
    let disp_addr = rec_addr - 0x10;
    let call_esp = disp_addr - 0x10;
    write_context(m, ctx_addr, &c);
    m.mem.fill(rec_addr, 0x50, 0);
    m.mem.write_u32(rec_addr, code);
    m.mem.write_u32(rec_addr + 4, flags);
    m.mem.write_u32(rec_addr + 12, address);
    let n = params.len().min(15);
    m.mem.write_u32(rec_addr + 16, n as u32);
    for (i, p) in params.iter().take(n).enumerate() {
        m.mem.write_u32(rec_addr + 20 + 4 * i as u32, *p);
    }
    m.mem.fill(disp_addr, 0x10, 0);
    let teb_addr = m.threads[slot].teb;
    let frame = m.mem.read_u32(teb_addr + teb::EXCEPTION_LIST);
    m.threads[slot].seh.push(Dispatch { rec: rec_addr, ctx: ctx_addr, frame, call_esp });
    call_next_handler(m)
}

fn frame_valid(m: &Machine, frame: u32) -> bool {
    let t = &m.threads[m.cur];
    frame != 0xFFFF_FFFF && frame >= t.stack_bottom && frame < t.stack_top && frame & 3 == 0
}

fn call_next_handler(m: &mut Machine) -> bool {
    let slot = m.cur;
    let Some(d) = m.threads[slot].seh.last().copied() else { return false };
    if !frame_valid(m, d.frame) {
        let code = m.mem.read_u32(d.rec);
        let addr = m.mem.read_u32(d.rec + 12);
        m.threads[slot].seh.pop();
        m.stop(StopReason::Fault(format!("unhandled exception {code:#x} at {addr:#x} (thread {slot})")));
        return false;
    }
    let handler = m.mem.read_u32(d.frame + 4);
    let ret = m.seh_return_thunk;
    let cpu = &mut m.threads[slot].cpu;
    cpu.regs[ESP] = d.call_esp;
    for v in [d.rec - 0x10 /* dispatcher ctx */, d.ctx, d.frame, d.rec] {
        cpu.push32(&mut m.mem, v);
    }
    cpu.push32(&mut m.mem, ret);
    cpu.eip = handler;
    true
}

/// `s3port.dll!seh_handler_return`: an exception handler returned its disposition in EAX.
pub(crate) fn handler_return(m: &mut Machine) -> ThunkResult {
    let slot = m.cur;
    let disposition = m.cpu().regs[EAX];
    let Some(d) = m.threads[slot].seh.last().copied() else {
        return ThunkResult::Error("seh_handler_return without dispatch".into());
    };
    match disposition {
        0 => {
            // ExceptionContinueExecution
            let c = read_context(m, d.ctx);
            m.threads[slot].seh.pop();
            m.cpu().set_context(&c);
        }
        _ => {
            // ExceptionContinueSearch (1) / nested (2) / collided unwind (3): next frame.
            let next = m.mem.read_u32(d.frame);
            if let Some(top) = m.threads[slot].seh.last_mut() {
                top.frame = next;
            }
            call_next_handler(m);
        }
    }
    ThunkResult::Done
}

/// `RtlUnwind(target_frame, target_ip, exception_record, return_value)` core: call every handler between
/// FS:[0] and `target_frame` with the UNWINDING flag, unlinking them. `target_frame == 0` unwinds everything.
pub fn rtl_unwind(m: &mut Machine, target_frame: u32, exc_rec: u32) {
    let slot = m.cur;
    let teb_addr = m.threads[slot].teb;
    let scratch = m.mem.alloc(0x50 + CONTEXT_SIZE + 0x10);
    let rec = if exc_rec != 0 {
        exc_rec
    } else {
        m.mem.write_u32(scratch, STATUS_UNWIND);
        scratch
    };
    let mut flags = m.mem.read_u32(rec + 4) | EXCEPTION_UNWINDING;
    if target_frame == 0 {
        flags |= EXCEPTION_EXIT_UNWIND;
    }
    m.mem.write_u32(rec + 4, flags);
    let c = m.cpu().context();
    let ctx_addr = scratch + 0x50;
    write_context(m, ctx_addr, &c);
    let disp = ctx_addr + CONTEXT_SIZE;
    let mut frame = m.mem.read_u32(teb_addr + teb::EXCEPTION_LIST);
    let mut guard = 0;
    while frame != target_frame && frame_valid(m, frame) && guard < 1000 {
        guard += 1;
        let handler = m.mem.read_u32(frame + 4);
        m.call_guest(handler, &[rec, frame, ctx_addr, disp]);
        if m.is_stopped() {
            break;
        }
        frame = m.mem.read_u32(frame);
        m.mem.write_u32(teb_addr + teb::EXCEPTION_LIST, frame);
    }
    if target_frame != 0 {
        m.mem.write_u32(teb_addr + teb::EXCEPTION_LIST, target_frame);
    }
    m.mem.free(scratch);
}
