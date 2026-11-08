//! Platform-independent part of the Android frontend: state shared between the activity (UI) thread and the
//! engine thread, the [`Host`] implementation, and the engine thread body. Compiles and is tested on Linux.
//!
//! Threads:
//! - activity thread (`android_main`): lifecycle, input (touch -> [`HostEvent`]s into `Shared::events`), draws the
//!   newest frame (or the status screen) into the ANativeWindow, owns the AAudio stream;
//! - engine thread: [`run_engine`] — waits for S3.EXE, builds the [`Engine`], runs it in slices while not paused.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[cfg(feature = "engine")]
use s3_engine::Engine;
#[cfg(feature = "engine")]
use s3_runtime::SliceResult;
use s3_runtime::{FrameBuffer, Host, HostEvent, LogLevel, Vfs};

use s3_mods::display::{DisplaySettings, Viewport};
use s3_mods::SceneSlot;

use crate::audio::{AudioRing, Resampler, OUT_RATE};

/// What the status screen shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Starting,
    MissingExe,
    Running,
    Stopped,
}

#[derive(Debug, Clone)]
pub struct Status {
    pub phase: Phase,
    pub title: String,
    pub body: String,
    /// Bumped on every change (the renderer redraws when it changes).
    pub seq: u64,
}

#[derive(Default)]
pub struct FrameSlot {
    pub fb: Option<FrameBuffer>,
    pub seq: u64,
}

/// State shared by the activity thread and the engine thread.
pub struct Shared {
    pub frame: Mutex<FrameSlot>,
    pub events: Mutex<Vec<HostEvent>>,
    pub status: Mutex<Status>,
    pub paused: AtomicBool,
    pub quit: AtomicBool,
    /// Engine thread sleeps here while paused / waiting for files.
    pub park: (Mutex<()>, Condvar),
    pub audio: Arc<AudioRing>,
    /// Wakes the activity thread's event loop (new frame / status).
    pub wake: Box<dyn Fn() + Send + Sync>,
    start: Instant,
    /// Total ms spent paused; and the ms timestamp when the current pause began (0 = not paused).
    paused_total: AtomicU64,
    paused_since: AtomicU64,
    /// Mouse/button events delivered to the engine (diagnostics, shown in the log).
    pub delivered: AtomicU64,
    /// The game ended itself (ExitProcess, e.g. "Exit Game" or Esc in the main menu): the app should close.
    pub guest_exited: AtomicBool,
    /// Engine -> UI: a three-finger tap asked for the soft keyboard.
    pub keyboard_req: AtomicBool,
    /// Signalled (with `frame` locked) on every new frame / status / display change: the render thread waits here.
    pub frame_cv: Condvar,
    /// Engine -> presenter: display settings (`s3_mods::display`), bumped `display_seq` on change.
    pub display: Mutex<DisplaySettings>,
    pub display_seq: AtomicU64,
    /// Presenter -> UI (touch mapping) and engine (mods): viewport of the shown frame, `viewport_seq` on change.
    pub viewport: Mutex<Option<Viewport>>,
    pub viewport_seq: AtomicU64,
    /// Presenter -> UI: CPU fallback letterbox (only when EGL failed; touch mapping uses it).
    pub letterbox: Mutex<Option<crate::layout::Letterbox>>,
    /// Presenter -> engine: display presents per second and the panel refresh rate (f32 bits; 0 = unknown).
    pub host_fps: AtomicU32,
    pub refresh_hz: AtomicU32,
    /// Engine (mods) -> presenter: the window-resolution overlay layer (`s3_mods::layer`); publishing wakes the
    /// render thread.
    pub layer: Arc<SceneSlot>,
}

impl Shared {
    pub fn new(wake: Box<dyn Fn() + Send + Sync>) -> Arc<Shared> {
        Arc::new_cyclic(|weak: &std::sync::Weak<Shared>| {
            let w = weak.clone();
            let layer = SceneSlot::new(Some(Box::new(move || {
                if let Some(s) = w.upgrade() {
                    s.notify_render();
                }
            })));
            Shared::with_layer(wake, layer)
        })
    }

    fn with_layer(wake: Box<dyn Fn() + Send + Sync>, layer: Arc<SceneSlot>) -> Shared {
        Shared {
            frame: Mutex::new(FrameSlot::default()),
            events: Mutex::new(Vec::new()),
            status: Mutex::new(Status { phase: Phase::Starting, title: "Starting".into(), body: String::new(), seq: 1 }),
            paused: AtomicBool::new(false),
            quit: AtomicBool::new(false),
            park: (Mutex::new(()), Condvar::new()),
            audio: Arc::new(AudioRing::new(OUT_RATE as usize)), // 1 s
            wake,
            start: Instant::now(),
            paused_total: AtomicU64::new(0),
            paused_since: AtomicU64::new(0),
            delivered: AtomicU64::new(0),
            guest_exited: AtomicBool::new(false),
            keyboard_req: AtomicBool::new(false),
            frame_cv: Condvar::new(),
            display: Mutex::new(DisplaySettings::default()),
            display_seq: AtomicU64::new(0),
            viewport: Mutex::new(None),
            viewport_seq: AtomicU64::new(0),
            letterbox: Mutex::new(None),
            host_fps: AtomicU32::new(0),
            refresh_hz: AtomicU32::new(0),
            layer,
        }
    }

    /// Wake the render thread (new frame, status, display settings or button state).
    pub fn notify_render(&self) {
        let _g = self.frame.lock().unwrap();
        self.frame_cv.notify_all();
    }

    /// Engine side: publish display settings if they changed.
    pub fn set_display(&self, d: DisplaySettings) {
        let changed = {
            let mut cur = self.display.lock().unwrap();
            if *cur != d {
                *cur = d;
                true
            } else {
                false
            }
        };
        if changed {
            self.display_seq.fetch_add(1, Ordering::AcqRel);
            self.notify_render();
        }
    }

    /// Presenter side: publish the viewport of the shown frame.
    pub fn set_viewport(&self, vp: Option<Viewport>) {
        let mut cur = self.viewport.lock().unwrap();
        if *cur != vp {
            *cur = vp;
            self.viewport_seq.fetch_add(1, Ordering::AcqRel);
        }
    }

    fn raw_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    /// Guest clock: wall time minus time spent paused (so the game does not jump after resume).
    pub fn clock_ms(&self) -> u64 {
        let since = self.paused_since.load(Ordering::Acquire);
        let now = if since != 0 { since } else { self.raw_ms() };
        now - self.paused_total.load(Ordering::Acquire)
    }

    pub fn set_status(&self, phase: Phase, title: &str, body: &str) {
        {
            let mut s = self.status.lock().unwrap();
            s.phase = phase;
            s.title = title.to_string();
            s.body = body.to_string();
            s.seq += 1;
        }
        self.notify_render();
        (self.wake)();
    }

    pub fn push_event(&self, ev: HostEvent) {
        self.events.lock().unwrap().push(ev);
    }

    pub fn set_paused(&self, p: bool) {
        if self.paused.swap(p, Ordering::AcqRel) == p {
            return;
        }
        if p {
            self.paused_since.store(self.raw_ms().max(1), Ordering::Release);
            self.push_event(HostEvent::Pause);
        } else {
            let since = self.paused_since.swap(0, Ordering::AcqRel);
            if since != 0 {
                self.paused_total.fetch_add(self.raw_ms().saturating_sub(since), Ordering::AcqRel);
            }
            self.push_event(HostEvent::Resume);
        }
        self.unpark();
    }

    pub fn request_quit(&self) {
        self.quit.store(true, Ordering::Release);
        self.push_event(HostEvent::Quit);
        self.unpark();
    }

    pub fn unpark(&self) {
        let _g = self.park.0.lock().unwrap();
        self.park.1.notify_all();
    }

    /// Sleep up to `ms` or until unparked.
    pub fn park_for(&self, ms: u64) {
        let g = self.park.0.lock().unwrap();
        let _ = self.park.1.wait_timeout(g, Duration::from_millis(ms)).unwrap();
    }
}

/// The [`Host`] the engine sees.
pub struct AndroidHost {
    pub shared: Arc<Shared>,
    pub game_dir: PathBuf,
    pub data_dir: PathBuf,
    resampler: Resampler,
    logged_audio: bool,
}

impl AndroidHost {
    pub fn new(shared: Arc<Shared>, game_dir: PathBuf, data_dir: PathBuf) -> Self {
        AndroidHost { shared, game_dir, data_dir, resampler: Resampler::default(), logged_audio: false }
    }
}

impl Host for AndroidHost {
    fn present(&mut self, fb: &FrameBuffer) {
        {
            let mut slot = self.shared.frame.lock().unwrap();
            match &mut slot.fb {
                Some(dst) if dst.width == fb.width && dst.height == fb.height => dst.pixels.copy_from_slice(&fb.pixels),
                _ => {
                    log::info!("present: new frame size {}x{}", fb.width, fb.height);
                    slot.fb = Some(fb.clone());
                }
            }
            slot.seq += 1;
            self.shared.frame_cv.notify_all();
        }
    }

    fn poll_events(&mut self) -> Vec<HostEvent> {
        let evs = std::mem::take(&mut *self.shared.events.lock().unwrap());
        for e in &evs {
            match e {
                HostEvent::MouseMove { .. } | HostEvent::Pointer { phase: s3_runtime::PointerPhase::Move, .. } => {
                    log::debug!("engine <- {e:?}")
                }
                _ => log::info!("engine <- {e:?}"),
            }
        }
        self.shared.delivered.fetch_add(evs.len() as u64, Ordering::Relaxed);
        evs
    }

    fn queue_audio(&mut self, samples: &[i16], rate: u32, channels: u16) {
        if !self.logged_audio {
            log::info!("audio: first queue_audio rate={rate} channels={channels} samples={}", samples.len());
            self.logged_audio = true;
        }
        let out = self.resampler.convert(samples, rate, channels);
        self.shared.audio.push(out);
    }

    fn audio_queued_frames(&self) -> usize {
        let q = self.shared.audio.queued_frames();
        let r = self.resampler.last_rate.max(1) as usize;
        q * r / OUT_RATE as usize
    }

    fn now_ms(&self) -> u64 {
        self.shared.clock_ms()
    }

    fn game_dir(&self) -> PathBuf {
        self.game_dir.clone()
    }

    fn data_dir(&self) -> PathBuf {
        self.data_dir.clone()
    }

    fn log(&self, level: LogLevel, msg: &str) {
        let l = match level {
            LogLevel::Error => log::Level::Error,
            LogLevel::Warn => log::Level::Warn,
            LogLevel::Info => log::Level::Info,
            LogLevel::Debug => log::Level::Debug,
            LogLevel::Trace => log::Level::Trace,
        };
        log::log!(l, "{msg}");
    }

    fn idle(&mut self, ms: u32) {
        self.shared.park_for(ms as u64);
    }
}

/// Parts of an install the engine needs before it can start (case-insensitive names).
pub const REQUIRED_PARTS: [&str; 4] = ["S3.EXE", "GFX", "SND", "Map"];

/// Required parts missing from `game_dir`.
pub fn missing_parts(game_dir: &Path) -> Vec<&'static str> {
    REQUIRED_PARTS.iter().copied().filter(|p| Vfs::lookup_ci(game_dir, &[p.to_string()]).is_none()).collect()
}

/// The path as the user sees it in a file manager / MTP ("Android/data/<pkg>/files/game/").
pub fn user_visible_path(game_dir: &Path) -> String {
    let s = game_dir.display().to_string();
    match s.find("/Android/data/") {
        Some(i) => format!("{}/", &s[i + 1..]),
        None => format!("{s}/"),
    }
}

/// First-run text (status screen body). Lines starting with "> " are drawn highlighted.
pub fn missing_exe_text(game_dir: &Path) -> String {
    let missing = missing_parts(game_dir);
    format!(
        "Copy your own Settlers III installation (GOG / Gold / Ultimate) to this folder on the phone:\n\
         > {}\n\
         full path: {}\n\
         \n\
         Copy the CONTENTS of the install folder (the folder that contains S3.EXE), so that the folder above \
         holds S3.EXE, GFX, SND, MUSIC, Map, S3 ...\n\
         Still missing: {}\n\
         \n\
         From a PC: adb push \"<GOG folder>/.\" /sdcard/{}\n\
         (Android 11+ file managers often cannot open Android/data; use adb or a USB/MTP connection.)\n\
         \n\
         No game files are included in this app. It checks again every 2 seconds and starts by itself.",
        user_visible_path(game_dir),
        game_dir.display(),
        if missing.is_empty() { "nothing".to_string() } else { missing.join(", ") },
        user_visible_path(game_dir),
    )
}

fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "(non-string panic)".into()
    }
}

/// Engine thread body. Returns when `shared.quit` is set.
pub fn run_engine(shared: Arc<Shared>, game_dir: PathBuf, data_dir: PathBuf) {
    let _ = std::fs::create_dir_all(&game_dir);
    let _ = std::fs::create_dir_all(&data_dir);
    log::info!("engine thread: game dir {} data dir {}", game_dir.display(), data_dir.display());
    // Multiplayer relay (docs/porting/multiplayer.md): `<data dir>/relay-url.txt` (one line, e.g.
    // `ws://10.0.2.2:7787/` for a relay on the emulator's host) overrides the default `ws://127.0.0.1:7777/`.
    if let Ok(s) = std::fs::read_to_string(data_dir.join("relay-url.txt")) {
        let url = s.trim();
        if !url.is_empty() {
            log::info!("multiplayer relay: {url} (from relay-url.txt)");
            std::env::set_var("S3_RELAY_URL", url);
        }
    }
    // Optional AArch64 JIT (docs/porting/cpu.md, default off): `<data dir>/jit.txt` = `1` switches it on.
    if let Ok(s) = std::fs::read_to_string(data_dir.join("jit.txt")) {
        let on = s.trim() == "1";
        s3_runtime::cpu::set_jit_enabled(on);
        log::info!("jit: {} (from jit.txt; built in: {})", on, s3_runtime::cpu::jit_available());
    }
    // Dev switch: `<data dir>/fps-cap.txt` = frame cap (0 = unlimited, for benchmarks; default 60).
    #[cfg(feature = "engine")]
    if let Some(cap) = std::fs::read_to_string(data_dir.join("fps-cap.txt")).ok().and_then(|s| s.trim().parse::<u32>().ok()) {
        s3_dx::ddraw::set_fps_cap(cap);
        // Wins over the in-game FPS limit setting (s3_mods::display::apply_fps_limit skips when it is set).
        std::env::set_var("S3_FPS_CAP", cap.to_string());
        log::info!("fps cap: {cap} (from fps-cap.txt)");
    }

    // 1. Wait for the game files.
    let mut announced: Option<Vec<&str>> = None;
    loop {
        if shared.quit.load(Ordering::Acquire) {
            return;
        }
        let missing = missing_parts(&game_dir);
        if missing.is_empty() {
            if announced.is_some() {
                // Files are arriving (adb push): give the copy a moment to finish.
                shared.set_status(Phase::Starting, "Game files found", "Starting in a few seconds ...");
                shared.park_for(3000);
            }
            break;
        }
        if announced.as_ref() != Some(&missing) {
            log::warn!("game files missing in {}: {}", game_dir.display(), missing.join(", "));
            shared.set_status(Phase::MissingExe, "Game files needed", &missing_exe_text(&game_dir));
            announced = Some(missing);
        }
        shared.park_for(2000);
    }

    // 2. Build and run the engine; never let a panic kill the app silently.
    shared.set_status(Phase::Starting, "Starting engine", &format!("Loading S3.EXE from {}", game_dir.display()));
    let host = AndroidHost::new(shared.clone(), game_dir.clone(), data_dir.clone());
    let sh = shared.clone();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || drive(sh, host)));
    let (title, body) = match result {
        Ok(Ok(summary)) => {
            // Stop reason first (it is the last line of the summary).
            let stop = summary.lines().find(|l| l.starts_with("stop:")).unwrap_or("stop: ?").to_string();
            if stop.contains("process exited") {
                log::info!("engine: the game exited ({stop}); closing the app");
                shared.guest_exited.store(true, Ordering::Release);
                (shared.wake)();
            }
            ("Engine stopped".to_string(), format!("{stop}\n{summary}"))
        }
        Ok(Err(e)) => ("Engine failed to start".to_string(), format!("error: {e}")),
        Err(p) => ("Engine panicked".to_string(), format!("panic: {}", panic_text(&*p))),
    };
    if shared.quit.load(Ordering::Acquire) {
        return;
    }
    for line in body.lines() {
        log::warn!("{title}: {line}");
    }
    shared.set_status(Phase::Stopped, &title, &body);
    while !shared.quit.load(Ordering::Acquire) {
        shared.park_for(1000);
    }
}

/// `S3.EXE` in the game dir (case-insensitive).
pub fn find_exe(game_dir: &Path) -> Option<PathBuf> {
    Vfs::lookup_ci(game_dir, &["S3.EXE".to_string()])
}

/// Shell-only build: no engine compiled in.
#[cfg(not(feature = "engine"))]
fn drive(shared: Arc<Shared>, _host: AndroidHost) -> Result<String, String> {
    shared.set_status(Phase::Stopped, "Shell build", "This build has no engine (built without the `engine` feature).");
    Err("built without the `engine` feature".into())
}

/// Run the engine until it stops or the app quits. `Ok(summary)` when the guest stopped.
#[cfg(feature = "engine")]
fn drive(shared: Arc<Shared>, host: AndroidHost) -> Result<String, String> {
    let mut engine = Engine::new(Box::new(host), None)?;
    // Mods draw their UI into the window layer; the presenter composites it (docs/porting/display.md).
    s3_mods::set_layer_sink(&mut engine.machine, shared.layer.clone());
    shared.set_status(Phase::Running, "Running", "S3.EXE loaded; waiting for the first frame.");
    let mut last_report = Instant::now();
    let mut seen_viewport = u64::MAX;
    // CPU boost hint for this thread (APerformanceHint, API 33+; no-op elsewhere).
    let mut hint = crate::perf_hint::EngineHint::new(crate::perf_hint::target_ns(s3_dx::ddraw::fps_cap()));
    let mut presents_seen = engine.machine.stats.presents;
    loop {
        if shared.quit.load(Ordering::Acquire) {
            log::info!("engine: quit requested\n{}", engine.summary());
            return Ok(engine.summary());
        }
        if shared.paused.load(Ordering::Acquire) {
            shared.park_for(200);
            continue;
        }
        if engine.run_for_ms(16) == SliceResult::Stopped {
            return Ok(engine.summary());
        }
        let p = engine.machine.stats.presents;
        hint.slice_done(p - presents_seen, crate::perf_hint::target_ns(s3_dx::ddraw::fps_cap()));
        presents_seen = p;
        sync_touch_flags(&shared, &mut engine.machine);
        sync_display(&shared, &mut engine.machine, &mut seen_viewport);
        if last_report.elapsed() > Duration::from_secs(10) {
            last_report = Instant::now();
            let s = &engine.machine.stats;
            log::info!(
                "engine: {} instructions, {} thunk calls, {} presents, {} input events delivered",
                s.instructions,
                s.thunk_calls,
                s.presents,
                shared.delivered.load(Ordering::Relaxed)
            );
        }
    }
}

/// Exchange display state with the mod layer: settings -> presenter, viewport + display rates -> mods.
#[cfg(feature = "engine")]
fn sync_display(shared: &Shared, m: &mut s3_runtime::Machine, seen_viewport: &mut u64) {
    shared.set_display(s3_mods::display::display_settings(m));
    let seq = shared.viewport_seq.load(Ordering::Acquire);
    if seq != *seen_viewport {
        *seen_viewport = seq;
        let vp = *shared.viewport.lock().unwrap();
        s3_mods::display::set_viewport(m, vp);
    }
    let f = |a: &AtomicU32| {
        let v = f32::from_bits(a.load(Ordering::Relaxed));
        (v > 0.0).then_some(v)
    };
    s3_mods::display::set_host_rates(m, f(&shared.host_fps), f(&shared.refresh_hz));
}

/// Pass the mod layer's three-finger keyboard request to the UI thread.
#[cfg(feature = "engine")]
fn sync_touch_flags(shared: &Shared, m: &mut s3_runtime::Machine) {
    if s3_mods::take_keyboard_request(m) {
        shared.keyboard_req.store(true, Ordering::Release);
        (shared.wake)();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_stops_while_paused() {
        let sh = Shared::new(Box::new(|| {}));
        sh.set_paused(true);
        let a = sh.clock_ms();
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(sh.clock_ms(), a);
        sh.set_paused(false);
        assert!(sh.clock_ms() - a < 20);
        let evs = sh.events.lock().unwrap().clone();
        assert_eq!(evs, vec![HostEvent::Pause, HostEvent::Resume]);
    }

    #[test]
    fn missing_exe_sets_status_and_quits() {
        let dir = std::env::temp_dir().join(format!("s3android-test-{}", std::process::id()));
        let sh = Shared::new(Box::new(|| {}));
        let sh2 = sh.clone();
        let (g, d) = (dir.join("game"), dir.join("data"));
        let t = std::thread::spawn(move || run_engine(sh2, g, d));
        for _ in 0..100 {
            if sh.status.lock().unwrap().phase == Phase::MissingExe {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(sh.status.lock().unwrap().phase, Phase::MissingExe);
        assert!(sh.status.lock().unwrap().body.contains("S3.EXE, GFX, SND, Map"));
        sh.request_quit();
        t.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn first_run_text_shows_user_path() {
        let p = Path::new("/storage/emulated/0/Android/data/com.settlers3port.app/files/game");
        assert_eq!(user_visible_path(p), "Android/data/com.settlers3port.app/files/game/");
        let t = missing_exe_text(p);
        assert!(t.contains("> Android/data/com.settlers3port.app/files/game/\n"));
        assert!(t.contains("adb push \"<GOG folder>/.\" /sdcard/Android/data/com.settlers3port.app/files/game/"));
    }

    #[test]
    fn present_copies_frame() {
        let sh = Shared::new(Box::new(|| {}));
        let mut h = AndroidHost::new(sh.clone(), "g".into(), "d".into());
        let mut fb = FrameBuffer::new(4, 2);
        fb.pixels[3] = 7;
        h.present(&fb);
        h.present(&fb);
        let slot = sh.frame.lock().unwrap();
        assert_eq!(slot.seq, 2);
        assert_eq!(slot.fb.as_ref().unwrap().pixels[3], 7);
    }
}
