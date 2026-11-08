//! Overlay drawing on the presented RGB565 frame: rectangles, rounded rectangles (anti-aliased), circles,
//! RGBA image blits and text (fontdue + bundled Liberation Sans, SIL OFL 1.1, same files as s3-win32).
//!
//! [`Canvas`] draws into an RGB565 frame (game pixels). Mods draw their UI into the window-resolution layer
//! ([`crate::layer::Layer`], same drawing functions, rasterized by the frontend); the frame canvas remains for
//! frame-space drawing (`Mod::draw`, tests).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use s3_runtime::FrameBuffer;

use crate::layer::{self, Surf};

static REGULAR_TTF: &[u8] = include_bytes!("../../s3-win32/assets/fonts/LiberationSans-Regular.ttf");
static BOLD_TTF: &[u8] = include_bytes!("../../s3-win32/assets/fonts/LiberationSans-Bold.ttf");

/// Straight-alpha RGBA colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Rgba {
        Rgba { r, g, b, a: 255 }
    }
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Rgba {
        Rgba { r, g, b, a }
    }
    pub const WHITE: Rgba = Rgba::rgb(255, 255, 255);
    pub const BLACK: Rgba = Rgba::rgb(0, 0, 0);
    /// Same colour with alpha multiplied by `k` (0..=1).
    pub fn fade(self, k: f32) -> Rgba {
        Rgba { a: (self.a as f32 * k.clamp(0.0, 1.0)).round() as u8, ..self }
    }
}

/// An RGBA8 image (straight alpha, row-major).
#[derive(Debug, Clone, Default)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn new(width: u32, height: u32) -> Image {
        Image { width, height, rgba: vec![0; (width * height * 4) as usize] }
    }
}

/// Font weight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Weight {
    Regular,
    Bold,
}

#[inline]
fn unpack(p: u16) -> (u32, u32, u32) {
    let (r, g, b) = s3_runtime::host::rgb565_to_rgb888(p);
    (r as u32, g as u32, b as u32)
}

#[inline]
fn pack(r: u32, g: u32, b: u32) -> u16 {
    (((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3)) as u16
}

/// Blend `c` with extra coverage `cov` (0..=255) over pixel `p`.
#[inline]
fn blend(p: u16, c: Rgba, cov: u32) -> u16 {
    let a = c.a as u32 * cov / 255;
    if a == 0 {
        return p;
    }
    if a >= 255 {
        return pack(c.r as u32, c.g as u32, c.b as u32);
    }
    let (r, g, b) = unpack(p);
    let ia = 255 - a;
    pack((c.r as u32 * a + r * ia) / 255, (c.g as u32 * a + g * ia) / 255, (c.b as u32 * a + b * ia) / 255)
}

/// Blend `c` at full coverage over every pixel of `row` (opaque colours are a plain fill).
#[inline]
fn blend_span(row: &mut [u16], c: Rgba) {
    match c.a {
        0 => {}
        255 => row.fill(pack(c.r as u32, c.g as u32, c.b as u32)),
        _ => {
            for p in row {
                *p = blend(*p, c, 255);
            }
        }
    }
}

/// Drawing surface over a (RGB565) frame, in game pixels. Mods draw their UI into the window layer
/// ([`crate::layer::Layer`]) instead; this stays for frame-space effects and tests.
pub struct Canvas<'a> {
    pub fb: &'a mut FrameBuffer,
    /// Optional clip rectangle (x0, y0, x1, y1), exclusive end.
    clip: (i32, i32, i32, i32),
}

impl Surf for Canvas<'_> {
    fn clip(&self) -> (i32, i32, i32, i32) {
        self.clip
    }
    #[inline]
    fn blend_px(&mut self, x: i32, y: i32, c: Rgba, cov: u32) {
        let i = (y as u32 * self.fb.width + x as u32) as usize;
        if let Some(p) = self.fb.pixels.get_mut(i) {
            *p = blend(*p, c, cov);
        }
    }
    fn blend_row(&mut self, y: i32, x0: i32, x1: i32, c: Rgba) {
        let fw = self.fb.width as usize;
        let row = &mut self.fb.pixels[y as usize * fw..y as usize * fw + fw];
        blend_span(&mut row[x0 as usize..x1 as usize], c);
    }
}

impl<'a> Canvas<'a> {
    pub fn new(fb: &'a mut FrameBuffer) -> Self {
        let clip = (0, 0, fb.width as i32, fb.height as i32);
        Canvas { fb, clip }
    }

    pub fn width(&self) -> u32 {
        self.fb.width
    }
    pub fn height(&self) -> u32 {
        self.fb.height
    }

    /// Restrict drawing to a rectangle (intersected with the frame).
    pub fn set_clip(&mut self, x: i32, y: i32, w: i32, h: i32) {
        self.clip = (x.max(0), y.max(0), (x + w).min(self.fb.width as i32), (y + h).min(self.fb.height as i32));
    }
    pub fn reset_clip(&mut self) {
        self.clip = (0, 0, self.fb.width as i32, self.fb.height as i32);
    }

    /// Blend one pixel with coverage `cov` (0..=255).
    #[inline]
    pub fn blend_pixel(&mut self, x: i32, y: i32, c: Rgba, cov: u32) {
        self.blend_clipped(x, y, c, cov);
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: Rgba) {
        layer::r_fill_rect(self, x, y, w, h, c);
    }

    /// Outline of width `t` inside the rectangle.
    pub fn stroke_rect(&mut self, x: i32, y: i32, w: i32, h: i32, t: i32, c: Rgba) {
        self.fill_rect(x, y, w, t, c);
        self.fill_rect(x, y + h - t, w, t, c);
        self.fill_rect(x, y + t, t, h - 2 * t, c);
        self.fill_rect(x + w - t, y + t, t, h - 2 * t, c);
    }

    /// Anti-aliased filled rounded rectangle.
    pub fn fill_rounded_rect(&mut self, x: i32, y: i32, w: i32, h: i32, radius: f32, c: Rgba) {
        layer::r_rounded(self, x, y, w, h, radius, None, c)
    }

    /// Anti-aliased rounded rectangle outline of width `t`.
    pub fn stroke_rounded_rect(&mut self, x: i32, y: i32, w: i32, h: i32, radius: f32, t: f32, c: Rgba) {
        layer::r_rounded(self, x, y, w, h, radius, Some(t), c)
    }

    /// Anti-aliased filled circle.
    pub fn fill_circle(&mut self, cx: f32, cy: f32, r: f32, c: Rgba) {
        layer::r_circle(self, cx, cy, r, c)
    }

    /// Anti-aliased ellipse outline.
    pub fn ellipse_ring(&mut self, cx: f32, cy: f32, rx: f32, ry: f32, t: f32, c: Rgba) {
        layer::r_ring(self, cx, cy, rx, ry, t, c)
    }

    /// Blit an RGBA image with its top-left at (`x`,`y`), alpha `opacity` (0..=1).
    pub fn blit(&mut self, img: &Image, x: i32, y: i32, opacity: f32) {
        self.blit_scaled(img, x, y, img.width as i32, img.height as i32, opacity)
    }

    /// Blit an RGBA image scaled (nearest neighbour) into the `w` x `h` rectangle at (`x`,`y`).
    pub fn blit_scaled(&mut self, img: &Image, x: i32, y: i32, w: i32, h: i32, opacity: f32) {
        layer::r_blit_scaled(self, img, x, y, w, h, opacity)
    }

    /// Draw `text` with its top-left at (`x`,`y`), font size `px`. Returns the advance width.
    pub fn text(&mut self, x: i32, y: i32, px: f32, weight: Weight, c: Rgba, text: &str) -> i32 {
        layer::r_text(self, x, y, px, weight, c, text)
    }

    /// Text centred in a rectangle.
    pub fn text_centered(&mut self, x: i32, y: i32, w: i32, h: i32, px: f32, weight: Weight, c: Rgba, text: &str) {
        let (tw, th) = measure_text(px, weight, text);
        self.text(x + (w - tw) / 2, y + (h - th) / 2, px, weight, c, text);
    }
}

/// (width, line height) of `text` at size `px`.
pub fn measure_text(px: f32, weight: Weight, text: &str) -> (i32, i32) {
    let mut fonts = fonts().lock().unwrap();
    let w: f32 = text.chars().map(|ch| fonts.glyph(weight, px, ch).advance).sum();
    (w.round() as i32, fonts.line_height(weight, px))
}

pub(crate) struct Glyph {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) xmin: i32,
    pub(crate) ymin: i32,
    pub(crate) advance: f32,
    pub(crate) bitmap: Vec<u8>,
}

pub(crate) struct Fonts {
    faces: [fontdue::Font; 2],
    cache: HashMap<(Weight, u32, char), Glyph>,
}

impl Fonts {
    fn face(&self, w: Weight) -> &fontdue::Font {
        &self.faces[(w == Weight::Bold) as usize]
    }
    pub(crate) fn ascent(&self, w: Weight, px: f32) -> i32 {
        self.face(w).horizontal_line_metrics(px).map(|m| m.ascent.round() as i32).unwrap_or(px as i32)
    }
    fn line_height(&self, w: Weight, px: f32) -> i32 {
        self.face(w).horizontal_line_metrics(px).map(|m| (m.ascent - m.descent).round() as i32).unwrap_or(px as i32)
    }
    pub(crate) fn glyph(&mut self, w: Weight, px: f32, ch: char) -> &Glyph {
        if self.cache.len() > 4096 {
            self.cache.clear();
        }
        let key = (w, px.to_bits(), ch);
        if !self.cache.contains_key(&key) {
            let (m, bitmap) = self.face(w).rasterize(ch, px);
            self.cache.insert(
                key,
                Glyph { width: m.width, height: m.height, xmin: m.xmin, ymin: m.ymin, advance: m.advance_width, bitmap },
            );
        }
        &self.cache[&key]
    }
}

pub(crate) fn fonts() -> &'static Mutex<Fonts> {
    static F: OnceLock<Mutex<Fonts>> = OnceLock::new();
    F.get_or_init(|| {
        let s = fontdue::FontSettings::default();
        Mutex::new(Fonts {
            faces: [
                fontdue::Font::from_bytes(REGULAR_TTF, s).expect("bundled regular font"),
                fontdue::Font::from_bytes(BOLD_TTF, s).expect("bundled bold font"),
            ],
            cache: HashMap::new(),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_blend_and_clip() {
        let mut fb = FrameBuffer::new(10, 10);
        let mut c = Canvas::new(&mut fb);
        c.fill_rect(-5, -5, 8, 8, Rgba::WHITE);
        c.fill_rect(5, 5, 100, 100, Rgba::rgba(255, 0, 0, 128));
        assert_eq!(fb.pixels[0], 0xFFFF);
        assert_eq!(fb.pixels[2 * 10 + 2], 0xFFFF);
        assert_eq!(fb.pixels[3 * 10 + 3], 0);
        let (r, g, b) = s3_runtime::host::rgb565_to_rgb888(fb.pixels[9 * 10 + 9]);
        assert!((120..=135).contains(&r) && g == 0 && b == 0, "{r} {g} {b}");
    }

    #[test]
    fn rounded_spans_match_per_pixel_reference() {
        // The old per-pixel algorithm, kept as the reference.
        fn reference(fb: &mut FrameBuffer, x: i32, y: i32, w: i32, h: i32, radius: f32, stroke: Option<f32>, c: Rgba) {
            let r = radius.clamp(0.0, (w.min(h) as f32) / 2.0);
            let (hw, hh) = (w as f32 / 2.0, h as f32 / 2.0);
            let (cx, cy) = (x as f32 + hw, y as f32 + hh);
            let ri = r.ceil() as i32 + 1;
            let mut cv = Canvas::new(fb);
            for yy in y..y + h {
                let near_y = yy < y + ri || yy >= y + h - ri;
                for xx in x..x + w {
                    let near_x = xx < x + ri || xx >= x + w - ri;
                    let cov = if stroke.is_none() && !near_x && !near_y {
                        255
                    } else {
                        let d = crate::layer::rbox_sd(xx as f32 + 0.5, yy as f32 + 0.5, cx, cy, hw, hh, r);
                        let outer = (0.5 - d).clamp(0.0, 1.0);
                        let k = match stroke {
                            Some(t) => outer * (0.5 + d + t).clamp(0.0, 1.0),
                            None => outer,
                        };
                        (k * 255.0) as u32
                    };
                    if cov > 0 {
                        cv.blend_pixel(xx, yy, c, cov);
                    }
                }
            }
        }
        let mut seed = 12345u32;
        let mut rnd = |n: u32| {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            (seed >> 8) % n
        };
        for _ in 0..400 {
            let (x, y) = (rnd(60) as i32 - 20, rnd(60) as i32 - 20);
            let (w, h) = (rnd(90) as i32 + 1, rnd(70) as i32 + 1);
            let r = rnd(30) as f32 * 0.7;
            let stroke = if rnd(2) == 0 { None } else { Some(rnd(12) as f32 * 0.6 + 0.5) };
            let c = Rgba { r: rnd(256) as u8, g: rnd(256) as u8, b: rnd(256) as u8, a: [255, 128, 40][rnd(3) as usize] };
            let mut base = FrameBuffer::new(100, 80);
            for (i, p) in base.pixels.iter_mut().enumerate() {
                *p = (i as u16).wrapping_mul(40503);
            }
            let (mut a, mut b) = (base.clone(), base.clone());
            reference(&mut a, x, y, w, h, r, stroke, c);
            let mut cv = Canvas::new(&mut b);
            match stroke {
                Some(t) => cv.stroke_rounded_rect(x, y, w, h, r, t, c),
                None => cv.fill_rounded_rect(x, y, w, h, r, c),
            }
            assert!(a.pixels == b.pixels, "x {x} y {y} w {w} h {h} r {r} stroke {stroke:?} c {c:?}");
        }
    }

    #[test]
    fn rounded_rect_corners_are_soft() {
        let mut fb = FrameBuffer::new(40, 40);
        Canvas::new(&mut fb).fill_rounded_rect(0, 0, 40, 40, 10.0, Rgba::WHITE);
        assert_eq!(fb.pixels[0], 0, "corner outside");
        assert_eq!(fb.pixels[20 * 40 + 20], 0xFFFF, "centre");
        assert_eq!(fb.pixels[20 * 40], 0xFFFF, "edge middle");
        let mut fb2 = FrameBuffer::new(40, 40);
        Canvas::new(&mut fb2).stroke_rounded_rect(0, 0, 40, 40, 10.0, 2.0, Rgba::WHITE);
        assert_eq!(fb2.pixels[20 * 40 + 20], 0, "stroke leaves centre");
        assert_eq!(fb2.pixels[20 * 40], 0xFFFF);
    }

    #[test]
    fn text_draws_pixels_and_measures() {
        let mut fb = FrameBuffer::new(120, 40);
        let w = Canvas::new(&mut fb).text(2, 2, 20.0, Weight::Bold, Rgba::WHITE, "Hello");
        let (mw, mh) = measure_text(20.0, Weight::Bold, "Hello");
        assert!((w - mw).abs() <= 1 && w > 30, "{w} {mw}");
        assert!(mh >= 18);
        let lit = fb.pixels.iter().filter(|&&p| p != 0).count();
        assert!(lit > 50, "{lit}");
        // Nothing drawn above y=2.
        assert!(fb.pixels[..2 * 120].iter().all(|&p| p == 0));
    }

    #[test]
    fn blit_alpha() {
        let mut img = Image::new(2, 1);
        img.rgba.copy_from_slice(&[255, 0, 0, 255, 0, 0, 255, 0]);
        let mut fb = FrameBuffer::new(4, 4);
        Canvas::new(&mut fb).blit_scaled(&img, 0, 0, 4, 2, 1.0);
        assert_eq!(fb.pixels[0], 0xF800);
        assert_eq!(fb.pixels[1], 0xF800);
        assert_eq!(fb.pixels[2], 0, "transparent half");
        assert_eq!(fb.pixels[4 + 1], 0xF800);
        assert_eq!(fb.pixels[8], 0);
    }
}
