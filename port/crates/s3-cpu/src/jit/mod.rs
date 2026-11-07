//! Optional AArch64 JIT for hot decoded blocks (perf-jit, 2026-11-07). See `docs/porting/cpu.md` "JIT".
//!
//! Built only with the `jit` cargo feature on `target_arch = "aarch64"`; switched on at run time with env
//! `S3_JIT=1` or [`crate::set_jit_enabled`] (default off). iOS builds leave the feature out.
//!
//! Model (everything here exists to keep results bit-identical to the interpreter, see determinism.md):
//! - A compiled block is a translation of one decoded [`Block`]: same instructions, same retired count. The
//!   run loop enters JIT code only when the remaining budget covers the block; every compiled block re-checks
//!   `left >= n` on entry and otherwise exits to the run loop (which runs the partial block through the
//!   interpreter), so budget stops / thread switches happen at the same instruction counts.
//! - Guest EAX..EDI live in x19..x26 while JIT code runs (x27 = `Cpu`, x28 = guest memory base, x13 = remaining
//!   budget). The trampoline loads them from `Cpu::regs` and stores them back on every exit.
//! - Flags: between blocks they are in the `Cpu` in the interpreter's lazy format (`lk/lsz/la/lb/lr/lcf`, or
//!   `ef` with `lk = NONE`). Inside a block the compiler tracks the last flag writer at compile time and keeps
//!   its operands in x9/x10/x11 (+ x12 = saved CF of inc/dec); conditions are evaluated with native compares.
//! - Faults: every guest memory access checks `addr < 0x10000` first; if it fails, the JIT code exits *before*
//!   the instruction ("side exit" at index i) and the run loop continues the block from op i with the
//!   interpreter's handlers, so faults (and their partial effects) are exactly the interpreter's.
//! - Instructions without a native translation call the interpreter's handler for that one instruction
//!   (`jit_op`, registers and flags spilled to the `Cpu` around the call).
//! - Blocks link to static successors through per-EIP slots (an indirect `br` through a slot that holds either
//!   the successor's code or the exit stub) and to indirect targets through a direct-mapped (eip -> code)
//!   table. Hooked / native blocks are never compiled, so reaching one always exits to the run loop.
//! - `invalidate_range` resets all links (slots, table) and re-registers the surviving blocks; `clear` drops all
//!   code. Code memory is W^X (`execmem.rs`).

pub(crate) mod a64;
mod compile;
mod execmem;

use std::collections::HashMap;
use std::mem::offset_of;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::fast::Op;
use crate::{Block, Cpu};
use a64::Asm;
use execmem::ExecMem;

/// Block entries (through the run loop) before a block is compiled.
pub(crate) const THRESHOLD: u64 = 8;

/// Code arena size per decode cache (per guest thread).
const ARENA: usize = 16 << 20;

/// Entries of the indirect-branch table (power of two).
pub(crate) const JTAB: usize = 4096;

/// Exit codes of the trampoline.
pub(crate) const EXIT_EIP: u32 = 0;
pub(crate) const EXIT_SIDE: u32 = 1;
pub(crate) const EXIT_FAIL: u32 = 2;

/// Trampoline frame: x29/x30 + x19..x28 + one save slot for x13 around helper calls.
pub(crate) const FRAME: i32 = 112;
pub(crate) const SAVE_LEFT: u32 = 96;

// `Cpu` field offsets used by the generated code (`Cpu` is `repr(C)`).
pub(crate) const OFF_REGS: u32 = offset_of!(Cpu, regs) as u32;
pub(crate) const OFF_EIP: u32 = offset_of!(Cpu, eip) as u32;
pub(crate) const OFF_EF: u32 = offset_of!(Cpu, ef) as u32;
pub(crate) const OFF_LA: u32 = offset_of!(Cpu, la) as u32;
pub(crate) const OFF_LB: u32 = offset_of!(Cpu, lb) as u32;
pub(crate) const OFF_LR: u32 = offset_of!(Cpu, lr) as u32;
pub(crate) const OFF_LCF: u32 = offset_of!(Cpu, lcf) as u32;
pub(crate) const OFF_LK: u32 = offset_of!(Cpu, lk) as u32;
pub(crate) const OFF_LSZ: u32 = offset_of!(Cpu, lsz) as u32;
pub(crate) const OFF_LEFT: u32 = offset_of!(Cpu, jit_left) as u32;
pub(crate) const OFF_BLK: u32 = offset_of!(Cpu, jit_blk) as u32;
pub(crate) const OFF_IDX: u32 = offset_of!(Cpu, jit_idx) as u32;

const _: () = {
    assert!(OFF_REGS == 0);
    assert!(OFF_LB == OFF_LA + 4);
    assert!(OFF_LSZ == OFF_LK + 1 && OFF_LK % 2 == 0);
    assert!(OFF_LEFT % 8 == 0 && OFF_BLK % 8 == 0 && OFF_IDX % 4 == 0);
    assert!(OFF_IDX < 1024 && OFF_LEFT < 1024 && OFF_BLK < 1024);
};

// ------------------------------------------------------------------------------------------------- switch

/// 0 = not decided yet (read `S3_JIT`), 1 = off, 2 = on.
static MODE: AtomicU8 = AtomicU8::new(0);

pub(crate) fn enabled() -> bool {
    match MODE.load(Ordering::Relaxed) {
        2 => true,
        1 => false,
        _ => {
            let on = std::env::var("S3_JIT").map(|v| v.trim() == "1").unwrap_or(false);
            MODE.store(if on { 2 } else { 1 }, Ordering::Relaxed);
            on
        }
    }
}

pub(crate) fn set_enabled(on: bool) {
    MODE.store(if on { 2 } else { 1 }, Ordering::Relaxed);
}

// ------------------------------------------------------------------------------------------------- helpers

/// Called by JIT code for an instruction without a native translation: run the interpreter handler of one
/// instruction (`op` points at `[op, block_end]`, see [`Jit::fallback_op`]). Returns the handler's result.
pub(crate) unsafe extern "C" fn jit_op(cpu: *mut Cpu, op: *const Op) -> u32 {
    // SAFETY: called from JIT code with the running CPU (registers and flags spilled to it) and an op pair
    // owned by the JIT; `jit_mem` was set by `Cpu::run_jit` for this run.
    unsafe {
        let cpu = &mut *cpu;
        let mem = &mut *(cpu.jit_mem as *mut crate::GuestMemory);
        ((*op).h)(cpu, mem, &*op) as u32
    }
}

/// Condition `cc` (iced `ConditionCode as u8`) from the flags stored in the `Cpu`.
pub(crate) unsafe extern "C" fn jit_cond(cpu: *const Cpu, cc: u32) -> u32 {
    // SAFETY: called from JIT code with the running CPU.
    let f = unsafe { (*cpu).eflags() };
    crate::fast::cc_eval_rt(f, cc as u8) as u32
}

// ------------------------------------------------------------------------------------------------- state

/// One entry of the indirect-branch table.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct JEntry {
    pub eip: u32,
    _pad: u32,
    pub code: u64,
}

const EMPTY: JEntry = JEntry { eip: u32::MAX, _pad: 0, code: 0 };

/// Per-decode-cache JIT state.
pub(crate) struct Jit {
    mem: ExecMem,
    /// Bytes used in `mem`.
    used: usize,
    /// Size of the fixed part (trampoline + exit stubs) at the start of `mem`.
    fixed: usize,
    pub(crate) enter: usize,
    pub(crate) exit_eip: usize,
    pub(crate) exit_common: usize,
    /// Static-successor slots: start EIP -> slot (holds the target's code address or `exit_eip`).
    slots: HashMap<u32, *mut u64>,
    slot_chunks: Vec<Box<[u64; 1024]>>,
    slot_used: usize,
    pub(crate) jtab: Box<[JEntry; JTAB]>,
    /// `[op, block_end]` pairs for fallback calls (stable addresses).
    fallback: Vec<Box<[Op; 2]>>,
    /// Literal pool of the last compile (debugging).
    pub(crate) compiled: u64,
}

// SAFETY: the raw slot pointers point into `slot_chunks`, owned by this struct; the JIT moves with its CPU.
unsafe impl Send for Jit {}

/// Index of `eip` in the indirect-branch table (the generated lookup computes the same).
#[inline]
pub(crate) fn jtab_index(eip: u32) -> usize {
    ((eip ^ (eip >> 12)) as usize) & (JTAB - 1)
}

impl Jit {
    pub(crate) fn new() -> Option<Box<Jit>> {
        let mem = ExecMem::new(ARENA)?;
        let mut j = Box::new(Jit {
            mem,
            used: 0,
            fixed: 0,
            enter: 0,
            exit_eip: 0,
            exit_common: 0,
            slots: HashMap::new(),
            slot_chunks: Vec::new(),
            slot_used: 0,
            jtab: Box::new([EMPTY; JTAB]),
            fallback: Vec::new(),
            compiled: 0,
        });
        j.emit_fixed();
        Some(j)
    }

    /// Trampoline and the shared exit stubs.
    fn emit_fixed(&mut self) {
        use a64::{SP, ZR};
        let base = self.mem.ptr() as usize;
        let mut a = Asm::new_at(base);
        // enter(x0 = cpu, x1 = guest memory base, x2 = code)
        a.stp_x_pre(29, 30, SP, -FRAME);
        a.add_xi(29, SP, 0);
        a.stp_x(19, 20, SP, 16);
        a.stp_x(21, 22, SP, 32);
        a.stp_x(23, 24, SP, 48);
        a.stp_x(25, 26, SP, 64);
        a.stp_x(27, 28, SP, 80);
        a.mov_x(27, 0);
        a.mov_x(28, 1);
        for k in 0..4 {
            a.ldp_w(19 + 2 * k, 20 + 2 * k, 27, (OFF_REGS + 8 * k) as i32);
        }
        a.ldr_x(13, 27, OFF_LEFT);
        a.br(2);
        // exit_eip (w0 = next EIP)
        let exit_eip = base + a.pos() * 4;
        a.str_w(0, 27, OFF_EIP);
        a.movz_w(0, EXIT_EIP, 0);
        // exit_common (w0 = exit code)
        let exit_common = base + a.pos() * 4;
        for k in 0..4 {
            a.stp_w(19 + 2 * k, 20 + 2 * k, 27, (OFF_REGS + 8 * k) as i32);
        }
        a.str_x(13, 27, OFF_LEFT);
        a.ldp_x(19, 20, SP, 16);
        a.ldp_x(21, 22, SP, 32);
        a.ldp_x(23, 24, SP, 48);
        a.ldp_x(25, 26, SP, 64);
        a.ldp_x(27, 28, SP, 80);
        a.ldp_x_post(29, 30, SP, FRAME);
        a.ret();
        let _ = ZR;
        a.finish();
        let bytes = words_to_bytes(&a.code);
        self.mem.write(0, &bytes);
        self.enter = base;
        self.exit_eip = exit_eip;
        self.exit_common = exit_common;
        self.used = (bytes.len() + 63) & !63;
        self.fixed = self.used;
    }

    /// Drop all code and links (the decode cache was cleared or the arena is full).
    pub(crate) fn reset(&mut self) {
        self.used = self.fixed;
        self.slots.clear();
        self.slot_used = 0;
        self.jtab.fill(EMPTY);
        self.fallback.clear();
    }

    /// Unlink everything (blocks were dropped); the caller re-registers the surviving compiled blocks.
    pub(crate) fn unlink_all(&mut self) {
        let e = self.exit_eip as u64;
        for &s in self.slots.values() {
            // SAFETY: slots point into our chunks.
            unsafe { *s = e };
        }
        self.jtab.fill(EMPTY);
    }

    /// Make the compiled code of the block at `eip` reachable from other JIT blocks.
    pub(crate) fn register(&mut self, eip: u32, code: usize) {
        let s = self.slot(eip);
        // SAFETY: slot points into our chunks.
        unsafe { *s = code as u64 };
        self.jtab[jtab_index(eip)] = JEntry { eip, _pad: 0, code: code as u64 };
    }

    /// Slot for static links to `eip` (created on first use, initially the exit stub).
    pub(crate) fn slot(&mut self, eip: u32) -> *mut u64 {
        if let Some(&s) = self.slots.get(&eip) {
            return s;
        }
        if self.slot_used % 1024 == 0 && self.slot_used / 1024 == self.slot_chunks.len() {
            self.slot_chunks.push(Box::new([0u64; 1024]));
        }
        let c = &mut self.slot_chunks[self.slot_used / 1024];
        let s: *mut u64 = &mut c[self.slot_used % 1024];
        self.slot_used += 1;
        // SAFETY: just allocated in a boxed chunk (stable address).
        unsafe { *s = self.exit_eip as u64 };
        self.slots.insert(eip, s);
        s
    }

    /// `[op, block_end]` for a fallback call to the interpreter handler of `ins` (unfused, see `jit_op`).
    pub(crate) fn fallback_op(&mut self, ins: &iced_x86::Instruction) -> usize {
        let mut op = crate::fast::select(ins);
        if op.h as usize == crate::fast::generic as crate::fast::Handler as usize {
            op.set_ins(ins);
        }
        let mut end = crate::fast::select_generic(ins);
        end.h = crate::fast::block_end;
        let b = Box::new([op, end]);
        let p = b.as_ptr() as usize;
        self.fallback.push(b);
        p
    }

    /// Compile `blk`; returns its code address, or `None` if the arena is full (caller resets and retries).
    pub(crate) fn compile(&mut self, blk: &Block) -> Option<usize> {
        let base = self.mem.ptr() as usize + self.used;
        let code = compile::compile(self, blk, base);
        let bytes = words_to_bytes(&code);
        if self.used + bytes.len() > self.mem.len() {
            return None;
        }
        self.mem.write(self.used, &bytes);
        self.used = (self.used + bytes.len() + 15) & !15;
        self.compiled += 1;
        if std::env::var_os("S3_JIT_DUMP").is_some() {
            dump(blk, base, &code);
        }
        Some(base)
    }
}

fn words_to_bytes(w: &[u32]) -> Vec<u8> {
    let mut b = Vec::with_capacity(w.len() * 4);
    for x in w {
        b.extend_from_slice(&x.to_le_bytes());
    }
    b
}

/// Debug aid (`S3_JIT_DUMP=1`): append the guest block and its code words as `.inst` lines to `s3jit.s`
/// (assemble + disassemble with clang/llvm-objdump to read it).
fn dump(blk: &Block, base: usize, code: &[u32]) {
    use iced_x86::Formatter;
    use std::io::Write;
    let mut f = iced_x86::IntelFormatter::new();
    let mut s = format!("// block {:#x} ({} insns) at {base:#x}\n", blk.start, blk.n);
    for ins in &blk.insns {
        let mut t = String::new();
        f.format(ins, &mut t);
        s.push_str(&format!("//   {:#x} {t}\n", ins.ip32()));
    }
    for w in code {
        s.push_str(&format!(".inst {w:#010x}\n"));
    }
    if let Ok(mut o) = std::fs::OpenOptions::new().create(true).append(true).open("s3jit.s") {
        let _ = o.write_all(s.as_bytes());
    }
}

/// Signature of the trampoline.
pub(crate) type EnterFn = unsafe extern "C" fn(*mut Cpu, *mut u8, usize) -> u32;
