//! Visual touch feedback drawn into the window layer ([`crate::layer`]): tap ripples, move/attack target flags
//! (anchored to the map), selection rings under selected units, long-press progress, the box-selection rectangle
//! and the build placement cancel button.
//!
//! Effects are kept in game pixels (where the touch landed); drawing maps them to window pixels and sizes them with
//! `u` (window px per reference px = window height / 600), so they keep their on-screen size at every zoom level.
//! Selection rings hug the unit sprites and use the map scale instead.

use crate::layer::Layer;
use crate::overlay::{Rgba, Weight};

pub const RIPPLE_MS: u64 = 380;
pub const FLAG_MS: u64 = 1400;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Fx {
    /// Expanding ring at a game-screen point.
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

/// Draw one effect. `cam` = current camera (world px) for map-anchored effects, `to_win` maps game pixels to
/// window pixels, `u` = UI scale.
pub fn draw(c: &mut Layer, fx: &Fx, now: u64, cam: (f32, f32), to_win: &dyn Fn(f32, f32) -> (f32, f32), u: f32) {
    let age = now.saturating_sub(fx.t0()) as f32 / fx.life() as f32;
    if age >= 1.0 {
        return;
    }
    match *fx {
        Fx::Ripple { x, y, color, .. } => {
            let (x, y) = to_win(x, y);
            let r = (6.0 + 22.0 * age.sqrt()) * u;
            c.ring(x, y, r, 3.0 * u, color.fade(1.0 - age));
            c.fill_circle(x, y, 4.0 * (1.0 - age) * u, color.fade(1.0 - age));
        }
        Fx::Flag { wx, wy, attack, .. } => {
            let (x, y) = to_win(wx - cam.0, wy - cam.1);
            let k = if age < 0.7 { 1.0 } else { 1.0 - (age - 0.7) / 0.3 };
            let col = if attack { ATTACK } else { MOVE };
            // Ground ring pulsing.
            let p = (age * 3.0).fract();
            c.ellipse_ring(x, y, (6.0 + 10.0 * p) * u, (3.0 + 5.0 * p) * u, 2.0 * u, col.fade(k * (1.0 - p)));
            c.ellipse_ring(x, y, 7.0 * u, 3.5 * u, 2.0 * u, col.fade(k));
            // Pole and pennant (drops in during the first 150 ms).
            let q = |v: f32| (v * u).round() as i32;
            let drop = (1.0 - age / 0.1).max(0.0) * 12.0;
            let (px, py) = (x as i32, (y - drop * u) as i32);
            c.fill_rect(px - q(2.0), py - q(34.0), q(3.0).max(1), q(34.0), Rgba::rgba(30, 22, 14, (235.0 * k) as u8));
            // Pennant pointing right (one column per reference pixel, scaled).
            let cols = q(16.0).max(1);
            for i in 0..cols {
                let f = i as f32 / u;
                let h = ((16.0 - f) * u).round() as i32;
                c.fill_rect(px + q(1.0) + i, py - q(34.0) + (f / 2.0 * u) as i32, 1, h, col.fade(k));
            }
            c.fill_rect(px + q(1.0), py - q(35.0), q(16.0), q(1.0).max(1), Rgba::rgba(0, 0, 0, (120.0 * k) as u8));
            if attack {
                c.text(px + q(4.0), py - q(33.0), 12.0 * u, Weight::Bold, Rgba::WHITE.fade(k), "!");
            }
        }
    }
}

/// Small ring under a selected unit (feet at window `x, y`); `s` = map scale (window px per game px).
pub fn selection_marker(c: &mut Layer, x: f32, y: f32, s: f32) {
    c.ellipse_ring(x, y + s, 10.0 * s, 4.5 * s, 2.0 * s.max(0.75), SELECT);
}

/// Long-press progress around the finger (window `x, y`, `k` 0..1); at 1 a box selection drag starts.
pub fn press_progress(c: &mut Layer, x: f32, y: f32, k: f32, u: f32) {
    let r = (34.0 - 16.0 * k) * u;
    c.ring(x, y, r, 3.0 * u, Rgba::rgba(255, 255, 255, (60.0 + 160.0 * k) as u8));
}

/// Box selection rectangle (window corners).
pub fn selection_box(c: &mut Layer, a: (f32, f32), b: (f32, f32), u: f32) {
    let (x0, y0) = (a.0.min(b.0) as i32, a.1.min(b.1) as i32);
    let (x1, y1) = (a.0.max(b.0) as i32, a.1.max(b.1) as i32);
    c.fill_rect(x0, y0, x1 - x0, y1 - y0, Rgba::rgba(120, 255, 120, 40));
    c.stroke_rect(x0, y0, x1 - x0, y1 - y0, (2.0 * u).round().max(1.0) as i32, SELECT);
}

/// Build placement cancel button (✕) at window rect `r`.
pub fn cancel_button(c: &mut Layer, r: (i32, i32, i32, i32), u: f32, label: &str) {
    let (x, y, w, h) = r;
    c.fill_rounded_rect(x, y, w, h, 10.0 * u, Rgba::rgba(30, 20, 10, 190));
    c.stroke_rounded_rect(x, y, w, h, 10.0 * u, 2.0 * u, CANCEL);
    // Diagonal cross from small discs (anti-aliased).
    let (cx, cy, s) = (x as f32 + w as f32 / 2.0, y as f32 + h as f32 / 2.0, w as f32 * 0.26);
    let n = 14;
    for i in 0..=n {
        let k = -1.0 + 2.0 * i as f32 / n as f32;
        c.fill_circle(cx + k * s, cy + k * s, 2.2 * u, Rgba::WHITE);
        c.fill_circle(cx + k * s, cy - k * s, 2.2 * u, Rgba::WHITE);
    }
    let px = 11.0 * u;
    let (tw, th) = crate::overlay::measure_text(px, Weight::Bold, label);
    let q = |v: f32| (v * u).round() as i32;
    let lx = (x + w / 2 - tw / 2).min(c.width() as i32 - tw - q(2.0));
    c.fill_rounded_rect(lx - q(4.0), y + h + q(3.0), tw + q(8.0), th + q(3.0), 4.0 * u, Rgba::rgba(0, 0, 0, 150));
    c.text(lx, y + h + q(4.0), px, Weight::Bold, CANCEL, label);
}

/// Window rectangle of the ✕ button: top right of the shown game area, below the FPS counter (`fps_top` reference
/// px), 40 reference px.
pub fn cancel_rect(vis: (i32, i32, i32, i32), fps_top: f32, u: f32) -> (i32, i32, i32, i32) {
    let s = (40.0 * u).round() as i32;
    let (vx, vy, vw, _) = vis;
    (vx + vw - s - (8.0 * u).round() as i32, vy + ((8.0 + fps_top) * u).round() as i32, s, s)
}
