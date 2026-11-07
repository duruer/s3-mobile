//! x86-32 interpreter for the Settlers III port.
//!
//! - Decoding: `iced-x86` (32-bit mode). Decoded instructions are cached per *basic block* keyed by the start
//!   EIP ([`DecodeCache`]); a block ends at the first control-flow instruction or after
//!   [`MAX_BLOCK_INSNS`] instructions. [`Cpu::invalidate_range`] drops blocks overlapping written code.
//! - Execution: [`Cpu::run`] executes until the instruction budget is exhausted or something needs the host
//!   (EIP enters the thunk page, the magic return address is reached, a fault, `int n`, `hlt`). Inside a block
//!   the handlers of `fast.rs` tail-call each other (threaded code); see `docs/porting/cpu.md`.
//! - Flags are lazy (`flags.rs`): read them with [`Cpu::eflags`], write with [`Cpu::set_eflags`].
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

/// Native replacement of a guest code region ("fast path"), see [`NativeBlock`].
///
/// Called at the start of the block at the registered address with the remaining instruction budget. It either
/// declines (`None`: nothing changed, the interpreter runs the original code) or runs the region natively and
/// returns `Some(k)`: the number of guest instructions the original code would have retired (`k <= budget`),
/// with registers, EFLAGS, memory and EIP exactly as the original code leaves them at that point. It must stop at
/// a point the original code passes through (e.g. a loop head) and must decline instead of faulting.
pub type NativeFn = fn(&mut Cpu, &mut GuestMemory, u64) -> Option<u64>;

/// A native fast path for the guest code at a fixed address (registered per exe by the host, e.g. s3-mods).
#[derive(Clone, Copy)]
pub struct NativeBlock {
    pub name: &'static str,
    /// Length of the guest code region the fast path replaces (from the start address).
    pub len: u32,
    /// [`code_hash`] of those bytes; the fast path is used only if it matches when the block is decoded (so no
    /// game code needs to be stored in our sources).
    pub hash: u64,
    pub f: NativeFn,
}

/// FNV-1a 64-bit hash of guest code bytes (for [`NativeBlock::hash`]).
pub fn code_hash(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &x in b {
        h ^= x as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}

impl std::fmt::Debug for NativeBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NativeBlock({})", self.name)
    }
}

/// Native fast paths by guest start address.
pub type NativeBlocks = HashMap<u32, NativeBlock>;

/// Register snapshot (for host->guest calls, thread switches and SEH `CONTEXT`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Context {
    pub regs: [u32; 8],
    pub eip: u32,
    pub eflags: u32,
}

/// A decoded basic block.
///
/// `repr(C)` + 64-byte alignment: the fields the run loop reads at every block entry (`succ_eip` ..
/// `special`) share one cache line. Blocks are boxed (stable addresses) so chaining can use pointers.
#[derive(Debug)]
#[repr(C, align(64))]
pub struct Block {
    /// Block chaining: the last two successors seen after this block, start EIP (0 = empty) and block
    /// (pointer into the owning cache's arena). Cleared whenever blocks are dropped (`invalidate_range`).
    succ_eip: [u32; 2],
    succ_blk: [*mut Block; 2],
    /// Times this block was entered (profiling; see [`DecodeCache::profile`]).
    pub entries: u64,
    /// Native fast path for this block's start address (see [`NativeFn`]).
    native: Option<NativeFn>,
    /// `ops.as_ptr()`.
    op0: *const fast::Op,
    /// Address after the last instruction.
    pub end: u32,
    /// Number of instructions (`insns.len()`).
    n: u32,
    /// `hooked || native.is_some()` (one test per block entry on the common path).
    special: bool,
    /// `start` is a hook address (see [`HookAddrs`]).
    pub hooked: bool,
    /// Handler + pre-decoded operands per instruction (threaded dispatch, see `fast.rs`), plus a final
    /// `fast::block_end` sentinel (so `ops.len() == insns.len() + 1`).
    ops: Box<[fast::Op]>,
    pub start: u32,
    pub insns: Vec<Instruction>,
    /// End of the guest code covered by `native` (for invalidation; 0 = none).
    native_end: u32,
}

// SAFETY: the raw pointers (`succ_blk`, `op0`) only point into the cache that owns this block (its arena and its
// own `ops`); a cache and its CPU move between host threads as a whole and are never shared.
unsafe impl Send for Block {}
unsafe impl Sync for Block {}

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
    blocks: Vec<Box<Block>>,
    map: HashMap<u32, u32, BuildHasherDefault<AddrHasher>>,
    /// `(eip, arena index + 1)`; index 0 = empty. Allocated lazily (so `Default` is allocation-free).
    l1: Vec<(u32, u32)>,
    /// Use only the generic `exec::step` handler (tests / debugging; see `Cpu::set_generic_only`).
    generic_only: bool,
    /// Hook addresses (blocks are split before them and flagged).
    hooks: Option<std::sync::Arc<HookAddrs>>,
    /// Native fast paths (looked up when a block is decoded).
    natives: Option<std::sync::Arc<NativeBlocks>>,
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
        self.blocks.retain(|b| !((b.start as u64) < end && (b.end.max(b.native_end) as u64) > start as u64));
        if self.blocks.len() != before {
            self.map.clear();
            for (i, b) in self.blocks.iter_mut().enumerate() {
                self.map.insert(b.start, i as u32);
                b.succ_eip = [0; 2];
                b.succ_blk = [std::ptr::null_mut(); 2];
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
        self.lookup_slow(eip, slot, mem)
    }

    /// [`DecodeCache::lookup`] after an L1 miss: hash map, else decode the block.
    #[inline(never)]
    fn lookup_slow(&mut self, eip: u32, slot: usize, mem: &GuestMemory) -> Result<usize, CpuExit> {
        let idx = match self.map.get(&eip) {
            Some(&i) => i as usize,
            None => {
                let mut b = decode_block(eip, mem, self.generic_only, self.hooks.as_deref())?;
                if let Some(nb) = self.natives.as_ref().and_then(|n| n.get(&eip)) {
                    if code_hash(mem.slice(eip, nb.len as usize)) == nb.hash {
                        b.native = Some(nb.f);
                        b.native_end = eip.wrapping_add(nb.len);
                        b.special = true;
                    }
                }
                self.blocks.push(Box::new(b));
                let i = self.blocks.len() - 1;
                self.map.insert(eip, i as u32);
                i
            }
        };
        self.l1[slot] = (eip, idx as u32 + 1);
        Ok(idx)
    }

    /// Profile: `(start, instruction count, entries)` of every cached block (entries since decode).
    pub fn profile(&self) -> Vec<(u32, u32, u64)> {
        self.blocks.iter().map(|b| (b.start, b.insns.len() as u32, b.entries)).collect()
    }

    /// Disassembly of the `top` blocks with most executed instructions; instructions that use the generic
    /// (slow) handler are marked `*` (profiling aid).
    pub fn profile_disasm(&self, top: usize) -> String {
        use iced_x86::Formatter;
        let mut v: Vec<&Block> = self.blocks.iter().map(|b| &**b).filter(|b| b.entries > 0).collect();
        v.sort_by_key(|b| std::cmp::Reverse(b.entries * b.insns.len() as u64));
        let mut f = iced_x86::IntelFormatter::new();
        let mut out = String::new();
        for b in v.into_iter().take(top) {
            out.push_str(&format!("block {:#x} entries {} len {}\n", b.start, b.entries, b.insns.len()));
            for (ins, op) in b.insns.iter().zip(&b.ops) {
                let mut t = String::new();
                f.format(ins, &mut t);
                let g = if op.h as usize == fast::generic as fast::Handler as usize { '*' } else { ' ' };
                out.push_str(&format!("  {g} {:#x} {t}\n", ins.ip32()));
            }
        }
        out
    }

    /// Executed instruction forms (mnemonic + operand kinds, `*` = generic handler), weighted by block entries.
    pub fn profile_forms(&self) -> Vec<(String, u64)> {
        let mut m: HashMap<String, u64> = HashMap::new();
        for b in self.blocks.iter().filter(|b| b.entries > 0) {
            for (ins, op) in b.insns.iter().zip(&b.ops) {
                *m.entry(form_key(ins, op)).or_default() += b.entries;
            }
        }
        let mut v: Vec<_> = m.into_iter().collect();
        v.sort_by_key(|e| std::cmp::Reverse(e.1));
        v
    }

    /// Adjacent instruction pairs inside blocks (`"form ; form"`, same keys as [`DecodeCache::profile_forms`]),
    /// weighted by block entries: candidates for superinstructions. The first entry is `"#blocks"` with the total
    /// block entries (average executed block length = total instructions / that).
    pub fn profile_pairs(&self) -> Vec<(String, u64)> {
        let mut m: HashMap<String, u64> = HashMap::new();
        let mut blocks = 0u64;
        for b in self.blocks.iter().filter(|b| b.entries > 0) {
            blocks += b.entries;
            let keys: Vec<String> = b.insns.iter().zip(&b.ops).map(|(i, o)| form_key(i, o)).collect();
            for w in keys.windows(2) {
                *m.entry(format!("{} ; {}", w[0], w[1])).or_default() += b.entries;
            }
        }
        let mut v: Vec<_> = m.into_iter().collect();
        v.sort_by_key(|e| std::cmp::Reverse(e.1));
        v.insert(0, ("#blocks".into(), blocks));
        v
    }

    /// Zero all block entry counters.
    pub fn reset_profile(&mut self) {
        for b in &mut self.blocks {
            b.entries = 0;
        }
    }

    /// The block starting at `eip`, if it is cached.
    pub fn get(&self, eip: u32) -> Option<&Block> {
        self.map.get(&eip).map(|&i| &*self.blocks[i as usize])
    }
}

/// Profile key of one instruction form: `*` (generic handler) or ` `, mnemonic, operand kinds.
fn form_key(ins: &Instruction, op: &fast::Op) -> String {
    let g = if op.h as usize == fast::generic as fast::Handler as usize { "*" } else { " " };
    let mut k = format!("{g}{:?}", ins.mnemonic());
    for i in 0..ins.op_count() {
        let kind = ins.op_kind(i);
        let d = match kind {
            iced_x86::OpKind::Register => format!("r{}", ins.op_register(i).size() * 8),
            iced_x86::OpKind::Memory => format!("m{}", ins.memory_size().size() * 8),
            _ => format!("{kind:?}"),
        };
        k.push(' ');
        k.push_str(&d);
    }
    k
}

fn decode_block(eip: u32, mem: &GuestMemory, generic_only: bool, hooks: Option<&HookAddrs>) -> Result<Block, CpuExit> {
    let avail = (layout::THUNK_BASE as u64).saturating_sub(eip as u64).min(16 * MAX_BLOCK_INSNS as u64) as usize;
    let bytes = mem.slice(eip, avail.max(1));
    let mut dec = Decoder::with_ip(32, bytes, eip as u64, DecoderOptions::NONE);
    let mut insns = Vec::with_capacity(8);
    let mut ops = Vec::with_capacity(8);
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
        ops.push(if generic_only { fast::select_generic(&ins) } else { fast::select(&ins) });
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
    for (op, ins) in ops.iter_mut().zip(&insns) {
        if op.h as usize == fast::generic as fast::Handler as usize {
            op.set_ins(ins);
        }
    }
    // Superinstruction: a compare right before the block-ending `jcc` (see `fast::cmp_jcc`).
    let k = insns.len();
    if !generic_only && k >= 2 && fast::is_fast_jcc(&insns[k - 1]) {
        let g = fast::generic as fast::Handler as usize;
        if ops[k - 2].h as usize != g {
            if let Some(h) = fast::fuse_cmp_jcc(&insns[k - 2], insns[k - 1].condition_code()) {
                ops[k - 2].h = h;
            }
        }
    }
    // Sentinel: the handlers dispatch to `op + 1` after each instruction (see `fast::next`).
    let mut sentinel = fast::select_generic(&insns[insns.len() - 1]);
    sentinel.h = fast::block_end;
    ops.push(sentinel);
    let hooked = hooks.is_some_and(|h| h.contains(&eip));
    let n = insns.len() as u32;
    let ops = ops.into_boxed_slice();
    Ok(Block {
        start: eip,
        end,
        n,
        insns,
        op0: ops.as_ptr(),
        ops,
        hooked,
        special: hooked,
        entries: 0,
        native: None,
        native_end: 0,
        succ_eip: [0; 2],
        succ_blk: [std::ptr::null_mut(); 2],
    })
}

/// One guest CPU (one per guest thread).
pub struct Cpu {
    /// General registers, see [`EAX`]..[`EDI`].
    pub regs: [u32; 8],
    pub eip: u32,
    /// EFLAGS storage: all bits when `lk == NONE`, else only the non-status bits are valid. Read with
    /// [`Cpu::eflags`], write with [`Cpu::set_eflags`] (lazy flags, see `flags.rs`).
    pub(crate) ef: u32,
    /// Lazy-flag state: kind (`flags::lk`), operand size, operands, result, saved CF (inc/dec).
    pub(crate) lk: u8,
    pub(crate) lsz: u8,
    pub(crate) la: u32,
    pub(crate) lb: u32,
    pub(crate) lr: u32,
    pub(crate) lcf: u32,
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
    /// Exit produced by a handler that returned `false` (see `fast.rs`).
    pending_exit: Option<CpuExit>,
    /// Address of the `fast::Op` whose handler failed (set together with `pending_exit`).
    fault_op: usize,
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
            ef: 0x202,
            lk: 0,
            lsz: 4,
            la: 0,
            lb: 0,
            lr: 0,
            lcf: 0,
            fs_base: 0,
            gs_base: 0,
            fpu: Fpu::new(),
            instructions: 0,
            cache: DecodeCache::default(),
            cur_ip: 0,
            hook_skip: 0,
            pending_exit: None,
            fault_op: 0,
        }
    }

    #[inline]
    pub fn esp(&self) -> u32 {
        self.regs[ESP]
    }

    /// Snapshot of registers, EIP and EFLAGS.
    pub fn context(&self) -> Context {
        Context { regs: self.regs, eip: self.eip, eflags: self.eflags() }
    }

    /// Restore a snapshot.
    pub fn set_context(&mut self, c: &Context) {
        self.regs = c.regs;
        self.eip = c.eip;
        self.set_eflags(c.eflags);
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

    /// Install native fast paths (`None` = none). Clears the decode cache.
    pub fn set_native_blocks(&mut self, natives: Option<std::sync::Arc<NativeBlocks>>) {
        self.cache.clear();
        self.cache.natives = natives.filter(|n| !n.is_empty());
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
        // Retired instructions are counted through `left` only and stored into `self.instructions` (= base +
        // max_instructions - left) before anything that may read it (native blocks, exits).
        let base = self.instructions;
        let mut left = max_instructions;
        // Previous block (for chaining), null = none.
        let mut prev: *mut Block = std::ptr::null_mut();
        let exit = 'run: loop {
            let eip = self.eip;
            if eip >= layout::THUNK_BASE {
                break if eip == layout::MAGIC_RETURN { CpuExit::ReturnMagic } else { CpuExit::Thunk(eip) };
            }
            if left == 0 {
                break CpuExit::Budget;
            }
            // Block pointers stay valid while this loop runs: blocks are boxed and only appended while the cache
            // is out of the `Cpu` (`invalidate_range`, which drops blocks, also clears every `succ_blk`).
            let bp: *mut Block = 'find: {
                if !prev.is_null() {
                    // SAFETY: see above.
                    let p = unsafe { &mut *prev };
                    if p.succ_eip[0] == eip {
                        break 'find p.succ_blk[0];
                    }
                    if p.succ_eip[1] == eip {
                        break 'find p.succ_blk[1];
                    }
                }
                let i = match cache.lookup(eip, mem) {
                    Ok(i) => i,
                    Err(e) => break 'run e,
                };
                let b: *mut Block = &mut *cache.blocks[i];
                if !prev.is_null() {
                    // SAFETY: see above.
                    let p = unsafe { &mut *prev };
                    p.succ_eip[1] = p.succ_eip[0];
                    p.succ_blk[1] = p.succ_blk[0];
                    p.succ_eip[0] = eip;
                    p.succ_blk[0] = b;
                }
                b
            };
            prev = bp;
            // SAFETY: see above.
            let blk = unsafe { &mut *bp };
            blk.entries += 1;
            let blk = &*blk;
            if blk.special {
                if blk.hooked {
                    if self.hook_skip == eip {
                        self.hook_skip = 0;
                    } else {
                        break CpuExit::Hook(eip);
                    }
                }
                if let Some(f) = blk.native {
                    self.instructions = base + (max_instructions - left);
                    if let Some(k) = f(self, mem, left) {
                        debug_assert!(k <= left);
                        left -= k.min(left);
                        continue;
                    }
                }
            }
            let n = blk.n as u64;
            if n <= left {
                // Threaded code: the first handler runs the whole block (each handler tail-calls the next one,
                // see `fast::next`). Inside a block only the last instruction can change EIP (blocks end at the
                // first control-flow instruction) and no other instruction reads it, so EIP is set once to the
                // last instruction's next address before the block runs.
                self.eip = blk.end;
                // SAFETY: `ops` holds at least one instruction plus the sentinel.
                let op0 = unsafe { &*blk.op0 };
                if (op0.h)(self, mem, op0) {
                    left -= n;
                    continue;
                }
                let i = (self.fault_op - blk.ops.as_ptr() as usize) / std::mem::size_of::<fast::Op>();
                self.instructions = base + (max_instructions - left);
                return self.block_exit(blk, i);
            }
            // The budget ends inside this block.
            self.instructions = base + (max_instructions - left);
            if let Some(e) = self.run_partial(blk, mem, left as usize) {
                return e;
            }
            left = 0;
        };
        self.instructions = base + (max_instructions - left);
        exit
    }

    /// Execute the first `n` (< `blk.n`) instructions of `blk` one by one with the generic interpreter (same
    /// results as the fast handlers by their contract), so the CPU stops at exactly the same instruction as a
    /// complete block run would pass. `Some(exit)` if one of them failed (`self.instructions` updated).
    #[cold]
    #[inline(never)]
    fn run_partial(&mut self, blk: &Block, mem: &mut GuestMemory, n: usize) -> Option<CpuExit> {
        for (i, ins) in blk.insns[..n].iter().enumerate() {
            self.eip = ins.next_ip32();
            self.cur_ip = ins.ip32();
            if let Err(e) = exec::step(self, mem, ins) {
                self.pending_exit = Some(e);
                return Some(self.block_exit(blk, i));
            }
        }
        None
    }

    /// Exit after instruction `i` of `blk` failed (its exit is in `pending_exit`): a fault rewinds EIP to the
    /// faulting instruction (as Windows reports it); `Interrupt`/`Halt` complete the instruction.
    #[cold]
    #[inline(never)]
    fn block_exit(&mut self, blk: &Block, i: usize) -> CpuExit {
        let mut exit = self.pending_exit.take().unwrap_or(CpuExit::Halt);
        let ins = &blk.insns[i];
        if let CpuExit::Fault { eip, .. } = &mut exit {
            let ip = ins.ip32();
            *eip = ip;
            self.eip = ip;
            self.cur_ip = ip;
            self.instructions += i as u64;
        } else {
            // EIP after the instruction, unless it is a control transfer that set it already (e.g. `int n` via
            // the generic path leaves EIP after the instruction too).
            if ins.flow_control() == FlowControl::Next {
                self.eip = ins.next_ip32();
            }
            self.instructions += i as u64 + 1;
        }
        exit
    }

    /// Execute exactly one instruction (debugging / tests). Thunk/magic EIPs return their exits.
    pub fn step(&mut self, mem: &mut GuestMemory) -> CpuExit {
        self.run(mem, 1)
    }
}
