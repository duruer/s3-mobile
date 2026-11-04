//! kernel32: threads, synchronisation objects, waits, critical sections, TLS, waitable timers, Sleep.
//!
//! Everything runs on the cooperative scheduler of `s3_runtime::Machine`: a wait that cannot be satisfied
//! returns `ThunkResult::Block` and the handler is called again on the next scheduling round (see
//! `Machine::poll_wait`). Critical sections live in the guest struct
//! (`{DebugInfo, LockCount, RecursionCount, OwningThread, LockSemaphore, SpinCount}`); a contended `Enter`
//! blocks, and `Leave` hands ownership directly to one blocked waiter (FIFO by thread slot order) so a thread
//! that re-enters in a loop cannot starve the others.

use s3_runtime::handles::{CURRENT_PROCESS, CURRENT_THREAD};
use s3_runtime::machine::teb;
use s3_runtime::{KernelObject, Machine, Registry, ThreadState, ThunkResult, WaitPoll};

use super::DLL;

pub const WAIT_OBJECT_0: u32 = 0;
pub const WAIT_TIMEOUT: u32 = 0x102;
pub const WAIT_FAILED: u32 = 0xFFFF_FFFF;
pub const INFINITE: u32 = 0xFFFF_FFFF;
const STILL_ACTIVE: u32 = 259;
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_ALREADY_EXISTS: u32 = 183;
const ERROR_TOO_MANY_POSTS: u32 = 298;
const ERROR_NOT_OWNER: u32 = 288;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const MAXIMUM_WAIT_OBJECTS: u32 = 64;

pub(crate) fn register(reg: &mut Registry) {
    reg.register(DLL, "CreateThread", create_thread);
    reg.register(DLL, "ExitThread", exit_thread);
    reg.register(DLL, "ResumeThread", resume_thread);
    reg.register(DLL, "SuspendThread", suspend_thread);
    reg.register(DLL, "GetExitCodeThread", get_exit_code_thread);
    reg.register(DLL, "TerminateThread", terminate_thread);
    reg.register(DLL, "GetCurrentThread", |_m| ThunkResult::stdcall(CURRENT_THREAD, 0));
    reg.register(DLL, "GetCurrentThreadId", |m| ThunkResult::stdcall(m.current_tid(), 0));
    reg.register(DLL, "SetThreadPriority", set_thread_priority);
    reg.register(DLL, "GetThreadPriority", get_thread_priority);
    reg.register(DLL, "Sleep", sleep);
    reg.register(DLL, "SleepEx", sleep_ex);
    reg.register(DLL, "CreateEventA", create_event_a);
    reg.register(DLL, "OpenEventA", open_event_a);
    reg.register(DLL, "SetEvent", set_event);
    reg.register(DLL, "ResetEvent", reset_event);
    reg.register(DLL, "PulseEvent", pulse_event);
    reg.register(DLL, "CreateMutexA", create_mutex_a);
    reg.register(DLL, "OpenMutexA", open_mutex_a);
    reg.register(DLL, "ReleaseMutex", release_mutex);
    reg.register(DLL, "CreateSemaphoreA", create_semaphore_a);
    reg.register(DLL, "OpenSemaphoreA", open_semaphore_a);
    reg.register(DLL, "ReleaseSemaphore", release_semaphore);
    reg.register(DLL, "CreateWaitableTimerA", create_waitable_timer_a);
    reg.register(DLL, "OpenWaitableTimerA", open_waitable_timer_a);
    reg.register(DLL, "SetWaitableTimer", set_waitable_timer);
    reg.register(DLL, "CancelWaitableTimer", cancel_waitable_timer);
    reg.register(DLL, "WaitForSingleObject", wait_for_single_object);
    reg.register(DLL, "WaitForSingleObjectEx", wait_for_single_object_ex);
    reg.register(DLL, "WaitForMultipleObjects", wait_for_multiple_objects);
    reg.register(DLL, "WaitForMultipleObjectsEx", wait_for_multiple_objects_ex);
    reg.register(DLL, "InitializeCriticalSection", initialize_critical_section);
    reg.register(DLL, "InitializeCriticalSectionAndSpinCount", initialize_critical_section_spin);
    reg.register(DLL, "DeleteCriticalSection", |_m| ThunkResult::stdcall(0, 1));
    reg.register(DLL, "EnterCriticalSection", enter_critical_section);
    reg.register(DLL, "TryEnterCriticalSection", try_enter_critical_section);
    reg.register(DLL, "LeaveCriticalSection", leave_critical_section);
    reg.register(DLL, "TlsAlloc", tls_alloc);
    reg.register(DLL, "TlsFree", tls_free);
    reg.register(DLL, "TlsGetValue", tls_get_value);
    reg.register(DLL, "TlsSetValue", tls_set_value);
    reg.register(DLL, "DuplicateHandle", duplicate_handle);
    reg.register(DLL, "CloseHandle", close_handle);
    reg.register(DLL, "InterlockedIncrement", interlocked_increment);
    reg.register(DLL, "InterlockedDecrement", interlocked_decrement);
    reg.register(DLL, "InterlockedExchange", interlocked_exchange);
    reg.register(DLL, "InterlockedExchangeAdd", interlocked_exchange_add);
    reg.register(DLL, "InterlockedCompareExchange", interlocked_compare_exchange);
}

// ---------------------------------------------------------------------------------------------- threads

/// Thread slot for a thread handle (or the current-thread pseudo handle).
pub fn thread_slot(m: &Machine, h: u32) -> Option<usize> {
    if h == CURRENT_THREAD {
        return Some(m.cur);
    }
    match m.handles.get(h)? {
        KernelObject::Thread { tid } => m.thread_by_tid(*tid),
        _ => None,
    }
}

/// `CreateThread(sa, stack, start, param, flags, *tid)`.
fn create_thread(m: &mut Machine) -> ThunkResult {
    let (stack, start, param, flags, ptid) = (m.arg_u32(1), m.arg_u32(2), m.arg_u32(3), m.arg_u32(4), m.arg_u32(5));
    let size = if stack == 0 { m.process.stack_reserve } else { stack.max(0x1_0000) };
    let suspended = flags & 4 != 0; // CREATE_SUSPENDED
    match m.create_thread(start, param, size, suspended) {
        Ok(slot) => {
            let (tid, h) = (m.threads[slot].tid, m.threads[slot].handle);
            if ptid != 0 {
                m.mem.write_u32(ptid, tid);
            }
            m.log(s3_runtime::LogLevel::Info, &format!("CreateThread entry={start:#x} param={param:#x} -> tid {tid:#x} slot {slot}{}", if suspended { " (suspended)" } else { "" }));
            ThunkResult::stdcall(h, 6)
        }
        Err(e) => {
            m.log(s3_runtime::LogLevel::Warn, &format!("CreateThread failed: {e}"));
            m.set_last_error(8);
            ThunkResult::stdcall(0, 6)
        }
    }
}

fn exit_thread(m: &mut Machine) -> ThunkResult {
    ThunkResult::ExitThread(m.arg_u32(0))
}

fn resume_thread(m: &mut Machine) -> ThunkResult {
    let h = m.arg_u32(0);
    let Some(slot) = thread_slot(m, h) else {
        m.set_last_error(ERROR_INVALID_HANDLE);
        return ThunkResult::stdcall(u32::MAX, 1);
    };
    let t = &mut m.threads[slot];
    let prev = match t.state {
        ThreadState::Suspended(n) => {
            t.state = if n <= 1 { ThreadState::Runnable } else { ThreadState::Suspended(n - 1) };
            n
        }
        _ => 0,
    };
    ThunkResult::stdcall(prev, 1)
}

fn suspend_thread(m: &mut Machine) -> ThunkResult {
    let h = m.arg_u32(0);
    let Some(slot) = thread_slot(m, h) else {
        m.set_last_error(ERROR_INVALID_HANDLE);
        return ThunkResult::stdcall(u32::MAX, 1);
    };
    if slot == m.cur {
        // Suspending yourself would need to stop mid-thunk; not used by the game.
        m.log(s3_runtime::LogLevel::Warn, "SuspendThread(self) ignored");
        return ThunkResult::stdcall(0, 1);
    }
    let t = &mut m.threads[slot];
    let prev = match t.state {
        ThreadState::Suspended(n) => {
            t.state = ThreadState::Suspended(n + 1);
            n
        }
        ThreadState::Runnable => {
            t.state = ThreadState::Suspended(1);
            0
        }
        // A blocked thread stays blocked (HYPOTHESIS: good enough; the game never suspends threads).
        _ => 0,
    };
    ThunkResult::stdcall(prev, 1)
}

fn get_exit_code_thread(m: &mut Machine) -> ThunkResult {
    let (h, p) = (m.arg_u32(0), m.arg_u32(1));
    let Some(slot) = thread_slot(m, h) else {
        m.set_last_error(ERROR_INVALID_HANDLE);
        return ThunkResult::stdcall(0, 2);
    };
    let code = match m.threads[slot].state {
        ThreadState::Exited(c) => c,
        _ => STILL_ACTIVE,
    };
    m.mem.write_u32(p, code);
    ThunkResult::stdcall(1, 2)
}

fn terminate_thread(m: &mut Machine) -> ThunkResult {
    let (h, code) = (m.arg_u32(0), m.arg_u32(1));
    let Some(slot) = thread_slot(m, h) else {
        m.set_last_error(ERROR_INVALID_HANDLE);
        return ThunkResult::stdcall(0, 2);
    };
    if slot == m.cur {
        return ThunkResult::ExitThread(code);
    }
    m.exit_thread(slot, code);
    ThunkResult::stdcall(1, 2)
}

fn set_thread_priority(m: &mut Machine) -> ThunkResult {
    let (h, p) = (m.arg_u32(0), m.arg_i32(1));
    match thread_slot(m, h) {
        Some(slot) => {
            m.threads[slot].priority = p;
            ThunkResult::stdcall(1, 2)
        }
        None => {
            m.set_last_error(ERROR_INVALID_HANDLE);
            ThunkResult::stdcall(0, 2)
        }
    }
}

fn get_thread_priority(m: &mut Machine) -> ThunkResult {
    let h = m.arg_u32(0);
    match thread_slot(m, h) {
        Some(slot) => ThunkResult::stdcall(m.threads[slot].priority as u32, 1),
        None => ThunkResult::stdcall(0x7FFF_FFFF, 1),
    }
}

/// `Sleep(ms)`: always yields (also for 0); returns once `ms` host milliseconds have passed.
fn sleep(m: &mut Machine) -> ThunkResult {
    let ms = m.arg_u32(0);
    match m.thread().block_since {
        None => ThunkResult::Block,
        Some(t0) => {
            if ms == 0 || (ms != INFINITE && m.now_ms().saturating_sub(t0) >= ms as u64) {
                ThunkResult::stdcall(0, 1)
            } else {
                ThunkResult::Block
            }
        }
    }
}

fn sleep_ex(m: &mut Machine) -> ThunkResult {
    match sleep(m) {
        ThunkResult::Return { .. } => ThunkResult::stdcall(0, 2),
        r => r,
    }
}

// ---------------------------------------------------------------------------------------------- objects

/// Create (or open, if `name` exists) a named object. Sets LastError = ERROR_ALREADY_EXISTS when opened.
fn create_named(m: &mut Machine, name: Option<String>, obj: KernelObject) -> u32 {
    if let Some(n) = &name {
        if let Some(h) = m.handles.open_named(n) {
            m.set_last_error(ERROR_ALREADY_EXISTS);
            return h;
        }
    }
    m.set_last_error(0);
    m.handles.insert_named(obj, name)
}

fn open_named(m: &mut Machine, name: Option<String>, nargs: u32) -> ThunkResult {
    let h = name.and_then(|n| m.handles.open_named(&n)).unwrap_or(0);
    if h == 0 {
        m.set_last_error(ERROR_FILE_NOT_FOUND);
    }
    ThunkResult::stdcall(h, nargs)
}

/// `CreateEventA(sa, manual, initial, name)`.
fn create_event_a(m: &mut Machine) -> ThunkResult {
    let (manual, initial, name) = (m.arg_u32(1) != 0, m.arg_u32(2) != 0, m.arg_str(3));
    let h = create_named(m, name, KernelObject::Event { manual_reset: manual, signaled: initial });
    ThunkResult::stdcall(h, 4)
}

fn open_event_a(m: &mut Machine) -> ThunkResult {
    let name = m.arg_str(2);
    open_named(m, name, 3)
}

fn with_event(m: &mut Machine, f: impl FnOnce(&mut bool)) -> u32 {
    let h = m.arg_u32(0);
    match m.handles.get_mut(h) {
        Some(KernelObject::Event { signaled, .. }) => {
            f(signaled);
            1
        }
        _ => {
            m.set_last_error(ERROR_INVALID_HANDLE);
            0
        }
    }
}

fn set_event(m: &mut Machine) -> ThunkResult {
    let r = with_event(m, |s| *s = true);
    ThunkResult::stdcall(r, 1)
}

fn reset_event(m: &mut Machine) -> ThunkResult {
    let r = with_event(m, |s| *s = false);
    ThunkResult::stdcall(r, 1)
}

fn pulse_event(m: &mut Machine) -> ThunkResult {
    let h = m.arg_u32(0);
    if m.pulse_event(h) {
        ThunkResult::stdcall(1, 1)
    } else {
        m.set_last_error(ERROR_INVALID_HANDLE);
        ThunkResult::stdcall(0, 1)
    }
}

/// `CreateMutexA(sa, initial_owner, name)`.
fn create_mutex_a(m: &mut Machine) -> ThunkResult {
    let (own, name) = (m.arg_u32(1) != 0, m.arg_str(2));
    let tid = m.current_tid();
    let obj = KernelObject::Mutex { owner: own.then_some(tid), recursion: own as u32 };
    let h = create_named(m, name, obj);
    ThunkResult::stdcall(h, 3)
}

fn open_mutex_a(m: &mut Machine) -> ThunkResult {
    let name = m.arg_str(2);
    open_named(m, name, 3)
}

fn release_mutex(m: &mut Machine) -> ThunkResult {
    let h = m.arg_u32(0);
    let tid = m.current_tid();
    match m.handles.get_mut(h) {
        Some(KernelObject::Mutex { owner, recursion }) if *owner == Some(tid) => {
            *recursion = recursion.saturating_sub(1);
            if *recursion == 0 {
                *owner = None;
            }
            ThunkResult::stdcall(1, 1)
        }
        Some(KernelObject::Mutex { .. }) => {
            m.set_last_error(ERROR_NOT_OWNER);
            ThunkResult::stdcall(0, 1)
        }
        _ => {
            m.set_last_error(ERROR_INVALID_HANDLE);
            ThunkResult::stdcall(0, 1)
        }
    }
}

/// `CreateSemaphoreA(sa, initial, max, name)`.
fn create_semaphore_a(m: &mut Machine) -> ThunkResult {
    let (initial, max, name) = (m.arg_i32(1), m.arg_i32(2), m.arg_str(3));
    if max <= 0 || initial < 0 || initial > max {
        m.set_last_error(ERROR_INVALID_PARAMETER);
        return ThunkResult::stdcall(0, 4);
    }
    let h = create_named(m, name, KernelObject::Semaphore { count: initial, max });
    ThunkResult::stdcall(h, 4)
}

fn open_semaphore_a(m: &mut Machine) -> ThunkResult {
    let name = m.arg_str(2);
    open_named(m, name, 3)
}

/// `ReleaseSemaphore(h, n, *prev)`.
fn release_semaphore(m: &mut Machine) -> ThunkResult {
    let (h, n, prevp) = (m.arg_u32(0), m.arg_i32(1), m.arg_u32(2));
    let r = match m.handles.get_mut(h) {
        Some(KernelObject::Semaphore { count, max }) => {
            if n <= 0 || *count as i64 + n as i64 > *max as i64 {
                Err(ERROR_TOO_MANY_POSTS)
            } else {
                let prev = *count;
                *count += n;
                Ok(prev)
            }
        }
        _ => Err(ERROR_INVALID_HANDLE),
    };
    match r {
        Ok(prev) => {
            if prevp != 0 {
                m.mem.write_u32(prevp, prev as u32);
            }
            ThunkResult::stdcall(1, 3)
        }
        Err(e) => {
            m.set_last_error(e);
            ThunkResult::stdcall(0, 3)
        }
    }
}

// ---------------------------------------------------------------------------------------------- waitable timers

/// `CreateWaitableTimerA(sa, manual_reset, name)`.
fn create_waitable_timer_a(m: &mut Machine) -> ThunkResult {
    let (manual, name) = (m.arg_u32(1) != 0, m.arg_str(2));
    let h = create_named(m, name, KernelObject::Timer { manual_reset: manual, signaled: false, due_ms: None, period_ms: 0 });
    ThunkResult::stdcall(h, 3)
}

fn open_waitable_timer_a(m: &mut Machine) -> ThunkResult {
    let name = m.arg_str(2);
    open_named(m, name, 3)
}

/// `SetWaitableTimer(h, *due (LARGE_INTEGER, 100 ns; <0 relative, >=0 absolute FILETIME), period_ms,
/// completion_routine, arg, resume)`. Completion routines (APCs) are not supported (logged).
fn set_waitable_timer(m: &mut Machine) -> ThunkResult {
    let (h, pdue, period, apc) = (m.arg_u32(0), m.arg_u32(1), m.arg_i32(2), m.arg_u32(3));
    let due = m.mem.read_u64(pdue) as i64;
    let now = m.now_ms();
    let delay_ms = if due < 0 {
        (-due) as u64 / 10_000
    } else {
        let now_ft = s3_runtime::hosttime::now_filetime() as i64;
        (due - now_ft).max(0) as u64 / 10_000
    };
    if apc != 0 {
        m.log(s3_runtime::LogLevel::Warn, "SetWaitableTimer: completion routine ignored");
    }
    match m.handles.get_mut(h) {
        Some(KernelObject::Timer { signaled, due_ms, period_ms, .. }) => {
            *signaled = false;
            *due_ms = Some(now + delay_ms);
            *period_ms = period.max(0) as u32;
            ThunkResult::stdcall(1, 6)
        }
        _ => {
            m.set_last_error(ERROR_INVALID_HANDLE);
            ThunkResult::stdcall(0, 6)
        }
    }
}

fn cancel_waitable_timer(m: &mut Machine) -> ThunkResult {
    let h = m.arg_u32(0);
    match m.handles.get_mut(h) {
        Some(KernelObject::Timer { due_ms, .. }) => {
            *due_ms = None;
            ThunkResult::stdcall(1, 1)
        }
        _ => {
            m.set_last_error(ERROR_INVALID_HANDLE);
            ThunkResult::stdcall(0, 1)
        }
    }
}

// ---------------------------------------------------------------------------------------------- waits

fn wait_result(m: &mut Machine, r: WaitPoll, nargs: u32) -> ThunkResult {
    match r {
        WaitPoll::Ready(i) => ThunkResult::stdcall(WAIT_OBJECT_0 + i, nargs),
        WaitPoll::Timeout => ThunkResult::stdcall(WAIT_TIMEOUT, nargs),
        WaitPoll::Failed => {
            m.set_last_error(ERROR_INVALID_HANDLE);
            ThunkResult::stdcall(WAIT_FAILED, nargs)
        }
        WaitPoll::Pending => ThunkResult::Block,
    }
}

fn wait_single(m: &mut Machine, nargs: u32) -> ThunkResult {
    let (h, ms) = (m.arg_u32(0), m.arg_u32(1));
    let h = if h == CURRENT_THREAD { m.threads[m.cur].handle } else { h };
    let r = m.poll_wait(&[h], false, ms);
    wait_result(m, r, nargs)
}

fn wait_for_single_object(m: &mut Machine) -> ThunkResult {
    wait_single(m, 2)
}

fn wait_for_single_object_ex(m: &mut Machine) -> ThunkResult {
    wait_single(m, 3)
}

fn wait_multiple(m: &mut Machine, nargs: u32) -> ThunkResult {
    let (n, p, all, ms) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2) != 0, m.arg_u32(3));
    if n == 0 || n > MAXIMUM_WAIT_OBJECTS {
        m.set_last_error(ERROR_INVALID_PARAMETER);
        return ThunkResult::stdcall(WAIT_FAILED, nargs);
    }
    let hs: Vec<u32> = (0..n).map(|i| m.mem.read_u32(p + 4 * i)).collect();
    let r = m.poll_wait(&hs, all, ms);
    wait_result(m, r, nargs)
}

fn wait_for_multiple_objects(m: &mut Machine) -> ThunkResult {
    wait_multiple(m, 4)
}

fn wait_for_multiple_objects_ex(m: &mut Machine) -> ThunkResult {
    wait_multiple(m, 5)
}

// ---------------------------------------------------------------------------------------------- critical sections

const CS_LOCK_COUNT: u32 = 4;
const CS_RECURSION: u32 = 8;
const CS_OWNER: u32 = 12;

fn init_cs(m: &mut Machine, cs: u32, spin: u32) {
    m.mem.write_u32(cs, 0);
    m.mem.write_u32(cs + CS_LOCK_COUNT, u32::MAX);
    m.mem.write_u32(cs + CS_RECURSION, 0);
    m.mem.write_u32(cs + CS_OWNER, 0);
    m.mem.write_u32(cs + 16, 0);
    m.mem.write_u32(cs + 20, spin);
}

fn initialize_critical_section(m: &mut Machine) -> ThunkResult {
    let cs = m.arg_u32(0);
    init_cs(m, cs, 0);
    ThunkResult::stdcall(0, 1)
}

fn initialize_critical_section_spin(m: &mut Machine) -> ThunkResult {
    let (cs, spin) = (m.arg_u32(0), m.arg_u32(1));
    init_cs(m, cs, spin);
    ThunkResult::stdcall(1, 2)
}

/// Try to take `cs` for the running thread. Returns true on success.
fn cs_try_take(m: &mut Machine, cs: u32) -> bool {
    let tid = m.current_tid();
    let slot = m.cur;
    if m.threads[slot].cs_granted && m.threads[slot].wait_cs == cs {
        // Ownership was handed over by LeaveCriticalSection (owner/recursion already written).
        m.threads[slot].cs_granted = false;
        m.threads[slot].wait_cs = 0;
        return true;
    }
    let owner = m.mem.read_u32(cs + CS_OWNER);
    if owner == 0 {
        m.mem.write_u32(cs + CS_OWNER, tid);
        m.mem.write_u32(cs + CS_RECURSION, 1);
        m.mem.write_u32(cs + CS_LOCK_COUNT, 0);
        m.threads[slot].wait_cs = 0;
        return true;
    }
    if owner == tid {
        let r = m.mem.read_u32(cs + CS_RECURSION);
        m.mem.write_u32(cs + CS_RECURSION, r + 1);
        let l = m.mem.read_u32(cs + CS_LOCK_COUNT);
        m.mem.write_u32(cs + CS_LOCK_COUNT, l.wrapping_add(1));
        return true;
    }
    // Owner thread gone (exited while holding it)? Take it over instead of deadlocking.
    let owner_alive = m
        .threads
        .iter()
        .any(|t| t.tid == owner && !matches!(t.state, ThreadState::Exited(_)));
    if !owner_alive {
        m.log(s3_runtime::LogLevel::Warn, &format!("critical section {cs:#x}: owner {owner:#x} is gone; taking it"));
        m.mem.write_u32(cs + CS_OWNER, tid);
        m.mem.write_u32(cs + CS_RECURSION, 1);
        m.mem.write_u32(cs + CS_LOCK_COUNT, 0);
        m.threads[slot].wait_cs = 0;
        return true;
    }
    false
}

fn enter_critical_section(m: &mut Machine) -> ThunkResult {
    let cs = m.arg_u32(0);
    if cs_try_take(m, cs) {
        return ThunkResult::stdcall(0, 1);
    }
    m.thread().wait_cs = cs;
    ThunkResult::Block
}

fn try_enter_critical_section(m: &mut Machine) -> ThunkResult {
    let cs = m.arg_u32(0);
    let ok = cs_try_take(m, cs);
    ThunkResult::stdcall(ok as u32, 1)
}

fn leave_critical_section(m: &mut Machine) -> ThunkResult {
    let cs = m.arg_u32(0);
    let tid = m.current_tid();
    if m.mem.read_u32(cs + CS_OWNER) != tid {
        return ThunkResult::stdcall(0, 1);
    }
    let r = m.mem.read_u32(cs + CS_RECURSION).saturating_sub(1);
    if r > 0 {
        m.mem.write_u32(cs + CS_RECURSION, r);
        let l = m.mem.read_u32(cs + CS_LOCK_COUNT);
        m.mem.write_u32(cs + CS_LOCK_COUNT, l.wrapping_sub(1));
        return ThunkResult::stdcall(0, 1);
    }
    // Hand over to the next blocked waiter (search after the current slot for fairness).
    let n = m.threads.len();
    let cur = m.cur;
    let next = (1..=n).map(|k| (cur + k) % n).find(|&s| {
        let t = &m.threads[s];
        s != cur && t.state == ThreadState::Blocked && t.wait_cs == cs && !t.cs_granted
    });
    match next {
        Some(s) => {
            let ntid = m.threads[s].tid;
            m.threads[s].cs_granted = true;
            m.mem.write_u32(cs + CS_OWNER, ntid);
            m.mem.write_u32(cs + CS_RECURSION, 1);
            m.mem.write_u32(cs + CS_LOCK_COUNT, 0);
        }
        None => {
            m.mem.write_u32(cs + CS_OWNER, 0);
            m.mem.write_u32(cs + CS_RECURSION, 0);
            m.mem.write_u32(cs + CS_LOCK_COUNT, u32::MAX);
        }
    }
    ThunkResult::stdcall(0, 1)
}

// ---------------------------------------------------------------------------------------------- TLS

#[derive(Default)]
struct TlsState {
    used: u64,
}

fn tls_alloc(m: &mut Machine) -> ThunkResult {
    let st = m.state::<TlsState>();
    let idx = {
        let mut s = st.borrow_mut();
        let free = (0..teb::TLS_SLOT_COUNT).find(|i| s.used & (1u64 << i) == 0);
        if let Some(i) = free {
            s.used |= 1u64 << i;
        }
        free
    };
    match idx {
        Some(i) => {
            // New slots read as 0 in every thread.
            let tebs: Vec<u32> = m.threads.iter().map(|t| t.teb).collect();
            for t in tebs {
                m.mem.write_u32(t + teb::TLS_SLOTS + 4 * i, 0);
            }
            ThunkResult::stdcall(i, 0)
        }
        None => {
            m.set_last_error(8);
            ThunkResult::stdcall(0xFFFF_FFFF, 0)
        }
    }
}

fn tls_free(m: &mut Machine) -> ThunkResult {
    let i = m.arg_u32(0);
    if i >= teb::TLS_SLOT_COUNT {
        m.set_last_error(ERROR_INVALID_PARAMETER);
        return ThunkResult::stdcall(0, 1);
    }
    let st = m.state::<TlsState>();
    st.borrow_mut().used &= !(1u64 << i);
    ThunkResult::stdcall(1, 1)
}

fn tls_get_value(m: &mut Machine) -> ThunkResult {
    let i = m.arg_u32(0);
    if i >= teb::TLS_SLOT_COUNT {
        m.set_last_error(ERROR_INVALID_PARAMETER);
        return ThunkResult::stdcall(0, 1);
    }
    let t = m.threads[m.cur].teb;
    let v = m.mem.read_u32(t + teb::TLS_SLOTS + 4 * i);
    m.set_last_error(0);
    ThunkResult::stdcall(v, 1)
}

fn tls_set_value(m: &mut Machine) -> ThunkResult {
    let (i, v) = (m.arg_u32(0), m.arg_u32(1));
    if i >= teb::TLS_SLOT_COUNT {
        m.set_last_error(ERROR_INVALID_PARAMETER);
        return ThunkResult::stdcall(0, 2);
    }
    let t = m.threads[m.cur].teb;
    m.mem.write_u32(t + teb::TLS_SLOTS + 4 * i, v);
    ThunkResult::stdcall(1, 2)
}

// ---------------------------------------------------------------------------------------------- handles

/// `DuplicateHandle(src_proc, src, dst_proc, *dst, access, inherit, options)`.
fn duplicate_handle(m: &mut Machine) -> ThunkResult {
    let (src, pdst, opts) = (m.arg_u32(1), m.arg_u32(3), m.arg_u32(6));
    let tid = m.current_tid();
    let h = match src {
        CURRENT_THREAD => Some(m.handles.insert(KernelObject::Thread { tid })),
        CURRENT_PROCESS => Some(m.handles.insert(KernelObject::Process)),
        h => m.handles.duplicate(h),
    };
    let Some(h) = h else {
        m.set_last_error(ERROR_INVALID_HANDLE);
        return ThunkResult::stdcall(0, 7);
    };
    if pdst != 0 {
        m.mem.write_u32(pdst, h);
    }
    if opts & 1 != 0 {
        // DUPLICATE_CLOSE_SOURCE
        let _ = m.handles.close(src);
    }
    ThunkResult::stdcall(1, 7)
}

fn close_handle(m: &mut Machine) -> ThunkResult {
    let h = m.arg_u32(0);
    if h == CURRENT_PROCESS || h == CURRENT_THREAD {
        return ThunkResult::stdcall(1, 1);
    }
    if super::kernel32_file::is_std_handle(h) {
        return ThunkResult::stdcall(1, 1);
    }
    match m.handles.close(h) {
        Ok(_) => ThunkResult::stdcall(1, 1),
        Err(()) => {
            m.set_last_error(ERROR_INVALID_HANDLE);
            ThunkResult::stdcall(0, 1)
        }
    }
}

// ---------------------------------------------------------------------------------------------- interlocked

fn interlocked_increment(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    let v = m.mem.read_u32(p).wrapping_add(1);
    m.mem.write_u32(p, v);
    ThunkResult::stdcall(v, 1)
}

fn interlocked_decrement(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    let v = m.mem.read_u32(p).wrapping_sub(1);
    m.mem.write_u32(p, v);
    ThunkResult::stdcall(v, 1)
}

fn interlocked_exchange(m: &mut Machine) -> ThunkResult {
    let (p, v) = (m.arg_u32(0), m.arg_u32(1));
    let old = m.mem.read_u32(p);
    m.mem.write_u32(p, v);
    ThunkResult::stdcall(old, 2)
}

fn interlocked_exchange_add(m: &mut Machine) -> ThunkResult {
    let (p, v) = (m.arg_u32(0), m.arg_u32(1));
    let old = m.mem.read_u32(p);
    m.mem.write_u32(p, old.wrapping_add(v));
    ThunkResult::stdcall(old, 2)
}

fn interlocked_compare_exchange(m: &mut Machine) -> ThunkResult {
    let (p, x, c) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    let old = m.mem.read_u32(p);
    if old == c {
        m.mem.write_u32(p, x);
    }
    ThunkResult::stdcall(old, 3)
}
