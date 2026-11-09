//! GLES drawing shared by the Android presenter (`present.rs`, EGL surface) and the iOS frontend (`s3-ios`, an
//! EAGLContext made current by the Swift shell): frame upload, filters, menu layouts, the window layer.
//!
//! [`GlRenderer`] only issues GL calls into whatever context and framebuffer are current; it never touches EGL.
//! [`Composer`] picks what to show (game frame / status screen / launcher), keeps the window layer raster and draws
//! one complete frame with a [`GlRenderer`]. See docs/porting/display.md.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};

use s3_mods::display::{DisplayMode, PanelSplit, ScaleFilter, Viewport};
use s3_mods::layer::Scene;
use s3_mods::menu_layout::Background;
use s3_mods::LayerRaster;
use s3_runtime::FrameBuffer;

use crate::frontend::{Phase, Shared};
use crate::gl::*;
use crate::screen;

/// GL objects of the presenter (programs and textures) in the current context.
pub struct GlRenderer {
    plain: Program,
    sharp: Program,
    /// xBR-lv2 upscaler (`gl::FS_XBR`); id 0 if it failed to compile (then sharp bilinear is used).
    xbr: Program,
    /// The context is GLES 3 (needed by the AI filter).
    es3: bool,
    /// AI upscaler (created on first use; `ai_failed` after an error, then sharp bilinear is used). Android only.
    #[cfg(target_os = "android")]
    ai: Option<crate::upscale_ai::AiUpscaler>,
    ai_failed: bool,
    /// AI output texture for the current game frame (`None` = run the network on the next draw).
    ai_out: Option<GLuint>,
    tex: GLuint,
    tex_size: (u32, u32),
    tex_filter: GLint,
    /// Window layer texture (RGBA8, premultiplied) and its size.
    ltex: GLuint,
    pub ltex_size: (u32, u32),
    /// Generated stone tile behind menu layouts (`s3_mods::stone`, `GL_REPEAT`), 0 until first needed.
    stex: GLuint,
}

impl GlRenderer {
    /// Create the programs and textures in the current context (`es3`: it is a GLES 3 context).
    pub unsafe fn new(es3: bool) -> Result<GlRenderer, String> {
        let plain = program(FS_PLAIN)?;
        let sharp = program(FS_SHARP)?;
        let xbr = match program(FS_XBR) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("xBR shader unavailable: {e}");
                Program { id: 0, u_rect: -1, u_uv: -1, u_tex: -1, u_size: -1, u_scale: -1 }
            }
        };
        let mut t = 0;
        glGenTextures(1, &mut t);
        glBindTexture(GL_TEXTURE_2D, t);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
        let mut lt = 0;
        glGenTextures(1, &mut lt);
        glBindTexture(GL_TEXTURE_2D, lt);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
        glBindTexture(GL_TEXTURE_2D, t);
        let r = GlRenderer {
            plain,
            sharp,
            xbr,
            es3,
            #[cfg(target_os = "android")]
            ai: None,
            ai_failed: false,
            ai_out: None,
            tex: t,
            tex_size: (0, 0),
            tex_filter: -1,
            ltex: lt,
            ltex_size: (0, 0),
            stex: 0,
        };
        r.bind_state();
        Ok(r)
    }

    /// Fixed GL state the drawing relies on (set again before each frame on iOS, where the shell may change it).
    pub unsafe fn bind_state(&self) {
        glPixelStorei(GL_UNPACK_ALIGNMENT, 2);
        glDisable(GL_DITHER);
        glEnableVertexAttribArray(0);
        glVertexAttribPointer(0, 2, GL_FLOAT, 0, 0, QUAD.as_ptr() as *const c_void);
    }

    pub unsafe fn upload(&mut self, fb: &FrameBuffer) {
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
    pub unsafe fn upload_layer(&mut self, r: &LayerRaster, dirty: Option<(i32, i32, i32, i32)>) {
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
    pub unsafe fn draw_layer(&mut self, ww: u32, wh: u32) {
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
    #[cfg(target_os = "android")]
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

    /// No AI upscaler outside Android (its compute passes are not ported): sharp bilinear is used instead.
    #[cfg(not(target_os = "android"))]
    unsafe fn ai_frame(&mut self, _vp: &Viewport, _filter: ScaleFilter) -> Option<GLuint> {
        let _ = (self.es3, self.ai_failed, self.ai_out);
        None
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

    /// Draw the frame for `vp`, the whole picture moved up by `pan_y` window pixels (soft keyboard, `crate::ime`;
    /// the layer drawn next uses the same GL viewport, so it moves with it).
    pub unsafe fn draw(&mut self, vp: &Viewport, filter: ScaleFilter, insets: &[([f32; 4], [f32; 4])], pan_y: i32) {
        let ai = self.ai_frame(vp, filter);
        let (ww, wh) = (vp.win_w as f32, vp.win_h as f32);
        // GL window y grows upwards: a viewport origin `pan_y` above the bottom moves the picture up.
        glViewport(0, pan_y, vp.win_w as i32, vp.win_h as i32);
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
        if l.background == Background::Stone {
            self.draw_stone(vp, sx);
            glBindTexture(GL_TEXTURE_2D, ai.unwrap_or(self.tex));
        }
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
        // Edge strips repeated over the bands above / below the canvas (main menu borders).
        for (s, d) in s3_mods::menu_layout::band_quads(l, vp.win_w, vp.win_h) {
            let r = clip_rect(d[0], d[1], d[2], d[3], ww, wh);
            glUniform4f(prog.u_rect, r[0], r[1], r[2], r[3]);
            glUniform4f(prog.u_uv, s[0] / gw, s[1] / gh, (s[0] + s[2]) / gw, (s[1] + s[3]) / gh);
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

    /// The generated stone texture over the whole window, one texel per game pixel, aligned to the layout's canvas
    /// (same mapping as the CPU renderer `menu_layout::render_xrgb`).
    unsafe fn draw_stone(&mut self, vp: &Viewport, s: f32) {
        use s3_mods::stone::{tile_rgba, TILE};
        if self.stex == 0 {
            let mut t = 0;
            glGenTextures(1, &mut t);
            self.stex = t;
            glBindTexture(GL_TEXTURE_2D, t);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_REPEAT);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_REPEAT);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
            glPixelStorei(GL_UNPACK_ALIGNMENT, 4);
            let px = tile_rgba();
            glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA as GLint, TILE as i32, TILE as i32, 0, GL_RGBA, GL_UNSIGNED_BYTE, px.as_ptr() as *const c_void);
            glPixelStorei(GL_UNPACK_ALIGNMENT, 2);
        } else {
            glBindTexture(GL_TEXTURE_2D, self.stex);
        }
        let (ww, wh) = (vp.win_w as f32, vp.win_h as f32);
        let span = s.max(0.01) * TILE as f32;
        let (u0, v0) = (-vp.dst_x / span, -vp.dst_y / span);
        glUseProgram(self.plain.id);
        glUniform1i(self.plain.u_tex, 0);
        let r = clip_rect(0.0, 0.0, ww, wh, ww, wh);
        glUniform4f(self.plain.u_rect, r[0], r[1], r[2], r[3]);
        glUniform4f(self.plain.u_uv, u0, v0, u0 + ww / span, v0 + wh / span);
        glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
    }
}

// ---------------------------------------------------------------- what to show

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

/// One picked image: the frame (in [`Composer::local`]), the window layer scene and the panel split.
pub struct Picked {
    /// A game frame (not a status screen, not the launcher).
    pub game_frame: bool,
    /// The scene to draw over it (the launcher's draw list replaces the mods' one).
    pub scene: Scene,
    pub panel: Option<PanelSplit>,
    /// The window layer is drawn (game frame or launcher, non-empty list).
    pub layer_on: bool,
}

/// Per-surface presenter state: what was shown last, the local frame copy and the window layer raster.
pub struct Composer {
    pub local: Option<FrameBuffer>,
    status_cache: Option<(u64, FrameBuffer)>,
    /// frame seq, status seq, display seq, layer seq of the shown image.
    shown: (u64, u64, u64, u64),
    pub raster: LayerRaster,
    layer_dirty: Option<(i32, i32, i32, i32)>,
}

impl Default for Composer {
    fn default() -> Self {
        Self::new()
    }
}

impl Composer {
    pub fn new() -> Composer {
        Composer { local: None, status_cache: None, shown: (u64::MAX, u64::MAX, u64::MAX, u64::MAX), raster: LayerRaster::new(), layer_dirty: None }
    }

    /// New surface: draw again and re-upload the whole layer.
    pub fn reset(&mut self) {
        self.shown = (u64::MAX, u64::MAX, u64::MAX, u64::MAX);
        self.raster = LayerRaster::new();
        self.layer_dirty = None;
    }

    fn seqs(shared: &Shared) -> (u64, u64, u64, u64) {
        let status_seq = shared.status.lock().unwrap().seq;
        let layer_seq = shared.layer.seq().wrapping_add(shared.launcher_seq.load(Ordering::Acquire) << 32);
        let disp_seq = shared.display_seq.load(Ordering::Acquire);
        let frame_seq = shared.frame.lock().unwrap().seq;
        (frame_seq, status_seq, disp_seq, layer_seq)
    }

    /// Something new to show (frame, status, display settings or layer).
    pub fn has_news(&self, shared: &Shared) -> bool {
        Self::seqs(shared) != self.shown
    }

    /// Wait up to `timeout` for something new (the frame seq is checked under the frame lock, so a frame published
    /// just before the wait is not missed). Returns whether there is something new.
    pub fn wait_news(&self, shared: &Shared, timeout: std::time::Duration) -> bool {
        let status_seq = shared.status.lock().unwrap().seq;
        let layer_seq = shared.layer.seq().wrapping_add(shared.launcher_seq.load(Ordering::Acquire) << 32);
        let disp_seq = shared.display_seq.load(Ordering::Acquire);
        let g = shared.frame.lock().unwrap();
        if (g.seq, status_seq, disp_seq, layer_seq) != self.shown {
            return true;
        }
        let _ = shared.frame_cv.wait_timeout(g, timeout).unwrap();
        self.has_news(shared)
    }

    /// Pick the image to show now and mark it shown.
    pub fn pick(&mut self, shared: &Shared) -> Picked {
        let disp_seq = shared.display_seq.load(Ordering::Acquire);
        let mut frame_seq = 0;
        let mut scene = shared.layer.latest();
        let launcher_seq = shared.launcher_seq.load(Ordering::Acquire);
        let launcher = shared.launcher.lock().unwrap().clone();
        let launcher_on = launcher.is_some();
        let (game_frame, status_seq) = match &launcher {
            // Launcher (crate::launcher): its draw list covers the whole window; the frame below is a plain fill.
            Some(_) => {
                let mut fb = FrameBuffer::new(4, 4);
                fb.pixels.fill(screen::rgb(14, 20, 34));
                self.local = Some(fb);
                LOCAL_SEQ.store(u64::MAX, Ordering::Release);
                frame_seq = shared.frame.lock().unwrap().seq;
                (false, shared.status.lock().unwrap().seq)
            }
            None => pick_image(shared, &mut self.local, &mut self.status_cache, &mut frame_seq),
        };
        let fb = self.local.as_ref().unwrap();
        self.shown = (frame_seq, status_seq, disp_seq, scene.seq.wrapping_add(launcher_seq << 32));
        let panel = if game_frame { s3_mods::display::panel_for_frame(scene.panel, scene.frame, (fb.width, fb.height)) } else { None };
        if let Some(l) = launcher {
            scene.list = l;
        }
        let layer_on = (game_frame || launcher_on) && !scene.list.is_empty();
        Picked { game_frame, scene, panel, layer_on }
    }

    /// Draw the picked image into the current framebuffer of size `sz` with `r` and publish the viewport (touch
    /// mapping). The caller presents (swap / GLKView).
    pub unsafe fn draw_gl(&mut self, shared: &Shared, r: &mut GlRenderer, sz: (u32, u32), p: &Picked) {
        let display = *shared.display.lock().unwrap();
        let fb = self.local.as_ref().unwrap();
        let mode = if p.game_frame { display.mode } else { DisplayMode::Fit };
        let vp = Viewport::for_frame_split(mode, &fb.pixels, fb.width, fb.height, sz.0, sz.1, p.panel);
        let layer_fits = p.layer_on && (p.scene.list.width, p.scene.list.height) == sz;
        if layer_fits {
            if let Some(d) = self.raster.update(&p.scene.list) {
                self.layer_dirty = Some(self.layer_dirty.map_or(d, |o| (o.0.min(d.0), o.1.min(d.1), o.2.max(d.2), o.3.max(d.3))));
            }
        }
        r.upload(fb);
        let insets = if layer_fits { p.scene.list.insets() } else { Vec::new() };
        // Keep the game text field the soft keyboard is open for above the keyboard.
        let field = *shared.text_field.lock().unwrap();
        let pan = match field {
            Some(f) if p.game_frame => crate::ime::keyboard_pan(&vp, f, shared.ime_h.load(Ordering::Acquire)),
            _ => 0,
        };
        shared.pan_y.store(pan, Ordering::Release);
        r.draw(&vp, if p.game_frame { display.filter } else { ScaleFilter::Linear }, &insets, pan);
        if layer_fits {
            r.upload_layer(&self.raster, self.layer_dirty.take());
            if !self.raster.is_empty() {
                r.draw_layer(sz.0, sz.1);
            }
        }
        shared.set_viewport(if p.game_frame { Some(vp) } else { None });
        *shared.letterbox.lock().unwrap() = None;
    }
}
