//! Visual touch feedback drawn on the overlay: tap ripples, move/attack target flags (anchored to the map),
//! selection rings under selected units, long-press progress, the box-selection rectangle and the build
//! placement cancel button.

use crate::overlay::{Canvas, Rgba, Weight};

pub const RIPPLE_MS: u64 = 380;
pub const FLAG_MS: u64 = 1400;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Fx {
    /// Expanding ring at a screen point.
    Ripple { x: f32, y: f32, t0: u64, color: Rgba },
    /// Flag at a world point (screen + camera at creation time).
    Flag { wx: f32, wy: f32, t0: u64, attack: bool },
}

impl Fx {
    fn t0(&self) -> u64 {
        match *self {
            Fx::Ripple { t0, .. } | Fx::Flag { t0, .. } => t0,
        }
    }
    fn life(&self) -> u64 {
        match self {
            Fx::Ripple { .. } => RIPPLE_MS,
            Fx::Flag { .. } => FLAG_MS,
        }
    }
    pub fn alive(&self, now: u64) -> bool {
        now.saturating_sub(self.t0()) < self.life()
    }
}

pub const TAP: Rgba = Rgba::rgba(255, 255, 255, 220);
pub const MOVE: Rgba = Rgba::rgba(90, 230, 90, 230);
pub const ATTACK: Rgba = Rgba::rgba(255, 70, 50, 235);
pub const CANCEL: Rgba = Rgba::rgba(255, 190, 60, 230);
pub const SELECT: Rgba = Rgba::rgba(120, 255, 120, 210);

/// Anti-aliased ellipse outline (`rx`, `ry` radii, thickness `t` px).
pub fn ellipse_ring(c: &mut Canvas, cx: f32, cy: f32, rx: f32, ry: f32, t: f32, col: Rgba) {
    if rx <= 0.0 || ry <= 0.0 {
        return;
    }
    let (x0, y0) = ((cx - rx - t - 1.0).floor() as i32, (cy - ry - t - 1.0).floor() as i32);
    let (x1, y1) = ((cx + rx + t + 1.0).ceil() as i32, (cy + ry + t + 1.0).ceil() as i32);
    let r = rx.min(ry);
    for yy in y0..y1 {
        for xx in x0..x1 {
            let (dx, dy) = ((xx as f32 + 0.5 - cx) / rx, (yy as f32 + 0.5 - cy) / ry);
            // Approximate distance to the ellipse in pixels.
            let d = ((dx * dx + dy * dy).sqrt() - 1.0).abs() * r;
            let cov = ((t * 0.5 + 0.5 - d).clamp(0.0, 1.0) * 255.0) as u32;
            if cov > 0 {
                c.blend_pixel(xx, yy, col, cov);
            }
        }
    }
}

pub fn ring(c: &mut Canvas, cx: f32, cy: f32, r: f32, t: f32, col: Rgba) {
    ellipse_ring(c, cx, cy, r, r, t, col);
}

/// Draw one effect. `cam` = current camera (world px) for map-anchored effects.
pub fn draw(c: &mut Canvas, fx: &Fx, now: u64, cam: (f32, f32)) {
    let age = now.saturating_sub(fx.t0()) as f32 / fx.life() as f32;
    if age >= 1.0 {
        return;
    }
    match *fx {
        Fx::Ripple { x, y, color, .. } => {
            let r = 6.0 + 22.0 * age.sqrt();
            ring(c, x, y, r, 3.0, color.fade(1.0 - age));
            c.fill_circle(x, y, 4.0 * (1.0 - age), color.fade(1.0 - age));
        }
        Fx::Flag { wx, wy, attack, .. } => {
            let (x, y) = (wx - cam.0, wy - cam.1);
            let k = if age < 0.7 { 1.0 } else { 1.0 - (age - 0.7) / 0.3 };
            let col = if attack { ATTACK } else { MOVE };
            // Ground ring pulsing.
            let p = (age * 3.0).fract();
            ellipse_ring(c, x, y, 6.0 + 10.0 * p, 3.0 + 5.0 * p, 2.0, col.fade(k * (1.0 - p)));
            ellipse_ring(c, x, y, 7.0, 3.5, 2.0, col.fade(k));
            // Pole and pennant (drops in during the first 150 ms).
            let drop = ((1.0 - age / 0.1).max(0.0) * 12.0) as i32;
            let (px, py) = (x as i32, y as i32 - drop);
            c.fill_rect(px - 2, py - 34, 3, 34, Rgba::rgba(30, 22, 14, (235.0 * k) as u8));
            for i in 0..16 {
                // Pennant pointing right, with a dark outline.
                let h = 16 - i;
                c.fill_rect(px + 1 + i, py - 34 + i / 2, 1, h, col.fade(k));
            }
            c.fill_rect(px + 1, py - 35, 16, 1, Rgba::rgba(0, 0, 0, (120.0 * k) as u8));
            if attack {
                c.text(px + 4, py - 33, 12.0, Weight::Bold, Rgba::WHITE.fade(k), "!");
            }
        }
    }
}

/// Small ring under a selected unit (feet at `x, y`).
pub fn selection_marker(c: &mut Canvas, x: f32, y: f32) {
    ellipse_ring(c, x, y + 1.0, 10.0, 4.5, 2.0, SELECT);
}

/// Long-press progress around the finger (`k` 0..1); at 1 a box selection drag starts.
pub fn press_progress(c: &mut Canvas, x: f32, y: f32, k: f32) {
    let r = 34.0 - 16.0 * k;
    ring(c, x, y, r, 3.0, Rgba::rgba(255, 255, 255, (60.0 + 160.0 * k) as u8));
}

/// Box selection rectangle (screen corners).
pub fn selection_box(c: &mut Canvas, a: (f32, f32), b: (f32, f32)) {
    let (x0, y0) = (a.0.min(b.0) as i32, a.1.min(b.1) as i32);
    let (x1, y1) = (a.0.max(b.0) as i32, a.1.max(b.1) as i32);
    c.fill_rect(x0, y0, x1 - x0, y1 - y0, Rgba::rgba(120, 255, 120, 40));
    c.stroke_rect(x0, y0, x1 - x0, y1 - y0, 2, SELECT);
}

/// Build placement cancel button (✕) at `r`.
pub fn cancel_button(c: &mut Canvas, r: (i32, i32, i32, i32)) {
    let (x, y, w, h) = r;
    c.fill_rounded_rect(x, y, w, h, 10.0, Rgba::rgba(30, 20, 10, 190));
    c.stroke_rounded_rect(x, y, w, h, 10.0, 2.0, CANCEL);
    // Diagonal cross from small discs (anti-aliased, no line primitive needed).
    let (cx, cy, s) = (x as f32 + w as f32 / 2.0, y as f32 + h as f32 / 2.0, w as f32 * 0.26);
    let n = 14;
    for i in 0..=n {
        let k = -1.0 + 2.0 * i as f32 / n as f32;
        c.fill_circle(cx + k * s, cy + k * s, 2.2, Rgba::WHITE);
        c.fill_circle(cx + k * s, cy - k * s, 2.2, Rgba::WHITE);
    }
    let label = "Cancel";
    let (tw, _) = crate::overlay::measure_text(11.0, Weight::Bold, label);
    let lx = (x + w / 2 - tw / 2).min(c.width() as i32 - tw - 2);
    c.fill_rounded_rect(lx - 4, y + h + 3, tw + 8, 16, 4.0, Rgba::rgba(0, 0, 0, 150));
    c.text(lx, y + h + 4, 11.0, Weight::Bold, CANCEL, label);
}
