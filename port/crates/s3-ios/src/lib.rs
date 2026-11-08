//! iOS frontend core: `libs3ios.a`, driven by the Swift shell in `port/ios` through the C API in
//! `port/ios/Sources/CS3iOS/include/s3ios.h` (docs/porting/ios.md).
//!
//! The Swift side owns UIKit: the window, a GLKView with an OpenGL ES context, CADisplayLink, touches, the soft
//! keyboard, haptics and the audio unit. Everything else is shared with the Android frontend (`s3_frontend`): the
//! launcher, the engine thread ([`run_engine`]), the GLES renderer ([`GlRenderer`] / [`Composer`]), touch ->
//! pointer events, the audio ring.
//!
//! Threads: every `s3ios_*` call except [`s3ios_audio_pull`] comes from the main thread. The engine runs on its own
//! thread (started by Play); the audio unit's render thread only reads the [`AudioRing`].

use std::ffi::{c_char, CStr};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use s3_frontend::audio::AudioRing;
use s3_frontend::frontend::{run_engine, EngineEnd, Phase, Shared};
use s3_frontend::glrender::{Composer, GlRenderer};
use s3_frontend::launcher::{Launcher, Request, Touch};
use s3_frontend::touch::{pointer_events, Action, PointerFilter, Pos};
use s3_mods::display::Viewport;
use s3_runtime::HostEvent;

/// What the shell should do after [`s3ios_tick`] (`S3Out` in s3ios.h).
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct S3Out {
    /// Something new to show: call `display()` on the GL view (it calls [`s3ios_draw`]).
    pub redraw: u8,
    /// Soft keyboard: 0 = no change, 1 = show, 2 = hide, 3 = toggle.
    pub keyboard: u8,
    /// The app asked to close (launcher Quit).
    pub quit: u8,
    /// A game is running (not the launcher): the shell may keep the screen awake.
    pub in_game: u8,
    /// Vibrate this long (ms, 0 = no).
    pub haptic_ms: u32,
    /// Game folder: 1 = show the folder picker, 2 = forget the picked folder (use the app's own folder); the shell
    /// answers with [`s3ios_set_game_dir`].
    pub pick_folder: u8,
}

/// Frontend state owned by the shell (one per process).
pub struct App {
    shared: Arc<Shared>,
    woken: Arc<AtomicBool>,
    data_dir: PathBuf,
    launcher: Launcher,
    in_launcher: bool,
    launcher_drawn: Option<(u32, u32, u64)>,
    launcher_dirty: bool,
    engine_thread: Option<std::thread::JoinHandle<()>>,
    reqs: Vec<Request>,
    size: (u32, u32),
    dpi: u32,
    pointers: PointerFilter,
    /// Viewport frozen while two or more fingers are down (see the Android frontend: pinch zoom switches the
    /// render size every few frames).
    pinch_vp: Option<Viewport>,
    renderer: Option<GlRenderer>,
    gl_failed: bool,
    comp: Composer,
    keyboard: u8,
    pick_folder: u8,
    quit: bool,
    fps_t0: Instant,
    fps_n: u32,
}

// ---------------------------------------------------------------- logging

struct Logger {
    file: Mutex<Option<std::fs::File>>,
    t0: Instant,
}

impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Info
    }
    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let line = format!("{:8.3} {:5} {}\n", self.t0.elapsed().as_secs_f32(), r.level(), r.args());
        let _ = std::io::stderr().write_all(line.as_bytes());
        if let Some(f) = self.file.lock().unwrap().as_mut() {
            let _ = f.write_all(line.as_bytes());
        }
    }
    fn flush(&self) {
        if let Some(f) = self.file.lock().unwrap().as_mut() {
            let _ = f.flush();
        }
    }
}

static LOGGER: std::sync::OnceLock<Logger> = std::sync::OnceLock::new();

/// Log to stderr (Xcode / `idevicesyslog`) and to `<data dir>/log.txt` (last run; visible in the Files app when
/// the data dir is under Documents).
fn init_log(data_dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(data_dir);
    let file = std::fs::File::create(data_dir.join("log.txt")).ok();
    let l = LOGGER.get_or_init(|| Logger { file: Mutex::new(file), t0: Instant::now() });
    if log::set_logger(l).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    std::panic::set_hook(Box::new(|info| {
        log::error!("PANIC: {info}");
        log::logger().flush();
    }));
}

unsafe fn path_arg(p: *const c_char) -> PathBuf {
    if p.is_null() {
        return PathBuf::new();
    }
    PathBuf::from(CStr::from_ptr(p).to_string_lossy().into_owned())
}

// ---------------------------------------------------------------- app logic

impl App {
    fn new(game_dir: PathBuf, data_dir: PathBuf) -> App {
        let woken = Arc::new(AtomicBool::new(true));
        let w2 = woken.clone();
        let shared = Shared::new(Box::new(move || w2.store(true, Ordering::Release)));
        let game_dir = s3_frontend::launcher::game_dir_override(&data_dir, game_dir);
        let _ = std::fs::create_dir_all(&game_dir);
        log::info!("Settlers 3 Mobile {} (iOS) starting; game dir {}, data dir {}", env!("CARGO_PKG_VERSION"), game_dir.display(), data_dir.display());
        // The audio unit pulls from the ring from the start (silence until the engine plays something).
        shared.audio.running.store(true, Ordering::Release);
        let mut launcher = Launcher::new(game_dir, data_dir.clone());
        launcher.folder_picker = Some(false);
        App {
            launcher,
            shared,
            woken,
            data_dir,
            in_launcher: true,
            launcher_drawn: None,
            launcher_dirty: true,
            engine_thread: None,
            reqs: Vec::new(),
            size: (1, 1),
            dpi: 0,
            pointers: PointerFilter::default(),
            pinch_vp: None,
            renderer: None,
            gl_failed: false,
            comp: Composer::new(),
            keyboard: 0,
            pick_folder: 0,
            quit: false,
            fps_t0: Instant::now(),
            fps_n: 0,
        }
    }

    fn start_engine(&mut self) {
        self.shared.engine_end.lock().unwrap().take();
        self.shared.guest_exited.store(false, Ordering::Release);
        self.shared.events.lock().unwrap().clear();
        self.shared.frame.lock().unwrap().fb = None;
        self.shared.set_status(Phase::Starting, "Starting", "Loading the game ...");
        self.keyboard = 2;
        let sh = self.shared.clone();
        let (g, d) = (self.launcher.game_dir().to_path_buf(), self.data_dir.clone());
        self.engine_thread = Some(
            std::thread::Builder::new()
                .name("s3-engine".into())
                .stack_size(16 << 20)
                .spawn(move || {
                    let end = run_engine(sh.clone(), g, d).unwrap_or(EngineEnd { title: "Stopped".into(), body: String::new(), exited: false });
                    *sh.engine_end.lock().unwrap() = Some(end);
                    (sh.wake)();
                })
                .expect("spawn engine thread"),
        );
        self.in_launcher = false;
        self.shared.set_launcher(None);
    }

    fn tick(&mut self) -> S3Out {
        let mut out = S3Out::default();
        // The engine run ended (Exit Game, error): back to the launcher.
        if self.engine_thread.is_some() {
            if let Some(end) = self.shared.engine_end.lock().unwrap().take() {
                if let Some(t) = self.engine_thread.take() {
                    let _ = t.join();
                }
                log::info!("launcher: engine ended ({}{})", end.title, if end.exited { ", game exited" } else { "" });
                self.shared.in_game.store(false, Ordering::Release);
                self.shared.frame.lock().unwrap().fb = None;
                self.launcher.message = Some(if end.exited {
                    ("The game was closed. Tap Play to start it again.".to_string(), false)
                } else {
                    let first = end.body.lines().next().unwrap_or("").to_string();
                    (format!("{}: {first}", end.title), true)
                });
                self.launcher.reload_settings();
                self.launcher.recheck();
                self.in_launcher = true;
                self.launcher_drawn = None;
                self.launcher_dirty = true;
            }
        }
        if self.in_launcher {
            for r in std::mem::take(&mut self.reqs) {
                log::info!("launcher: {r:?}");
                match r {
                    Request::Play if self.engine_thread.is_none() => self.start_engine(),
                    Request::Play => {}
                    Request::Keyboard(show) => self.keyboard = if show { 1 } else { 2 },
                    Request::PickFolder => self.pick_folder = 1,
                    Request::UseAppFolder => self.pick_folder = 2,
                    Request::Quit => {
                        self.launcher.stop_upload();
                        self.quit = true;
                    }
                }
            }
        }
        if self.in_launcher {
            self.launcher_dirty |= self.launcher.tick();
            let (w, h) = self.size;
            if self.launcher_dirty || self.launcher_drawn.map(|d| (d.0, d.1)) != Some((w, h)) {
                self.launcher_dirty = false;
                let list = self.launcher.draw(w, h, self.dpi);
                if self.launcher_drawn != Some((w, h, list.hash)) {
                    self.launcher_drawn = Some((w, h, list.hash));
                    self.shared.set_launcher(Some(Arc::new(list)));
                }
            }
        }
        if self.shared.keyboard_req.swap(false, Ordering::AcqRel) {
            self.keyboard = 3;
        }
        let buzz = std::mem::take(&mut *self.shared.haptic.lock().unwrap());
        out.haptic_ms = buzz.iter().copied().max().unwrap_or(0);
        out.keyboard = std::mem::take(&mut self.keyboard);
        out.pick_folder = std::mem::take(&mut self.pick_folder);
        out.quit = self.quit as u8;
        out.in_game = !self.in_launcher as u8;
        self.woken.store(false, Ordering::Release);
        out.redraw = self.comp.has_news(&self.shared) as u8;
        // Display rate (FPS counter) while nothing is drawn.
        let el = self.fps_t0.elapsed();
        if el >= Duration::from_secs(1) {
            self.shared.host_fps.store((self.fps_n as f32 / el.as_secs_f32()).to_bits(), Ordering::Relaxed);
            self.fps_n = 0;
            self.fps_t0 = Instant::now();
        }
        out
    }

    /// Draw into the GL view's framebuffer (bound by GLKit) of `w` x `h` pixels.
    unsafe fn draw(&mut self, w: u32, h: u32) {
        if self.gl_failed {
            return;
        }
        if self.renderer.is_none() {
            match GlRenderer::new(false) {
                Ok(r) => {
                    log::info!("present: GLES renderer ready ({w}x{h})");
                    self.renderer = Some(r);
                }
                Err(e) => {
                    log::error!("present: GL setup failed: {e}");
                    self.gl_failed = true;
                    return;
                }
            }
        }
        let r = self.renderer.as_mut().unwrap();
        r.bind_state();
        let picked = self.comp.pick(&self.shared);
        self.comp.draw_gl(&self.shared, r, (w.max(1), h.max(1)), &picked);
        self.fps_n += 1;
    }

    fn resize(&mut self, w: u32, h: u32, dpi: u32) {
        let (w, h) = (w.max(1), h.max(1));
        if (w, h) != self.size || dpi != self.dpi {
            log::info!("window {w}x{h} px, {dpi} dpi");
            self.size = (w, h);
            self.dpi = dpi;
            self.shared.density_dpi.store(dpi, Ordering::Relaxed);
            self.comp.reset();
            if let Some(r) = self.renderer.as_mut() {
                r.ltex_size = (0, 0);
            }
            self.launcher_dirty = true;
            self.shared.display_seq.fetch_add(1, Ordering::AcqRel);
            self.shared.notify_render();
        }
    }

    fn touch(&mut self, action: Action, id: i32, x: f32, y: f32, all: &[(i32, f32, f32)]) {
        if self.in_launcher {
            // Launcher: the first finger only, in window pixels.
            let first = all.first().map(|a| a.0).unwrap_or(id);
            if id != first && !matches!(action, Action::Cancel) {
                return;
            }
            let t = match action {
                Action::Down => Touch::Down,
                Action::Move => Touch::Move,
                Action::Up => Touch::Up,
                Action::Cancel => Touch::Cancel,
                _ => return,
            };
            if t == Touch::Down {
                log::info!("launcher: touch at ({x:.0},{y:.0})");
            }
            if let Some(r) = self.launcher.touch(t, x as i32, y as i32) {
                self.reqs.push(r);
            }
            self.launcher_dirty = true;
            return;
        }
        let Some(cur_vp) = *self.shared.viewport.lock().unwrap() else { return };
        let vp = match action {
            Action::PointerDown => *self.pinch_vp.get_or_insert(cur_vp),
            Action::Up | Action::Cancel => self.pinch_vp.take().unwrap_or(cur_vp),
            _ => self.pinch_vp.unwrap_or(cur_vp),
        };
        let pos = |x: f32, y: f32| -> Pos {
            let g = vp.window_to_game(x, y);
            (g.0, g.1, x.max(0.0) as i32, y.max(0.0) as i32)
        };
        let all: Vec<(i32, Pos)> = all.iter().map(|&(i, ax, ay)| (i, pos(ax, ay))).collect();
        for ev in pointer_events(action, (id, pos(x, y)), &all) {
            if self.pointers.keep(&ev) {
                self.shared.push_event(ev);
            }
        }
    }

    fn pause(&mut self) {
        log::info!("lifecycle: background (paused)");
        self.shared.set_paused(true);
    }

    fn resume(&mut self) {
        log::info!("lifecycle: foreground");
        self.shared.set_paused(false);
        // Drop sound queued before the pause (it would play late).
        self.shared.audio.clear();
        self.shared.display_seq.fetch_add(1, Ordering::AcqRel);
        self.shared.notify_render();
    }

    fn text(&mut self, c: u32) {
        if self.in_launcher {
            if let Some(r) = self.launcher.char(c) {
                self.reqs.push(r);
            }
            self.launcher_dirty = true;
            return;
        }
        // Backspace / Enter also as keys (the game's text fields read WM_KEYDOWN for them).
        let vk = match c {
            8 => Some(0x08),
            13 | 10 => Some(0x0d),
            _ => char::from_u32(c).and_then(s3_frontend::keys::vk_for_char),
        };
        if let Some(vk) = vk {
            self.shared.push_event(HostEvent::Key { vk, down: true });
        }
        self.shared.push_event(HostEvent::Char(if c == 10 { 13 } else { c }));
        if let Some(vk) = vk {
            self.shared.push_event(HostEvent::Key { vk, down: false });
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        log::info!("shutting down");
        self.launcher.stop_upload();
        self.shared.request_quit();
        if let Some(t) = self.engine_thread.take() {
            let _ = t.join();
        }
        log::logger().flush();
    }
}

// ---------------------------------------------------------------- C API (port/ios/Sources/CS3iOS/include/s3ios.h)

/// Create the app. `game_dir`: where the user's Settlers III files go (Documents/..., visible in Files);
/// `data_dir`: settings, saves of our own, logs.
///
/// # Safety
/// Both arguments are NUL-terminated UTF-8 paths (or null).
#[no_mangle]
pub unsafe extern "C" fn s3ios_create(game_dir: *const c_char, data_dir: *const c_char) -> *mut App {
    let (g, d) = (path_arg(game_dir), path_arg(data_dir));
    init_log(&d);
    Box::into_raw(Box::new(App::new(g, d)))
}

/// # Safety
/// `app` comes from [`s3ios_create`] and is not used afterwards.
#[no_mangle]
pub unsafe extern "C" fn s3ios_destroy(app: *mut App) {
    if !app.is_null() {
        drop(Box::from_raw(app));
    }
}

/// View size in pixels (points * contentScaleFactor) and the screen density.
///
/// # Safety
/// `app` comes from [`s3ios_create`].
#[no_mangle]
pub unsafe extern "C" fn s3ios_resize(app: *mut App, w: u32, h: u32, dpi: u32) {
    (*app).resize(w, h, dpi);
}

/// Once per display refresh (CADisplayLink), before drawing.
///
/// # Safety
/// `app` comes from [`s3ios_create`].
#[no_mangle]
pub unsafe extern "C" fn s3ios_tick(app: *mut App) -> S3Out {
    (*app).tick()
}

/// Draw the current image into the bound framebuffer (GLKView `draw(in:)`, its EAGLContext current).
///
/// # Safety
/// `app` comes from [`s3ios_create`]; a GLES context is current on this thread.
#[no_mangle]
pub unsafe extern "C" fn s3ios_draw(app: *mut App, w: u32, h: u32) {
    (*app).draw(w, h);
}

/// Touch event. `action`: 0 down (first finger), 1 pointer down (another finger), 2 move, 3 pointer up (not the
/// last finger), 4 up (last finger), 5 cancel. `id`, `x`, `y`: the finger the action is about (window pixels);
/// `ids` / `xs` / `ys` (`n` entries): every finger currently down, including the acting one.
///
/// # Safety
/// `app` comes from [`s3ios_create`]; the arrays hold `n` elements (or `n` is 0).
#[no_mangle]
pub unsafe extern "C" fn s3ios_touch(app: *mut App, action: i32, id: i32, x: f32, y: f32, n: i32, ids: *const i32, xs: *const f32, ys: *const f32) {
    let action = match action {
        0 => Action::Down,
        1 => Action::PointerDown,
        2 => Action::Move,
        3 => Action::PointerUp,
        4 => Action::Up,
        _ => Action::Cancel,
    };
    let n = n.max(0) as usize;
    let all: Vec<(i32, f32, f32)> = if n == 0 || ids.is_null() || xs.is_null() || ys.is_null() {
        vec![(id, x, y)]
    } else {
        let (ids, xs, ys) = (std::slice::from_raw_parts(ids, n), std::slice::from_raw_parts(xs, n), std::slice::from_raw_parts(ys, n));
        (0..n).map(|i| (ids[i], xs[i], ys[i])).collect()
    };
    (*app).touch(action, id, x, y, &all);
}

/// Typed character from the soft / hardware keyboard (Unicode scalar; 8 = backspace, 10/13 = return).
///
/// # Safety
/// `app` comes from [`s3ios_create`].
#[no_mangle]
pub unsafe extern "C" fn s3ios_text(app: *mut App, c: u32) {
    (*app).text(c);
}

/// Hardware key (Windows virtual key code) down / up (UIPress from a hardware keyboard).
///
/// # Safety
/// `app` comes from [`s3ios_create`].
#[no_mangle]
pub unsafe extern "C" fn s3ios_key(app: *mut App, vk: u32, down: u8) {
    if !(*app).in_launcher {
        (*app).shared.push_event(HostEvent::Key { vk, down: down != 0 });
    }
}

/// Character typed on a hardware keyboard (its key went through [`s3ios_key`] already): only the WM_CHAR part in
/// the game; the launcher gets it like typed text.
///
/// # Safety
/// `app` comes from [`s3ios_create`].
#[no_mangle]
pub unsafe extern "C" fn s3ios_char(app: *mut App, c: u32) {
    let app = &mut *app;
    if app.in_launcher {
        app.text(c);
    } else {
        app.shared.push_event(HostEvent::Char(if c == 10 { 13 } else { c }));
    }
}

/// Use `dir` as the game folder: a folder the user picked (`external` = 1, security-scoped access already started by
/// the shell) or the app's own one (0). Ignored while a game runs.
///
/// # Safety
/// `app` comes from [`s3ios_create`]; `dir` is a NUL-terminated UTF-8 path.
#[no_mangle]
pub unsafe extern "C" fn s3ios_set_game_dir(app: *mut App, dir: *const c_char, external: u8) {
    let app = &mut *app;
    if !app.in_launcher {
        log::warn!("set_game_dir while a game runs: ignored");
        return;
    }
    let d = path_arg(dir);
    if external == 0 {
        let _ = std::fs::create_dir_all(&d);
    }
    app.launcher.set_game_dir(d, external != 0);
    app.launcher_dirty = true;
}

/// App goes to the background (`background` = 1) or comes back (0).
///
/// # Safety
/// `app` comes from [`s3ios_create`].
#[no_mangle]
pub unsafe extern "C" fn s3ios_set_background(app: *mut App, background: u8) {
    if background != 0 {
        (*app).pause();
    } else {
        (*app).resume();
    }
}

/// Audio source for the audio unit's render thread (stays valid until [`s3ios_audio_release`]).
///
/// # Safety
/// `app` comes from [`s3ios_create`].
#[no_mangle]
pub unsafe extern "C" fn s3ios_audio_handle(app: *mut App) -> *const AudioRing {
    let app = &*app;
    Arc::into_raw(app.shared.audio.clone())
}

/// Fill `frames` interleaved stereo i16 frames at 44100 Hz (silence when nothing is queued).
///
/// # Safety
/// `ring` comes from [`s3ios_audio_handle`]; `out` holds `frames * 2` samples.
#[no_mangle]
pub unsafe extern "C" fn s3ios_audio_pull(ring: *const AudioRing, out: *mut i16, frames: u32) {
    let out = std::slice::from_raw_parts_mut(out, frames as usize * 2);
    (*ring).pop_into(out);
}

/// # Safety
/// `ring` comes from [`s3ios_audio_handle`] and is not used afterwards.
#[no_mangle]
pub unsafe extern "C" fn s3ios_audio_release(ring: *const AudioRing) {
    if !ring.is_null() {
        drop(Arc::from_raw(ring));
    }
}
