//! Guest-address hooks.
//!
//! A hook runs host code when a guest thread's EIP reaches an address (function entry or any instruction,
//! e.g. a call site), *before* that instruction executes. The CPU splits decoded blocks so a hooked address
//! always starts a block; when no hook is installed the cost is one flag test per executed block.
//!
//! In a hook ([`HookCtx`]) you can read/write registers and memory, call guest functions, and then
//! - continue with the original code ([`HookFlow::Continue`]),
//! - skip the original function and return a value ([`HookCtx::return_from_function`], function entry only),
//! - jump elsewhere ([`HookCtx::jump`]),
//! - get a post-call callback when the function returns ([`HookCtx::on_return`], function entry only): the
//!   return address on the stack is replaced by a trampoline thunk (`s3mods.dll!post_call_return`), which runs
//!   the callback with the function's EAX/EDX and then continues at the real return address.

use std::collections::HashMap;
use std::sync::Arc;

use s3_cpu::{HookAddrs, EAX, ESP};
use s3_runtime::{HookFlow, LogLevel, Machine, ThunkResult};

use crate::ModsState;

/// A hook callback.
pub type HookFn = Arc<dyn Fn(&mut HookCtx) -> HookFlow + Send + Sync>;

/// A post-call callback (`EAX`/`EDX` of the returning function are in `ctx.m.cpu().regs`; it may change them).
pub type PostFn = Box<dyn FnOnce(&mut PostCtx) + Send>;

pub(crate) struct HookEntry {
    pub name: String,
    pub f: HookFn,
    pub hits: u64,
}

/// One pending post-call callback (per guest thread, LIFO).
pub(crate) struct PostFrame {
    /// ESP at function entry (pointing at the replaced return address).
    entry_esp: u32,
    ret_addr: u32,
    f: PostFn,
}

#[derive(Default)]
pub(crate) struct HookTable {
    pub by_addr: HashMap<u32, Vec<HookEntry>>,
    /// Hooks registered since the last [`commit`].
    pub dirty: bool,
    /// Pending post-call frames by guest thread id.
    pub post: HashMap<u32, Vec<PostFrame>>,
    /// Address of the `s3mods.dll!post_call_return` thunk (0 until installed).
    pub trampoline: u32,
    /// Host nanoseconds spent in hook callbacks (benchmarking).
    pub ns: u64,
}

/// Context passed to a hook.
pub struct HookCtx<'a> {
    pub m: &'a mut Machine,
    /// The hooked address (= EIP).
    pub eip: u32,
}

impl HookCtx<'_> {
    pub fn reg(&mut self, r: usize) -> u32 {
        self.m.cpu().regs[r]
    }
    pub fn set_reg(&mut self, r: usize, v: u32) {
        self.m.cpu().regs[r] = v;
    }
    pub fn esp(&mut self) -> u32 {
        self.m.cpu().regs[ESP]
    }
    /// `[esp + 4 + 4*i]`: argument `i` at a function entry (ESP points at the return address).
    pub fn arg(&mut self, i: u32) -> u32 {
        let esp = self.esp();
        self.m.mem.read_u32(esp.wrapping_add(4 + 4 * i))
    }
    /// Return address at a function entry.
    pub fn return_address(&mut self) -> u32 {
        let esp = self.esp();
        self.m.mem.read_u32(esp)
    }
    /// Skip the original function (call at its entry): EAX = `eax`, pop the return address and `stdcall_args`
    /// dwords (0 for cdecl/thiscall-without-args), continue at the caller. Return the result from the hook.
    pub fn return_from_function(&mut self, eax: u32, stdcall_args: u32) -> HookFlow {
        let m = &mut *self.m;
        let cpu = &mut m.threads[m.cur].cpu;
        let ret = cpu.pop32(&m.mem);
        cpu.regs[EAX] = eax;
        cpu.regs[ESP] = cpu.regs[ESP].wrapping_add(4 * stdcall_args);
        cpu.eip = ret;
        HookFlow::Redirected
    }
    /// Continue at another address (registers unchanged).
    pub fn jump(&mut self, addr: u32) -> HookFlow {
        self.m.cpu().eip = addr;
        HookFlow::Redirected
    }
    /// Run `f` when the function whose entry this is returns (call at a function entry only).
    pub fn on_return(&mut self, f: impl FnOnce(&mut PostCtx) + Send + 'static) {
        let st = self.m.state::<ModsState>();
        let mut s = st.borrow_mut();
        let tramp = s.hooks.trampoline;
        if tramp == 0 {
            drop(s);
            self.m.log(LogLevel::Warn, "mods: on_return without installed trampoline");
            return;
        }
        let tid = self.m.current_tid();
        let esp = self.m.threads[self.m.cur].cpu.regs[ESP];
        let ret_addr = self.m.mem.read_u32(esp);
        self.m.mem.write_u32(esp, tramp);
        s.hooks.post.entry(tid).or_default().push(PostFrame { entry_esp: esp, ret_addr, f: Box::new(f) });
    }
}

/// Context passed to a post-call callback.
pub struct PostCtx<'a> {
    pub m: &'a mut Machine,
    /// Return value (EAX) of the function; writes go back via [`PostCtx::set_eax`].
    pub eax: u32,
    /// Where the function returns to.
    pub ret_addr: u32,
}

impl PostCtx<'_> {
    pub fn set_eax(&mut self, v: u32) {
        self.eax = v;
        self.m.cpu().regs[EAX] = v;
    }
}

/// Register a hook at `addr`. Takes effect after [`commit`] (done automatically at the end of
/// `install` / patch-set application). Several hooks at one address run in registration order; the first
/// one that returns `Redirected` stops the chain.
pub fn add_hook(m: &mut Machine, addr: u32, name: &str, f: impl Fn(&mut HookCtx) -> HookFlow + Send + Sync + 'static) {
    let st = m.state::<ModsState>();
    let mut s = st.borrow_mut();
    s.hooks.by_addr.entry(addr).or_default().push(HookEntry { name: name.to_string(), f: Arc::new(f), hits: 0 });
    s.hooks.dirty = true;
}

/// Remove all hooks named `name`.
pub fn remove_hooks(m: &mut Machine, name: &str) {
    let st = m.state::<ModsState>();
    let mut s = st.borrow_mut();
    for v in s.hooks.by_addr.values_mut() {
        v.retain(|h| h.name != name);
    }
    s.hooks.by_addr.retain(|_, v| !v.is_empty());
    s.hooks.dirty = true;
}

/// Remove every hook (e.g. after a patch set failed half way).
pub fn remove_hooks_all(m: &mut Machine) {
    let st = m.state::<ModsState>();
    let mut s = st.borrow_mut();
    s.hooks.by_addr.clear();
    s.hooks.dirty = true;
    drop(s);
    st.borrow_mut().safe_points.clear();
}

/// Push the hook address set to every guest CPU (clears decode caches; cheap at load time, avoid per frame).
pub fn commit(m: &mut Machine) {
    let st = m.state::<ModsState>();
    let addrs: Option<HookAddrs> = {
        let mut s = st.borrow_mut();
        if !s.hooks.dirty {
            return;
        }
        s.hooks.dirty = false;
        Some(s.hooks.by_addr.keys().copied().collect())
    };
    m.set_hook_addrs(addrs);
    m.ext.hook_dispatch = Some(dispatch);
}

/// Hook statistics: (address, name, hits).
pub fn hook_stats(m: &mut Machine) -> Vec<(u32, String, u64)> {
    let st = m.state::<ModsState>();
    let s = st.borrow_mut();
    let mut v: Vec<_> =
        s.hooks.by_addr.iter().flat_map(|(a, hs)| hs.iter().map(move |h| (*a, h.name.clone(), h.hits))).collect();
    v.sort();
    v
}

/// Host time spent inside hook callbacks so far (ns).
pub fn hook_time_ns(m: &mut Machine) -> u64 {
    m.state::<ModsState>().borrow_mut().hooks.ns
}

/// `MachineExt::hook_dispatch` implementation.
pub(crate) fn dispatch(m: &mut Machine, eip: u32) -> HookFlow {
    let t0 = std::time::Instant::now();
    let st = m.state::<ModsState>();
    let fs: Vec<HookFn> = {
        let mut s = st.borrow_mut();
        match s.hooks.by_addr.get_mut(&eip) {
            Some(v) => v.iter_mut().map(|h| {
                h.hits += 1;
                h.f.clone()
            }).collect(),
            None => Vec::new(),
        }
    };
    let mut flow = HookFlow::Continue;
    for f in fs {
        let mut ctx = HookCtx { m, eip };
        flow = f(&mut ctx);
        if flow == HookFlow::Redirected {
            break;
        }
    }
    // Deferred host->guest work runs at safe points.
    let safe = st.borrow_mut().safe_points.contains(&eip);
    if safe && flow == HookFlow::Continue {
        crate::run_deferred(m);
    }
    st.borrow_mut().hooks.ns += t0.elapsed().as_nanos() as u64;
    flow
}

/// `s3mods.dll!post_call_return`: a hooked function returned into the trampoline.
pub(crate) fn post_call_return(m: &mut Machine) -> ThunkResult {
    let tid = m.current_tid();
    let esp_now = m.threads[m.cur].cpu.regs[ESP];
    let st = m.state::<ModsState>();
    let frame = {
        let mut s = st.borrow_mut();
        let Some(stack) = s.hooks.post.get_mut(&tid) else {
            return ThunkResult::Error("mods: post_call_return without a pending frame".into());
        };
        // The returning frame is the newest one whose entry ESP lies below the current ESP; newer frames were
        // abandoned (exception unwinding) and are dropped.
        let mut found = None;
        while let Some(f) = stack.last() {
            if f.entry_esp < esp_now {
                found = stack.pop();
            } else {
                break;
            }
        }
        found
    };
    let Some(frame) = frame else {
        return ThunkResult::Error(format!("mods: post_call_return: no frame for esp {esp_now:#x}"));
    };
    let eax = m.cpu().regs[EAX];
    let mut ctx = PostCtx { m, eax, ret_addr: frame.ret_addr };
    (frame.f)(&mut ctx);
    m.cpu().eip = frame.ret_addr;
    ThunkResult::Done
}
