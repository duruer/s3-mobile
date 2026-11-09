//! Render thread: presents the newest frame on the ANativeWindow with EGL + GLES 2 (docs/porting/display.md).
//!
//! - The engine thread only copies its frame into `Shared::frame` and signals `Shared::frame_cv`; it never waits
//!   for the display.
//! - This thread uploads the RGB565 frame (`glTexSubImage2D`, no CPU conversion or scaling) and draws one textured
//!   quad placed by [`Viewport`] (Fit / Fill / Stretch / Native wide) with the selected filter (nearest, linear,
//!   sharp bilinear shader); `eglSwapBuffers` with swap interval 1
//!   paces it to the display (60/90/120 Hz). It requests the panel's high refresh rate with
//!   `ANativeWindow_setFrameRate[WithChangeStrategy]` (looked up with dlsym: API 30/31).
//! - The mods' window layer (`s3_mods::layer`: HUD, FPS counter, touch feedback, Classic floating UI at window
//!   resolution) is rasterized here ([`LayerRaster`], only the changed rectangle), uploaded as a second RGBA texture
//!   (only the changed rows) and blended over the game quad (premultiplied alpha). In UI mode A the scene's panel
//!   split draws the panel part of the frame at its own scale ([`Viewport::quads`]).
//! - If EGL / GLES fails, it falls back to the old CPU path (`ANativeWindow_lock`, letterbox, compositor scaling).
//! - Window changes come from the activity thread through [`Presenter::set_window`]; losing the window waits until
//!   the EGL surface is destroyed (Android requires that before `onNativeWindowDestroyed` returns).

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use ndk::hardware_buffer_format::HardwareBufferFormat;
use ndk::native_window::NativeWindow;
use s3_mods::display::{DisplayMode, Viewport};

use crate::frontend::Shared;
use crate::gl::*;
use crate::glrender::{Composer, GlRenderer};
use crate::layout::{blit_letterboxed, Letterbox};

struct CtlState {
    window: Option<NativeWindow>,
    gen: u64,
    acked: u64,
    quit: bool,
}

struct Ctl {
    m: Mutex<CtlState>,
    cv: Condvar,
    /// Mirror of `CtlState::gen` readable without the lock.
    gen: AtomicU64,
}

pub struct Presenter {
    ctl: Arc<Ctl>,
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Presenter {
    pub fn start(shared: Arc<Shared>) -> Presenter {
        let ctl = Arc::new(Ctl { m: Mutex::new(CtlState { window: None, gen: 0, acked: 0, quit: false }), cv: Condvar::new(), gen: AtomicU64::new(0) });
        let (c2, s2) = (ctl.clone(), shared.clone());
        let thread = std::thread::Builder::new()
            .name("s3-present".into())
            .spawn(move || render_thread(c2, s2))
            .expect("spawn render thread");
        Presenter { ctl, shared, thread: Some(thread) }
    }

    /// New window (or `None` when it goes away). Waits (up to 2 s) until the render thread switched.
    pub fn set_window(&self, w: Option<NativeWindow>) {
        let mut c = self.ctl.m.lock().unwrap();
        c.window = w;
        c.gen += 1;
        let gen = c.gen;
        self.ctl.gen.store(gen, Ordering::Release);
        drop(c);
        self.shared.notify_render();
        let c = self.ctl.m.lock().unwrap();
        let _ = self.ctl.cv.wait_timeout_while(c, Duration::from_secs(2), |c| c.acked < gen && !c.quit).unwrap();
    }

    /// Redraw the current image (RedrawNeeded / resume).
    pub fn invalidate(&self) {
        self.shared.display_seq.fetch_add(1, Ordering::AcqRel);
        self.shared.notify_render();
    }
}

impl Drop for Presenter {
    fn drop(&mut self) {
        self.ctl.m.lock().unwrap().quit = true;
        self.ctl.gen.fetch_add(1, Ordering::AcqRel);
        self.shared.notify_render();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

// ---------------------------------------------------------------- frame rate request (dlsym, API 30+)

extern "C" {
    fn dlopen(name: *const std::ffi::c_char, flags: i32) -> *mut c_void;
    fn dlsym(h: *mut c_void, name: *const std::ffi::c_char) -> *mut c_void;
}

type SetFrameRate = unsafe extern "C" fn(*mut c_void, f32, i8) -> i32;
type SetFrameRateStrategy = unsafe extern "C" fn(*mut c_void, f32, i8, i8) -> i32;

fn request_frame_rate(win: &NativeWindow, hz: f32) {
    unsafe {
        let h = dlopen(c"libandroid.so".as_ptr(), 0);
        let h = if h.is_null() { dlopen(c"libnativewindow.so".as_ptr(), 0) } else { h };
        if h.is_null() {
            log::warn!("present: dlopen libandroid failed; no frame rate request");
            return;
        }
        let p = win.ptr().as_ptr() as *mut c_void;
        let f = dlsym(h, c"ANativeWindow_setFrameRateWithChangeStrategy".as_ptr());
        if !f.is_null() {
            let f: SetFrameRateStrategy = std::mem::transmute(f);
            // COMPATIBILITY_DEFAULT (0), CHANGE_FRAME_RATE_ALWAYS (1).
            let r = f(p, hz, 0, 1);
            log::info!("present: ANativeWindow_setFrameRateWithChangeStrategy({hz}) -> {r}");
            return;
        }
        let f = dlsym(h, c"ANativeWindow_setFrameRate".as_ptr());
        if !f.is_null() {
            let f: SetFrameRate = std::mem::transmute(f);
            let r = f(p, hz, 0);
            log::info!("present: ANativeWindow_setFrameRate({hz}) -> {r}");
        } else {
            log::info!("present: ANativeWindow_setFrameRate unavailable (API < 30)");
        }
    }
}

// ---------------------------------------------------------------- GL state

struct Gles {
    display: EGLDisplay,
    config: EGLConfig,
    context: EGLContext,
    surface: EGLSurface,
    /// The context is GLES 3 (needed by the AI filter).
    es3: bool,
    /// Programs and textures (`crate::glrender`), created on the first attach.
    r: Option<GlRenderer>,
}

impl Gles {
    unsafe fn new() -> Result<Gles, String> {
        let display = eglGetDisplay(EGL_DEFAULT_DISPLAY);
        if display == EGL_NO_DISPLAY || eglInitialize(display, std::ptr::null_mut(), std::ptr::null_mut()) == 0 {
            return Err(format!("eglInitialize: {:#x}", eglGetError()));
        }
        // GLES 3 capable configs first (the AI filter needs a GLES 3 context), else GLES 2.
        let mut configs = [std::ptr::null_mut(); 32];
        let mut n = 0;
        let mut es3 = false;
        for bits in [EGL_OPENGL_ES2_BIT | EGL_OPENGL_ES3_BIT_KHR, EGL_OPENGL_ES2_BIT] {
            let attribs = [
                EGL_SURFACE_TYPE, EGL_WINDOW_BIT, EGL_RENDERABLE_TYPE, bits,
                EGL_RED_SIZE, 5, EGL_GREEN_SIZE, 6, EGL_BLUE_SIZE, 5, EGL_DEPTH_SIZE, 0, EGL_NONE,
            ];
            if eglChooseConfig(display, attribs.as_ptr(), configs.as_mut_ptr(), 32, &mut n) != 0 && n > 0 {
                es3 = bits & EGL_OPENGL_ES3_BIT_KHR != 0;
                break;
            }
        }
        if n == 0 {
            return Err(format!("eglChooseConfig: {:#x}", eglGetError()));
        }
        // Prefer an exact RGB565 / no-alpha config (cheapest scan-out), else the first one.
        let mut config = configs[0];
        for &c in &configs[..n as usize] {
            let (mut r, mut g, mut b, mut a) = (0, 0, 0, 0);
            eglGetConfigAttrib(display, c, EGL_RED_SIZE, &mut r);
            eglGetConfigAttrib(display, c, EGL_GREEN_SIZE, &mut g);
            eglGetConfigAttrib(display, c, EGL_BLUE_SIZE, &mut b);
            eglGetConfigAttrib(display, c, EGL_ALPHA_SIZE, &mut a);
            if (r, g, b, a) == (8, 8, 8, 0) || (r, g, b, a) == (5, 6, 5, 0) {
                config = c;
                if r == 8 {
                    break; // RGBX8888 avoids 565 dithering of the sharp-bilinear edges.
                }
            }
        }
        let mut context = EGL_NO_CONTEXT;
        if es3 {
            let ctx3 = [EGL_CONTEXT_CLIENT_VERSION, 3, EGL_NONE];
            context = eglCreateContext(display, config, EGL_NO_CONTEXT, ctx3.as_ptr());
            es3 = context != EGL_NO_CONTEXT;
        }
        if context == EGL_NO_CONTEXT {
            let ctx_attribs = [EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE];
            context = eglCreateContext(display, config, EGL_NO_CONTEXT, ctx_attribs.as_ptr());
        }
        if context == EGL_NO_CONTEXT {
            return Err(format!("eglCreateContext: {:#x}", eglGetError()));
        }
        log::info!("present: GLES {} context", if es3 { 3 } else { 2 });
        Ok(Gles { display, config, context, surface: EGL_NO_SURFACE, es3, r: None })
    }

    /// Create the window surface (and on first use the GL objects). Returns the surface size.
    unsafe fn attach(&mut self, win: &NativeWindow) -> Result<(u32, u32), String> {
        self.detach();
        let mut visual = 0;
        eglGetConfigAttrib(self.display, self.config, EGL_NATIVE_VISUAL_ID, &mut visual);
        let fmt = HardwareBufferFormat::from(visual);
        let _ = win.set_buffers_geometry(0, 0, Some(fmt));
        let s = eglCreateWindowSurface(self.display, self.config, win.ptr().as_ptr() as *mut c_void, [EGL_NONE].as_ptr());
        if s == EGL_NO_SURFACE {
            return Err(format!("eglCreateWindowSurface: {:#x}", eglGetError()));
        }
        if eglMakeCurrent(self.display, s, s, self.context) == 0 {
            eglDestroySurface(self.display, s);
            return Err(format!("eglMakeCurrent: {:#x}", eglGetError()));
        }
        self.surface = s;
        eglSwapInterval(self.display, 1);
        if self.r.is_none() {
            self.r = Some(GlRenderer::new(self.es3)?);
        }
        Ok(self.size())
    }

    unsafe fn size(&self) -> (u32, u32) {
        let (mut w, mut h) = (0, 0);
        eglQuerySurface(self.display, self.surface, EGL_WIDTH, &mut w);
        eglQuerySurface(self.display, self.surface, EGL_HEIGHT, &mut h);
        (w.max(1) as u32, h.max(1) as u32)
    }

    unsafe fn detach(&mut self) {
        if self.surface != EGL_NO_SURFACE {
            eglMakeCurrent(self.display, EGL_NO_SURFACE, EGL_NO_SURFACE, self.context);
            eglDestroySurface(self.display, self.surface);
            self.surface = EGL_NO_SURFACE;
        }
    }

    /// Swap; `false` if the surface is gone (re-attach needed).
    unsafe fn swap(&mut self) -> bool {
        if eglSwapBuffers(self.display, self.surface) == 0 {
            let e = eglGetError();
            log::warn!("present: eglSwapBuffers failed {e:#x}");
            return false;
        }
        true
    }
}

impl Drop for Gles {
    fn drop(&mut self) {
        unsafe {
            self.detach();
            eglDestroyContext(self.display, self.context);
            eglTerminate(self.display);
        }
    }
}

// ---------------------------------------------------------------- the thread

fn render_thread(ctl: Arc<Ctl>, shared: Arc<Shared>) {
    let mut gles: Option<Gles> = None;
    let mut gl_failed = false;
    let mut window: Option<NativeWindow> = None;
    let mut my_gen = 0u64;
    let mut win_size = (1u32, 1u32);
    let mut comp = Composer::new();
    let mut cpu_geometry: Option<(u32, u32)> = None;
    let mut fps_t0 = Instant::now();
    let mut fps_n = 0u32;
    let mut requested_hz: Option<u32> = None;
    log::info!("present: render thread started");
    loop {
        // 1. Window changes.
        if ctl.gen.load(Ordering::Acquire) != my_gen {
            let mut c = ctl.m.lock().unwrap();
            if c.quit {
                c.acked = c.gen;
                ctl.cv.notify_all();
                break;
            }
            my_gen = c.gen;
            let w = c.window.clone();
            if let Some(g) = gles.as_mut() {
                unsafe { g.detach() };
            }
            window = w;
            cpu_geometry = None;
            // New surface: re-upload the whole layer texture.
            comp.reset();
            if let Some(r) = gles.as_mut().and_then(|g| g.r.as_mut()) {
                r.ltex_size = (0, 0);
            }
            requested_hz = None;
            if let Some(w) = &window {
                if gles.is_none() && !gl_failed {
                    match unsafe { Gles::new() } {
                        Ok(g) => gles = Some(g),
                        Err(e) => {
                            log::error!("present: EGL init failed ({e}); using the CPU path");
                            gl_failed = true;
                        }
                    }
                }
                if let Some(g) = gles.as_mut() {
                    match unsafe { g.attach(w) } {
                        Ok(sz) => {
                            win_size = sz;
                            log::info!("present: GLES surface {}x{} (window {}x{})", sz.0, sz.1, w.width(), w.height());
                        }
                        Err(e) => {
                            log::error!("present: surface failed ({e}); using the CPU path");
                            gles = None;
                            gl_failed = true;
                        }
                    }
                }
                if gles.is_none() {
                    let _ = w.set_buffers_geometry(0, 0, None);
                    win_size = (w.width().max(1) as u32, w.height().max(1) as u32);
                }
            }
            c.acked = my_gen;
            ctl.cv.notify_all();
        }
        let Some(win) = window.clone() else {
            let g = shared.frame.lock().unwrap();
            let _ = shared.frame_cv.wait_timeout(g, Duration::from_millis(200)).unwrap();
            continue;
        };
        // 2. Wait for something new.
        if !comp.wait_news(&shared, Duration::from_millis(250)) {
            // Keep the display rate current while nothing new arrives (paused game, menus).
            let el = fps_t0.elapsed();
            if el >= Duration::from_secs(1) {
                shared.host_fps.store((fps_n as f32 / el.as_secs_f32()).to_bits(), Ordering::Relaxed);
                fps_n = 0;
                fps_t0 = Instant::now();
            }
            continue;
        }
        let display = *shared.display.lock().unwrap();
        let want_hz = if display.refresh_hz == 0 { 120 } else { display.refresh_hz };
        if requested_hz != Some(want_hz) {
            request_frame_rate(&win, want_hz as f32);
            requested_hz = Some(want_hz);
        }
        let picked = comp.pick(&shared);

        if let Some(g) = gles.as_mut() {
            // GPU path.
            let sz = unsafe { g.size() };
            win_size = sz;
            unsafe { comp.draw_gl(&shared, g.r.as_mut().unwrap(), sz, &picked) };
            if !unsafe { g.swap() } {
                // Surface lost: wait for the activity to give us a window again.
                unsafe { g.detach() };
                window = None;
                continue;
            }
        } else {
            // CPU fallback: letterbox into the window buffer, the compositor scales (Fit only).
            let fb = comp.local.as_ref().unwrap();
            let lb = Letterbox::new(fb.width, fb.height, win_size.0, win_size.1);
            if cpu_geometry != Some((lb.buf_w, lb.buf_h)) {
                if let Err(e) = win.set_buffers_geometry(lb.buf_w as i32, lb.buf_h as i32, Some(HardwareBufferFormat::R5G6B5_UNORM)) {
                    log::error!("set_buffers_geometry: {e}");
                }
                cpu_geometry = Some((lb.buf_w, lb.buf_h));
            }
            match win.lock(None) {
                Ok(mut guard) => {
                    let (bw, bh, stride) = (guard.width(), guard.height(), guard.stride());
                    let mut l = lb;
                    l.off_x = (bw as u32).saturating_sub(lb.game_w) / 2;
                    l.off_y = (bh as u32).saturating_sub(lb.game_h) / 2;
                    let dst = unsafe { std::slice::from_raw_parts_mut(guard.bits() as *mut u16, stride * bh) };
                    blit_letterboxed(&l, &fb.pixels, dst, stride, bh);
                    // Window layer, sampled down to the buffer the compositor scales to the window.
                    if picked.layer_on && (picked.scene.list.width, picked.scene.list.height) == win_size {
                        comp.raster.update(&picked.scene.list);
                        comp.raster.blend_onto_rgb565_scaled(dst, stride, bw as u32, bh as u32);
                    }
                }
                Err(e) => log::error!("ANativeWindow_lock: {e}"),
            }
            *shared.letterbox.lock().unwrap() = Some(lb);
            shared.pan_y.store(0, Ordering::Release); // the CPU path does not move the picture for the keyboard
            shared.set_viewport(if picked.game_frame { Some(Viewport::new(DisplayMode::Fit, fb.width, fb.height, win_size.0, win_size.1)) } else { None });
        }
        // 3. Display rate.
        fps_n += 1;
        let el = fps_t0.elapsed();
        if el >= Duration::from_secs(1) {
            let f = fps_n as f32 / el.as_secs_f32();
            shared.host_fps.store(f.to_bits(), Ordering::Relaxed);
            fps_n = 0;
            fps_t0 = Instant::now();
        }
    }
    drop(gles);
    log::info!("present: render thread stopped");
}
