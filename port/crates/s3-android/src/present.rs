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
use s3_mods::display::{DisplayMode, ScaleFilter, Viewport};
use s3_mods::menu_layout::Background;
use s3_mods::LayerRaster;
use s3_runtime::FrameBuffer;

use crate::frontend::{Phase, Shared};
use crate::gl::*;
use crate::layout::{blit_letterboxed, Letterbox};
use crate::screen;

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
    plain: Program,
    sharp: Program,
    /// xBR-lv2 upscaler (`gl::FS_XBR`); id 0 if it failed to compile (then sharp bilinear is used).
    xbr: Program,
    /// The context is GLES 3 (needed by the AI filter).
    es3: bool,
    /// AI upscaler (created on first use; `ai_failed` after an error, then sharp bilinear is used).
    ai: Option<crate::upscale_ai::AiUpscaler>,
    ai_failed: bool,
    /// AI output texture for the current game frame (`None` = run the network on the next draw).
    ai_out: Option<GLuint>,
    tex: GLuint,
    tex_size: (u32, u32),
    tex_filter: GLint,
    /// Window layer texture (RGBA8, premultiplied) and its size.
    ltex: GLuint,
    ltex_size: (u32, u32),
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
        Ok(Gles {
            display,
            config,
            context,
            surface: EGL_NO_SURFACE,
            plain: Program { id: 0, u_rect: -1, u_uv: -1, u_tex: -1, u_size: -1, u_scale: -1 },
            sharp: Program { id: 0, u_rect: -1, u_uv: -1, u_tex: -1, u_size: -1, u_scale: -1 },
            xbr: Program { id: 0, u_rect: -1, u_uv: -1, u_tex: -1, u_size: -1, u_scale: -1 },
            es3,
            ai: None,
            ai_failed: false,
            ai_out: None,
            tex: 0,
            tex_size: (0, 0),
            tex_filter: -1,
            ltex: 0,
            ltex_size: (0, 0),
        })
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
        if self.plain.id == 0 {
            self.plain = program(FS_PLAIN)?;
            self.sharp = program(FS_SHARP)?;
            match program(FS_XBR) {
                Ok(p) => self.xbr = p,
                Err(e) => log::warn!("xBR shader unavailable: {e}"),
            }
            let mut t = 0;
            glGenTextures(1, &mut t);
            self.tex = t;
            glBindTexture(GL_TEXTURE_2D, t);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
            let mut lt = 0;
            glGenTextures(1, &mut lt);
            self.ltex = lt;
            glBindTexture(GL_TEXTURE_2D, lt);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
            glBindTexture(GL_TEXTURE_2D, t);
            glPixelStorei(GL_UNPACK_ALIGNMENT, 2);
            glDisable(GL_DITHER);
            glEnableVertexAttribArray(0);
            glVertexAttribPointer(0, 2, GL_FLOAT, 0, 0, QUAD.as_ptr() as *const c_void);
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

    unsafe fn upload(&mut self, fb: &FrameBuffer) {
        self.ai_out = None;
        glActiveTexture(GL_TEXTURE0);
        glBindTexture(GL_TEXTURE_2D, self.tex);
        let p = fb.pixels.as_ptr() as *const c_void;
        if self.tex_size != (fb.width, fb.height) {
            glTexImage2D(GL_TEXTURE_2D, 0, GL_RGB as GLint, fb.width as i32, fb.height as i32, 0, GL_RGB, GL_UNSIGNED_SHORT_5_6_5, p);
            self.tex_size = (fb.width, fb.height);
        } else {
            glTexSubImage2D(GL_TEXTURE_2D, 0, 0, 0, fb.width as i32, fb.height as i32, GL_RGB, GL_UNSIGNED_SHORT_5_6_5, p);
        }
    }

    /// Bring the layer texture up to date: whole texture after a size change, else only the rows of `dirty`.
    unsafe fn upload_layer(&mut self, r: &LayerRaster, dirty: Option<(i32, i32, i32, i32)>) {
        if r.width == 0 || r.height == 0 {
            return;
        }
        glActiveTexture(GL_TEXTURE0);
        glBindTexture(GL_TEXTURE_2D, self.ltex);
        if self.ltex_size != (r.width, r.height) {
            glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA as GLint, r.width as i32, r.height as i32, 0, GL_RGBA, GL_UNSIGNED_BYTE, r.rgba.as_ptr() as *const c_void);
            self.ltex_size = (r.width, r.height);
        } else if let Some((_, y0, _, y1)) = dirty {
            let (y0, y1) = (y0.clamp(0, r.height as i32), y1.clamp(0, r.height as i32));
            if y1 > y0 {
                let p = r.rgba[(y0 as usize * r.width as usize * 4)..].as_ptr() as *const c_void;
                glTexSubImage2D(GL_TEXTURE_2D, 0, 0, y0, r.width as i32, y1 - y0, GL_RGBA, GL_UNSIGNED_BYTE, p);
            }
        }
    }

    /// Blend the layer texture over the whole window.
    unsafe fn draw_layer(&mut self, ww: u32, wh: u32) {
        if self.ltex_size != (ww, wh) {
            return;
        }
        glActiveTexture(GL_TEXTURE0);
        glBindTexture(GL_TEXTURE_2D, self.ltex);
        glUseProgram(self.plain.id);
        glUniform1i(self.plain.u_tex, 0);
        glUniform4f(self.plain.u_rect, -1.0, 1.0, 1.0, -1.0);
        glUniform4f(self.plain.u_uv, 0.0, 0.0, 1.0, 1.0);
        glEnable(GL_BLEND);
        glBlendFunc(GL_ONE, GL_ONE_MINUS_SRC_ALPHA);
        glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
        glDisable(GL_BLEND);
        glBindTexture(GL_TEXTURE_2D, self.tex);
    }

    /// AI filter: the 2x texture of the current game frame (runs the network once per new frame), `None` when it
    /// does not apply (not wanted, scale below 1.2, menu layout, no GLES 3, or it failed).
    unsafe fn ai_frame(&mut self, vp: &Viewport, filter: ScaleFilter) -> Option<GLuint> {
        // Run when any part is enlarged >= 1.2x: the map, the Classic panel (shown at ~2-4x even when the map is
        // zoomed out) or a menu layout.
        let up = if vp.layout.is_some() {
            let (sx, sy) = vp.scale();
            sx.min(sy)
        } else {
            vp.quads().iter().map(|(s, d)| (d[2] / s[2].max(1.0)).min(d[3] / s[3].max(1.0))).fold(0.0, f32::max)
        };
        if filter != ScaleFilter::Ai || up < 1.2 || !self.es3 || self.ai_failed {
            return None;
        }
        if let Some(t) = self.ai_out {
            return Some(t);
        }
        if self.ai.is_none() {
            match crate::upscale_ai::AiUpscaler::new() {
                Ok(a) => self.ai = Some(a),
                Err(e) => {
                    log::warn!("AI upscaler unavailable: {e}");
                    self.ai_failed = true;
                    return None;
                }
            }
        }
        let (w, h) = self.tex_size;
        let r = self.ai.as_mut().unwrap().run(self.tex, w, h);
        self.tex_filter = -1; // the passes changed the frame texture's filtering
        match r {
            Ok(t) => {
                self.ai_out = Some(t);
                Some(t)
            }
            Err(e) => {
                log::warn!("AI upscaler failed: {e}");
                self.ai_failed = true;
                None
            }
        }
    }

    /// Select and bind the program (+ frame texture filtering) for a part shown at `sx` x `sy` window px per game
    /// px; sets `u_tex`, `u_size`, `u_scale`. Returns the program's uniform locations.
    unsafe fn use_filter(&mut self, filter: ScaleFilter, sx: f32, sy: f32, ai: bool, vp: &Viewport) -> Program {
        let integer = (sx - sx.round()).abs() < 1e-3 && (sy - sy.round()).abs() < 1e-3;
        let down = sx < 1.0 || sy < 1.0;
        let (prog, tf) = match filter {
            _ if ai => (&self.plain, GL_LINEAR),
            ScaleFilter::Nearest => (&self.plain, GL_NEAREST),
            ScaleFilter::Linear => (&self.plain, GL_LINEAR),
            ScaleFilter::SharpBilinear if integer || down => (&self.plain, if down { GL_LINEAR } else { GL_NEAREST }),
            ScaleFilter::SharpBilinear => (&self.sharp, GL_LINEAR),
            ScaleFilter::Xbr if down => (&self.plain, GL_LINEAR),
            ScaleFilter::Xbr if self.xbr.id != 0 => (&self.xbr, GL_NEAREST),
            ScaleFilter::Xbr => (&self.sharp, GL_LINEAR),
            ScaleFilter::Ai if down => (&self.plain, GL_LINEAR),
            ScaleFilter::Ai => (&self.sharp, GL_LINEAR),
        };
        let p = Program { id: prog.id, u_rect: prog.u_rect, u_uv: prog.u_uv, u_tex: prog.u_tex, u_size: prog.u_size, u_scale: prog.u_scale };
        if !ai && self.tex_filter != tf {
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, tf);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, tf);
            self.tex_filter = tf;
        }
        glUseProgram(p.id);
        glUniform1i(p.u_tex, 0);
        if p.u_size >= 0 {
            glUniform2f(p.u_size, vp.game_w as f32, vp.game_h as f32);
            glUniform2f(p.u_scale, sx.max(1.0), sy.max(1.0));
        }
        p
    }

    unsafe fn draw(&mut self, vp: &Viewport, filter: ScaleFilter, insets: &[([f32; 4], [f32; 4])]) {
        let ai = self.ai_frame(vp, filter);
        let (ww, wh) = (vp.win_w as f32, vp.win_h as f32);
        glViewport(0, 0, vp.win_w as i32, vp.win_h as i32);
        match vp.layout.map(|l| l.background) {
            Some(Background::Color([r, g, b])) => glClearColor(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0),
            _ => glClearColor(0.0, 0.0, 0.0, 1.0),
        }
        glClear(GL_COLOR_BUFFER_BIT);
        // Game frame.
        let (sx, sy) = vp.scale();
        glActiveTexture(GL_TEXTURE0);
        if let Some(t) = ai {
            // The 2x network output (linear filtering set at creation); same normalized coordinates.
            glBindTexture(GL_TEXTURE_2D, t);
        } else {
            glBindTexture(GL_TEXTURE_2D, self.tex);
        }
        let Some(l) = vp.layout else {
            // The whole frame, or (UI mode A panel split) the panel and the map part at their own scales: the
            // filter is chosen per part (the panel is enlarged even when the map is zoomed out).
            let (gw, gh) = (vp.game_w as f32, vp.game_h as f32);
            for (s, d) in vp.quads() {
                let (qx, qy) = (d[2] / s[2].max(1.0), d[3] / s[3].max(1.0));
                let prog = self.use_filter(filter, qx, qy, ai.is_some(), vp);
                let r = clip_rect(d[0], d[1], d[2], d[3], ww, wh);
                glUniform4f(prog.u_rect, r[0], r[1], r[2], r[3]);
                glUniform4f(prog.u_uv, s[0] / gw, s[1] / gh, (s[0] + s[2]) / gw, (s[1] + s[3]) / gh);
                if prog.u_size >= 0 {
                    glUniform2f(prog.u_scale, qx.max(1.0), qy.max(1.0));
                }
                glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
            }
            // The layer's frame insets (Classic magnifier), same texture and filter choice, below the layer.
            for &(s, d) in insets {
                let (qx, qy) = (d[2] / s[2].max(1.0), d[3] / s[3].max(1.0));
                let prog = self.use_filter(filter, qx, qy, ai.is_some(), vp);
                let r = clip_rect(d[0], d[1], d[2], d[3], ww, wh);
                glUniform4f(prog.u_rect, r[0], r[1], r[2], r[3]);
                glUniform4f(prog.u_uv, s[0] / gw, s[1] / gh, (s[0] + s[2]) / gw, (s[1] + s[3]) / gh);
                glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
            }
            return;
        };
        let prog = self.use_filter(filter, sx, sy, ai.is_some(), vp);
        // Full-screen menu layout (s3_mods::menu_layout): background, then one quad per piece.
        let (gw, gh) = (vp.game_w as f32, vp.game_h as f32);
        let uv = |x: u16, y: u16, w: u16, h: u16| [x as f32 / gw, y as f32 / gh, (x + w) as f32 / gw, (y + h) as f32 / gh];
        if let Background::Stretch([x, y, w, h]) = l.background {
            let r = clip_rect(0.0, 0.0, ww, wh, ww, wh);
            let t = uv(x, y, w, h);
            glUniform4f(prog.u_rect, r[0], r[1], r[2], r[3]);
            glUniform4f(prog.u_uv, t[0], t[1], t[2], t[3]);
            glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
        }
        for p in l.pieces {
            let [x, y, w, h] = p.src;
            let r = clip_rect(vp.dst_x + p.at[0] as f32 * sx, vp.dst_y + p.at[1] as f32 * sy, w as f32 * sx, h as f32 * sy, ww, wh);
            let t = uv(x, y, w, h);
            glUniform4f(prog.u_rect, r[0], r[1], r[2], r[3]);
            glUniform4f(prog.u_uv, t[0], t[1], t[2], t[3]);
            glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
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

/// Frame sequence number held in the presenter's local buffer (only the render thread uses it).
static LOCAL_SEQ: AtomicU64 = AtomicU64::new(u64::MAX);

/// What to show: the game frame, the frame with the "engine stopped" panel, or a status screen.
fn pick_image(shared: &Shared, local: &mut Option<FrameBuffer>, status_cache: &mut Option<(u64, FrameBuffer)>, frame_seq: &mut u64) -> (bool, u64) {
    let status = shared.status.lock().unwrap().clone();
    let mut slot = shared.frame.lock().unwrap();
    *frame_seq = slot.seq;
    let seq = slot.seq;
    match &mut slot.fb {
        Some(fb) => {
            match local {
                // A new frame of the same size: swap buffers with the slot instead of copying (perf-r2). The slot
                // then holds an older frame, which the engine overwrites completely on its next present.
                Some(l) if l.width == fb.width && l.height == fb.height => {
                    if LOCAL_SEQ.swap(seq, Ordering::AcqRel) != seq {
                        std::mem::swap(&mut l.pixels, &mut fb.pixels);
                    }
                }
                _ => {
                    *local = Some(fb.clone());
                    LOCAL_SEQ.store(seq, Ordering::Release);
                }
            }
            drop(slot);
            if status.phase == Phase::Stopped {
                screen::draw_panel(local.as_mut().unwrap(), &status.title, &status.body, true);
            }
            (true, status.seq)
        }
        None => {
            drop(slot);
            if status_cache.as_ref().map(|c| c.0) != Some(status.seq) {
                let img = if status.phase == Phase::MissingExe {
                    screen::first_run_screen(&status.title, &status.body)
                } else {
                    screen::status_screen(&status.title, &status.body)
                };
                *status_cache = Some((status.seq, img));
            }
            *local = Some(status_cache.as_ref().unwrap().1.clone());
            (false, status.seq)
        }
    }
}

fn render_thread(ctl: Arc<Ctl>, shared: Arc<Shared>) {
    let mut gles: Option<Gles> = None;
    let mut gl_failed = false;
    let mut window: Option<NativeWindow> = None;
    let mut my_gen = 0u64;
    let mut win_size = (1u32, 1u32);
    let mut local: Option<FrameBuffer> = None;
    let mut status_cache: Option<(u64, FrameBuffer)> = None;
    let mut shown = (u64::MAX, u64::MAX, u64::MAX, u64::MAX); // frame seq, status seq, display seq, layer seq
    let mut raster = LayerRaster::new();
    let mut layer_dirty: Option<(i32, i32, i32, i32)> = None;
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
            shown = (u64::MAX, u64::MAX, u64::MAX, u64::MAX);
            // New surface: re-upload the whole layer texture.
            raster = LayerRaster::new();
            if let Some(g) = gles.as_mut() {
                g.ltex_size = (0, 0);
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
        let disp_seq = shared.display_seq.load(Ordering::Acquire);
        {
            let status_seq = shared.status.lock().unwrap().seq;
            let layer_seq = shared.layer.seq();
            let g = shared.frame.lock().unwrap();
            if g.seq == shown.0 && status_seq == shown.1 && disp_seq == shown.2 && layer_seq == shown.3 {
                let _ = shared.frame_cv.wait_timeout(g, Duration::from_millis(250)).unwrap();
                // Keep the display rate current while nothing new arrives (paused game, menus).
                let el = fps_t0.elapsed();
                if el >= Duration::from_secs(1) {
                    shared.host_fps.store((fps_n as f32 / el.as_secs_f32()).to_bits(), Ordering::Relaxed);
                    fps_n = 0;
                    fps_t0 = Instant::now();
                }
                continue;
            }
        }
        let display = *shared.display.lock().unwrap();
        let want_hz = if display.refresh_hz == 0 { 120 } else { display.refresh_hz };
        if requested_hz != Some(want_hz) {
            request_frame_rate(&win, want_hz as f32);
            requested_hz = Some(want_hz);
        }
        let mut frame_seq = 0;
        let scene = shared.layer.latest();
        let (game_frame, status_seq) = pick_image(&shared, &mut local, &mut status_cache, &mut frame_seq);
        let fb = local.as_ref().unwrap();
        shown = (frame_seq, status_seq, disp_seq, scene.seq);
        let panel = if game_frame { s3_mods::display::panel_for_frame(scene.panel, scene.frame, (fb.width, fb.height)) } else { None };
        // Window layer for this window size (a scene made for another size waits for the next one).
        let layer_on = game_frame && !scene.list.is_empty();

        if let Some(g) = gles.as_mut() {
            // GPU path.
            let sz = unsafe { g.size() };
            win_size = sz;
            let mode = if game_frame { display.mode } else { DisplayMode::Fit };
            let vp = Viewport::for_frame_split(mode, &fb.pixels, fb.width, fb.height, sz.0, sz.1, panel);
            let layer_fits = layer_on && (scene.list.width, scene.list.height) == sz;
            if layer_fits {
                if let Some(d) = raster.update(&scene.list) {
                    layer_dirty = Some(layer_dirty.map_or(d, |o| (o.0.min(d.0), o.1.min(d.1), o.2.max(d.2), o.3.max(d.3))));
                }
            }
            unsafe {
                g.upload(fb);
                let insets = if layer_fits { scene.list.insets() } else { Vec::new() };
                g.draw(&vp, if game_frame { display.filter } else { ScaleFilter::Linear }, &insets);
                if layer_fits {
                    g.upload_layer(&raster, layer_dirty.take());
                    if !raster.is_empty() {
                        g.draw_layer(sz.0, sz.1);
                    }
                }
            }
            shared.set_viewport(if game_frame { Some(vp) } else { None });
            *shared.letterbox.lock().unwrap() = None;
            if !unsafe { g.swap() } {
                // Surface lost: wait for the activity to give us a window again.
                unsafe { g.detach() };
                window = None;
                continue;
            }
        } else {
            // CPU fallback: letterbox into the window buffer, the compositor scales (Fit only).
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
                    if layer_on && (scene.list.width, scene.list.height) == win_size {
                        raster.update(&scene.list);
                        raster.blend_onto_rgb565_scaled(dst, stride, bw as u32, bh as u32);
                    }
                }
                Err(e) => log::error!("ANativeWindow_lock: {e}"),
            }
            *shared.letterbox.lock().unwrap() = Some(lb);
            shared.set_viewport(if game_frame { Some(Viewport::new(DisplayMode::Fit, fb.width, fb.height, win_size.0, win_size.1)) } else { None });
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
