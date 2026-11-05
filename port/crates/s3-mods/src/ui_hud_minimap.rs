//! Mobile HUD minimap widget + full-screen map overlay (docs/porting/ui-hud.md "Minimap").
//!
//! The game keeps its minimap as a square RGB565 image of `mw x mw` pixels (`mw = [0x730f28]`) at
//! `[snap+0x4c]` (`snap` = the per-frame render snapshot, argument of the panel renderer `0x4b4d00`). Image column
//! `c` / row `r` is tile `(c*W/mw, r*W/mw)` (W = world size in tiles). The frame renderer `0x4b26c0` draws it
//! sheared: screen `x = mx + (mw - r - 1)/2 + c`, `y = my + r` (blocks `0x4b2b34..0x4b2ce5` and
//! `0x4b366a..0x4b3cf3`, which also draw the frame art and the white camera rectangle). In HUD mode those blocks
//! are skipped (`ui_hud_game`) and this module draws the same image itself, in the same sheared shape, from guest
//! memory (nothing shipped). Camera rectangle, in image coordinates, as the game computes it at `0x4b3a3c`:
//! `k = (mw << 8) / [snap+0x2c]`, `x = ([snap+0x24] + 11)*k >> 8`, `y = ([snap+0x28] + 16)*k >> 8`,
//! `w = [snap+0x30]*k >> 8`, `h = ([snap+0x34] - 40)*k >> 8` (`>>` = signed division by 256).
//!
//! Pure presentation + `gd::CenterOn` queued on the logic thread (the same call the game's minimap click makes),
//! so lockstep multiplayer is unaffected.

use s3_runtime::Machine;

use crate::overlay::{Canvas, Rgba};
use crate::ui_hud_game::{self as game, readable};

/// Minimap data read from the game (host copy).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MiniData {
    /// Image side in pixels (`[0x730f28]`).
    pub n: u32,
    /// `n * n` RGB565 pixels, row-major.
    pub pixels: Vec<u16>,
    /// World size in tiles.
    pub world: u32,
    /// Camera rectangle in image coordinates (x, y, w, h).
    pub cam: (i32, i32, i32, i32),
}

/// Signed `a >> 8` the way MSVC emits it (`cdq; and edx, 0xff; add; sar 8` = truncating division by 256).
fn div256(a: i32) -> i32 {
    a / 256
}

/// The game's camera rectangle formula (`0x4b3a3c..`), in image coordinates.
pub fn cam_rect(n: u32, s24: i32, s28: i32, s2c: u32, s30: i32, s34: i32) -> (i32, i32, i32, i32) {
    if s2c == 0 {
        return (0, 0, 0, 0);
    }
    let k = ((n << 8) / s2c) as i32;
    (
        div256((s24 + 11).wrapping_mul(k)),
        div256((s28 + 16).wrapping_mul(k)),
        div256(s30.wrapping_mul(k)),
        div256((s34 - 40).wrapping_mul(k)),
    )
}

/// Read the minimap image and camera rectangle (`None` if no game / snapshot yet).
pub fn read(m: &mut Machine) -> Option<MiniData> {
    let snap = m.state::<game::HudShared>().borrow_mut().snap;
    if snap == 0 || !readable(m, snap, 0x50) {
        return None;
    }
    let n = m.mem.read_u32(game::L_MINI_W);
    if !(16..=512).contains(&n) {
        return None;
    }
    let img = m.mem.read_u32(snap + 0x4c);
    let len = n * n * 2;
    if !readable(m, img, len) {
        return None;
    }
    let bytes = m.mem.slice(img, len as usize);
    let pixels: Vec<u16> = bytes.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
    let g = game::gd(m)?;
    let world = m.mem.read_u32(g + game::GD_WORLD_W);
    if world == 0 || world > 1024 {
        return None;
    }
    let r = |o: u32| m.mem.read_u32(snap + o);
    let cam = cam_rect(n, r(0x24) as i32, r(0x28) as i32, r(0x2c), r(0x30) as i32, r(0x34) as i32);
    Some(MiniData { n, pixels, world, cam })
}

/// Where a map image is drawn: the sheared parallelogram of an `n x n` image scaled by `s` with its bounding box
/// at `(x, y)` (box size `1.5*n*s x n*s`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapGeom {
    pub x: f32,
    pub y: f32,
    pub s: f32,
    pub n: u32,
}

impl MapGeom {
    /// The largest geometry that fits `(x, y, w, h)`, centred.
    pub fn fit(n: u32, x: i32, y: i32, w: i32, h: i32) -> MapGeom {
        let nf = n.max(1) as f32;
        let s = (w as f32 / (1.5 * nf)).min(h as f32 / nf).max(0.01);
        let (bw, bh) = (1.5 * nf * s, nf * s);
        MapGeom { x: x as f32 + (w as f32 - bw) / 2.0, y: y as f32 + (h as f32 - bh) / 2.0, s, n }
    }
    pub fn box_size(&self) -> (f32, f32) {
        (1.5 * self.n as f32 * self.s, self.n as f32 * self.s)
    }
    /// Image point (col, row) -> screen.
    pub fn to_screen(&self, c: f32, r: f32) -> (f32, f32) {
        let n = self.n as f32;
        (self.x + ((n - r) / 2.0 + c) * self.s, self.y + r * self.s)
    }
    /// Screen -> image point (col, row), `None` outside the parallelogram.
    pub fn to_image(&self, sx: f32, sy: f32) -> Option<(f32, f32)> {
        let n = self.n as f32;
        let r = (sy - self.y) / self.s;
        let c = (sx - self.x) / self.s - (n - r) / 2.0;
        if r < 0.0 || r >= n || c < 0.0 || c >= n {
            return None;
        }
        Some((c, r))
    }
    /// Is the screen point inside the bounding box (a touch slightly outside the parallelogram still counts)?
    pub fn in_box(&self, sx: f32, sy: f32) -> bool {
        let (w, h) = self.box_size();
        sx >= self.x && sy >= self.y && sx < self.x + w && sy < self.y + h
    }
    /// Screen point -> tile, clamped into the image (for touches near the parallelogram's slanted edges).
    pub fn to_tile_clamped(&self, sx: f32, sy: f32, world: u32) -> (u32, u32) {
        let n = self.n as f32;
        let r = ((sy - self.y) / self.s).clamp(0.0, n - 0.01);
        let c = ((sx - self.x) / self.s - (n - r) / 2.0).clamp(0.0, n - 0.01);
        image_to_tile(c, r, self.n, world)
    }
}

/// Image point -> tile.
pub fn image_to_tile(c: f32, r: f32, n: u32, world: u32) -> (u32, u32) {
    let k = world as f32 / n.max(1) as f32;
    let w1 = world.saturating_sub(1);
    (((c * k) as u32).min(w1), ((r * k) as u32).min(w1))
}

/// Draw the image (nearest sampling) and the camera rectangle.
pub fn draw(c: &mut Canvas, d: &MiniData, g: &MapGeom, cam_color: Rgba) {
    let (bw, bh) = g.box_size();
    let (fw, fh) = (c.fb.width as i32, c.fb.height as i32);
    let x0 = (g.x.floor() as i32).max(0);
    let y0 = (g.y.floor() as i32).max(0);
    let x1 = ((g.x + bw).ceil() as i32).min(fw);
    let y1 = ((g.y + bh).ceil() as i32).min(fh);
    let n = d.n as i32;
    if d.pixels.len() < (n * n) as usize {
        return;
    }
    for py in y0..y1 {
        let r = ((py as f32 + 0.5 - g.y) / g.s) as i32;
        if r < 0 || r >= n {
            continue;
        }
        let shift = (n - r) as f32 / 2.0;
        let row = &d.pixels[(r * n) as usize..((r + 1) * n) as usize];
        let line = (py * fw) as usize;
        for px in x0..x1 {
            let cf = (px as f32 + 0.5 - g.x) / g.s - shift;
            if cf < 0.0 {
                continue;
            }
            let col = cf as i32;
            if col >= n {
                break;
            }
            c.fb.pixels[line + px as usize] = row[col as usize];
        }
    }
    // Camera rectangle (a parallelogram on screen, like the game's white frame).
    let (cx, cy, cw, ch) = d.cam;
    if cw > 0 && ch > 0 {
        let pts = [
            g.to_screen(cx as f32, cy as f32),
            g.to_screen((cx + cw) as f32, cy as f32),
            g.to_screen((cx + cw) as f32, (cy + ch) as f32),
            g.to_screen(cx as f32, (cy + ch) as f32),
        ];
        let t = (g.s * 0.9).clamp(1.0, 3.0);
        for i in 0..4 {
            let (a, b) = (pts[i], pts[(i + 1) % 4]);
            thick_line(c, a.0, a.1, b.0, b.1, t, cam_color);
        }
    }
}

/// Outline of the parallelogram (frame around the map).
pub fn draw_outline(c: &mut Canvas, g: &MapGeom, col: Rgba) {
    let n = g.n as f32;
    let pts = [g.to_screen(0.0, 0.0), g.to_screen(n, 0.0), g.to_screen(n, n), g.to_screen(0.0, n)];
    for i in 0..4 {
        let (a, b) = (pts[i], pts[(i + 1) % 4]);
        thick_line(c, a.0, a.1, b.0, b.1, 1.0, col);
    }
}

fn thick_line(c: &mut Canvas, x0: f32, y0: f32, x1: f32, y1: f32, t: f32, col: Rgba) {
    let n = ((x1 - x0).abs().max((y1 - y0).abs()) as i32).max(1);
    let ti = t.round().max(1.0) as i32;
    for i in 0..=n {
        let k = i as f32 / n as f32;
        let (x, y) = (x0 + (x1 - x0) * k, y0 + (y1 - y0) * k);
        c.fill_rect(x as i32 - ti / 2, y as i32 - ti / 2, ti, ti, col);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geom_roundtrip() {
        let g = MapGeom::fit(128, 10, 20, 300, 200);
        let (w, h) = g.box_size();
        assert!(w <= 300.0 + 0.01 && h <= 200.0 + 0.01);
        for &(c, r) in &[(0.5, 0.5), (64.0, 64.0), (127.0, 3.0), (5.0, 120.0)] {
            let (sx, sy) = g.to_screen(c, r);
            let (c2, r2) = g.to_image(sx, sy).unwrap();
            assert!((c - c2).abs() < 0.01 && (r - r2).abs() < 0.01, "{c},{r} -> {c2},{r2}");
        }
        // Top-left corner of the bounding box is outside the sheared shape.
        assert!(g.to_image(g.x + 1.0, g.y + 1.0).is_none());
        assert!(g.in_box(g.x + 1.0, g.y + 1.0));
    }

    #[test]
    fn same_shear_as_game() {
        // The game: x = mx + (mw - r - 1)/2 + c. Ours with s = 1, x = mx: x = mx + (mw - r)/2 + c (half a pixel
        // to the right, pixel centres).
        let g = MapGeom { x: 5.0, y: 10.0, s: 1.0, n: 128 };
        let (sx, sy) = g.to_screen(10.0, 20.0);
        assert_eq!(sy, 30.0);
        assert_eq!(sx, 5.0 + (128.0 - 20.0) / 2.0 + 10.0);
    }

    #[test]
    fn tiles() {
        assert_eq!(image_to_tile(64.0, 32.0, 128, 384), (192, 96));
        assert_eq!(image_to_tile(127.99, 127.99, 128, 384), (383, 383));
        let g = MapGeom { x: 0.0, y: 0.0, s: 2.0, n: 128 };
        // Far left of the box: clamped to column 0.
        assert_eq!(g.to_tile_clamped(0.0, 0.0, 384).0, 0);
    }

    #[test]
    fn cam_formula() {
        // 128 px image, 384 world: k = 85.
        assert_eq!(cam_rect(128, 0, 0, 384, 0, 40), (11 * 85 / 256, 16 * 85 / 256, 0, 0));
        assert_eq!(cam_rect(128, 1, 1, 0, 1, 1), (0, 0, 0, 0));
        assert_eq!(div256(-255), 0);
        assert_eq!(div256(-256), -1);
    }
}
