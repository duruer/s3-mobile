//! Letterboxing: the window buffer is set to a size with the *window's* aspect ratio that contains the game
//! frame unscaled; the compositor scales the buffer to the window. Black bars fill the rest.

/// Mapping between a game frame (`game_w` x `game_h`) and a window (`win_w` x `win_h` physical pixels).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Letterbox {
    pub game_w: u32,
    pub game_h: u32,
    pub win_w: u32,
    pub win_h: u32,
    /// Size of the window buffer (`ANativeWindow_setBuffersGeometry`).
    pub buf_w: u32,
    pub buf_h: u32,
    /// Top-left corner of the game frame inside the buffer.
    pub off_x: u32,
    pub off_y: u32,
}

impl Letterbox {
    pub fn new(game_w: u32, game_h: u32, win_w: u32, win_h: u32) -> Letterbox {
        let (game_w, game_h) = (game_w.max(1), game_h.max(1));
        let (win_w, win_h) = (win_w.max(1), win_h.max(1));
        let (mut buf_w, mut buf_h) = (game_w, game_h);
        // Compare aspect ratios with integer math: win_w/win_h vs game_w/game_h.
        if (win_w as u64) * (game_h as u64) >= (game_w as u64) * (win_h as u64) {
            // Window is wider: pillarbox (bars left/right).
            buf_w = ((game_h as u64 * win_w as u64 + win_h as u64 - 1) / win_h as u64) as u32;
            buf_w = (buf_w + 1) & !1;
            buf_w = buf_w.max(game_w);
        } else {
            // Window is taller: letterbox (bars top/bottom).
            buf_h = ((game_w as u64 * win_h as u64 + win_w as u64 - 1) / win_w as u64) as u32;
            buf_h = (buf_h + 1) & !1;
            buf_h = buf_h.max(game_h);
        }
        Letterbox {
            game_w,
            game_h,
            win_w,
            win_h,
            buf_w,
            buf_h,
            off_x: (buf_w - game_w) / 2,
            off_y: (buf_h - game_h) / 2,
        }
    }

    /// Window pixel -> game pixel (clamped to the game frame).
    pub fn window_to_game(&self, x: f32, y: f32) -> (i32, i32) {
        let bx = x as f64 * self.buf_w as f64 / self.win_w as f64;
        let by = y as f64 * self.buf_h as f64 / self.win_h as f64;
        let gx = (bx - self.off_x as f64).floor() as i64;
        let gy = (by - self.off_y as f64).floor() as i64;
        (gx.clamp(0, self.game_w as i64 - 1) as i32, gy.clamp(0, self.game_h as i64 - 1) as i32)
    }

    /// Window pixels per game pixel (for touch slop thresholds).
    pub fn scale(&self) -> f32 {
        self.win_w as f32 / self.buf_w as f32
    }
}

/// Copy an RGB565 frame into a locked window buffer (`stride` in pixels) with black bars.
/// `dst` must hold at least `stride * buf_h` pixels.
pub fn blit_letterboxed(lb: &Letterbox, src: &[u16], dst: &mut [u16], stride: usize, dst_h: usize) {
    let (gw, gh) = (lb.game_w as usize, lb.game_h as usize);
    let (ox, oy) = (lb.off_x as usize, lb.off_y as usize);
    let rows = dst_h.min(dst.len() / stride.max(1));
    for y in 0..rows {
        let row = &mut dst[y * stride..(y + 1) * stride];
        let gy = y.wrapping_sub(oy);
        if gy >= gh || ox >= stride {
            row.fill(0);
            continue;
        }
        let w = gw.min(stride - ox);
        row[..ox].fill(0);
        let s = &src[gy * gw..gy * gw + w];
        row[ox..ox + w].copy_from_slice(s);
        row[ox + w..].fill(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pillarbox_on_wide_phone() {
        // 2400x1080 landscape phone, 800x600 game.
        let lb = Letterbox::new(800, 600, 2400, 1080);
        assert_eq!(lb.buf_h, 600);
        assert_eq!(lb.buf_w, 1334);
        assert_eq!(lb.off_x, 267);
        assert_eq!(lb.off_y, 0);
        // Window centre -> game centre.
        assert_eq!(lb.window_to_game(1200.0, 540.0), (400, 300));
        // Left bar clamps to x=0.
        assert_eq!(lb.window_to_game(10.0, 0.0), (0, 0));
    }

    #[test]
    fn letterbox_on_tall_window() {
        let lb = Letterbox::new(640, 480, 1080, 2400);
        assert_eq!(lb.buf_w, 640);
        assert!(lb.buf_h > 480);
        assert_eq!(lb.window_to_game(540.0, 1200.0), (320, 240));
    }

    #[test]
    fn exact_aspect_has_no_bars() {
        let lb = Letterbox::new(800, 600, 1600, 1200);
        assert_eq!((lb.buf_w, lb.buf_h, lb.off_x, lb.off_y), (800, 600, 0, 0));
        assert_eq!(lb.window_to_game(1599.0, 1199.0), (799, 599));
    }

    #[test]
    fn blit_puts_frame_in_the_middle() {
        let lb = Letterbox::new(2, 2, 4, 2); // buffer 4x2, offset 1
        let src = [1u16, 2, 3, 4];
        let mut dst = vec![9u16; 6 * 2]; // stride 6 > buf_w
        blit_letterboxed(&lb, &src, &mut dst, 6, 2);
        assert_eq!(&dst[..6], &[0, 1, 2, 0, 0, 0]);
        assert_eq!(&dst[6..], &[0, 3, 4, 0, 0, 0]);
    }
}
