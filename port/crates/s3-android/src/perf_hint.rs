//! CPU performance hint for the engine thread (docs/porting/performance.md, perf-r2 item 3).
//!
//! The engine thread is CPU bound, but the governor kept the prime core at ~2 GHz average of 3.19 GHz (simpleperf,
//! 2026-11-05). `APerformanceHint` (NDK, API 33+) lets an app tell the system how long its work should take per
//! frame and how long it really took; when it is late, the system raises the clock / moves the thread to a faster
//! core. Looked up with dlsym (minSdk is 26). Off elsewhere (no-op).
//!
//! Work per frame = engine-thread CPU time of a slice / presents in that slice; target = 1000 / fps cap ms.

#[cfg(target_os = "android")]
mod imp {
    use std::ffi::c_void;

    extern "C" {
        fn dlopen(name: *const std::ffi::c_char, flags: i32) -> *mut c_void;
        fn dlsym(h: *mut c_void, name: *const std::ffi::c_char) -> *mut c_void;
        fn gettid() -> i32;
        fn clock_gettime(clk: i32, ts: *mut [i64; 2]) -> i32;
    }

    const CLOCK_THREAD_CPUTIME_ID: i32 = 3;

    type GetManager = unsafe extern "C" fn() -> *mut c_void;
    type CreateSession = unsafe extern "C" fn(*mut c_void, *const i32, usize, i64) -> *mut c_void;
    type Report = unsafe extern "C" fn(*mut c_void, i64) -> i32;
    type UpdateTarget = unsafe extern "C" fn(*mut c_void, i64) -> i32;

    pub struct EngineHint {
        session: *mut c_void,
        report: Option<Report>,
        update: Option<UpdateTarget>,
        target_ns: i64,
        last_cpu_ns: i64,
        reports: u64,
    }

    // The session is only used from the engine thread that created it.
    unsafe impl Send for EngineHint {}

    fn thread_cpu_ns() -> i64 {
        let mut ts = [0i64; 2];
        unsafe { clock_gettime(CLOCK_THREAD_CPUTIME_ID, &mut ts) };
        ts[0] * 1_000_000_000 + ts[1]
    }

    impl EngineHint {
        /// Create a session for the calling thread (call on the engine thread).
        pub fn new(target_ns: i64) -> EngineHint {
            let mut h = EngineHint { session: std::ptr::null_mut(), report: None, update: None, target_ns, last_cpu_ns: thread_cpu_ns(), reports: 0 };
            unsafe {
                let lib = dlopen(c"libandroid.so".as_ptr(), 0);
                if lib.is_null() {
                    return h;
                }
                let gm = dlsym(lib, c"APerformanceHint_getManager".as_ptr());
                let cs = dlsym(lib, c"APerformanceHint_createSession".as_ptr());
                let rp = dlsym(lib, c"APerformanceHint_reportActualWorkDuration".as_ptr());
                let ut = dlsym(lib, c"APerformanceHint_updateTargetWorkDuration".as_ptr());
                if gm.is_null() || cs.is_null() || rp.is_null() {
                    log::info!("perf hint: APerformanceHint unavailable (API < 33)");
                    return h;
                }
                let gm: GetManager = std::mem::transmute(gm);
                let cs: CreateSession = std::mem::transmute(cs);
                let mgr = gm();
                if mgr.is_null() {
                    log::info!("perf hint: no manager");
                    return h;
                }
                let tid = gettid();
                h.session = cs(mgr, &tid, 1, target_ns);
                h.report = Some(std::mem::transmute::<*mut c_void, Report>(rp));
                h.update = (!ut.is_null()).then(|| std::mem::transmute::<*mut c_void, UpdateTarget>(ut));
                log::info!("perf hint: session {:?} for tid {tid}, target {} us", h.session, target_ns / 1000);
            }
            h
        }

        /// After one engine slice: `presents` = frames presented during it.
        pub fn slice_done(&mut self, presents: u64, target_ns: i64) {
            if self.session.is_null() {
                return;
            }
            let now = thread_cpu_ns();
            let cpu = now - self.last_cpu_ns;
            self.last_cpu_ns = now;
            if target_ns != self.target_ns && target_ns > 0 {
                self.target_ns = target_ns;
                if let Some(u) = self.update {
                    unsafe { u(self.session, target_ns) };
                }
            }
            // No frame in this slice (loading, menus between presents): report the slice itself.
            let work = if presents > 0 { cpu / presents as i64 } else { cpu };
            if work > 0 {
                if let Some(r) = self.report {
                    let rc = unsafe { r(self.session, work) };
                    self.reports += 1;
                    if self.reports == 1 || self.reports % 3000 == 0 {
                        log::info!("perf hint: report #{} work {} us (target {} us) -> {rc}", self.reports, work / 1000, self.target_ns / 1000);
                    }
                }
            }
        }
    }
}

#[cfg(not(target_os = "android"))]
mod imp {
    /// No-op off Android.
    pub struct EngineHint;

    impl EngineHint {
        pub fn new(_target_ns: i64) -> EngineHint {
            EngineHint
        }
        pub fn slice_done(&mut self, _presents: u64, _target_ns: i64) {}
    }
}

pub use imp::EngineHint;

/// Target work duration per frame for an fps cap (0 = uncapped -> 60 Hz budget).
pub fn target_ns(fps_cap: u32) -> i64 {
    1_000_000_000 / fps_cap.max(1) as i64 * (fps_cap > 0) as i64 + (fps_cap == 0) as i64 * 16_666_667
}

#[cfg(test)]
mod tests {
    #[test]
    fn target_for_caps() {
        assert_eq!(super::target_ns(60), 16_666_666);
        assert_eq!(super::target_ns(0), 16_666_667);
        assert_eq!(super::target_ns(120), 8_333_333);
    }
}
