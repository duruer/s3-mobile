//! Window-resolution overlay layer (docs/porting/mods.md "Window layer", display.md "Overlay layer").
//!
//! Mods draw their UI (Mobile HUD, touch feedback, FPS counter, Classic floating buttons, text) in **window
//! pixels** into a [`Layer`], independent of the game's render size (zoom), so it keeps its on-screen size and stays
//! sharp at every zoom level. The guest frame itself then holds only game pixels.
//!
//! - [`Layer`] is a recorder: every call adds one command ([`Cmd`]) with its clipped bounds and a hash to a
//!   [`DrawList`]. Recording is cheap (engine thread, every presented frame); nothing is rasterized there.
//! - The mods framework publishes a [`Scene`] (draw list + the Classic panel split for this frame) to the
//!   [`SceneSlot`] the frontend registered ([`crate::set_layer_sink`]), only when it changed (list hash).
//! - The frontend keeps a [`LayerRaster`] (premultiplied RGBA8 at window size) and calls [`LayerRaster::update`]:
//!   only the rectangle covered by commands that appeared or disappeared since the last list is cleared and
//!   redrawn (all commands intersecting it, in order), and that rectangle is returned for the texture upload.
//!   Android uploads it as a second texture blended over the game quad; `s3run` blends it on the CPU.
//!
//! Images are passed as [`ImageRef`] (`Arc<Image>`): the pointer identifies the image (the previous list holds a
//! reference, so an address cannot be reused while it is compared). [`Layer::blit_image`] takes a one-off image
//! and hashes its pixels instead.
//!
//! Text for other mods (e.g. the game's own messages redrawn in this layer): [`Layer::text`],
//! [`Layer::text_centered`], [`Layer::text_box`] with [`crate::overlay::measure_text`]; sizes in window pixels, use
//! [`Layer::ui`] (window height / 600) to scale like the rest of the UI.
//!
//! Pure presentation: nothing here touches guest state.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::display::PanelSplit;
use crate::overlay::{self, Image, Rgba, Weight};

/// A shared, immutable RGBA image for [`Layer::blit_scaled`].
pub type ImageRef = Arc<Image>;

/// Rectangle (x0, y0, x1, y1), exclusive end.
pub type Bounds = (i32, i32, i32, i32);

/// One drawing command (window pixels).
#[derive(Debug, Clone)]
pub enum Cmd {
    Rect { x: i32, y: i32, w: i32, h: i32, c: Rgba },
    RRect { x: i32, y: i32, w: i32, h: i32, r: f32, stroke: Option<f32>, c: Rgba },
    Circle { cx: f32, cy: f32, r: f32, c: Rgba },
    /// Ellipse outline (radii `rx`, `ry`, thickness `t`).
    Ring { cx: f32, cy: f32, rx: f32, ry: f32, t: f32, c: Rgba },
    /// Nearest-neighbour scaled image (`key` identifies the pixels).
    Image { img: ImageRef, key: u64, x: i32, y: i32, w: i32, h: i32, opacity: f32 },
    /// The game's sheared minimap shape (`crate::ui_hud_minimap`): image row `r` col `c` at
    /// `x + ((n - r)/2 + c) * s`, `y + r * s`.
    Map { img: ImageRef, key: u64, x: f32, y: f32, s: f32 },
    /// Thick line from small squares (`t` px).
    Line { x0: f32, y0: f32, x1: f32, y1: f32, t: f32, c: Rgba },
    Text { x: i32, y: i32, px: f32, weight: Weight, c: Rgba, text: Box<str> },
}

/// A recorded command with its clip, clipped bounds and hash.
#[derive(Debug, Clone)]
pub struct Item {
    pub clip: Bounds,
    pub bounds: Bounds,
    pub hash: u64,
    pub cmd: Cmd,
}

/// A frame's overlay: commands in drawing order.
#[derive(Debug, Clone, Default)]
pub struct DrawList {
    pub width: u32,
    pub height: u32,
    pub items: Vec<Item>,
    /// Order-dependent hash of all items (+ size).
    pub hash: u64,
}

impl DrawList {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    /// Union of all item bounds (`None` = empty).
    pub fn bounds(&self) -> Option<Bounds> {
        self.items.iter().map(|i| i.bounds).reduce(union)
    }
}

// ---------------------------------------------------------------- hashing (FNV-1a 64)

#[derive(Clone, Copy)]
struct H(u64);

impl H {
    fn new() -> H {
        H(0xcbf2_9ce4_8422_2325)
    }
    fn u(mut self, v: u64) -> H {
        for b in v.to_le_bytes() {
            self.0 ^= b as u64;
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
        self
    }
    fn i(self, v: i32) -> H {
        self.u(v as u32 as u64)
    }
    fn f(self, v: f32) -> H {
        self.u(v.to_bits() as u64)
    }
    fn c(self, c: Rgba) -> H {
        self.u(u32::from_le_bytes([c.r, c.g, c.b, c.a]) as u64)
    }
    fn bytes(mut self, b: &[u8]) -> H {
        // Word-wise for speed (only one-off images go through here).
        let mut chunks = b.chunks_exact(8);
        for ch in &mut chunks {
            self.0 = (self.0 ^ u64::from_le_bytes(ch.try_into().unwrap())).wrapping_mul(0x0100_0000_01b3).rotate_left(23);
        }
        for &x in chunks.remainder() {
            self.0 = (self.0 ^ x as u64).wrapping_mul(0x0100_0000_01b3);
        }
        self
    }
}

fn cmd_hash(cmd: &Cmd, clip: Bounds) -> u64 {
    let h = H::new().i(clip.0).i(clip.1).i(clip.2).i(clip.3);
    let h = match cmd {
        Cmd::Rect { x, y, w, h: hh, c } => h.u(1).i(*x).i(*y).i(*w).i(*hh).c(*c),
        Cmd::RRect { x, y, w, h: hh, r, stroke, c } => h.u(2).i(*x).i(*y).i(*w).i(*hh).f(*r).f(stroke.unwrap_or(-1.0)).c(*c),
        Cmd::Circle { cx, cy, r, c } => h.u(3).f(*cx).f(*cy).f(*r).c(*c),
        Cmd::Ring { cx, cy, rx, ry, t, c } => h.u(4).f(*cx).f(*cy).f(*rx).f(*ry).f(*t).c(*c),
        Cmd::Image { key, x, y, w, h: hh, opacity, .. } => h.u(5).u(*key).i(*x).i(*y).i(*w).i(*hh).f(*opacity),
        Cmd::Map { key, x, y, s, .. } => h.u(6).u(*key).f(*x).f(*y).f(*s),
        Cmd::Line { x0, y0, x1, y1, t, c } => h.u(7).f(*x0).f(*y0).f(*x1).f(*y1).f(*t).c(*c),
        Cmd::Text { x, y, px, weight, c, text } => h.u(8).i(*x).i(*y).f(*px).u(*weight as u64).c(*c).bytes(text.as_bytes()),
    };
    h.0
}

fn image_ptr_key(img: &ImageRef) -> u64 {
    (Arc::as_ptr(img) as usize as u64) ^ ((img.width as u64) << 40) ^ ((img.height as u64) << 52)
}

fn image_content_key(img: &Image) -> u64 {
    H::new().u(img.width as u64).u(img.height as u64).bytes(&img.rgba).0 | (1 << 63)
}

pub(crate) fn union(a: Bounds, b: Bounds) -> Bounds {
    (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3))
}

fn intersect(a: Bounds, b: Bounds) -> Option<Bounds> {
    let r = (a.0.max(b.0), a.1.max(b.1), a.2.min(b.2), a.3.min(b.3));
    (r.0 < r.2 && r.1 < r.3).then_some(r)
}

// ---------------------------------------------------------------- recorder

/// Recorder for one frame's window overlay (see the module docs). Coordinates are window pixels.
pub struct Layer {
    list: DrawList,
    clip: Bounds,
    ui: f32,
}

impl Layer {
    /// A layer for a `width` x `height` window.
    pub fn new(width: u32, height: u32) -> Layer {
        let (w, h) = (width.max(1), height.max(1));
        Layer {
            list: DrawList { width: w, height: h, items: Vec::new(), hash: 0 },
            clip: (0, 0, w as i32, h as i32),
            ui: h as f32 / 600.0,
        }
    }

    pub fn width(&self) -> u32 {
        self.list.width
    }
    pub fn height(&self) -> u32 {
        self.list.height
    }
    /// UI scale: window height / 600 (1 unit = 1 px of a 600 px high frame shown full height). Sizes that should
    /// look like they did when overlays were drawn into a 600 px high frame are multiplied by this.
    pub fn ui(&self) -> f32 {
        self.ui
    }

    /// Restrict drawing to a rectangle (intersected with the window).
    pub fn set_clip(&mut self, x: i32, y: i32, w: i32, h: i32) {
        let (ww, wh) = (self.list.width as i32, self.list.height as i32);
        self.clip = (x.max(0), y.max(0), (x + w).min(ww), (y + h).min(wh));
    }
    pub fn reset_clip(&mut self) {
        self.clip = (0, 0, self.list.width as i32, self.list.height as i32);
    }

    fn push(&mut self, cmd: Cmd, b: Bounds) {
        let Some(bounds) = intersect(b, self.clip) else { return };
        let hash = cmd_hash(&cmd, self.clip);
        self.list.items.push(Item { clip: self.clip, bounds, hash, cmd });
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: Rgba) {
        if w <= 0 || h <= 0 || c.a == 0 {
            return;
        }
        self.push(Cmd::Rect { x, y, w, h, c }, (x, y, x + w, y + h));
    }

    /// Outline of width `t` inside the rectangle.
    pub fn stroke_rect(&mut self, x: i32, y: i32, w: i32, h: i32, t: i32, c: Rgba) {
        self.fill_rect(x, y, w, t, c);
        self.fill_rect(x, y + h - t, w, t, c);
        self.fill_rect(x, y + t, t, h - 2 * t, c);
        self.fill_rect(x + w - t, y + t, t, h - 2 * t, c);
    }

    pub fn fill_rounded_rect(&mut self, x: i32, y: i32, w: i32, h: i32, r: f32, c: Rgba) {
        if w <= 0 || h <= 0 || c.a == 0 {
            return;
        }
        self.push(Cmd::RRect { x, y, w, h, r, stroke: None, c }, (x, y, x + w, y + h));
    }

    pub fn stroke_rounded_rect(&mut self, x: i32, y: i32, w: i32, h: i32, r: f32, t: f32, c: Rgba) {
        if w <= 0 || h <= 0 || c.a == 0 {
            return;
        }
        self.push(Cmd::RRect { x, y, w, h, r, stroke: Some(t), c }, (x, y, x + w, y + h));
    }

    pub fn fill_circle(&mut self, cx: f32, cy: f32, r: f32, c: Rgba) {
        if r <= 0.0 || c.a == 0 {
            return;
        }
        let b = ((cx - r - 1.0).floor() as i32, (cy - r - 1.0).floor() as i32, (cx + r + 1.0).ceil() as i32, (cy + r + 1.0).ceil() as i32);
        self.push(Cmd::Circle { cx, cy, r, c }, b);
    }

    /// Anti-aliased ellipse outline.
    pub fn ellipse_ring(&mut self, cx: f32, cy: f32, rx: f32, ry: f32, t: f32, c: Rgba) {
        if rx <= 0.0 || ry <= 0.0 || c.a == 0 {
            return;
        }
        let b = ((cx - rx - t - 1.0).floor() as i32, (cy - ry - t - 1.0).floor() as i32, (cx + rx + t + 1.0).ceil() as i32, (cy + ry + t + 1.0).ceil() as i32);
        self.push(Cmd::Ring { cx, cy, rx, ry, t, c }, b);
    }

    pub fn ring(&mut self, cx: f32, cy: f32, r: f32, t: f32, c: Rgba) {
        self.ellipse_ring(cx, cy, r, r, t, c);
    }

    /// Blit an image at its own size.
    pub fn blit(&mut self, img: &ImageRef, x: i32, y: i32, opacity: f32) {
        self.blit_scaled(img, x, y, img.width as i32, img.height as i32, opacity)
    }

    /// Blit an image scaled (nearest neighbour) into `w` x `h` at (`x`,`y`).
    pub fn blit_scaled(&mut self, img: &ImageRef, x: i32, y: i32, w: i32, h: i32, opacity: f32) {
        if w <= 0 || h <= 0 || img.width == 0 || img.height == 0 || opacity <= 0.0 {
            return;
        }
        let key = image_ptr_key(img);
        self.push(Cmd::Image { img: img.clone(), key, x, y, w, h, opacity }, (x, y, x + w, y + h));
    }

    /// Blit a one-off image (its pixels are hashed; prefer [`Layer::blit_scaled`] with a kept `ImageRef`).
    pub fn blit_image(&mut self, img: Image, x: i32, y: i32, w: i32, h: i32, opacity: f32) {
        if w <= 0 || h <= 0 || img.width == 0 || img.height == 0 || opacity <= 0.0 {
            return;
        }
        let key = image_content_key(&img);
        self.push(Cmd::Image { img: Arc::new(img), key, x, y, w, h, opacity }, (x, y, x + w, y + h));
    }

    /// The game's sheared minimap shape of a square image (`n` = image width) scaled by `s` with its bounding box
    /// (1.5 n s x n s) at (`x`,`y`).
    pub fn blit_map(&mut self, img: &ImageRef, x: f32, y: f32, s: f32) {
        let n = img.width as f32;
        if img.width == 0 || img.height < img.width || s <= 0.0 {
            return;
        }
        let key = image_ptr_key(img);
        let b = (x.floor() as i32, y.floor() as i32, (x + 1.5 * n * s).ceil() as i32, (y + n * s).ceil() as i32);
        self.push(Cmd::Map { img: img.clone(), key, x, y, s }, b);
    }

    /// Thick line (squares of `t` px along the line).
    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, t: f32, c: Rgba) {
        if c.a == 0 {
            return;
        }
        let ti = t.round().max(1.0) as i32;
        let b = (x0.min(x1) as i32 - ti, y0.min(y1) as i32 - ti, x0.max(x1) as i32 + ti + 1, y0.max(y1) as i32 + ti + 1);
        self.push(Cmd::Line { x0, y0, x1, y1, t, c }, b);
    }

    /// Text with its top-left at (`x`,`y`), size `px`. Returns the advance width.
    pub fn text(&mut self, x: i32, y: i32, px: f32, weight: Weight, c: Rgba, text: &str) -> i32 {
        let (w, h) = overlay::measure_text(px, weight, text);
        if text.is_empty() || c.a == 0 {
            return w;
        }
        let pad = (px * 0.3).ceil() as i32 + 1;
        let b = (x - pad, y - pad, x + w + pad, y + h + pad);
        self.push(Cmd::Text { x, y, px, weight, c, text: text.into() }, b);
        w
    }

    /// Text centred in a rectangle.
    /// Text centered in a box; a label wider than the box (minus a small margin) is drawn smaller to fit (down to
    /// 60 % of `px`), e.g. long setting labels on buttons.
    pub fn text_centered(&mut self, x: i32, y: i32, w: i32, h: i32, px: f32, weight: Weight, c: Rgba, text: &str) {
        let (mut px, (mut tw, mut th)) = (px, overlay::measure_text(px, weight, text));
        let room = (w as f32 - 0.6 * px).max(1.0);
        if tw as f32 > room {
            px = (px * room / tw as f32).max(px * 0.6);
            (tw, th) = overlay::measure_text(px, weight, text);
        }
        self.text(x + (w - tw) / 2, y + (h - th) / 2, px, weight, c, text);
    }

    /// Text on a rounded, semi-transparent box (message / tooltip style). Returns the box (x, y, w, h).
    pub fn text_box(&mut self, x: i32, y: i32, px: f32, weight: Weight, fg: Rgba, bg: Rgba, text: &str) -> (i32, i32, i32, i32) {
        let (tw, th) = overlay::measure_text(px, weight, text);
        let pad = (px * 0.4).round() as i32;
        let r = (x, y, tw + 2 * pad, th + pad);
        self.fill_rounded_rect(r.0, r.1, r.2, r.3, px * 0.35, bg);
        self.text(x + pad, y + pad / 2, px, weight, fg, text);
        r
    }

    /// Finish recording.
    pub fn finish(mut self) -> DrawList {
        let mut h = H::new().u(self.list.width as u64).u(self.list.height as u64);
        for it in &self.list.items {
            h = h.u(it.hash);
        }
        self.list.hash = h.0;
        self.list
    }
}

// ---------------------------------------------------------------- shared rasterizer (any pixel surface)

/// A pixel target the rasterizer draws into (the RGB565 frame [`crate::Canvas`] and the RGBA layer).
pub(crate) trait Surf {
    /// Current clip (x0, y0, x1, y1), within the surface.
    fn clip(&self) -> Bounds;
    /// Blend `c` with coverage `cov` (0..=255) at a pixel inside the clip.
    fn blend_px(&mut self, x: i32, y: i32, c: Rgba, cov: u32);
    /// Blend `c` at full coverage over `x0..x1` of row `y` (already clipped).
    fn blend_row(&mut self, y: i32, x0: i32, x1: i32, c: Rgba);

    #[inline]
    fn blend_clipped(&mut self, x: i32, y: i32, c: Rgba, cov: u32) {
        let (x0, y0, x1, y1) = self.clip();
        if x >= x0 && y >= y0 && x < x1 && y < y1 {
            self.blend_px(x, y, c, cov);
        }
    }
    #[inline]
    fn span(&mut self, y: i32, x0: i32, x1: i32, c: Rgba) {
        let (cx0, cy0, cx1, cy1) = self.clip();
        if y < cy0 || y >= cy1 {
            return;
        }
        let (a, b) = (x0.max(cx0), x1.min(cx1));
        if a < b {
            self.blend_row(y, a, b, c);
        }
    }
}

pub(crate) fn r_fill_rect<S: Surf + ?Sized>(s: &mut S, x: i32, y: i32, w: i32, h: i32, c: Rgba) {
    let (cx0, cy0, cx1, cy1) = s.clip();
    let (x0, y0, x1, y1) = (x.max(cx0), y.max(cy0), (x + w).min(cx1), (y + h).min(cy1));
    if x0 >= x1 || y0 >= y1 || c.a == 0 {
        return;
    }
    for yy in y0..y1 {
        s.blend_row(yy, x0, x1, c);
    }
}

/// Signed distance from (`px`,`py`) to a rounded box centred at (`cx`,`cy`) with half size (`hw`,`hh`).
pub(crate) fn rbox_sd(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let qx = (px - cx).abs() - (hw - r);
    let qy = (py - cy).abs() - (hh - r);
    let ox = qx.max(0.0);
    let oy = qy.max(0.0);
    (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) - r
}

pub(crate) fn r_rounded<S: Surf + ?Sized>(s: &mut S, x: i32, y: i32, w: i32, h: i32, radius: f32, stroke: Option<f32>, c: Rgba) {
    if w <= 0 || h <= 0 {
        return;
    }
    let r = radius.clamp(0.0, (w.min(h) as f32) / 2.0);
    let (hw, hh) = (w as f32 / 2.0, h as f32 / 2.0);
    let (cx, cy) = (x as f32 + hw, y as f32 + hh);
    let ri = r.ceil() as i32 + 1;
    let (cx0, cy0, cx1, cy1) = s.clip();
    let cov_at = |xx: i32, yy: i32| -> u32 {
        let d = rbox_sd(xx as f32 + 0.5, yy as f32 + 0.5, cx, cy, hw, hh, r);
        let outer = (0.5 - d).clamp(0.0, 1.0);
        let k = match stroke {
            Some(t) => outer * (0.5 + d + t).clamp(0.0, 1.0),
            None => outer,
        };
        (k * 255.0) as u32
    };
    // Same coverage as evaluating the distance at every pixel (perf-r2: that was ~5 % of the engine thread on the
    // phone): rows away from the corners are exact spans; the distance is only computed where it can be fractional
    // (corner rows, the outer `edge` columns of the other rows).
    let edge = match stroke {
        // A stroke row is covered only within t + 1 px of the sides (coverage 0 further in).
        Some(t) => (t.ceil() as i32 + 1).max(ri),
        None => ri,
    };
    let (px0, px1) = (x.max(cx0), (x + w).min(cx1));
    for yy in y.max(cy0)..(y + h).min(cy1) {
        let near_y = yy < y + edge || yy >= y + h - edge;
        if near_y || 2 * edge >= w {
            for xx in px0..px1 {
                let cov = cov_at(xx, yy);
                if cov > 0 {
                    s.blend_px(xx, yy, c, cov);
                }
            }
            continue;
        }
        for xx in (x..x + edge).chain(x + w - edge..x + w) {
            if xx < cx0 || xx >= cx1 {
                continue;
            }
            let cov = cov_at(xx, yy);
            if cov > 0 {
                s.blend_px(xx, yy, c, cov);
            }
        }
        if stroke.is_none() {
            s.span(yy, x + edge, x + w - edge, c);
        }
    }
}

pub(crate) fn r_circle<S: Surf + ?Sized>(s: &mut S, cx: f32, cy: f32, r: f32, c: Rgba) {
    let (x0, y0) = ((cx - r - 1.0).floor() as i32, (cy - r - 1.0).floor() as i32);
    let (x1, y1) = ((cx + r + 1.0).ceil() as i32, (cy + r + 1.0).ceil() as i32);
    let (kx0, ky0, kx1, ky1) = s.clip();
    for yy in y0.max(ky0)..y1.min(ky1) {
        for xx in x0.max(kx0)..x1.min(kx1) {
            let d = ((xx as f32 + 0.5 - cx).powi(2) + (yy as f32 + 0.5 - cy).powi(2)).sqrt() - r;
            let cov = ((0.5 - d).clamp(0.0, 1.0) * 255.0) as u32;
            if cov > 0 {
                s.blend_px(xx, yy, c, cov);
            }
        }
    }
}

pub(crate) fn r_ring<S: Surf + ?Sized>(s: &mut S, cx: f32, cy: f32, rx: f32, ry: f32, t: f32, col: Rgba) {
    if rx <= 0.0 || ry <= 0.0 {
        return;
    }
    let (x0, y0) = ((cx - rx - t - 1.0).floor() as i32, (cy - ry - t - 1.0).floor() as i32);
    let (x1, y1) = ((cx + rx + t + 1.0).ceil() as i32, (cy + ry + t + 1.0).ceil() as i32);
    let r = rx.min(ry);
    let (kx0, ky0, kx1, ky1) = s.clip();
    for yy in y0.max(ky0)..y1.min(ky1) {
        for xx in x0.max(kx0)..x1.min(kx1) {
            let (dx, dy) = ((xx as f32 + 0.5 - cx) / rx, (yy as f32 + 0.5 - cy) / ry);
            // Approximate distance to the ellipse in pixels.
            let d = ((dx * dx + dy * dy).sqrt() - 1.0).abs() * r;
            let cov = ((t * 0.5 + 0.5 - d).clamp(0.0, 1.0) * 255.0) as u32;
            if cov > 0 {
                s.blend_px(xx, yy, col, cov);
            }
        }
    }
}

pub(crate) fn r_blit_scaled<S: Surf + ?Sized>(s: &mut S, img: &Image, x: i32, y: i32, w: i32, h: i32, opacity: f32) {
    if w <= 0 || h <= 0 || img.width == 0 || img.height == 0 {
        return;
    }
    let op = (opacity.clamp(0.0, 1.0) * 255.0) as u32;
    let (kx0, ky0, kx1, ky1) = s.clip();
    for dy in (ky0 - y).max(0)..h.min(ky1 - y) {
        let sy = (dy as u64 * img.height as u64 / h as u64) as u32;
        for dx in (kx0 - x).max(0)..w.min(kx1 - x) {
            let sx = (dx as u64 * img.width as u64 / w as u64) as u32;
            let i = ((sy * img.width + sx) * 4) as usize;
            let px = &img.rgba[i..i + 4];
            if px[3] == 0 {
                continue;
            }
            s.blend_px(x + dx, y + dy, Rgba::rgba(px[0], px[1], px[2], px[3]), op);
        }
    }
}

pub(crate) fn r_blit_map<S: Surf + ?Sized>(s: &mut S, img: &Image, gx: f32, gy: f32, sc: f32) {
    let n = img.width as i32;
    if n == 0 || (img.height as i32) < n || img.rgba.len() < (n * n * 4) as usize {
        return;
    }
    let (bw, bh) = (1.5 * n as f32 * sc, n as f32 * sc);
    let (kx0, ky0, kx1, ky1) = s.clip();
    let x0 = (gx.floor() as i32).max(kx0);
    let y0 = (gy.floor() as i32).max(ky0);
    let x1 = ((gx + bw).ceil() as i32).min(kx1);
    let y1 = ((gy + bh).ceil() as i32).min(ky1);
    for py in y0..y1 {
        let r = ((py as f32 + 0.5 - gy) / sc) as i32;
        if r < 0 || r >= n {
            continue;
        }
        let shift = (n - r) as f32 / 2.0;
        let row = &img.rgba[(r * n * 4) as usize..((r + 1) * n * 4) as usize];
        for px in x0..x1 {
            let cf = (px as f32 + 0.5 - gx) / sc - shift;
            if cf < 0.0 {
                continue;
            }
            let col = cf as i32;
            if col >= n {
                break;
            }
            let p = &row[(col * 4) as usize..(col * 4 + 4) as usize];
            s.blend_px(px, py, Rgba::rgba(p[0], p[1], p[2], p[3]), 255);
        }
    }
}

pub(crate) fn r_line<S: Surf + ?Sized>(s: &mut S, x0: f32, y0: f32, x1: f32, y1: f32, t: f32, col: Rgba) {
    let n = ((x1 - x0).abs().max((y1 - y0).abs()) as i32).max(1);
    let ti = t.round().max(1.0) as i32;
    for i in 0..=n {
        let k = i as f32 / n as f32;
        let (x, y) = (x0 + (x1 - x0) * k, y0 + (y1 - y0) * k);
        r_fill_rect(s, x as i32 - ti / 2, y as i32 - ti / 2, ti, ti, col);
    }
}

pub(crate) fn r_text<S: Surf + ?Sized>(s: &mut S, x: i32, y: i32, px: f32, weight: Weight, c: Rgba, text: &str) -> i32 {
    let mut fonts = overlay::fonts().lock().unwrap();
    let ascent = fonts.ascent(weight, px);
    let mut pen = x as f32;
    for ch in text.chars() {
        let g = fonts.glyph(weight, px, ch);
        let gx = pen.round() as i32 + g.xmin;
        let gy = y + ascent - g.height as i32 - g.ymin;
        for row in 0..g.height {
            for col in 0..g.width {
                let cov = g.bitmap[row * g.width + col] as u32;
                if cov > 0 {
                    s.blend_clipped(gx + col as i32, gy + row as i32, c, cov);
                }
            }
        }
        pen += g.advance;
    }
    (pen - x as f32).round() as i32
}

/// Draw one command onto a surface (its clip already set by the caller).
pub(crate) fn r_cmd<S: Surf + ?Sized>(s: &mut S, cmd: &Cmd) {
    match cmd {
        Cmd::Rect { x, y, w, h, c } => r_fill_rect(s, *x, *y, *w, *h, *c),
        Cmd::RRect { x, y, w, h, r, stroke, c } => r_rounded(s, *x, *y, *w, *h, *r, *stroke, *c),
        Cmd::Circle { cx, cy, r, c } => r_circle(s, *cx, *cy, *r, *c),
        Cmd::Ring { cx, cy, rx, ry, t, c } => r_ring(s, *cx, *cy, *rx, *ry, *t, *c),
        Cmd::Image { img, x, y, w, h, opacity, .. } => r_blit_scaled(s, img, *x, *y, *w, *h, *opacity),
        Cmd::Map { img, x, y, s: sc, .. } => r_blit_map(s, img, *x, *y, *sc),
        Cmd::Line { x0, y0, x1, y1, t, c } => r_line(s, *x0, *y0, *x1, *y1, *t, *c),
        Cmd::Text { x, y, px, weight, c, text } => {
            r_text(s, *x, *y, *px, *weight, *c, text);
        }
    }
}

// ---------------------------------------------------------------- RGBA raster (frontends)

/// Premultiplied RGBA8 surface with a clip.
struct RgbaSurf<'a> {
    w: i32,
    px: &'a mut [u8],
    clip: Bounds,
}

impl Surf for RgbaSurf<'_> {
    fn clip(&self) -> Bounds {
        self.clip
    }
    #[inline]
    fn blend_px(&mut self, x: i32, y: i32, c: Rgba, cov: u32) {
        let a = c.a as u32 * cov / 255;
        if a == 0 {
            return;
        }
        let i = ((y * self.w + x) * 4) as usize;
        over(&mut self.px[i..i + 4], c, a);
    }
    fn blend_row(&mut self, y: i32, x0: i32, x1: i32, c: Rgba) {
        let a = c.a as u32;
        let row = &mut self.px[((y * self.w + x0) * 4) as usize..((y * self.w + x1) * 4) as usize];
        if a == 255 {
            for p in row.chunks_exact_mut(4) {
                p.copy_from_slice(&[c.r, c.g, c.b, 255]);
            }
        } else if a > 0 {
            for p in row.chunks_exact_mut(4) {
                over(p, c, a);
            }
        }
    }
}

/// Premultiplied "over": straight colour `c` with alpha `a` (0..=255) onto `p`.
#[inline]
fn over(p: &mut [u8], c: Rgba, a: u32) {
    let ia = 255 - a;
    let m = |s: u8, d: u8| ((s as u32 * a + d as u32 * ia + 127) / 255) as u8;
    p[0] = m(c.r, p[0]);
    p[1] = m(c.g, p[1]);
    p[2] = m(c.b, p[2]);
    p[3] = (a + (p[3] as u32 * ia + 127) / 255) as u8;
}

/// The frontend's rasterized copy of the newest [`DrawList`] (premultiplied RGBA8, `width * height * 4`).
#[derive(Default)]
pub struct LayerRaster {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    shown: Option<Arc<DrawList>>,
    /// Per row: covered x range (x0, x1) of the shown list (x0 >= x1 = empty row), for CPU blending.
    spans: Vec<(i32, i32)>,
}

impl LayerRaster {
    pub fn new() -> LayerRaster {
        LayerRaster::default()
    }

    /// Nothing drawn.
    pub fn is_empty(&self) -> bool {
        self.shown.as_ref().is_none_or(|l| l.is_empty())
    }

    /// The list currently rasterized.
    pub fn shown(&self) -> Option<&Arc<DrawList>> {
        self.shown.as_ref()
    }

    /// Bring the raster up to date with `list`. Returns the rectangle that changed (x0, y0, x1, y1), `None` if
    /// nothing changed.
    pub fn update(&mut self, list: &Arc<DrawList>) -> Option<Bounds> {
        if let Some(old) = &self.shown {
            if Arc::ptr_eq(old, list) || (old.hash == list.hash && old.items.len() == list.items.len() && old.width == list.width && old.height == list.height) {
                self.shown = Some(list.clone());
                return None;
            }
        }
        let (w, h) = (list.width, list.height);
        let resized = (w, h) != (self.width, self.height) || self.rgba.len() != (w * h * 4) as usize;
        let dirty = if resized {
            self.width = w;
            self.height = h;
            self.rgba.clear();
            self.rgba.resize((w * h * 4) as usize, 0);
            Some((0, 0, w as i32, h as i32))
        } else {
            let old = self.shown.clone().unwrap_or_default();
            diff_bounds(&old, list)
        };
        let full = (0, 0, w as i32, h as i32);
        let dirty = dirty.and_then(|d| intersect(d, full));
        if let Some(d) = dirty {
            self.redraw(list, d);
        }
        self.shown = Some(list.clone());
        self.compute_spans();
        dirty.or(if resized { Some(full) } else { None })
    }

    fn redraw(&mut self, list: &DrawList, d: Bounds) {
        let w = self.width as i32;
        for y in d.1..d.3 {
            self.rgba[((y * w + d.0) * 4) as usize..((y * w + d.2) * 4) as usize].fill(0);
        }
        let mut s = RgbaSurf { w, px: &mut self.rgba, clip: d };
        for it in &list.items {
            let Some(clip) = intersect(it.bounds, d).and_then(|_| intersect(it.clip, d)) else { continue };
            s.clip = clip;
            r_cmd(&mut s, &it.cmd);
        }
    }

    fn compute_spans(&mut self) {
        let h = self.height as usize;
        self.spans.clear();
        self.spans.resize(h, (i32::MAX, i32::MIN));
        let Some(l) = &self.shown else { return };
        for it in &l.items {
            let (x0, y0, x1, y1) = it.bounds;
            for y in y0.max(0) as usize..(y1.max(0) as usize).min(h) {
                let s = &mut self.spans[y];
                s.0 = s.0.min(x0);
                s.1 = s.1.max(x1);
            }
        }
    }

    /// Covered x range of row `y` (`None` = nothing drawn in that row).
    pub fn row_span(&self, y: u32) -> Option<(u32, u32)> {
        let &(a, b) = self.spans.get(y as usize)?;
        (a < b).then(|| (a.max(0) as u32, (b.max(0) as u32).min(self.width)))
    }

    /// Blend over an XRGB8888 buffer of the same size (`stride` pixels per row).
    pub fn blend_onto_xrgb(&self, dst: &mut [u32], stride: usize) {
        for y in 0..self.height {
            let Some((x0, x1)) = self.row_span(y) else { continue };
            let row = &self.rgba[(y * self.width * 4) as usize..((y + 1) * self.width * 4) as usize];
            let out = &mut dst[y as usize * stride..y as usize * stride + self.width as usize];
            for x in x0 as usize..x1 as usize {
                let p = &row[x * 4..x * 4 + 4];
                let a = p[3] as u32;
                if a == 0 {
                    continue;
                }
                let d = out[x];
                let ia = 255 - a;
                let ch = |s: u8, d: u32| (s as u32 + (d * ia + 127) / 255).min(255);
                let r = ch(p[0], (d >> 16) & 0xff);
                let g = ch(p[1], (d >> 8) & 0xff);
                let b = ch(p[2], d & 0xff);
                out[x] = (r << 16) | (g << 8) | b;
            }
        }
    }

    /// Blend over an RGB565 buffer of `bw` x `bh` pixels that the screen shows scaled to the layer size
    /// (nearest sampling; Android CPU fallback path).
    pub fn blend_onto_rgb565_scaled(&self, dst: &mut [u16], stride: usize, bw: u32, bh: u32) {
        if self.is_empty() || bw == 0 || bh == 0 || self.width == 0 {
            return;
        }
        for by in 0..bh {
            let ly = ((by as u64 * 2 + 1) * self.height as u64 / (bh as u64 * 2)) as u32;
            if self.row_span(ly).is_none() {
                continue;
            }
            for bx in 0..bw {
                let lx = ((bx as u64 * 2 + 1) * self.width as u64 / (bw as u64 * 2)) as u32;
                let i = ((ly * self.width + lx) * 4) as usize;
                let p = &self.rgba[i..i + 4];
                let a = p[3] as u32;
                if a == 0 {
                    continue;
                }
                let o = &mut dst[by as usize * stride + bx as usize];
                let (r, g, b) = s3_runtime::host::rgb565_to_rgb888(*o);
                let ia = 255 - a;
                let ch = |s: u8, d: u8| (s as u32 + (d as u32 * ia + 127) / 255).min(255);
                *o = (((ch(p[0], r) >> 3) << 11) | ((ch(p[1], g) >> 2) << 5) | (ch(p[2], b) >> 3)) as u16;
            }
        }
    }
}

/// Rectangle covering every item that is in one list and not the other (as a multiset of item hashes); the whole
/// union of both lists if only the order changed.
fn diff_bounds(old: &DrawList, new: &DrawList) -> Option<Bounds> {
    let mut count: HashMap<u64, i32> = HashMap::with_capacity(new.items.len());
    for it in &new.items {
        *count.entry(it.hash).or_default() += 1;
    }
    let mut dirty: Option<Bounds> = None;
    let add = |b: Bounds, d: &mut Option<Bounds>| *d = Some(d.map_or(b, |x| union(x, b)));
    for it in &old.items {
        match count.get_mut(&it.hash) {
            Some(n) if *n > 0 => *n -= 1,
            _ => add(it.bounds, &mut dirty),
        }
    }
    let mut count_old: HashMap<u64, i32> = HashMap::with_capacity(old.items.len());
    for it in &old.items {
        *count_old.entry(it.hash).or_default() += 1;
    }
    for it in &new.items {
        match count_old.get_mut(&it.hash) {
            Some(n) if *n > 0 => *n -= 1,
            _ => add(it.bounds, &mut dirty),
        }
    }
    if dirty.is_none() && old.hash != new.hash {
        // Same items, other order.
        dirty = match (old.bounds(), new.bounds()) {
            (Some(a), Some(b)) => Some(union(a, b)),
            (a, b) => a.or(b),
        };
    }
    dirty
}

// ---------------------------------------------------------------- scene hand-over (mods -> frontend)

/// What the frontend shows on top of (and around) one game frame.
#[derive(Debug, Clone, Default)]
pub struct Scene {
    /// Bumped whenever the list or the panel split changes.
    pub seq: u64,
    pub list: Arc<DrawList>,
    /// UI mode A panel shown at a constant scale (see [`crate::display::Viewport`]); `None` = plain frame.
    pub panel: Option<PanelSplit>,
    /// Game frame size this scene was made for.
    pub frame: (u32, u32),
}

/// Mailbox between the engine thread (mods publish) and the frontend (render thread / window loop).
pub struct SceneSlot {
    scene: Mutex<Scene>,
    notify: Option<Box<dyn Fn() + Send + Sync>>,
}

impl SceneSlot {
    /// `notify` is called after each new scene (e.g. wake the render thread).
    pub fn new(notify: Option<Box<dyn Fn() + Send + Sync>>) -> Arc<SceneSlot> {
        Arc::new(SceneSlot { scene: Mutex::new(Scene::default()), notify })
    }
    /// The newest scene (cheap clone: the list is shared).
    pub fn latest(&self) -> Scene {
        self.scene.lock().unwrap().clone()
    }
    pub fn seq(&self) -> u64 {
        self.scene.lock().unwrap().seq
    }
    pub(crate) fn publish(&self, list: Arc<DrawList>, panel: Option<PanelSplit>, frame: (u32, u32)) {
        {
            let mut s = self.scene.lock().unwrap();
            s.seq += 1;
            s.list = list;
            s.panel = panel;
            s.frame = frame;
        }
        if let Some(n) = &self.notify {
            n();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(r: &LayerRaster, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * r.width + x) * 4) as usize;
        r.rgba[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn record_hash_and_raster() {
        let mut l = Layer::new(100, 50);
        l.fill_rect(10, 10, 20, 10, Rgba::rgb(255, 0, 0));
        l.fill_rect(200, 10, 20, 10, Rgba::WHITE); // off screen: not recorded
        l.fill_rounded_rect(40, 5, 30, 30, 6.0, Rgba::rgba(0, 0, 255, 128));
        let a = Arc::new(l.finish());
        assert_eq!(a.items.len(), 2);
        let mut r = LayerRaster::new();
        assert_eq!(r.update(&a), Some((0, 0, 100, 50)));
        assert_eq!(px(&r, 15, 15), [255, 0, 0, 255]);
        assert_eq!(px(&r, 0, 0), [0, 0, 0, 0]);
        let p = px(&r, 55, 20);
        assert!((126..=130).contains(&p[3]) && (126..=130).contains(&p[2]) && p[0] == 0, "{p:?}");
        // Same content again: unchanged.
        let mut l = Layer::new(100, 50);
        l.fill_rect(10, 10, 20, 10, Rgba::rgb(255, 0, 0));
        l.fill_rounded_rect(40, 5, 30, 30, 6.0, Rgba::rgba(0, 0, 255, 128));
        let b = Arc::new(l.finish());
        assert_eq!(a.hash, b.hash);
        assert_eq!(r.update(&b), None);
        // Move the red rect: dirty = old + new rect only.
        let mut l = Layer::new(100, 50);
        l.fill_rect(12, 12, 20, 10, Rgba::rgb(255, 0, 0));
        l.fill_rounded_rect(40, 5, 30, 30, 6.0, Rgba::rgba(0, 0, 255, 128));
        let c = Arc::new(l.finish());
        assert_eq!(r.update(&c), Some((10, 10, 32, 22)));
        assert_eq!(px(&r, 10, 10), [0, 0, 0, 0]);
        assert_eq!(px(&r, 31, 21), [255, 0, 0, 255]);
        // The blue box is untouched and identical to a fresh raster.
        let mut fresh = LayerRaster::new();
        fresh.update(&c);
        assert!(fresh.rgba == r.rgba);
        assert_eq!(r.row_span(15), Some((12, 70)));
        assert_eq!(r.row_span(45), None);
    }

    #[test]
    fn incremental_equals_fresh_for_random_lists() {
        let mut seed = 99u32;
        let mut rnd = |n: u32| {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            (seed >> 8) % n
        };
        let img: ImageRef = Arc::new(Image { width: 2, height: 2, rgba: vec![255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0, 9, 9, 9, 255] });
        let mut r = LayerRaster::new();
        for _ in 0..60 {
            let mut l = Layer::new(120, 80);
            for _ in 0..rnd(8) {
                let (x, y) = (rnd(140) as i32 - 10, rnd(100) as i32 - 10);
                let c = Rgba::rgba(rnd(256) as u8, rnd(256) as u8, rnd(256) as u8, [255, 128, 30][rnd(3) as usize]);
                match rnd(6) {
                    0 => l.fill_rect(x, y, rnd(40) as i32, rnd(30) as i32, c),
                    1 => l.stroke_rounded_rect(x, y, rnd(40) as i32 + 1, rnd(30) as i32 + 1, 5.0, 2.0, c),
                    2 => l.fill_circle(x as f32, y as f32, rnd(15) as f32 + 1.0, c),
                    3 => {
                        l.text(x, y, 12.0, Weight::Bold, c, "Ab1");
                    }
                    4 => l.blit_scaled(&img, x, y, 9, 7, 1.0),
                    _ => {
                        l.set_clip(20, 20, 50, 30);
                        l.ring(x as f32, y as f32, 9.0, 2.0, c);
                        l.reset_clip();
                    }
                }
            }
            let list = Arc::new(l.finish());
            r.update(&list);
            let mut fresh = LayerRaster::new();
            fresh.update(&list);
            assert!(fresh.rgba == r.rgba);
        }
    }

    #[test]
    fn blend_onto_xrgb_premultiplied() {
        let mut l = Layer::new(4, 1);
        l.fill_rect(0, 0, 1, 1, Rgba::rgb(255, 255, 255));
        l.fill_rect(1, 0, 1, 1, Rgba::rgba(255, 0, 0, 128));
        let mut r = LayerRaster::new();
        r.update(&Arc::new(l.finish()));
        let mut dst = vec![0x0000_00ffu32; 4];
        r.blend_onto_xrgb(&mut dst, 4);
        assert_eq!(dst[0], 0x00ff_ffff);
        assert_eq!(dst[2], 0xff, "untouched");
        let (rr, bb) = ((dst[1] >> 16) & 0xff, dst[1] & 0xff);
        assert!((126..=130).contains(&rr) && (125..=129).contains(&bb), "{:x}", dst[1]);
    }
}
