//! Multi-core mode (perf-mt, `docs/porting/multicore.md`): the presenting guest thread (the game's draw thread)
//! runs its CPU on a second host thread, the "draw worker", truly in parallel with the cooperative scheduler on
//! the main host thread.
//!
//! Rules:
//! - Guest memory is shared (one mapping; the worker uses a non-owning [`GuestMemory::alias_view`]).
//! - Everything else in [`Machine`] (HLE state, handles, threads, host) is guarded by one **big lock**. The main
//!   host thread holds it all the time (also while the frontend runs between slices) and releases it only while
//!   it executes guest code of a top-level scheduled thread ([`Machine::run_cpu_unlocked`]) or idles
//!   ([`Machine::idle`]). The worker takes it for every CPU exit (thunk, hook, fault, budget) and releases it
//!   while its CPU runs.
//! - A CPU that runs without the lock is moved out of `Machine::threads` (swapped with a spare) so the other host
//!   thread never sees it; `Thread::offloaded` / `Thread::running_unlocked` make both schedulers skip it.
//! - Off by default: nothing here runs unless a frontend calls [`Machine::enable_mt`]. `s3det` never does, so the
//!   deterministic single-thread scheduler stays the reference.

use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering::SeqCst};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use s3_cpu::{Cpu, CpuExit};
use s3_mem::GuestMemory;

use crate::host::LogLevel;
use crate::machine::{Disp, Machine, StopReason, ThreadState};

/// Frames a non-main guest thread must present in a row before it is moved to the draw worker.
const OFFLOAD_AFTER_FRAMES: u32 = 30;
/// Instructions the worker runs per lock-free window (same as the scheduler quantum).
const WORKER_QUANTUM: u64 = crate::machine::DEFAULT_QUANTUM;
/// Yield-spin before a waiting host thread sleeps (µs), env `S3_MT_SPIN_US` (default 300).
static SPIN_US: AtomicU32 = AtomicU32::new(300);
/// Delay before the main thread composes a frame queued by the draw worker (µs), env `S3_MT_PRESENT_DELAY_US`.
static PRESENT_DELAY_US: AtomicU32 = AtomicU32::new(600);

/// The big HLE lock: a flag under a mutex (held across arbitrary code, so not a `MutexGuard`), with a
/// "worker wants it" hint so the main thread does not re-take it in a tight loop.
pub(crate) struct BigLock {
    held: Mutex<bool>,
    cv: Condvar,
    want: AtomicBool,
}

impl BigLock {
    fn new() -> Self {
        BigLock {
            held: Mutex::new(false),
            cv: Condvar::new(),
            want: AtomicBool::new(false),
        }
    }

    pub(crate) fn acquire(&self) {
        let mut g = self.held.lock().unwrap_or_else(|p| p.into_inner());
        while *g {
            g = self.cv.wait(g).unwrap_or_else(|p| p.into_inner());
        }
        *g = true;
    }

    /// Main thread: let a waiting worker go first.
    pub(crate) fn acquire_main(&self) {
        let mut spins = 0u32;
        while self.want.load(SeqCst) && spins < 10_000 {
            std::thread::yield_now();
            spins += 1;
        }
        self.acquire();
    }

    fn acquire_worker(&self) {
        self.want.store(true, SeqCst);
        self.acquire();
        self.want.store(false, SeqCst);
    }

    pub(crate) fn release(&self) {
        let mut g = self.held.lock().unwrap_or_else(|p| p.into_inner());
        *g = false;
        drop(g);
        self.cv.notify_one();
    }
}

/// "Something changed" notification between the two host threads (any HLE call may signal an object the other
/// side waits for). Waiters sleep until the epoch moves or the timeout passes (timers, timeouts).
pub(crate) struct Wake {
    epoch: AtomicU64,
    waiters: AtomicU32,
    m: Mutex<()>,
    cv: Condvar,
}

impl Wake {
    fn new() -> Self {
        Wake {
            epoch: AtomicU64::new(0),
            waiters: AtomicU32::new(0),
            m: Mutex::new(()),
            cv: Condvar::new(),
        }
    }

    pub(crate) fn epoch(&self) -> u64 {
        self.epoch.load(SeqCst)
    }

    pub(crate) fn bump(&self) {
        self.epoch.fetch_add(1, SeqCst);
        if self.waiters.load(SeqCst) > 0 {
            let _g = self.m.lock().unwrap_or_else(|p| p.into_inner());
            self.cv.notify_all();
        }
    }

    /// Wait until the epoch differs from `seen` or `timeout` passed: first a short yield-spin (the other side
    /// usually answers within a fraction of a frame: snapshot handover), then a condvar wait that the other side's
    /// `bump` ends at once.
    pub(crate) fn wait(&self, seen: u64, timeout: Duration) {
        let start = std::time::Instant::now();
        let spin = Duration::from_micros(SPIN_US.load(SeqCst) as u64).min(timeout);
        while start.elapsed() < spin {
            if self.epoch() != seen {
                return;
            }
            std::thread::yield_now();
        }
        if cfg!(windows) {
            // Condvar timeouts on Windows follow the system tick (15.6 ms, measured: the draw thread's timed
            // waits took 15 ms instead of 1). `thread::sleep` uses a high-resolution timer: poll in 100 µs steps.
            while self.epoch() == seen && start.elapsed() < timeout {
                std::thread::sleep(Duration::from_micros(100));
            }
            return;
        }
        self.waiters.fetch_add(1, SeqCst);
        let g = self.m.lock().unwrap_or_else(|p| p.into_inner());
        if self.epoch() == seen {
            let _ = self.cv.wait_timeout(
                g,
                timeout
                    .saturating_sub(start.elapsed())
                    .max(Duration::from_micros(100)),
            );
        }
        self.waiters.fetch_sub(1, SeqCst);
    }
}

/// State shared between the main host thread and the draw worker.
pub(crate) struct MtShared {
    pub(crate) lock: BigLock,
    pub(crate) wake: Wake,
    stop: AtomicBool,
    /// The machine (updated by the main thread under the lock if the `Machine` value moved).
    machine: AtomicPtr<Machine>,
    /// Host ns: [main waits for the lock, main idle, worker waits for the lock, worker waits for a change,
    /// worker holds the lock].
    pub(crate) prof: [AtomicU64; 6],
    /// Bumped by `set_hook_addrs` / `set_native_blocks`; the worker re-applies them to its CPU.
    pub(crate) ext_gen: AtomicU64,
}

/// `S3_MT_DEBUG=1`: log long waits of the worker.
static MT_DEBUG: std::sync::LazyLock<bool> =
    std::sync::LazyLock::new(|| std::env::var_os("S3_MT_DEBUG").is_some());

/// Host µs since the first call (debug timelines).
pub fn now_us() -> u64 {
    static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    T0.get_or_init(std::time::Instant::now)
        .elapsed()
        .as_micros() as u64
}

/// Run `f` and add its host time to `c`.
#[inline]
fn timed<R>(c: &AtomicU64, f: impl FnOnce() -> R) -> R {
    let t0 = std::time::Instant::now();
    let r = f();
    c.fetch_add(t0.elapsed().as_nanos() as u64, SeqCst);
    r
}

/// A CPU moved to the worker thread. `Cpu` holds raw pointers into its own decode cache, so it is not `Send`;
/// it is moved, never shared.
struct SendCpu(Cpu);
unsafe impl Send for SendCpu {}

/// Per-machine multi-core state ([`Machine::mt`]).
pub struct MtState {
    pub(crate) shared: Arc<MtShared>,
    worker: Option<JoinHandle<()>>,
    /// Guest thread id running on the worker.
    pub(crate) worker_tid: Option<u32>,
    /// Offload candidate: (tid, frames presented in a row).
    candidate: (u32, u32),
    /// Memory view used by the main thread's lock-free windows.
    pub(crate) main_view: GuestMemory,
    /// Spare CPU swapped into `threads[slot]` while the main thread runs a CPU without the lock.
    pub(crate) spare: Cpu,
    /// Total worker instructions / host ns in `Cpu::run` (also added to `Machine::stats`).
    pub worker_instructions: u64,
    /// Frame queued by the draw worker's present, composed and presented by the main thread.
    pending_frame: Option<crate::FrameBuffer>,
    /// When `pending_frame` was queued: the main thread composes it only after `PRESENT_DELAY_US`, when the
    /// draw thread is past its frame-start HLE calls and inside the long lock-free render stretch.
    pending_since: Option<std::time::Instant>,
    /// Spare frame buffers handed back to the worker's presents.
    frame_pool: Vec<crate::FrameBuffer>,
    /// Frames replaced in `pending_frame` before the main thread presented them.
    pub frames_dropped: u64,
    pub frames_presented: u64,
}

impl Machine {
    /// Turn on the multi-core mode (`S3_MT=1` in the frontends). Must be called on the thread that runs the
    /// machine, before or between slices. Not deterministic (host timing decides the interleaving); never used by
    /// `s3det`.
    pub fn enable_mt(&mut self) {
        if self.mt.is_some() {
            return;
        }
        if let Some(v) = std::env::var("S3_MT_SPIN_US")
            .ok()
            .and_then(|v| v.parse().ok())
        {
            SPIN_US.store(v, SeqCst);
        }
        if let Some(v) = std::env::var("S3_MT_PRESENT_DELAY_US")
            .ok()
            .and_then(|v| v.parse().ok())
        {
            PRESENT_DELAY_US.store(v, SeqCst);
        }
        let shared = Arc::new(MtShared {
            lock: BigLock::new(),
            wake: Wake::new(),
            stop: AtomicBool::new(false),
            machine: AtomicPtr::new(self as *mut Machine),
            prof: Default::default(),
            ext_gen: AtomicU64::new(0),
        });
        // The main thread holds the big lock from now on (see module docs).
        shared.lock.acquire();
        // SAFETY: the view lives in `self.mt`, dropped before `self.mem` (Machine::drop shuts MT down first).
        let main_view = unsafe { self.mem.alias_view() };
        self.mt = Some(Box::new(MtState {
            shared,
            worker: None,
            worker_tid: None,
            candidate: (0, 0),
            main_view,
            spare: Cpu::new(),
            worker_instructions: 0,
            pending_frame: None,
            pending_since: None,
            frame_pool: Vec::new(),
            frames_dropped: 0,
            frames_presented: 0,
        }));
        self.log(
            LogLevel::Info,
            "mt: multi-core mode on (draw thread on its own host thread once it presents)",
        );
    }

    /// Is a draw worker running?
    pub fn mt_active(&self) -> bool {
        self.mt.as_ref().is_some_and(|m| m.worker_tid.is_some())
    }

    /// Guest thread id currently on the draw worker.
    pub fn mt_worker_tid(&self) -> Option<u32> {
        self.mt.as_ref().and_then(|m| m.worker_tid)
    }

    /// Wait/lock statistics of the multi-core mode (empty when off).
    pub fn mt_report(&self) -> String {
        let Some(mt) = &self.mt else {
            return String::new();
        };
        let p: Vec<u64> = mt
            .shared
            .prof
            .iter()
            .map(|a| a.load(SeqCst) / 1_000_000)
            .collect();
        format!(
            "mt: worker {} M instr in {} ms Cpu::run; ms: main lock wait {}, main idle {}, worker lock wait {}, worker wait {}, worker lock held {}; frames presented by main {}, dropped {}\n",
            mt.worker_instructions / 1_000_000, p[5], p[0], p[1], p[2], p[3], p[4], mt.frames_presented, mt.frames_dropped
        )
    }

    /// Called on every present by the presenting guest thread.
    pub(crate) fn mt_note_present(&mut self) {
        let tid = self.current_tid();
        let slot = self.cur;
        let Some(mt) = self.mt.as_mut() else { return };
        if slot == 0 || mt.worker_tid == Some(tid) {
            return;
        }
        if mt.candidate.0 == tid {
            mt.candidate.1 += 1;
        } else {
            mt.candidate = (tid, 1);
        }
    }

    /// Draw worker (under the lock): queue a presented frame for the main thread and return a buffer for the
    /// next one. A frame the main thread has not taken yet is replaced (dropped), like a triple-buffered swap chain.
    pub(crate) fn mt_defer_present(&mut self, fb: crate::FrameBuffer) -> crate::FrameBuffer {
        let mt = self.mt.as_mut().unwrap();
        mt.pending_since = Some(std::time::Instant::now());
        let back = match mt.pending_frame.replace(fb) {
            Some(old) => {
                mt.frames_dropped += 1;
                old
            }
            None => mt.frame_pool.pop().unwrap_or_default(),
        };
        mt.shared.wake.bump();
        back
    }

    /// Main thread: compose and present a frame queued by the draw worker.
    pub(crate) fn mt_flush_present(&mut self) {
        let delay = Duration::from_micros(PRESENT_DELAY_US.load(SeqCst) as u64);
        let Some(mt) = self.mt.as_mut() else { return };
        if mt.pending_since.is_none_or(|t| t.elapsed() < delay) {
            return;
        }
        mt.pending_since = None;
        let Some(fb) = mt.pending_frame.take() else {
            return;
        };
        let back = self.present_owned_now(fb);
        let mt = self.mt.as_mut().unwrap();
        mt.frames_presented += 1;
        if mt.frame_pool.len() < 2 {
            mt.frame_pool.push(back);
        }
    }

    /// Main thread, under the lock, at the start of every `run_slice`: keep the machine pointer current, reap a
    /// finished worker, start one for the presenting thread, present a queued frame.
    pub(crate) fn mt_maintain(&mut self) {
        let me = self as *mut Machine;
        self.mt_flush_present();
        let Some(mt) = self.mt.as_mut() else { return };
        mt.shared.machine.store(me, SeqCst);
        if mt.worker_tid.is_none() {
            if let Some(h) = mt.worker.take() {
                let _ = h.join();
            }
        }
        if mt.worker_tid.is_some() || mt.candidate.1 < OFFLOAD_AFTER_FRAMES {
            return;
        }
        let tid = mt.candidate.0;
        mt.candidate = (0, 0);
        let Some(slot) = self.thread_by_tid(tid) else {
            return;
        };
        let t = &self.threads[slot];
        if t.nested > 0 || matches!(t.state, ThreadState::Exited(_)) || t.offloaded {
            return;
        }
        self.mt_offload(slot);
    }

    fn mt_offload(&mut self, slot: usize) {
        let tid = self.threads[slot].tid;
        let mut cpu = Cpu::new();
        std::mem::swap(&mut cpu, &mut self.threads[slot].cpu);
        // The placeholder shows the real counters until the next sync.
        self.threads[slot].cpu.instructions = cpu.instructions;
        self.threads[slot].cpu.eip = cpu.eip;
        self.threads[slot].offloaded = true;
        // SAFETY: the view is dropped by the worker before Machine::drop releases the mapping (it joins).
        let view = unsafe { self.mem.alias_view() };
        let mt = self.mt.as_mut().unwrap();
        let shared = mt.shared.clone();
        let gen = mt.shared.ext_gen.load(SeqCst);
        let cpu = SendCpu(cpu);
        let view = SendView(view);
        mt.worker_tid = Some(tid);
        let h = std::thread::Builder::new()
            .name("s3-draw".into())
            .spawn(move || worker_main(shared, tid, cpu, view, gen))
            .expect("spawn draw worker");
        self.mt.as_mut().unwrap().worker = Some(h);
        self.log(
            LogLevel::Info,
            &format!("mt: guest thread {slot} (tid {tid:#x}) now runs on its own host thread"),
        );
    }

    /// Main thread: run `cpu` (moved out of the machine) for `budget` instructions with the big lock released.
    /// Out of line and given the machine pointer, so the compiler cannot keep `Machine` fields in registers across
    /// the window in which the worker may change them.
    #[inline(never)]
    pub(crate) fn run_cpu_unlocked(&mut self, slot: usize, budget: u64) -> CpuExit {
        let me: *mut Machine = self;
        // SAFETY: see module docs; `mt` is boxed (stable address) and only replaced on this thread.
        unsafe { run_window(me, slot, budget) }
    }

    /// Idle for up to `ms` (all threads blocked). With a draw worker the big lock is released meanwhile and the
    /// wait ends early when the worker changes something.
    pub fn idle(&mut self, ms: u32) {
        if !self.mt_active() {
            self.host.idle(ms);
            return;
        }
        let shared = self.mt.as_ref().unwrap().shared.clone();
        let seen = shared.wake.epoch();
        let me: *mut Machine = self;
        // SAFETY: as in run_cpu_unlocked.
        unsafe { idle_window(me, &shared, seen, ms) }
    }

    /// Tell the other host thread that HLE state may have changed (cheap when nobody waits).
    #[inline]
    pub(crate) fn mt_bump(&self) {
        if let Some(mt) = &self.mt {
            mt.shared.wake.bump();
        }
    }

    /// Stop and join the worker (Machine::drop). The worker's CPU is not handed back.
    pub(crate) fn mt_shutdown(&mut self) {
        let Some(mt) = self.mt.as_mut() else { return };
        mt.shared.stop.store(true, SeqCst);
        if let Some(h) = mt.worker.take() {
            mt.shared.lock.release();
            mt.shared.wake.bump();
            let _ = h.join();
        }
        self.mt = None;
    }
}

struct SendView(GuestMemory);
unsafe impl Send for SendView {}

#[inline(never)]
unsafe fn run_window(me: *mut Machine, slot: usize, budget: u64) -> CpuExit {
    let m = &mut *me;
    let mt: *mut MtState = &mut **m.mt.as_mut().unwrap();
    let shared = (*mt).shared.clone();
    let mut cpu = std::mem::replace(&mut (*mt).spare, Cpu::new());
    std::mem::swap(&mut cpu, &mut m.threads[slot].cpu);
    m.threads[slot].running_unlocked = true;
    let view: *mut GuestMemory = &mut (*mt).main_view;
    shared.lock.release();
    let exit = cpu.run(&mut *view, budget);
    timed(&shared.prof[0], || shared.lock.acquire_main());
    let m = &mut *me;
    std::mem::swap(&mut cpu, &mut m.threads[slot].cpu);
    m.threads[slot].running_unlocked = false;
    (*mt).spare = cpu;
    exit
}

#[inline(never)]
unsafe fn idle_window(_me: *mut Machine, shared: &MtShared, seen: u64, ms: u32) {
    shared.lock.release();
    timed(&shared.prof[1], || {
        shared
            .wake
            .wait(seen, Duration::from_millis(ms.max(1) as u64))
    });
    timed(&shared.prof[0], || shared.lock.acquire_main());
}

/// What the worker does after handling one CPU exit.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Run the CPU again.
    Run,
    /// Blocked in a thunk (or the machine stopped): wait for a change, then run (re-dispatches the thunk).
    Wait,
    /// Suspended: wait, then check again without running.
    Suspended,
    /// The thread exited; the real CPU is back in the machine.
    Done,
}

fn worker_main(shared: Arc<MtShared>, tid: u32, cpu: SendCpu, view: SendView, mut gen: u64) {
    let mut cpu = cpu.0;
    let mut view = view.0;
    // The thread may be blocked in a thunk right now: running it re-dispatches that thunk at once.
    let mut step = Step::Run;
    let mut seen = shared.wake.epoch();
    let mut before = cpu.instructions;
    let mut ns = 0u64;
    let mut free_budgets = 0u32;
    loop {
        if matches!(step, Step::Wait | Step::Suspended) {
            let t0 = std::time::Instant::now();
            timed(&shared.prof[3], || {
                shared.wake.wait(seen, Duration::from_millis(1))
            });
            if *MT_DEBUG {
                let us = t0.elapsed().as_micros();
                if us > 1500 {
                    eprintln!(
                        "mt: worker waited {us} us at {:#x} (epoch moved: {})",
                        cpu.eip,
                        shared.wake.epoch() != seen
                    );
                }
            }
        }
        if shared.stop.load(SeqCst) {
            return;
        }
        let t0 = std::time::Instant::now();
        let exit = if step == Step::Suspended {
            CpuExit::Budget
        } else {
            cpu.run(&mut view, WORKER_QUANTUM)
        };
        let dt = t0.elapsed().as_nanos() as u64;
        ns += dt;
        shared.prof[5].fetch_add(dt, SeqCst);
        // A budget exit needs no HLE state: keep running without the lock (checking suspension, hooks and the
        // counters under the lock every 16th time).
        if matches!(exit, CpuExit::Budget)
            && step == Step::Run
            && free_budgets < 15
            && shared.ext_gen.load(SeqCst) == gen
        {
            free_budgets += 1;
            continue;
        }
        free_budgets = 0;
        timed(&shared.prof[2], || shared.lock.acquire_worker());
        let held = std::time::Instant::now();
        if shared.stop.load(SeqCst) {
            shared.lock.release();
            return;
        }
        // SAFETY: under the big lock (module docs); the main thread keeps the pointer current.
        let m = unsafe { &mut *shared.machine.load(SeqCst) };
        step = worker_step(m, tid, &mut cpu, exit, before, ns, &mut gen);
        before = cpu.instructions;
        ns = 0;
        if step == Step::Done {
            if let Some(mt) = m.mt.as_mut() {
                mt.worker_tid = None;
            }
        }
        seen = shared.wake.epoch();
        if step != Step::Wait && step != Step::Suspended {
            shared.wake.bump();
        }
        shared.prof[4].fetch_add(held.elapsed().as_nanos() as u64, SeqCst);
        shared.lock.release();
        if step == Step::Done {
            return;
        }
    }
}

/// Handle one CPU exit of the offloaded thread under the big lock. `cpu` is the real CPU; for the dispatch it is
/// swapped with the placeholder in `m.threads[slot]` and back.
fn worker_step(
    m: &mut Machine,
    tid: u32,
    cpu: &mut Cpu,
    exit: CpuExit,
    before: u64,
    ns: u64,
    gen: &mut u64,
) -> Step {
    let ran = cpu.instructions - before;
    m.stats.instructions += ran;
    m.stats.cpu_ns += ns;
    if let Some(mt) = m.mt.as_mut() {
        mt.worker_instructions += ran;
        let g = mt.shared.ext_gen.load(SeqCst);
        if g != *gen {
            *gen = g;
            cpu.set_hooks(m.ext.hook_addrs.clone());
            cpu.set_native_blocks(m.ext.native_blocks.clone());
        }
    }
    let Some(slot) = m.thread_by_tid(tid) else {
        return Step::Done;
    };
    m.threads[slot].cpu_ns += ns;
    match m.threads[slot].state {
        ThreadState::Exited(_) => {
            std::mem::swap(cpu, &mut m.threads[slot].cpu);
            m.threads[slot].offloaded = false;
            return Step::Done;
        }
        ThreadState::Suspended(_) if matches!(exit, CpuExit::Budget) => {
            sync_placeholder(m, slot, cpu);
            return Step::Suspended;
        }
        _ => {}
    }
    if m.is_stopped() {
        return Step::Wait;
    }
    std::mem::swap(cpu, &mut m.threads[slot].cpu); // machine: real CPU, `cpu`: placeholder
    let saved_cur = m.cur;
    m.cur = slot;
    let step = match exit {
        CpuExit::Budget => Step::Run,
        CpuExit::Thunk(a) => match m.dispatch_thunk(a) {
            Disp::Continue => Step::Run,
            Disp::Blocked | Disp::Stop => Step::Wait,
            Disp::ThreadExited => Step::Done,
        },
        CpuExit::Hook(a) => {
            m.dispatch_hook(a);
            Step::Run
        }
        CpuExit::ReturnMagic => {
            m.stop(StopReason::Error(format!(
                "thread {slot}: return to magic address outside call_guest"
            )));
            Step::Wait
        }
        other => {
            if crate::seh::handle_cpu_exit(m, other) {
                Step::Run
            } else {
                Step::Wait
            }
        }
    };
    m.cur = saved_cur;
    // The thread table may have grown during the dispatch (CreateThread); look the slot up again.
    let slot = m.thread_by_tid(tid).unwrap_or(slot);
    if step == Step::Done || matches!(m.threads[slot].state, ThreadState::Exited(_)) {
        // The real CPU stays in the machine.
        m.threads[slot].offloaded = false;
        return Step::Done;
    }
    std::mem::swap(cpu, &mut m.threads[slot].cpu); // back: `cpu` real, machine placeholder
    sync_placeholder(m, slot, cpu);
    step
}

/// Mirror the counters of the real CPU into the placeholder (S3_SAMPLE, summaries).
fn sync_placeholder(m: &mut Machine, slot: usize, cpu: &Cpu) {
    let ph = &mut m.threads[slot].cpu;
    ph.instructions = cpu.instructions;
    ph.eip = cpu.eip;
    ph.regs = cpu.regs;
}
