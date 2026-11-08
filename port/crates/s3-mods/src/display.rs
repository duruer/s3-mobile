//! Display modes and the one shared screen <-> game transform (docs/porting/display.md).
//!
//! Every frontend (Android GPU / CPU presenter, desktop `s3run` window) scales the game frame into its window with a
//! [`Viewport`] and maps touches / mouse positions back with [`Viewport::window_to_game`]. Mods read the active
//! viewport (window size, visible part of the game frame in `Fill` mode) with [`viewport`]; the frontend publishes
//! it with [`set_viewport`]. Settings ([`DisplaySettings`]) live in `mods-settings.json` under `display` and are read
//! by the frontends once per engine slice ([`display_settings`]); menus change them with [`set_display`] /
//! [`cycle_mode`] / [`cycle_filter`] / [`cycle_fps`].
//!
//! Pure presentation: nothing here changes guest-visible state, so lockstep multiplayer is unaffected.

use serde::{Deserialize, Serialize};

use s3_runtime::Machine;

use crate::menu_layout::{self, MenuLayout};

/// How the game frame is placed in the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DisplayMode {
    /// Keep the aspect ratio, whole frame visible, black bars (letterbox / pillarbox).
    Fit,
    /// Keep the aspect ratio, fill the whole window, crop the overflow (centred).
    Fill,
    /// Fill the whole window, distort the aspect ratio.
    Stretch,
    /// The game renders at a custom resolution with the window's aspect ratio (res-zoom, docs/porting/
    /// resolution-zoom.md), shown like `Fit` (so no bars once the resolution matches). Frames of another aspect
    /// (menus at 800x600) are shown like `Fit`.
    #[default]
    NativeWide,
}

impl DisplayMode {
    pub const ALL: [DisplayMode; 4] = [DisplayMode::Fit, DisplayMode::Fill, DisplayMode::Stretch, DisplayMode::NativeWide];
    pub fn label(self) -> &'static str {
        match self {
            DisplayMode::Fit => "Fit",
            DisplayMode::Fill => "Fill",
            DisplayMode::Stretch => "Stretch",
            DisplayMode::NativeWide => "Native wide",
        }
    }
    pub fn next(self) -> DisplayMode {
        let i = Self::ALL.iter().position(|&m| m == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
    /// Parse a CLI / settings name (`fit`, `fill`, `stretch`, `native`/`native-wide`).
    pub fn parse(s: &str) -> Option<DisplayMode> {
        match s.to_ascii_lowercase().as_str() {
            "fit" | "letterbox" => Some(DisplayMode::Fit),
            "fill" | "crop" => Some(DisplayMode::Fill),
            "stretch" => Some(DisplayMode::Stretch),
            "native" | "native-wide" | "nativewide" | "wide" => Some(DisplayMode::NativeWide),
            _ => None,
        }
    }
}

/// Texture filter used when scaling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ScaleFilter {
    /// Blocky pixels (uneven pixel sizes at non-integer scales).
    Nearest,
    /// Bilinear (soft).
    Linear,
    /// Sharp bilinear: nearest inside each source pixel, linear only across the edge band (crisp and even).
    #[default]
    SharpBilinear,
}

impl ScaleFilter {
    pub const ALL: [ScaleFilter; 3] = [ScaleFilter::Nearest, ScaleFilter::Linear, ScaleFilter::SharpBilinear];
    pub fn label(self) -> &'static str {
        match self {
            ScaleFilter::Nearest => "Pixel",
            ScaleFilter::Linear => "Smooth",
            ScaleFilter::SharpBilinear => "Sharp",
        }
    }
    pub fn next(self) -> ScaleFilter {
        let i = Self::ALL.iter().position(|&m| m == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
    pub fn parse(s: &str) -> Option<ScaleFilter> {
        match s.to_ascii_lowercase().as_str() {
            "nearest" | "pixel" => Some(ScaleFilter::Nearest),
            "linear" | "bilinear" | "smooth" => Some(ScaleFilter::Linear),
            "sharp" | "sharp-bilinear" => Some(ScaleFilter::SharpBilinear),
            _ => None,
        }
    }
}

/// FPS counter (drawn by [`crate::fps`] in the top-right corner of the visible game area).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FpsCounter {
    #[default]
    Off,
    /// Game frames per second.
    Fps,
    /// FPS + frame time (avg / worst) + guest MIPS (+ display refresh rate when the frontend reports it).
    Detailed,
}

impl FpsCounter {
    pub const ALL: [FpsCounter; 3] = [FpsCounter::Off, FpsCounter::Fps, FpsCounter::Detailed];
    pub fn label(self) -> &'static str {
        match self {
            FpsCounter::Off => "Off",
            FpsCounter::Fps => "FPS",
            FpsCounter::Detailed => "FPS + details",
        }
    }
    pub fn next(self) -> FpsCounter {
        let i = Self::ALL.iter().position(|&m| m == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
    pub fn parse(s: &str) -> Option<FpsCounter> {
        match s.to_ascii_lowercase().as_str() {
            "off" | "0" | "none" => Some(FpsCounter::Off),
            "fps" | "on" | "1" => Some(FpsCounter::Fps),
            "full" | "detailed" | "details" | "2" => Some(FpsCounter::Detailed),
            _ => None,
        }
    }
}

/// `display` section of `mods-settings.json`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplaySettings {
    pub mode: DisplayMode,
    pub filter: ScaleFilter,
    pub fps_counter: FpsCounter,
    /// Requested display refresh rate in Hz (Android `ANativeWindow_setFrameRate`); 0 = the highest the panel
    /// offers.
    pub refresh_hz: u32,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        DisplaySettings { mode: DisplayMode::default(), filter: ScaleFilter::default(), fps_counter: FpsCounter::Off, refresh_hz: 0 }
    }
}

/// UI mode A in `NativeWide` with the custom resolution (docs/porting/display.md "Classic panel"): the original
/// panel (frame `x < w`, `y < h` = the panel art of the current layout variant) is shown at a constant scale
/// (`window height / h`, the art fills the screen height at every zoom level) on the left; the map part of the frame
/// (`x >= w`) fills the rest of the window. The zoom picks render widths that keep the map's aspect
/// (`crate::zoom::classic_width`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelSplit {
    pub w: u32,
    pub h: u32,
}

/// Placement of a `game_w` x `game_h` frame in a `win_w` x `win_h` window (physical pixels). The destination
/// rectangle may extend past the window (`Fill`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    pub mode: DisplayMode,
    pub game_w: u32,
    pub game_h: u32,
    pub win_w: u32,
    pub win_h: u32,
    /// Destination rectangle of the whole game frame in window pixels.
    pub dst_x: f32,
    pub dst_y: f32,
    pub dst_w: f32,
    pub dst_h: f32,
    /// Full-screen menu layout ([`crate::menu_layout`]): the frame is shown re-arranged on a canvas. Then
    /// `dst_*` describe the canvas placement (origin, frame size times the canvas scale) and the window is fully
    /// covered (no bars).
    pub layout: Option<&'static MenuLayout>,
    /// Classic panel shown at its own scale ([`PanelSplit`]); then `dst_*` = the whole window.
    pub panel: Option<PanelSplit>,
}

impl Viewport {
    pub fn new(mode: DisplayMode, game_w: u32, game_h: u32, win_w: u32, win_h: u32) -> Viewport {
        let (gw, gh) = (game_w.max(1), game_h.max(1));
        let (ww, wh) = (win_w.max(1), win_h.max(1));
        let (sx, sy) = (ww as f64 / gw as f64, wh as f64 / gh as f64);
        let (dw, dh) = match mode {
            DisplayMode::Fit | DisplayMode::NativeWide => {
                let s = sx.min(sy);
                // Snap to the window edge when within half a pixel (avoid 1 px bars from rounding).
                let dw = if (gw as f64 * s - ww as f64).abs() < 1.0 { ww as f64 } else { (gw as f64 * s).round() };
                let dh = if (gh as f64 * s - wh as f64).abs() < 1.0 { wh as f64 } else { (gh as f64 * s).round() };
                (dw, dh)
            }
            DisplayMode::Fill => {
                let s = sx.max(sy);
                ((gw as f64 * s).round().max(ww as f64), (gh as f64 * s).round().max(wh as f64))
            }
            DisplayMode::Stretch => (ww as f64, wh as f64),
        };
        let dx = ((ww as f64 - dw) / 2.0).round();
        let dy = ((wh as f64 - dh) / 2.0).round();
        Viewport { mode, game_w: gw, game_h: gh, win_w: ww, win_h: wh, dst_x: dx as f32, dst_y: dy as f32, dst_w: dw as f32, dst_h: dh as f32, layout: None, panel: None }
    }

    /// Viewport for this frame: a full-screen menu layout when the frame is a known menu screen and the mode
    /// fills the screen (`NativeWide`, `Fill`), else [`Viewport::new`].
    pub fn for_frame(mode: DisplayMode, pixels: &[u16], game_w: u32, game_h: u32, win_w: u32, win_h: u32) -> Viewport {
        Viewport::for_frame_split(mode, pixels, game_w, game_h, win_w, win_h, None)
    }

    /// [`Viewport::for_frame`] with the Classic panel split the mods published for this frame
    /// ([`crate::layer::Scene::panel`]); a menu layout wins over it.
    pub fn for_frame_split(mode: DisplayMode, pixels: &[u16], game_w: u32, game_h: u32, win_w: u32, win_h: u32, panel: Option<PanelSplit>) -> Viewport {
        if matches!(mode, DisplayMode::NativeWide | DisplayMode::Fill) {
            if let Some(l) = menu_layout::detect(pixels, game_w, game_h) {
                return Viewport::with_layout(mode, l, win_w, win_h);
            }
        }
        Viewport::new_split(mode, game_w, game_h, win_w, win_h, panel)
    }

    /// [`Viewport::new`], with the Classic panel at its own scale when `panel` is given, the mode is `NativeWide`
    /// and the split fits (panel narrower than the frame and the window, art not taller than the frame).
    pub fn new_split(mode: DisplayMode, game_w: u32, game_h: u32, win_w: u32, win_h: u32, panel: Option<PanelSplit>) -> Viewport {
        let mut v = Viewport::new(mode, game_w, game_h, win_w, win_h);
        if let Some(p) = panel {
            let ok = mode == DisplayMode::NativeWide && p.w > 0 && p.h > 0 && p.w < v.game_w && p.h <= v.game_h;
            let pw_win = (p.w as f64 * v.win_h as f64 / p.h.max(1) as f64).round();
            if ok && pw_win + 16.0 < v.win_w as f64 {
                v.panel = Some(p);
                v.dst_x = 0.0;
                v.dst_y = 0.0;
                v.dst_w = v.win_w as f32;
                v.dst_h = v.win_h as f32;
            }
        }
        v
    }

    /// With a panel split: (panel width in window px, window px per game px of the panel).
    fn split(&self) -> Option<(PanelSplit, f32, f32)> {
        let p = self.panel?;
        let ps = self.win_h as f32 / p.h.max(1) as f32;
        Some((p, (p.w as f32 * ps).round(), ps))
    }

    /// Source rectangles (game px: x, y, w, h) and where they are drawn (window px: x, y, w, h) for a frame without
    /// a menu layout: the whole frame, or the panel and the map part with a [`PanelSplit`].
    pub fn quads(&self) -> Vec<([f32; 4], [f32; 4])> {
        let (gw, gh) = (self.game_w as f32, self.game_h as f32);
        match self.split() {
            Some((p, pww, ps)) => vec![
                ([0.0, 0.0, p.w as f32, p.h as f32], [0.0, 0.0, pww, p.h as f32 * ps]),
                ([p.w as f32, 0.0, gw - p.w as f32, gh], [pww, 0.0, self.win_w as f32 - pww, self.win_h as f32]),
            ],
            None => vec![([0.0, 0.0, gw, gh], [self.dst_x, self.dst_y, self.dst_w, self.dst_h])],
        }
    }

    /// Window px per game px of the panel part (`None` without a split).
    pub fn panel_scale(&self) -> Option<f32> {
        self.split().map(|s| s.2)
    }

    /// Viewport showing the frame through a menu layout.
    pub fn with_layout(mode: DisplayMode, l: &'static MenuLayout, win_w: u32, win_h: u32) -> Viewport {
        let (ww, wh) = (win_w.max(1), win_h.max(1));
        let (ox, oy, sc) = menu_layout::canvas_placement(l, ww, wh);
        let (gw, gh) = (l.frame[0] as u32, l.frame[1] as u32);
        Viewport { mode, game_w: gw, game_h: gh, win_w: ww, win_h: wh, dst_x: ox, dst_y: oy, dst_w: gw as f32 * sc, dst_h: gh as f32 * sc, layout: Some(l), panel: None }
    }

    /// Window pixel -> game pixel, clamped to the frame.
    pub fn window_to_game(&self, x: f32, y: f32) -> (i32, i32) {
        let (gx, gy) = self.window_to_game_f(x, y);
        (
            (gx.floor() as i64).clamp(0, self.game_w as i64 - 1) as i32,
            (gy.floor() as i64).clamp(0, self.game_h as i64 - 1) as i32,
        )
    }

    /// Window pixel -> game coordinates (unclamped, fractional).
    pub fn window_to_game_f(&self, x: f32, y: f32) -> (f32, f32) {
        if let Some((p, pww, ps)) = self.split() {
            if x < pww {
                return (x / ps, y / ps);
            }
            let mw = (self.win_w as f32 - pww).max(1.0);
            return (p.w as f32 + (x - pww) * (self.game_w - p.w) as f32 / mw, y * self.game_h as f32 / self.win_h as f32);
        }
        if let Some(l) = self.layout {
            let (sx, sy) = self.scale();
            return l.canvas_to_game((x - self.dst_x) / sx, (y - self.dst_y) / sy);
        }
        (
            ((x as f64 - self.dst_x as f64) * self.game_w as f64 / self.dst_w as f64) as f32,
            ((y as f64 - self.dst_y as f64) * self.game_h as f64 / self.dst_h as f64) as f32,
        )
    }

    /// Game pixel -> window pixel (top-left corner of that game pixel).
    pub fn game_to_window(&self, gx: f32, gy: f32) -> (f32, f32) {
        if let Some((p, pww, ps)) = self.split() {
            if gx < p.w as f32 {
                return (gx * ps, gy * ps);
            }
            let mw = self.win_w as f32 - pww;
            return (pww + (gx - p.w as f32) * mw / (self.game_w - p.w).max(1) as f32, gy * self.win_h as f32 / self.game_h as f32);
        }
        if let Some(l) = self.layout {
            // Game pixels not shown by any piece go far off screen.
            let (u, v) = l.game_to_canvas(gx, gy).unwrap_or((-1.0e6, -1.0e6));
            let (sx, sy) = self.scale();
            return (self.dst_x + u * sx, self.dst_y + v * sy);
        }
        (self.dst_x + gx * self.dst_w / self.game_w as f32, self.dst_y + gy * self.dst_h / self.game_h as f32)
    }

    /// Window pixels per game pixel (x, y); with a panel split: of the map part.
    pub fn scale(&self) -> (f32, f32) {
        if let Some((p, pww, _)) = self.split() {
            return ((self.win_w as f32 - pww) / (self.game_w - p.w).max(1) as f32, self.win_h as f32 / self.game_h as f32);
        }
        (self.dst_w / self.game_w as f32, self.dst_h / self.game_h as f32)
    }

    /// Part of the game frame that is on screen, in game pixels (x, y, w, h). The whole frame except in `Fill`.
    pub fn visible_game_rect(&self) -> (i32, i32, i32, i32) {
        if self.layout.is_some() || self.panel.is_some() {
            return (0, 0, self.game_w as i32, self.game_h as i32);
        }
        let (x0, y0) = self.window_to_game_f(0.0, 0.0);
        let (x1, y1) = self.window_to_game_f(self.win_w as f32, self.win_h as f32);
        let x0 = (x0.max(0.0).ceil() as i32).min(self.game_w as i32);
        let y0 = (y0.max(0.0).ceil() as i32).min(self.game_h as i32);
        let x1 = (x1.min(self.game_w as f32).floor() as i32).max(x0);
        let y1 = (y1.min(self.game_h as f32).floor() as i32).max(y0);
        (x0, y0, x1 - x0, y1 - y0)
    }

    /// Destination rectangle clipped to the window: (x, y, w, h) in whole window pixels.
    pub fn dst_rect_clipped(&self) -> (i32, i32, i32, i32) {
        if self.layout.is_some() || self.panel.is_some() {
            return (0, 0, self.win_w as i32, self.win_h as i32);
        }
        let x0 = self.dst_x.max(0.0).round() as i32;
        let y0 = self.dst_y.max(0.0).round() as i32;
        let x1 = (self.dst_x + self.dst_w).min(self.win_w as f32).round() as i32;
        let y1 = (self.dst_y + self.dst_h).min(self.win_h as f32).round() as i32;
        (x0, y0, (x1 - x0).max(0), (y1 - y0).max(0))
    }

    /// Black bar sizes in window pixels: (left, right, top, bottom).
    pub fn bars(&self) -> (u32, u32, u32, u32) {
        let (x, y, w, h) = self.dst_rect_clipped();
        (x as u32, (self.win_w as i32 - x - w).max(0) as u32, y as u32, (self.win_h as i32 - y - h).max(0) as u32)
    }

    /// Aspect-matched game resolution for `NativeWide`: the window's aspect ratio with height `game_h`
    /// (width rounded to a multiple of 8, as DirectDraw surfaces of the game like).
    pub fn native_wide_size(win_w: u32, win_h: u32, game_h: u32) -> (u32, u32) {
        let w = (game_h as u64 * win_w.max(1) as u64 + win_h.max(1) as u64 / 2) / win_h.max(1) as u64;
        ((((w + 4) / 8) * 8).max(8) as u32, game_h)
    }
}

// ---------------------------------------------------------------- CPU scaler (desktop window, CPU fallback)

/// One axis of a scaling: for each destination pixel the two source taps and the weight of the second (0..=256).
#[derive(Debug, Clone, Default)]
pub struct AxisTaps {
    pub i0: Vec<u32>,
    pub i1: Vec<u32>,
    pub w1: Vec<u16>,
}

/// Taps for `dst_n` destination pixels starting at window coordinate `dst_from`, for a source of `src_n` pixels
/// drawn at `dst_off` with size `dst_size` (window pixels).
pub fn axis_taps(filter: ScaleFilter, src_n: u32, dst_from: i32, dst_n: u32, dst_off: f32, dst_size: f32) -> AxisTaps {
    let mut t = AxisTaps { i0: Vec::with_capacity(dst_n as usize), i1: Vec::with_capacity(dst_n as usize), w1: Vec::with_capacity(dst_n as usize) };
    let n = src_n.max(1) as f64;
    let scale = dst_size as f64 / n; // window px per source px
    let last = src_n.saturating_sub(1);
    for d in 0..dst_n {
        // Source coordinate of the destination pixel centre.
        let s = ((dst_from + d as i32) as f64 + 0.5 - dst_off as f64) / scale;
        let (i0, i1, w) = match filter {
            ScaleFilter::Nearest => {
                let i = (s.floor().max(0.0) as u32).min(last);
                (i, i, 0.0)
            }
            ScaleFilter::Linear => {
                let c = s - 0.5;
                let f = c.floor();
                (f.max(0.0) as u32, ((f + 1.0).max(0.0) as u32), c - f)
            }
            ScaleFilter::SharpBilinear => {
                // Same as the GLES shader: linear only across a 1-window-pixel band around texel edges.
                let fl = s.floor();
                let fr = s - fl - 0.5;
                let range = 0.5 - 0.5 / scale.max(1.0);
                let f = (fr - fr.clamp(-range, range)) * scale.max(1.0) + 0.5;
                let c = fl + f - 0.5;
                let cf = c.floor();
                (cf.max(0.0) as u32, ((cf + 1.0).max(0.0) as u32), c - cf)
            }
        };
        t.i0.push(i0.min(last));
        t.i1.push(i1.min(last));
        t.w1.push((w.clamp(0.0, 1.0) * 256.0).round() as u16);
    }
    t
}

#[inline]
fn rgb565_to_888(p: u16) -> (u32, u32, u32) {
    let r = ((p >> 11) & 31) as u32;
    let g = ((p >> 5) & 63) as u32;
    let b = (p & 31) as u32;
    ((r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2))
}

/// Scale an RGB565 frame into an XRGB8888 window buffer (`dst` = `win_w * win_h`) with black bars.
pub fn scale_to_xrgb(vp: &Viewport, filter: ScaleFilter, src: &[u16], dst: &mut [u32]) {
    let (ww, wh) = (vp.win_w as usize, vp.win_h as usize);
    let (gw, gh) = (vp.game_w as usize, vp.game_h as usize);
    if dst.len() < ww * wh || src.len() < gw * gh {
        return;
    }
    if let Some(l) = vp.layout {
        menu_layout::render_xrgb(l, src, gw, gh, dst, ww, wh);
        return;
    }
    dst[..ww * wh].fill(0);
    for (s, d) in vp.quads() {
        scale_region(filter, src, gw, s, d, ww, wh, dst);
    }
}

/// Scale the source rectangle `s` (game px x, y, w, h) of an RGB565 frame (`gw` wide) to the window rectangle `d`
/// (clipped to the `ww` x `wh` window buffer).
fn scale_region(filter: ScaleFilter, src: &[u16], gw: usize, s: [f32; 4], d: [f32; 4], ww: usize, wh: usize, dst: &mut [u32]) {
    let (sx, sy, sw, sh) = (s[0] as usize, s[1] as usize, s[2] as u32, s[3] as u32);
    let rx = d[0].max(0.0).round() as i32;
    let ry = d[1].max(0.0).round() as i32;
    let rw = ((d[0] + d[2]).min(ww as f32).round() as i32 - rx).max(0);
    let rh = ((d[1] + d[3]).min(wh as f32).round() as i32 - ry).max(0);
    if rw == 0 || rh == 0 || sw == 0 || sh == 0 {
        return;
    }
    let tx = axis_taps(filter, sw, rx, rw as u32, d[0], d[2]);
    let ty = axis_taps(filter, sh, ry, rh as u32, d[1], d[3]);
    let plain = filter == ScaleFilter::Nearest;
    for (yi, y) in (ry as usize..ry as usize + rh as usize).enumerate() {
        let r0 = (sy + ty.i0[yi] as usize) * gw + sx;
        let r1 = (sy + ty.i1[yi] as usize) * gw + sx;
        let row0 = &src[r0..r0 + sw as usize];
        let row1 = &src[r1..r1 + sw as usize];
        let wy = ty.w1[yi] as u32;
        let out = &mut dst[y * ww + rx as usize..y * ww + rx as usize + rw as usize];
        if plain || (wy == 0 && tx.w1.iter().all(|&w| w == 0)) {
            for (xi, o) in out.iter_mut().enumerate() {
                let (r, g, b) = rgb565_to_888(row0[tx.i0[xi] as usize]);
                *o = (r << 16) | (g << 8) | b;
            }
            continue;
        }
        for (xi, o) in out.iter_mut().enumerate() {
            let wx = tx.w1[xi] as u32;
            let (a, b_) = (tx.i0[xi] as usize, tx.i1[xi] as usize);
            let p00 = rgb565_to_888(row0[a]);
            let p01 = rgb565_to_888(row0[b_]);
            let p10 = rgb565_to_888(row1[a]);
            let p11 = rgb565_to_888(row1[b_]);
            let lerp = |c00: u32, c01: u32, c10: u32, c11: u32| {
                let top = c00 * (256 - wx) + c01 * wx;
                let bot = c10 * (256 - wx) + c11 * wx;
                (top * (256 - wy) + bot * wy + 32768) >> 16
            };
            let r = lerp(p00.0, p01.0, p10.0, p11.0);
            let g = lerp(p00.1, p01.1, p10.1, p11.1);
            let b = lerp(p00.2, p01.2, p10.2, p11.2);
            *o = (r << 16) | (g << 8) | b;
        }
    }
}

// ---------------------------------------------------------------- machine state (frontend <-> mods)

/// Display info published by the frontend.
#[derive(Debug, Clone, Default)]
pub struct DisplayState {
    /// Viewport used for the last shown frame (`None` headless / before the first frame).
    pub viewport: Option<Viewport>,
    /// Display (host) presents per second, if the frontend measures them (Android render thread).
    pub host_fps: Option<f32>,
    /// Display refresh rate the frontend runs at, if known.
    pub refresh_hz: Option<f32>,
    /// Bumped whenever [`DisplaySettings`] change through this module (frontends may poll it).
    pub settings_seq: u64,
}

/// Frontend: publish the viewport of the shown frame.
pub fn set_viewport(m: &mut Machine, vp: Option<Viewport>) {
    m.state::<DisplayState>().borrow_mut().viewport = vp;
}

/// Viewport of the shown frame (`None` headless).
pub fn viewport(m: &mut Machine) -> Option<Viewport> {
    m.state::<DisplayState>().borrow_mut().viewport
}

/// Window size of the frontend (`None` headless).
pub fn window_size(m: &mut Machine) -> Option<(u32, u32)> {
    viewport(m).map(|v| (v.win_w, v.win_h))
}

/// Visible part of the game frame (game pixels); the whole last frame when no viewport is known.
pub fn visible_game_rect(m: &mut Machine) -> (i32, i32, i32, i32) {
    let size = m.last_frame_size();
    match viewport(m) {
        Some(v) if (v.game_w, v.game_h) == size || size == (0, 0) => v.visible_game_rect(),
        Some(v) => Viewport::new(v.mode, size.0, size.1, v.win_w, v.win_h).visible_game_rect(),
        None => (0, 0, size.0 as i32, size.1 as i32),
    }
}

/// Classic panel split for the last frame (see [`PanelSplit`]): UI mode A, display mode `NativeWide`, custom
/// resolution on, in game and the layout globals match the frame. `S3_PANEL_SPLIT=0` turns it off.
pub fn panel_split(m: &mut Machine) -> Option<PanelSplit> {
    let (fw, fh) = m.last_frame_size();
    let (mode, ui) = {
        let st = m.state::<crate::ModsState>();
        let s = st.borrow_mut();
        (s.settings.display.mode, s.settings.ui_mode)
    };
    if mode != DisplayMode::NativeWide || ui != crate::settings::UiMode::ClassicTouch || std::env::var("S3_PANEL_SPLIT").is_ok_and(|v| v == "0") {
        return None;
    }
    if !crate::zoom::enabled(m) || !crate::res::in_game(m) {
        return None;
    }
    if (m.mem.read_u32(crate::res::L_SCREEN_W), m.mem.read_u32(crate::res::L_SCREEN_H)) != (fw, fh) {
        return None;
    }
    let pw = m.mem.read_u32(crate::res::L_PANEL_W);
    let v = crate::res::variant(m.mem.read_u32(crate::res::L_RES))?;
    (pw > 0 && pw == v.panel_w && pw < fw && v.h <= fh).then_some(PanelSplit { w: pw, h: v.h })
}

/// Viewport of the last game frame as the frontend will show it: published window size, display mode, the panel
/// split of this frame (the published menu layout when it is for a frame of this size). Headless without a window
/// size: the frame itself (1:1).
pub fn frame_viewport(m: &mut Machine) -> Viewport {
    let (fw, fh) = m.last_frame_size();
    let (fw, fh) = (fw.max(1), fh.max(1));
    let panel = panel_split(m);
    match viewport(m) {
        Some(v) if v.layout.is_some() && (v.game_w, v.game_h) == (fw, fh) => v,
        Some(v) => Viewport::new_split(v.mode, fw, fh, v.win_w, v.win_h, panel),
        None => Viewport::new(DisplayMode::Stretch, fw, fh, fw, fh),
    }
}

/// Frontend: report display-side rates (shown by the detailed FPS counter).
pub fn set_host_rates(m: &mut Machine, host_fps: Option<f32>, refresh_hz: Option<f32>) {
    let st = m.state::<DisplayState>();
    let mut s = st.borrow_mut();
    s.host_fps = host_fps;
    s.refresh_hz = refresh_hz;
}

/// Current display settings (cheap copy; frontends call it once per slice).
pub fn display_settings(m: &mut Machine) -> DisplaySettings {
    m.state::<crate::ModsState>().borrow_mut().settings.display
}

/// Change and persist the display settings.
pub fn set_display(m: &mut Machine, d: DisplaySettings) {
    let mut s = crate::settings(m);
    if s.display == d {
        return;
    }
    s.display = d;
    if let Err(e) = crate::set_settings(m, s) {
        m.log(s3_runtime::LogLevel::Warn, &format!("display: saving settings failed: {e}"));
    }
    m.state::<DisplayState>().borrow_mut().settings_seq += 1;
    m.log(s3_runtime::LogLevel::Info, &format!("display: {:?} / {:?} / fps {:?}", d.mode, d.filter, d.fps_counter));
}

/// Set the display settings for this run only (not saved; e.g. `s3run --display/--fps`).
pub fn override_display(m: &mut Machine, d: DisplaySettings) {
    m.state::<crate::ModsState>().borrow_mut().settings.display = d;
    m.state::<DisplayState>().borrow_mut().settings_seq += 1;
}

/// Menu helpers: next value, persisted. They return the new value (for the button label).
pub fn cycle_mode(m: &mut Machine) -> DisplayMode {
    let mut d = display_settings(m);
    d.mode = d.mode.next();
    set_display(m, d);
    d.mode
}

pub fn cycle_filter(m: &mut Machine) -> ScaleFilter {
    let mut d = display_settings(m);
    d.filter = d.filter.next();
    set_display(m, d);
    d.filter
}

pub fn cycle_fps(m: &mut Machine) -> FpsCounter {
    let mut d = display_settings(m);
    d.fps_counter = d.fps_counter.next();
    set_display(m, d);
    d.fps_counter
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_pillarbox_on_fold_cover() {
        // 800x600 on the Fold4 cover screen in landscape (2229x904).
        let v = Viewport::new(DisplayMode::Fit, 800, 600, 2229, 904);
        assert_eq!(v.dst_h, 904.0);
        assert_eq!(v.dst_w, 1205.0);
        assert_eq!(v.dst_x, 512.0);
        assert_eq!(v.window_to_game(2229.0 / 2.0, 452.0), (400, 300));
        assert_eq!(v.window_to_game(10.0, 10.0), (0, 6));
        assert_eq!(v.visible_game_rect(), (0, 0, 800, 600));
        let (l, r, t, b) = v.bars();
        assert_eq!((l, t, b), (512, 0, 0));
        assert_eq!(r, 2229 - 512 - 1205);
    }

    #[test]
    fn fill_crops_and_maps() {
        let v = Viewport::new(DisplayMode::Fill, 800, 600, 2229, 904);
        assert_eq!(v.dst_w, 2229.0);
        assert!(v.dst_h > 904.0);
        assert_eq!(v.bars(), (0, 0, 0, 0));
        let (x, y, w, h) = v.visible_game_rect();
        assert_eq!((x, w), (0, 800));
        assert!(y > 100 && h < 340, "{y} {h}");
        // Window centre -> game centre; corners map inside the visible rect.
        assert_eq!(v.window_to_game(1114.5, 452.0), (400, 300));
        // The top window row shows game row y-1 partly; the visible rect counts fully visible rows only.
        let (_, gy) = v.window_to_game(0.0, 0.0);
        assert!(gy == y || gy == y - 1, "{gy} {y}");
    }

    #[test]
    fn stretch_and_round_trip() {
        let v = Viewport::new(DisplayMode::Stretch, 640, 480, 1280, 720);
        assert_eq!((v.dst_x, v.dst_y, v.dst_w, v.dst_h), (0.0, 0.0, 1280.0, 720.0));
        assert_eq!(v.window_to_game(1279.0, 719.0), (639, 479));
        let (wx, wy) = v.game_to_window(320.0, 240.0);
        assert_eq!(v.window_to_game(wx + 0.1, wy + 0.1), (320, 240));
        assert_eq!(v.scale(), (2.0, 1.5));
    }

    #[test]
    fn native_wide_matching_aspect_has_no_bars() {
        let (w, h) = Viewport::native_wide_size(2229, 904, 600);
        assert_eq!(h, 600);
        assert_eq!(w % 8, 0);
        assert!((1472..=1488).contains(&w), "{w}");
        let v = Viewport::new(DisplayMode::NativeWide, w, h, 2229, 904);
        let (l, r, t, b) = v.bars();
        assert!(l + r <= 4 && t + b == 0, "{:?}", v.bars());
    }

    #[test]
    fn cpu_scaler_nearest_and_sharp() {
        // 2x1 source: red, blue; scaled 2x into 4x2 window with Fit.
        let src = [0xF800u16, 0x001F];
        let v = Viewport::new(DisplayMode::Stretch, 2, 1, 4, 2);
        let mut dst = vec![0u32; 8];
        scale_to_xrgb(&v, ScaleFilter::Nearest, &src, &mut dst);
        assert_eq!(&dst[..4], &[0xFF0000, 0xFF0000, 0x0000FF, 0x0000FF]);
        assert_eq!(&dst[4..], &dst[..4].to_vec()[..]);
        // Sharp bilinear at an integer scale equals nearest.
        scale_to_xrgb(&v, ScaleFilter::SharpBilinear, &src, &mut dst);
        assert_eq!(&dst[..4], &[0xFF0000, 0xFF0000, 0x0000FF, 0x0000FF]);
        // Linear mixes in the middle.
        scale_to_xrgb(&v, ScaleFilter::Linear, &src, &mut dst);
        assert_eq!(dst[0], 0xFF0000);
        assert!(dst[1] >> 16 > 0 && dst[1] & 0xFF > 0, "{:x}", dst[1]);
        // Fit with bars: 2x1 in 4x4 -> rows 1..3 drawn, others black.
        let v = Viewport::new(DisplayMode::Fit, 2, 1, 4, 4);
        let mut dst = vec![7u32; 16];
        scale_to_xrgb(&v, ScaleFilter::Nearest, &src, &mut dst);
        assert_eq!(&dst[..4], &[0, 0, 0, 0]);
        assert_eq!(dst[4], 0xFF0000);
        assert_eq!(&dst[12..], &[0, 0, 0, 0]);
    }

    #[test]
    fn sharp_taps_are_monotonic_and_mostly_nearest() {
        let t = axis_taps(ScaleFilter::SharpBilinear, 800, 0, 2229, 0.0, 2229.0);
        let mut prev = 0.0f64;
        let mut blended = 0;
        for i in 0..t.i0.len() {
            let pos = t.i0[i] as f64 + t.w1[i] as f64 / 256.0;
            assert!(pos + 1e-9 >= prev, "not monotonic at {i}");
            prev = pos;
            if t.w1[i] != 0 && t.w1[i] != 256 {
                blended += 1;
            }
        }
        // ~2.8x scale: at most about one blended pixel per source pixel.
        assert!(blended <= 800, "{blended}");
    }

    #[test]
    fn panel_split_maps_both_parts() {
        // Fold4 cover 2316x904, Classic at 600 high: 800x600 art (panel 176), frame 1480x600.
        let p = PanelSplit { w: 176, h: 600 };
        let v = Viewport::new_split(DisplayMode::NativeWide, 1480, 600, 2316, 904, Some(p));
        assert_eq!(v.panel, Some(p));
        assert_eq!(v.bars(), (0, 0, 0, 0));
        let ps = v.panel_scale().unwrap();
        assert!((ps - 904.0 / 600.0).abs() < 1e-4);
        // Panel point.
        let (wx, wy) = v.game_to_window(100.0, 300.0);
        assert!((wx - 100.0 * ps).abs() < 0.01 && (wy - 452.0).abs() < 0.01);
        assert_eq!(v.window_to_game(wx + 0.1, wy + 0.1), (100, 300));
        // Map point round trip and the right edge.
        let (wx, wy) = v.game_to_window(1000.0, 450.0);
        assert_eq!(v.window_to_game(wx + 0.01, wy + 0.01), (1000, 450));
        assert_eq!(v.window_to_game(2315.0, 903.0), (1479, 599));
        // A taller frame (780 high, 1024x768 art): the panel keeps (almost) the same on-screen width.
        let v2 = Viewport::new_split(DisplayMode::NativeWide, 1880, 780, 2316, 904, Some(PanelSplit { w: 216, h: 768 }));
        let w1 = v.game_to_window(176.0, 0.0).0;
        let w2 = v2.game_to_window(216.0, 0.0).0;
        assert!((w1 - w2).abs() < 20.0, "{w1} {w2}");
        // Two quads; the CPU scaler fills the whole window.
        assert_eq!(v.quads().len(), 2);
        let src: Vec<u16> = (0..1480 * 600).map(|i| if i % 1480 < 176 { 0xF800 } else { 0x001F }).collect();
        let mut dst = vec![0u32; 2316 * 904];
        scale_to_xrgb(&v, ScaleFilter::Nearest, &src, &mut dst);
        assert_eq!(dst[0], 0xFF0000);
        assert_eq!(dst[903 * 2316 + 260], 0xFF0000);
        assert_eq!(dst[903 * 2316 + 2315], 0x0000FF);
        assert!(dst.iter().all(|&p| p != 0));
        // Not in Fit / without a panel.
        assert_eq!(Viewport::new_split(DisplayMode::Fit, 1480, 600, 2316, 904, Some(p)).panel, None);
        assert_eq!(Viewport::new_split(DisplayMode::NativeWide, 1480, 600, 2316, 904, None).panel, None);
    }

    #[test]
    fn enum_parsing_and_cycles() {
        assert_eq!(DisplayMode::parse("fill"), Some(DisplayMode::Fill));
        assert_eq!(DisplayMode::parse("native"), Some(DisplayMode::NativeWide));
        assert_eq!(DisplayMode::NativeWide.next(), DisplayMode::Fit);
        assert_eq!(FpsCounter::parse("full"), Some(FpsCounter::Detailed));
        assert_eq!(FpsCounter::Detailed.next(), FpsCounter::Off);
        assert_eq!(ScaleFilter::parse("sharp"), Some(ScaleFilter::SharpBilinear));
        let s: DisplaySettings = serde_json_roundtrip(&DisplaySettings { mode: DisplayMode::Fill, ..Default::default() });
        assert_eq!(s.mode, DisplayMode::Fill);
    }

    fn serde_json_roundtrip(d: &DisplaySettings) -> DisplaySettings {
        serde_json::from_str(&serde_json::to_string(d).unwrap()).unwrap()
    }
}
