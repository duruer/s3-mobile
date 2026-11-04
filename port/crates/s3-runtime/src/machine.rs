//! The machine: guest memory, guest threads (each with its own [`Cpu`]), the cooperative scheduler, thunk
//! dispatch, host->guest calls.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use s3_cpu::{Context, Cpu, CpuExit, EAX, EDX, ESP};
use s3_mem::layout;
use s3_mem::{align_up, GuestMemory, RegionKind};

use crate::com::ComRegistry;
use crate::handles::{HandleTable, KernelObject};
use crate::host::{Host, LogLevel};
use crate::input::InputState;
use crate::registry::{Registry, ThunkResult};
use crate::vfs::Vfs;

/// Default instructions per scheduling quantum.
pub const DEFAULT_QUANTUM: u64 = 200_000;

/// TEB field offsets (x86).
pub mod teb {
    pub const EXCEPTION_LIST: u32 = 0x00;
    pub const STACK_BASE: u32 = 0x04;
    pub const STACK_LIMIT: u32 = 0x08;
    pub const SELF: u32 = 0x18;
    pub const PROCESS_ID: u32 = 0x20;
    pub const THREAD_ID: u32 = 0x24;
    pub const TLS_POINTER: u32 = 0x2C;
    pub const PEB: u32 = 0x30;
    pub const LAST_ERROR: u32 = 0x34;
    /// 64 TLS slots (TlsGetValue/TlsSetValue).
    pub const TLS_SLOTS: u32 = 0xE10;
    pub const TLS_SLOT_COUNT: u32 = 64;
}

/// Fake process id.
pub const PROCESS_ID: u32 = 0x10;

/// Guest thread state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadState {
    Runnable,
    /// Waiting in a thunk that returned `Block`; retried every scheduling round.
    Blocked,
    /// `CREATE_SUSPENDED` / `SuspendThread` count.
    Suspended(u32),
    Exited(u32),
}

/// One guest thread.
pub struct Thread {
    pub tid: u32,
    /// Handle created with the thread (also returned by CreateThread).
    pub handle: u32,
    pub cpu: Cpu,
    pub state: ThreadState,
    pub teb: u32,
    /// Highest stack address (initial ESP area).
    pub stack_top: u32,
    /// Lowest stack address.
    pub stack_bottom: u32,
    /// When the current blocking wait started (host ms), cleared when the thunk returns.
    pub block_since: Option<u64>,
    /// Depth of `call_guest` frames on the host stack for this thread.
    pub nested: u32,
    /// Exception dispatches in progress (see seh.rs).
    pub(crate) seh: Vec<crate::seh::Dispatch>,
    /// Object ids (`HandleTable::object_id`) this thread is blocked on in [`Machine::poll_wait`].
    pub wait_objs: Vec<u32>,
    /// Set by [`Machine::pulse_event`]: the object id that released this thread's pending wait.
    pub pulsed: Option<u32>,
    /// Guest address of the CRITICAL_SECTION this thread is blocked on (0 = none).
    pub wait_cs: u32,
    /// The critical section in `wait_cs` was handed over to this thread by its previous owner.
    pub cs_granted: bool,
    /// Host-side scheduling priority (SetThreadPriority); informational.
    pub priority: i32,
}

/// Why the machine stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    /// ExitProcess or all threads ended.
    Exited(u32),
    /// A function without implementation was called.
    Unimplemented(String),
    /// Unhandled CPU exception / unimplemented instruction.
    Fault(String),
    /// Any other error (handler bug, bad state).
    Error(String),
    /// Host requested quit.
    Quit,
}

impl std::fmt::Display for StopReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StopReason::Exited(c) => write!(f, "process exited with code {c}"),
            StopReason::Unimplemented(s) => write!(f, "{s}"),
            StopReason::Fault(s) => write!(f, "fault: {s}"),
            StopReason::Error(s) => write!(f, "error: {s}"),
            StopReason::Quit => write!(f, "quit requested"),
        }
    }
}

/// Result of [`Machine::run_slice`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SliceResult {
    /// Guest code ran (instructions executed).
    Ran(u64),
    /// Every thread is blocked; the frontend may sleep.
    Idle,
    /// The machine stopped; see [`Machine::stop_reason`].
    Stopped,
}

/// Tracing (env `S3_TRACE=thunks` or `S3_TRACE=calls`, `S3_TRACE_LIMIT=n`, default 20000 lines).
#[derive(Debug, Clone, Default)]
pub struct TraceConfig {
    pub thunks: bool,
    pub calls: bool,
    pub limit: u64,
    pub printed: u64,
}

impl TraceConfig {
    pub fn from_env() -> Self {
        let v = std::env::var("S3_TRACE").unwrap_or_default();
        let limit = std::env::var("S3_TRACE_LIMIT").ok().and_then(|s| s.parse().ok()).unwrap_or(20_000);
        TraceConfig { thunks: v.contains("thunks") || v.contains("calls"), calls: v.contains("calls"), limit, printed: 0 }
    }
}

/// Counters.
#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub instructions: u64,
    pub thunk_calls: u64,
    pub presents: u64,
    pub exceptions: u64,
}

/// Process-wide guest data created by [`Machine::setup_process`].
#[derive(Debug, Clone, Default)]
pub struct ProcessInfo {
    pub image_base: u32,
    pub image_size: u32,
    pub entry: u32,
    pub peb: u32,
    pub params: u32,
    /// ANSI / UTF-16 command line (GetCommandLineA/W).
    pub cmdline_a: u32,
    pub cmdline_w: u32,
    /// Guest path of the exe, e.g. `C:\S3\S3.EXE`.
    pub module_path: String,
    /// ANSI / UTF-16 environment blocks (GetEnvironmentStrings[W]).
    pub env_a: u32,
    pub env_w: u32,
    /// Default stack reserve for new threads (from the PE header).
    pub stack_reserve: u32,
}

/// Shared HLE-module state stored in the machine (see [`Machine::state`]).
pub struct Shared<T>(Arc<Mutex<T>>);

impl<T> Clone for Shared<T> {
    fn clone(&self) -> Self {
        Shared(self.0.clone())
    }
}

impl<T> Shared<T> {
    /// Borrow the state. Panics if it is already borrowed (e.g. held across `call_guest` and re-entered) —
    /// drop the guard before calling into the guest.
    pub fn borrow_mut(&self) -> MutexGuard<'_, T> {
        match self.0.try_lock() {
            Ok(g) => g,
            Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                panic!("HLE state {} borrowed twice (held across call_guest?)", std::any::type_name::<T>())
            }
        }
    }
}

/// A periodic host-side hook run by the scheduler once per `run_slice` (audio mixing, timers, input pumps).
pub type Service = fn(&mut Machine);

/// The emulated machine.
pub struct Machine {
    pub mem: GuestMemory,
    pub threads: Vec<Thread>,
    /// Index into `threads` of the running thread.
    pub cur: usize,
    pub registry: Registry,
    pub handles: HandleTable,
    pub host: Box<dyn Host>,
    pub vfs: Vfs,
    pub com: ComRegistry,
    pub input: InputState,
    pub process: ProcessInfo,
    pub trace: TraceConfig,
    pub stats: Stats,
    /// Instructions per scheduling quantum.
    pub quantum: u64,
    services: Vec<(String, Service)>,
    state: HashMap<TypeId, Box<dyn Any + Send>>,
    stop: Option<StopReason>,
    pub(crate) thread_exit_thunk: u32,
    pub(crate) seh_return_thunk: u32,
    next_tid: u32,
    rr: usize,
}

impl Machine {
    /// New machine with empty address space, internal thunks registered and the VFS rooted at the host's
    /// game/data dirs. HLE modules are registered by the caller (s3-engine).
    pub fn new(host: Box<dyn Host>) -> Result<Machine, String> {
        let mut mem = GuestMemory::new().map_err(|e| e.to_string())?;
        // System area (PEB + TEBs) and the thunk page are runtime-owned allocations.
        mem.alloc_region(
            Some(layout::SYSTEM_AREA_START),
            layout::SYSTEM_AREA_END - layout::SYSTEM_AREA_START,
            RegionKind::Private,
            (0, 0),
        )
        .map_err(|e| e.to_string())?;
        mem.alloc_region(Some(layout::THUNK_BASE), layout::THUNK_END - layout::THUNK_BASE, RegionKind::Image, (0, 0))
            .map_err(|e| e.to_string())?;
        let vfs = Vfs::new(host.game_dir(), host.data_dir());
        let mut registry = Registry::new();
        let thread_exit_thunk = registry.register("s3port.dll", "thread_exit", thread_exit);
        let seh_return_thunk = registry.register("s3port.dll", "seh_handler_return", crate::seh::handler_return);
        Ok(Machine {
            mem,
            threads: Vec::new(),
            cur: 0,
            registry,
            handles: HandleTable::new(),
            host,
            vfs,
            com: ComRegistry::default(),
            input: InputState::new(),
            process: ProcessInfo::default(),
            trace: TraceConfig::from_env(),
            stats: Stats::default(),
            quantum: DEFAULT_QUANTUM,
            services: Vec::new(),
            state: HashMap::new(),
            stop: None,
            thread_exit_thunk,
            seh_return_thunk,
            next_tid: 0x104,
            rr: 0,
        })
    }

    // ------------------------------------------------------------------ small accessors

    /// The running thread.
    pub fn thread(&mut self) -> &mut Thread {
        &mut self.threads[self.cur]
    }

    /// The running thread's CPU.
    pub fn cpu(&mut self) -> &mut Cpu {
        &mut self.threads[self.cur].cpu
    }

    /// Thread id of the running thread.
    pub fn current_tid(&self) -> u32 {
        self.threads.get(self.cur).map(|t| t.tid).unwrap_or(0)
    }

    /// Thread slot by thread id.
    pub fn thread_by_tid(&self, tid: u32) -> Option<usize> {
        self.threads.iter().position(|t| t.tid == tid)
    }

    /// Host monotonic time in ms.
    pub fn now_ms(&self) -> u64 {
        self.host.now_ms()
    }

    /// Present a frame through the host (counts presents).
    pub fn present(&mut self, fb: &crate::FrameBuffer) {
        self.stats.presents += 1;
        self.host.present(fb);
    }

    /// Log through the host.
    pub fn log(&self, level: LogLevel, msg: &str) {
        self.host.log(level, msg);
    }

    /// Typed HLE-module state, created with `Default` on first use. Clone the handle, then
    /// `borrow_mut()`; do not hold the guard across [`Machine::call_guest`].
    pub fn state<T: Default + Send + 'static>(&mut self) -> Shared<T> {
        let e = self
            .state
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(Shared(Arc::new(Mutex::new(T::default())))));
        e.downcast_ref::<Shared<T>>().expect("state type").clone()
    }

    /// Add a host-side service run once per `run_slice`.
    pub fn add_service(&mut self, name: &str, f: Service) {
        if !self.services.iter().any(|(n, _)| n == name) {
            self.services.push((name.to_string(), f));
        }
    }

    /// Stop the machine (first reason wins).
    pub fn stop(&mut self, reason: StopReason) {
        if self.stop.is_none() {
            self.host.log(LogLevel::Warn, &format!("machine stopping: {reason}"));
            self.stop = Some(reason);
        }
    }

    pub fn stop_reason(&self) -> Option<&StopReason> {
        self.stop.as_ref()
    }

    pub fn is_stopped(&self) -> bool {
        self.stop.is_some()
    }

    /// Invalidate decoded code in all threads (after the guest wrote code / VirtualProtect to executable).
    pub fn invalidate_code(&mut self, start: u32, len: u32) {
        for t in &mut self.threads {
            t.cpu.invalidate_range(start, len);
        }
    }

    // ------------------------------------------------------------------ arguments (stdcall/cdecl)

    /// Argument `i` (0-based) of the current thunk call: `[esp + 4 + 4*i]` (ESP points at the return address).
    pub fn arg_u32(&self, i: u32) -> u32 {
        let esp = self.threads[self.cur].cpu.regs[ESP];
        self.mem.read_u32(esp.wrapping_add(4 + 4 * i))
    }

    pub fn arg_i32(&self, i: u32) -> i32 {
        self.arg_u32(i) as i32
    }

    /// Pointer argument (same as `arg_u32`, for readability).
    pub fn arg_ptr(&self, i: u32) -> u32 {
        self.arg_u32(i)
    }

    /// ANSI string argument (`None` for a NULL pointer).
    pub fn arg_str(&self, i: u32) -> Option<String> {
        let p = self.arg_u32(i);
        if p == 0 {
            None
        } else {
            Some(self.mem.read_cstr(p))
        }
    }

    /// UTF-16 string argument (`None` for a NULL pointer).
    pub fn arg_wstr(&self, i: u32) -> Option<String> {
        let p = self.arg_u32(i);
        if p == 0 {
            None
        } else {
            Some(self.mem.read_wstr(p))
        }
    }

    /// Return address of the current thunk call.
    pub fn return_address(&self) -> u32 {
        self.mem.read_u32(self.threads[self.cur].cpu.regs[ESP])
    }

    /// `SetLastError` for the running thread (TEB+0x34).
    pub fn set_last_error(&mut self, v: u32) {
        let teb = self.threads[self.cur].teb;
        self.mem.write_u32(teb + teb::LAST_ERROR, v);
    }

    /// `GetLastError` for the running thread.
    pub fn last_error(&self) -> u32 {
        self.mem.read_u32(self.threads[self.cur].teb + teb::LAST_ERROR)
    }

    // ------------------------------------------------------------------ process & threads

    /// Create PEB, process parameters, command line and environment. Call after loading the image.
    pub fn setup_process(&mut self, image_base: u32, image_size: u32, entry: u32, stack_reserve: u32, exe_name: &str) {
        crate::process::setup_process(self, image_base, image_size, entry, stack_reserve, exe_name);
    }

    /// Create a guest thread that starts at `entry(param)` (stdcall, returning into ExitThread).
    /// Returns the slot index; the thread's handle and tid are in `threads[slot]`.
    pub fn create_thread(&mut self, entry: u32, param: u32, stack_size: u32, suspended: bool) -> Result<usize, String> {
        let slot = self.threads.len();
        let teb_addr = layout::TEB_TOP - slot as u32 * layout::PAGE_SIZE;
        if teb_addr <= layout::SYSTEM_AREA_START + 0x1_0000 {
            return Err("too many threads".into());
        }
        let size = align_up(stack_size.max(0x1_0000), layout::ALLOC_GRANULARITY);
        let bottom = self
            .mem
            .alloc_region(None, size, RegionKind::Private, (layout::STACK_ARENA_START, layout::STACK_ARENA_END))
            .map_err(|e| format!("thread stack: {e}"))?;
        let top = bottom + size;
        let tid = self.next_tid;
        self.next_tid += 4;

        self.mem.fill(teb_addr, layout::PAGE_SIZE as usize, 0);
        self.mem.write_u32(teb_addr + teb::EXCEPTION_LIST, 0xFFFF_FFFF);
        self.mem.write_u32(teb_addr + teb::STACK_BASE, top);
        self.mem.write_u32(teb_addr + teb::STACK_LIMIT, bottom);
        self.mem.write_u32(teb_addr + teb::SELF, teb_addr);
        self.mem.write_u32(teb_addr + teb::PROCESS_ID, PROCESS_ID);
        self.mem.write_u32(teb_addr + teb::THREAD_ID, tid);
        self.mem.write_u32(teb_addr + teb::TLS_POINTER, teb_addr + teb::TLS_SLOTS);
        self.mem.write_u32(teb_addr + teb::PEB, self.process.peb);

        let mut cpu = Cpu::new();
        cpu.fs_base = teb_addr;
        cpu.regs[ESP] = top - 16;
        cpu.push32(&mut self.mem, param);
        cpu.push32(&mut self.mem, self.thread_exit_thunk);
        cpu.eip = entry;

        let handle = self.handles.insert(KernelObject::Thread { tid });
        self.threads.push(Thread {
            tid,
            handle,
            cpu,
            state: if suspended { ThreadState::Suspended(1) } else { ThreadState::Runnable },
            teb: teb_addr,
            stack_top: top,
            stack_bottom: bottom,
            block_since: None,
            nested: 0,
            seh: Vec::new(),
            wait_objs: Vec::new(),
            pulsed: None,
            wait_cs: 0,
            cs_granted: false,
            priority: 0,
        });
        Ok(slot)
    }

    /// Mark a thread exited (its handle becomes signaled).
    pub fn exit_thread(&mut self, slot: usize, code: u32) {
        self.threads[slot].state = ThreadState::Exited(code);
    }

    // ------------------------------------------------------------------ waits

    /// Is the object behind `h` signaled? `None` if `h` is not a waitable handle.
    pub fn is_signaled(&self, h: u32) -> Option<bool> {
        let tid = self.current_tid();
        match self.handles.get(h)? {
            KernelObject::Event { signaled, .. } => Some(*signaled),
            KernelObject::Mutex { owner, .. } => Some(owner.is_none() || *owner == Some(tid)),
            KernelObject::Semaphore { count, .. } => Some(*count > 0),
            KernelObject::Thread { tid: t } => {
                let s = self.thread_by_tid(*t)?;
                Some(matches!(self.threads[s].state, ThreadState::Exited(_)))
            }
            KernelObject::Process => Some(false),
            KernelObject::Timer { signaled, due_ms, .. } => {
                Some(*signaled || due_ms.map(|d| self.host.now_ms() >= d).unwrap_or(false))
            }
            KernelObject::Other(..) => None,
        }
    }

    /// If `h` is signaled, consume it (auto-reset event -> reset, semaphore -> count-1, mutex -> owned by
    /// the current thread) and return true.
    pub fn try_acquire(&mut self, h: u32) -> bool {
        if self.is_signaled(h) != Some(true) {
            return false;
        }
        let tid = self.current_tid();
        match self.handles.get_mut(h) {
            Some(KernelObject::Event { manual_reset, signaled }) => {
                if !*manual_reset {
                    *signaled = false;
                }
            }
            Some(KernelObject::Semaphore { count, .. }) => *count -= 1,
            Some(KernelObject::Mutex { owner, recursion }) => {
                *owner = Some(tid);
                *recursion += 1;
            }
            Some(KernelObject::Timer { manual_reset, signaled, due_ms, period_ms }) => {
                let now = self.host.now_ms();
                if let Some(d) = *due_ms {
                    if now >= d {
                        *due_ms = if *period_ms > 0 {
                            let next = d + *period_ms as u64;
                            Some(if next <= now { now + *period_ms as u64 } else { next })
                        } else {
                            None
                        };
                    }
                }
                *signaled = *manual_reset;
            }
            _ => {}
        }
        true
    }

    /// Generic Win32 wait for the running thread (WaitForSingleObject / WaitForMultipleObjects /
    /// MsgWaitForMultipleObjects). Call it from a thunk handler; on [`WaitPoll::Pending`] return
    /// `ThunkResult::Block` (the handler is called again later). `timeout_ms` = `INFINITE` (0xFFFFFFFF) never
    /// times out; the timeout is measured from the first call (`Thread::block_since`).
    /// On success the object is consumed (auto-reset event reset, semaphore decremented, mutex owned).
    pub fn poll_wait(&mut self, handles: &[u32], wait_all: bool, timeout_ms: u32) -> WaitPoll {
        let slot = self.cur;
        let mut ids = Vec::with_capacity(handles.len());
        for &h in handles {
            match (self.is_signaled(h), self.handles.object_id(h)) {
                (Some(_), Some(id)) => ids.push(id),
                _ => {
                    self.threads[slot].wait_objs.clear();
                    return WaitPoll::Failed;
                }
            }
        }
        // Released by PulseEvent while blocked?
        if let Some(p) = self.threads[slot].pulsed.take() {
            if let Some(i) = ids.iter().position(|&id| id == p) {
                if !wait_all || handles.len() == 1 {
                    self.threads[slot].wait_objs.clear();
                    return WaitPoll::Ready(i as u32);
                }
            }
        }
        if wait_all {
            if handles.iter().all(|&h| self.is_signaled(h) == Some(true)) {
                for &h in handles {
                    self.try_acquire(h);
                }
                self.threads[slot].wait_objs.clear();
                return WaitPoll::Ready(0);
            }
        } else {
            for (i, &h) in handles.iter().enumerate() {
                if self.try_acquire(h) {
                    self.threads[slot].wait_objs.clear();
                    return WaitPoll::Ready(i as u32);
                }
            }
        }
        if self.wait_timed_out(timeout_ms) {
            self.threads[slot].wait_objs.clear();
            return WaitPoll::Timeout;
        }
        self.threads[slot].wait_objs = ids;
        WaitPoll::Pending
    }

    /// Has the running thread's current wait (started at `block_since`) exceeded `timeout_ms`?
    /// `timeout_ms == 0` is always timed out; `INFINITE` never.
    pub fn wait_timed_out(&self, timeout_ms: u32) -> bool {
        if timeout_ms == 0 {
            return true;
        }
        if timeout_ms == 0xFFFF_FFFF {
            return false;
        }
        match self.threads[self.cur].block_since {
            Some(t0) => self.host.now_ms().saturating_sub(t0) >= timeout_ms as u64,
            None => false,
        }
    }

    /// `PulseEvent`: release waiters blocked on the event `h` (all for manual-reset, one for auto-reset),
    /// then leave the event non-signaled. Returns false if `h` is not an event.
    pub fn pulse_event(&mut self, h: u32) -> bool {
        let Some(id) = self.handles.object_id(h) else { return false };
        let manual = match self.handles.get_mut(h) {
            Some(KernelObject::Event { manual_reset, signaled }) => {
                *signaled = false;
                *manual_reset
            }
            _ => return false,
        };
        for t in self.threads.iter_mut() {
            if t.state == ThreadState::Blocked && t.pulsed.is_none() && t.wait_objs.contains(&id) {
                t.pulsed = Some(id);
                if !manual {
                    break;
                }
            }
        }
        true
    }

    // ------------------------------------------------------------------ scheduler

    /// Run guest code for about `max_instructions` instructions across all threads (round robin, each thread
    /// up to `quantum` per turn). Polls host input first and runs services.
    pub fn run_slice(&mut self, max_instructions: u64) -> SliceResult {
        if self.stop.is_some() {
            return SliceResult::Stopped;
        }
        for e in self.host.poll_events() {
            self.input.push(e);
        }
        let services: Vec<Service> = self.services.iter().map(|(_, f)| *f).collect();
        for s in services {
            s(self);
        }
        let start = self.stats.instructions;
        let mut progressed = false;
        loop {
            let n = self.threads.len();
            if n == 0 {
                self.stop(StopReason::Error("no threads".into()));
                return SliceResult::Stopped;
            }
            let mut round_progress = false;
            for k in 0..n {
                let slot = (self.rr + k) % n;
                let t = &self.threads[slot];
                if matches!(t.state, ThreadState::Exited(_) | ThreadState::Suspended(_)) || t.nested > 0 {
                    continue;
                }
                let budget = self.quantum.min(max_instructions.saturating_sub(self.stats.instructions - start)).max(1);
                if self.run_thread(slot, budget) {
                    round_progress = true;
                }
                if self.stop.is_some() {
                    return SliceResult::Stopped;
                }
            }
            self.rr = (self.rr + 1) % n.max(1);
            if self.threads.iter().all(|t| matches!(t.state, ThreadState::Exited(_))) {
                self.stop(StopReason::Exited(0));
                return SliceResult::Stopped;
            }
            progressed |= round_progress;
            if !round_progress || self.stats.instructions - start >= max_instructions {
                break;
            }
        }
        if progressed {
            SliceResult::Ran(self.stats.instructions - start)
        } else {
            SliceResult::Idle
        }
    }

    /// Run one thread for up to `budget` instructions. Returns true if it made progress.
    fn run_thread(&mut self, slot: usize, budget: u64) -> bool {
        self.cur = slot;
        let mut left = budget;
        let mut progress = false;
        loop {
            let t = &mut self.threads[slot];
            let before = t.cpu.instructions;
            let exit = t.cpu.run(&mut self.mem, left);
            let ran = t.cpu.instructions - before;
            self.stats.instructions += ran;
            left = left.saturating_sub(ran);
            if ran > 0 {
                progress = true;
            }
            self.cur = slot;
            match exit {
                CpuExit::Budget => return progress,
                CpuExit::Thunk(a) => match self.dispatch_thunk(a) {
                    Disp::Continue => {
                        progress = true;
                        if left == 0 {
                            return true;
                        }
                    }
                    Disp::Blocked => return progress,
                    Disp::ThreadExited => return true,
                    Disp::Stop => return progress,
                },
                CpuExit::ReturnMagic => {
                    self.stop(StopReason::Error(format!("thread {slot}: return to magic address outside call_guest")));
                    return progress;
                }
                other => {
                    if !crate::seh::handle_cpu_exit(self, other) {
                        return progress;
                    }
                    progress = true;
                }
            }
            if self.stop.is_some() {
                return progress;
            }
        }
    }

    /// Run all other runnable threads once (used while the current thread waits inside a nested call).
    fn run_others(&mut self, except: usize) -> bool {
        let mut progress = false;
        for slot in 0..self.threads.len() {
            if slot == except {
                continue;
            }
            let t = &self.threads[slot];
            if matches!(t.state, ThreadState::Exited(_) | ThreadState::Suspended(_)) || t.nested > 0 {
                continue;
            }
            if self.run_thread(slot, self.quantum) {
                progress = true;
            }
            if self.stop.is_some() {
                break;
            }
        }
        self.cur = except;
        progress
    }

    pub(crate) fn dispatch_thunk(&mut self, addr: u32) -> Disp {
        let slot = self.cur;
        let retry = self.threads[slot].block_since.is_some();
        let handler = match self.registry.entry_mut(addr) {
            Some(e) => {
                if !retry {
                    e.calls += 1;
                }
                e.handler
            }
            None => {
                self.stop(StopReason::Error(format!("jump into unused thunk {addr:#x}")));
                return Disp::Stop;
            }
        };
        if !retry {
            self.stats.thunk_calls += 1;
            if self.trace.thunks && self.trace.printed < self.trace.limit {
                self.trace.printed += 1;
                let mut s = format!("[t{}] {} from {:#x}", slot, self.registry.describe(addr), self.return_address());
                if self.trace.calls {
                    s.push_str(&format!(
                        " ({:#x}, {:#x}, {:#x}, {:#x})",
                        self.arg_u32(0),
                        self.arg_u32(1),
                        self.arg_u32(2),
                        self.arg_u32(3)
                    ));
                }
                self.host.log(LogLevel::Trace, &s);
            }
        }
        let Some(h) = handler else {
            let msg = format!("UNIMPLEMENTED {} (called from {:#x})", self.registry.describe(addr), self.return_address());
            self.stop(StopReason::Unimplemented(msg));
            return Disp::Stop;
        };
        let r = h(self);
        self.cur = slot;
        if self.trace.calls && self.trace.printed < self.trace.limit {
            if let ThunkResult::Return { eax, .. } | ThunkResult::Return64 { eax, .. } = r {
                self.trace.printed += 1;
                self.host.log(LogLevel::Trace, &format!("[t{}]   {} -> {:#x}", slot, self.registry.describe(addr), eax));
            }
        }
        self.apply_result(slot, r)
    }

    fn apply_result(&mut self, slot: usize, r: ThunkResult) -> Disp {
        let t = &mut self.threads[slot];
        match r {
            ThunkResult::Return { eax, pop_bytes } => {
                t.cpu.regs[EAX] = eax;
                let ret = t.cpu.pop32(&self.mem);
                t.cpu.regs[ESP] = t.cpu.regs[ESP].wrapping_add(pop_bytes);
                t.cpu.eip = ret;
                t.block_since = None;
                if t.state == ThreadState::Blocked {
                    t.state = ThreadState::Runnable;
                }
                Disp::Continue
            }
            ThunkResult::Return64 { eax, edx, pop_bytes } => {
                t.cpu.regs[EAX] = eax;
                t.cpu.regs[EDX] = edx;
                let ret = t.cpu.pop32(&self.mem);
                t.cpu.regs[ESP] = t.cpu.regs[ESP].wrapping_add(pop_bytes);
                t.cpu.eip = ret;
                t.block_since = None;
                if t.state == ThreadState::Blocked {
                    t.state = ThreadState::Runnable;
                }
                Disp::Continue
            }
            ThunkResult::Done => {
                t.block_since = None;
                if t.state == ThreadState::Blocked {
                    t.state = ThreadState::Runnable;
                }
                Disp::Continue
            }
            ThunkResult::Block => {
                if t.block_since.is_none() {
                    t.block_since = Some(self.host.now_ms());
                }
                t.state = ThreadState::Blocked;
                Disp::Blocked
            }
            ThunkResult::Exit(code) => {
                self.stop(StopReason::Exited(code));
                Disp::Stop
            }
            ThunkResult::ExitThread(code) => {
                t.state = ThreadState::Exited(code);
                t.block_since = None;
                Disp::ThreadExited
            }
            ThunkResult::Error(e) => {
                self.stop(StopReason::Error(e));
                Disp::Stop
            }
        }
    }

    /// Call guest code `addr(args...)` on the current thread and return EAX. Works for stdcall and cdecl
    /// callees (all registers, including ESP, are restored afterwards). Nested calls are fine. While the guest
    /// code blocks, other threads run. Returns 0 if the machine stopped meanwhile.
    pub fn call_guest(&mut self, addr: u32, args: &[u32]) -> u32 {
        let slot = self.cur;
        let saved: Context = self.threads[slot].cpu.context();
        {
            let t = &mut self.threads[slot];
            for &a in args.iter().rev() {
                t.cpu.push32(&mut self.mem, a);
            }
            t.cpu.push32(&mut self.mem, layout::MAGIC_RETURN);
            t.cpu.eip = addr;
            t.nested += 1;
        }
        let result = loop {
            if self.stop.is_some() {
                break 0;
            }
            self.cur = slot;
            let t = &mut self.threads[slot];
            let before = t.cpu.instructions;
            let exit = t.cpu.run(&mut self.mem, self.quantum);
            self.stats.instructions += t.cpu.instructions - before;
            match exit {
                CpuExit::ReturnMagic => break self.threads[slot].cpu.regs[EAX],
                CpuExit::Budget => {
                    self.run_others(slot);
                }
                CpuExit::Thunk(a) => match self.dispatch_thunk(a) {
                    Disp::Continue => {}
                    Disp::Blocked => {
                        if !self.run_others(slot) {
                            self.host.idle(1);
                        }
                    }
                    Disp::ThreadExited => {
                        self.stop(StopReason::Error("thread exited inside a host->guest call".into()));
                        break 0;
                    }
                    Disp::Stop => break 0,
                },
                other => {
                    if !crate::seh::handle_cpu_exit(self, other) {
                        break 0;
                    }
                }
            }
        };
        self.cur = slot;
        let t = &mut self.threads[slot];
        t.nested -= 1;
        t.cpu.set_context(&saved);
        result
    }
}

/// Result of [`Machine::poll_wait`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitPoll {
    /// Object `index` was signaled (and consumed); for wait-all always 0.
    Ready(u32),
    Timeout,
    /// A handle is invalid or not waitable (ERROR_INVALID_HANDLE).
    Failed,
    /// Not yet: return `ThunkResult::Block`.
    Pending,
}

/// Outcome of dispatching one thunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Disp {
    Continue,
    Blocked,
    ThreadExited,
    Stop,
}

/// `s3port.dll!thread_exit`: thread entry functions return here.
fn thread_exit(m: &mut Machine) -> ThunkResult {
    let code = m.cpu().regs[EAX];
    ThunkResult::ExitThread(code)
}
