//! x86-32 interpreter for the Settlers III port.
//!
//! - Decoding: `iced-x86` (32-bit mode). Decoded instructions are cached per *basic block* keyed by the start
//!   EIP ([`DecodeCache`]); a block ends at the first control-flow instruction or after
//!   [`MAX_BLOCK_INSNS`] instructions. [`Cpu::invalidate_range`] drops blocks overlapping written code.
//! - Execution: [`Cpu::run`] executes until the instruction budget is exhausted or something needs the host
//!   (EIP enters the thunk page, the magic return address is reached, a fault, `int n`, `hlt`).
//! - Flags are computed eagerly into [`Cpu::eflags`] by the helpers in `flags.rs`.
//! - Memory is flat; only FS/GS segment overrides add a base (`fs_base` = TEB).
//! - Accesses to guest addresses below `layout::NULL_GUARD_END` (64 KiB) raise
//!   `FaultKind::AccessViolation`, like a NULL dereference on Windows.
//! - x87 FPU lives in [`fpu`]; anything not handled by the integer core is offered to `fpu::exec`.

pub mod fpu;

mod exec;
mod fast;
mod flags;

#[cfg(test)]
mod tests;
#[cfg(all(test, target_arch = "x86_64"))]
mod diff_tests;

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use iced_x86::{Code, Decoder, DecoderOptions, FlowControl, Instruction};
pub use s3_mem::{layout, GuestMemory};

pub use fpu::Fpu;

/// Register indices into [`Cpu::regs`] (x86 encoding order).
pub const EAX: usize = 0;
pub const ECX: usize = 1;
pub const EDX: usize = 2;
pub const EBX: usize = 3;
pub const ESP: usize = 4;
pub const EBP: usize = 5;
pub const ESI: usize = 6;
pub const EDI: usize = 7;

/// EFLAGS bits.
pub mod fl {
    pub const CF: u32 = 1 << 0;
    pub const PF: u32 = 1 << 2;
    pub const AF: u32 = 1 << 4;
    pub const ZF: u32 = 1 << 6;
    pub const SF: u32 = 1 << 7;
    pub const TF: u32 = 1 << 8;
    pub const IF: u32 = 1 << 9;
    pub const DF: u32 = 1 << 10;
    pub const OF: u32 = 1 << 11;
    pub const ID: u32 = 1 << 21;
    /// Arithmetic status flags.
    pub const STATUS: u32 = CF | PF | AF | ZF | SF | OF;
}

/// Maximum instructions per decoded block.
pub const MAX_BLOCK_INSNS: usize = 64;

/// Why the CPU faulted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FaultKind {
    /// Undecodable bytes or `ud2`.
    InvalidOpcode,
    /// Valid instruction the interpreter does not implement yet (`detail` names it).
    Unimplemented,
    /// `div`/`idiv` by zero or quotient overflow (#DE).
    DivideError,
    /// Access to an address in the NULL guard (`addr`, `write`).
    AccessViolation { addr: u32, write: bool },
    /// Privileged / unsupported system instruction.
    GeneralProtection,
}

/// Why [`Cpu::run`] returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CpuExit {
    /// EIP is inside the thunk page; the value is EIP (the HLE function's address). ESP points at the
    /// guest return address, arguments follow.
    Thunk(u32),
    /// EIP reached [`layout::MAGIC_RETURN`] (a host->guest call returned).
    ReturnMagic,
    /// The instruction budget ran out.
    Budget,
    /// `hlt` executed.
    Halt,
    /// CPU exception. EIP still points at the faulting instruction.
    Fault { eip: u32, kind: FaultKind, detail: String },
    /// `int n` (`int3` => 3). EIP points after the instruction.
    Interrupt(u8),
    /// EIP reached a registered hook address ([`Cpu::set_hooks`]); nothing at that address has executed yet.
    /// The host runs its hook, then either resumes at the same EIP after [`Cpu::skip_hook_once`] (run the
    /// original code) or redirects EIP itself.
    Hook(u32),
}

/// Guest addresses that stop execution with [`CpuExit::Hook`] (shared by all CPUs of a machine).
///
/// Decoded blocks end *before* any hooked address, so a hooked address is always a block start and the check
/// costs one byte compare per executed block (`Block::hooked`). Changing the set requires clearing the decode
/// caches ([`Cpu::set_hooks`] does that).
pub type HookAddrs = std::collections::HashSet<u32>;

/// Register snapshot (for host->guest calls, thread switches and SEH `CONTEXT`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Context {
    pub regs: [u32; 8],
    pub eip: u32,
    pub eflags: u32,
}

/// A decoded basic block.
#[derive(Debug)]
pub struct Block {
    pub start: u32,
    /// Address after the last instruction.
    pub end: u32,
    pub insns: Vec<Instruction>,
    /// Handler per instruction (threaded dispatch, see `fast.rs`).
    handlers: Vec<fast::Handler>,
    /// `start` is a hook address (see [`HookAddrs`]).
    pub hooked: bool,
}

#[derive(Default)]
struct AddrHasher(u64);

impl Hasher for AddrHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 << 8) | b as u64;
        }
    }
    fn write_u32(&mut self, v: u32) {
        self.0 = (v as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

/// Entries in the direct-mapped first-level lookup (power of two).
const L1_SIZE: usize = 8192;

/// Decoded-block cache keyed by start EIP.
///
/// Blocks live in an arena (`Vec<Block>`); a `HashMap` maps start EIP -> arena index and a direct-mapped
/// array (`l1`, indexed by low EIP bits) short-cuts the common lookups. `Cpu::run` moves the cache out of the
/// `Cpu` while executing, so instructions are borrowed straight from the arena (no refcounting per block).
#[derive(Default)]
pub struct DecodeCache {
    blocks: Vec<Block>,
    map: HashMap<u32, u32, BuildHasherDefault<AddrHasher>>,
    /// `(eip, arena index + 1)`; index 0 = empty. Allocated lazily (so `Default` is allocation-free).
    l1: Vec<(u32, u32)>,
    /// Use only the generic `exec::step` handler (tests / debugging; see `Cpu::set_generic_only`).
    generic_only: bool,
    /// Hook addresses (blocks are split before them and flagged).
    hooks: Option<std::sync::Arc<HookAddrs>>,
}

impl DecodeCache {
    /// Number of cached blocks.
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Drop everything.
    pub fn clear(&mut self) {
        self.blocks.clear();
        self.map.clear();
        self.l1.clear();
    }

    /// Drop blocks overlapping `[start, start+len)`.
    pub fn invalidate_range(&mut self, start: u32, len: u32) {
        let end = start as u64 + len as u64;
        let before = self.blocks.len();
        self.blocks.retain(|b| !((b.start as u64) < end && (b.end as u64) > start as u64));
        if self.blocks.len() != before {
            self.map.clear();
            for (i, b) in self.blocks.iter().enumerate() {
                self.map.insert(b.start, i as u32);
            }
            self.l1.clear();
        }
    }

    /// Arena index of the block starting at `eip`, decoding it if needed.
    #[inline(always)]
    fn lookup(&mut self, eip: u32, mem: &GuestMemory) -> Result<usize, CpuExit> {
        if self.l1.is_empty() {
            self.l1 = vec![(0, 0); L1_SIZE];
        }
        let slot = (eip as usize ^ (eip as usize >> 13)) & (L1_SIZE - 1);
        let (tag, idx) = self.l1[slot];
        if tag == eip && idx != 0 {
            return Ok(idx as usize - 1);
        }
        let idx = match self.map.get(&eip) {
            Some(&i) => i as usize,
            None => {
                let b = decode_block(eip, mem, self.generic_only, self.hooks.as_deref())?;
                self.blocks.push(b);
                let i = self.blocks.len() - 1;
                self.map.insert(eip, i as u32);
                i
            }
        };
        self.l1[slot] = (eip, idx as u32 + 1);
        Ok(idx)
    }

    /// The block starting at `eip`, if it is cached.
    pub fn get(&self, eip: u32) -> Option<&Block> {
        self.map.get(&eip).map(|&i| &self.blocks[i as usize])
    }
}

fn decode_block(eip: u32, mem: &GuestMemory, generic_only: bool, hooks: Option<&HookAddrs>) -> Result<Block, CpuExit> {
    let avail = (layout::THUNK_BASE as u64).saturating_sub(eip as u64).min(16 * MAX_BLOCK_INSNS as u64) as usize;
    let bytes = mem.slice(eip, avail.max(1));
    let mut dec = Decoder::with_ip(32, bytes, eip as u64, DecoderOptions::NONE);
    let mut insns = Vec::with_capacity(8);
    let mut handlers = Vec::with_capacity(8);
    let mut end = eip;
    while dec.can_decode() && insns.len() < MAX_BLOCK_INSNS {
        let ins = dec.decode();
        if ins.code() == Code::INVALID {
            if insns.is_empty() {
                return Err(CpuExit::Fault {
                    eip,
                    kind: FaultKind::InvalidOpcode,
                    detail: format!("bytes {:02x?}", &bytes[..bytes.len().min(8)]),
                });
            }
            break;
        }
        end = ins.next_ip() as u32;
        let flow = ins.flow_control();
        handlers.push(if generic_only { fast::generic_handler() } else { fast::select(&ins) });
        insns.push(ins);
        if flow != FlowControl::Next {
            break;
        }
        // Never let a block run into the thunk page.
        if end >= layout::THUNK_BASE {
            break;
        }
        // A hooked address must start its own block.
        if let Some(h) = hooks {
            if h.contains(&end) {
                break;
            }
        }
    }
    if insns.is_empty() {
        return Err(CpuExit::Fault { eip, kind: FaultKind::InvalidOpcode, detail: "truncated".into() });
    }
    insns.shrink_to_fit();
    handlers.shrink_to_fit();
    let hooked = hooks.is_some_and(|h| h.contains(&eip));
    Ok(Block { start: eip, end, insns, handlers, hooked })
}

/// One guest CPU (one per guest thread).
pub struct Cpu {
    /// General registers, see [`EAX`]..[`EDI`].
    pub regs: [u32; 8],
    pub eip: u32,
    pub eflags: u32,
    /// Linear base of FS (the thread's TEB).
    pub fs_base: u32,
    pub gs_base: u32,
    /// x87 (and MMX) state.
    pub fpu: Fpu,
    /// Instructions retired by this CPU.
    pub instructions: u64,
    /// Decoded-block cache. Threads may share code, but each CPU keeps its own cache (simple, no locking).
    pub cache: DecodeCache,
    /// Address of the instruction currently executing (valid during `run`).
    pub cur_ip: u32,
    /// Hook address whose next block entry must not exit with [`CpuExit::Hook`] (0 = none).
    hook_skip: u32,
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

impl Cpu {
    pub fn new() -> Self {
        Cpu {
            regs: [0; 8],
            eip: 0,
            eflags: 0x202,
            fs_base: 0,
            gs_base: 0,
            fpu: Fpu::new(),
            instructions: 0,
            cache: DecodeCache::default(),
            cur_ip: 0,
            hook_skip: 0,
        }
    }

    #[inline]
    pub fn esp(&self) -> u32 {
        self.regs[ESP]
    }

    /// Snapshot of registers, EIP and EFLAGS.
    pub fn context(&self) -> Context {
        Context { regs: self.regs, eip: self.eip, eflags: self.eflags }
    }

    /// Restore a snapshot.
    pub fn set_context(&mut self, c: &Context) {
        self.regs = c.regs;
        self.eip = c.eip;
        self.eflags = c.eflags;
    }

    /// Push a dword on the guest stack.
    pub fn push32(&mut self, mem: &mut GuestMemory, v: u32) {
        self.regs[ESP] = self.regs[ESP].wrapping_sub(4);
        mem.write_u32(self.regs[ESP], v);
    }

    /// Pop a dword from the guest stack.
    pub fn pop32(&mut self, mem: &GuestMemory) -> u32 {
        let v = mem.read_u32(self.regs[ESP]);
        self.regs[ESP] = self.regs[ESP].wrapping_add(4);
        v
    }

    /// Route every instruction through the generic interpreter instead of the specialised fast handlers
    /// (debugging aid; also used by the tests to cross-check both paths). Clears the decode cache.
    pub fn set_generic_only(&mut self, on: bool) {
        self.cache.clear();
        self.cache.generic_only = on;
    }

    /// Install the hook address set (`None` = no hooks). Clears the decode cache so blocks are re-split.
    pub fn set_hooks(&mut self, hooks: Option<std::sync::Arc<HookAddrs>>) {
        self.cache.clear();
        self.cache.hooks = hooks.filter(|h| !h.is_empty());
        self.hook_skip = 0;
    }

    /// The installed hook address set.
    pub fn hooks(&self) -> Option<&std::sync::Arc<HookAddrs>> {
        self.cache.hooks.as_ref()
    }

    /// After a [`CpuExit::Hook`] at `eip`: execute the original code at `eip` once without exiting again.
    pub fn skip_hook_once(&mut self, eip: u32) {
        self.hook_skip = eip;
    }

    /// Invalidate decoded code in `[start, start+len)` (call after the guest or host writes code).
    pub fn invalidate_range(&mut self, start: u32, len: u32) {
        self.cache.invalidate_range(start, len);
    }

    /// Execute up to `max_instructions` instructions.
    pub fn run(&mut self, mem: &mut GuestMemory, max_instructions: u64) -> CpuExit {
        // Move the cache out so instructions can be borrowed from it while `self` is mutated.
        let mut cache = std::mem::take(&mut self.cache);
        let exit = self.run_with(&mut cache, mem, max_instructions);
        self.cache = cache;
        exit
    }

    fn run_with(&mut self, cache: &mut DecodeCache, mem: &mut GuestMemory, max_instructions: u64) -> CpuExit {
        let mut left = max_instructions;
        loop {
            if self.eip >= layout::THUNK_BASE {
                if self.eip == layout::MAGIC_RETURN {
                    return CpuExit::ReturnMagic;
                }
                return CpuExit::Thunk(self.eip);
            }
            if left == 0 {
                return CpuExit::Budget;
            }
            let idx = match cache.lookup(self.eip, mem) {
                Ok(i) => i,
                Err(e) => return e,
            };
            let blk = &cache.blocks[idx];
            if blk.hooked {
                if self.hook_skip == self.eip {
                    self.hook_skip = 0;
                } else {
                    return CpuExit::Hook(self.eip);
                }
            }
            let n = blk.insns.len().min(left.min(usize::MAX as u64) as usize);
            for (ins, h) in blk.insns[..n].iter().zip(&blk.handlers[..n]) {
                let next = ins.next_ip32();
                self.cur_ip = ins.ip32();
                self.eip = next;
                if let Err(exit) = h(self, mem, ins) {
                    if let CpuExit::Fault { .. } = exit {
                        self.eip = self.cur_ip;
                    } else {
                        // Interrupt/Halt complete the instruction.
                        self.instructions += 1;
                    }
                    return exit;
                }
                self.instructions += 1;
                left -= 1;
                if self.eip != next {
                    break;
                }
            }
        }
    }

    /// Execute exactly one instruction (debugging / tests). Thunk/magic EIPs return their exits.
    pub fn step(&mut self, mem: &mut GuestMemory) -> CpuExit {
        self.run(mem, 1)
    }
}
