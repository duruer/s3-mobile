//! Procedural stone texture for the window area around the full-screen menu layouts (user request 2026-11-08:
//! "use the Settlers stone background instead of a flat colour, so everything looks smooth").
//!
//! Our own generated image (no game asset): blue-grey value noise with diagonal streaks like the game's stone
//! screens, tileable ([`TILE`] x [`TILE`], every term periodic), drawn at the layout's scale (one texel = one game
//! pixel) by the CPU renderer (`menu_layout::render_xrgb`) and the Android GLES presenter (`GL_REPEAT` texture).

use std::sync::OnceLock;

/// Tile size (texels, power of two for `GL_REPEAT` on GLES 2).
pub const TILE: usize = 256;

/// Colour ramp dark -> light (sampled from the game's stone screens, rgb565-rounded there).
const RAMP: [(f32, [f32; 3]); 4] = [
    (0.0, [33.0, 48.0, 66.0]),
    (0.3, [70.0, 88.0, 108.0]),
    (0.6, [88.0, 120.0, 150.0]),
    (1.0, [132.0, 160.0, 172.0]),
];

fn hash(x: u32, y: u32, seed: u32) -> f32 {
    let mut h = x.wrapping_mul(0x8da6_b343) ^ y.wrapping_mul(0xd816_3841) ^ seed.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0xffff) as f32 / 65535.0
}

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Periodic 2D value noise: lattice of `cells` per tile in each direction.
fn noise2(x: f32, y: f32, cells: u32, seed: u32) -> f32 {
    let f = cells as f32 / TILE as f32;
    let (gx, gy) = (x * f, y * f);
    let (ix, iy) = (gx.floor() as i64, gy.floor() as i64);
    let (tx, ty) = (smooth(gx - ix as f32), smooth(gy - iy as f32));
    let c = cells as i64;
    let at = |dx: i64, dy: i64| hash((ix + dx).rem_euclid(c) as u32, (iy + dy).rem_euclid(c) as u32, seed);
    let a = at(0, 0) + (at(1, 0) - at(0, 0)) * tx;
    let b = at(0, 1) + (at(1, 1) - at(0, 1)) * tx;
    a + (b - a) * ty
}

/// Periodic 1D value noise over `t` in [0, TILE).
fn noise1(t: f32, cells: u32, seed: u32) -> f32 {
    let g = t * cells as f32 / TILE as f32;
    let i = g.floor() as i64;
    let f = smooth(g - i as f32);
    let c = cells as i64;
    let a = hash(i.rem_euclid(c) as u32, 7, seed);
    let b = hash((i + 1).rem_euclid(c) as u32, 7, seed);
    a + (b - a) * f
}

fn value(x: usize, y: usize) -> f32 {
    let (fx, fy) = (x as f32, y as f32);
    // Blotchy rock: fBm, contrast stretched (light blue patches on grey-blue).
    let mut base = 0.0;
    let mut amp = 0.5;
    let mut norm = 0.0;
    for (k, cells) in [16u32, 32, 64, 128].iter().enumerate() {
        base += amp * noise2(fx, fy, *cells, 11 + k as u32);
        norm += amp;
        amp *= 0.75;
    }
    let b = smooth(((base / norm - 0.28) / 0.44).clamp(0.0, 1.0));
    // Thin dark cracks rising to the right like the game's stone: ridges of a 1D noise along t = x + 2y (constant
    // along (2, -1); integer coefficients keep the tile periodic), warped and broken up by 2D noise.
    let warp = (noise2(fx, fy, 32, 31) - 0.5) * 10.0;
    let t = (x as i64 + 2 * y as i64) as f32 + warp;
    let r = (noise1(t, 160, 3) - 0.5).abs();
    let crack = (1.0 - r / 0.06).clamp(0.0, 1.0) * smooth(((noise2(fx, fy, 32, 41) - 0.15) / 0.3).clamp(0.0, 1.0));
    let r2 = (noise1(t + 37.0, 256, 9) - 0.5).abs();
    let crack2 = (1.0 - r2 / 0.06).clamp(0.0, 1.0) * smooth(((noise2(fx, fy, 64, 43) - 0.2) / 0.3).clamp(0.0, 1.0));
    let grain = (noise2(fx, fy, 256, 99) - 0.5) * 0.22;
    (0.17 + 0.58 * b - 0.6 * crack - 0.45 * crack2 + grain).clamp(0.0, 1.0)
}

fn ramp(v: f32) -> [u8; 3] {
    for w in RAMP.windows(2) {
        let ((a, ca), (b, cb)) = (w[0], w[1]);
        if v <= b {
            let t = ((v - a) / (b - a)).clamp(0.0, 1.0);
            return [0, 1, 2].map(|i| (ca[i] + (cb[i] - ca[i]) * t).round() as u8);
        }
    }
    RAMP[RAMP.len() - 1].1.map(|c| c as u8)
}

/// The tile as RGBA8 rows (`TILE * TILE * 4` bytes), generated once.
pub fn tile_rgba() -> &'static [u8] {
    static T: OnceLock<Vec<u8>> = OnceLock::new();
    T.get_or_init(|| {
        let mut out = Vec::with_capacity(TILE * TILE * 4);
        for y in 0..TILE {
            for x in 0..TILE {
                let [r, g, b] = ramp(value(x, y));
                out.extend_from_slice(&[r, g, b, 255]);
            }
        }
        out
    })
}

/// Texel at (x, y) (wrapped) as XRGB8888.
pub fn texel_xrgb(x: i64, y: i64) -> u32 {
    let t = tile_rgba();
    let (x, y) = (x.rem_euclid(TILE as i64) as usize, y.rem_euclid(TILE as i64) as usize);
    let i = (y * TILE + x) * 4;
    ((t[i] as u32) << 16) | ((t[i + 1] as u32) << 8) | t[i + 2] as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_wraps_seamlessly() {
        // Neighbouring texels across the wrap differ about as much as inside the tile.
        let d = |a: u32, b: u32| ((a >> 16 & 255) as i32 - (b >> 16 & 255) as i32).abs();
        let mut edge = 0;
        let mut inner = 0;
        for y in 0..TILE as i64 {
            edge += d(texel_xrgb(TILE as i64 - 1, y), texel_xrgb(TILE as i64, y));
            inner += d(texel_xrgb(100, y), texel_xrgb(101, y));
        }
        assert!(edge < inner * 2 + TILE as i32, "edge {edge} inner {inner}");
        assert_eq!(tile_rgba().len(), TILE * TILE * 4);
    }
}
