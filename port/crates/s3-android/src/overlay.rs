//! Touch buttons in the black letterbox margins (docs/porting/android.md "Controls"): Esc/menu, on-screen
//! keyboard, right-click toggle. Drawn by the host after the game frame is blitted (the game never sees them);
//! touches that start on a button are not passed to the game.

use crate::layout::Letterbox;
use crate::screen::rgb;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    /// Sends VK_ESCAPE (in-game menu, close dialogs, skip intro).
    Esc,
    /// Shows / hides the Android soft keyboard.
    Keyboard,
    /// Arms "next tap = right click".
    RightClick,
}

pub const BUTTONS: [Button; 3] = [Button::Esc, Button::Keyboard, Button::RightClick];

impl Button {
    pub fn label(self) -> &'static str {
        match self {
            Button::Esc => "ESC",
            Button::Keyboard => "ABC",
            Button::RightClick => "RMB",
        }
    }
}

/// Rectangle in window-buffer pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    fn contains_grown(&self, x: f64, y: f64, grow: f64) -> bool {
        x >= self.x as f64 - grow && y >= self.y as f64 - grow && x < (self.x + self.w) as f64 + grow && y < (self.y + self.h) as f64 + grow
    }
}

/// Button size as a fraction of the shorter window side (about 1.2 cm on a phone).
const SIZE_FRAC: f32 = 0.12;

/// Where the buttons go for this letterbox: a column centred in the right bar (pillarbox, phones in landscape) or
/// a row in the bottom bar (letterbox). Empty when the bars are too narrow (e.g. a 4:3 tablet).
pub fn layout(lb: &Letterbox) -> Vec<(Button, Rect)> {
    let short = lb.win_w.min(lb.win_h) as f32;
    let scale = lb.scale().max(0.01); // window px per buffer px
    let s = ((short * SIZE_FRAC) / scale).round().max(16.0) as u32;
    let mut out = Vec::new();
    let n = BUTTONS.len() as u32;
    if lb.off_x >= s + 4 {
        // Right bar, buttons at 1/6, 3/6, 5/6 of the height.
        let bar_x = lb.off_x + lb.game_w;
        let bar_w = lb.buf_w - bar_x;
        let x = bar_x + (bar_w - s) / 2;
        for (i, b) in BUTTONS.iter().enumerate() {
            let cy = lb.buf_h * (2 * i as u32 + 1) / (2 * n);
            out.push((*b, Rect { x, y: cy.saturating_sub(s / 2), w: s, h: s }));
        }
    } else if lb.off_y >= s + 4 {
        let bar_y = lb.off_y + lb.game_h;
        let bar_h = lb.buf_h - bar_y;
        let y = bar_y + (bar_h - s) / 2;
        for (i, b) in BUTTONS.iter().enumerate() {
            let cx = lb.buf_w * (2 * i as u32 + 1) / (2 * n);
            out.push((*b, Rect { x: cx.saturating_sub(s / 2), y, w: s, h: s }));
        }
    }
    out
}

/// Button under a window pixel, if any (with a little extra margin around each button).
pub fn hit(lb: &Letterbox, wx: f32, wy: f32) -> Option<Button> {
    let bx = wx as f64 * lb.buf_w as f64 / lb.win_w.max(1) as f64;
    let by = wy as f64 * lb.buf_h as f64 / lb.win_h.max(1) as f64;
    let rects = layout(lb);
    let grow = rects.first().map(|r| r.1.w as f64 * 0.25).unwrap_or(0.0);
    rects.into_iter().find(|(_, r)| r.contains_grown(bx, by, grow)).map(|(b, _)| b)
}

/// Visual state of the buttons.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OverlayState {
    /// Finger currently down on this button.
    pub pressed: Option<Button>,
    /// Right-click mode armed.
    pub right_armed: bool,
    /// Soft keyboard believed visible.
    pub keyboard: bool,
}

/// Draw the buttons into a locked RGB565 window buffer (`stride` pixels per row, `rows` rows).
pub fn draw(lb: &Letterbox, st: &OverlayState, dst: &mut [u16], stride: usize, rows: usize) {
    for (b, r) in layout(lb) {
        let active = match b {
            Button::RightClick => st.right_armed,
            Button::Keyboard => st.keyboard,
            Button::Esc => false,
        };
        let pressed = st.pressed == Some(b);
        let fill = if pressed {
            rgb(110, 110, 130)
        } else if active {
            rgb(120, 90, 20)
        } else {
            rgb(36, 36, 44)
        };
        let border = if active { rgb(255, 210, 60) } else { rgb(150, 150, 165) };
        let bw = (r.w / 24).max(1);
        for y in r.y..(r.y + r.h).min(rows as u32) {
            let row = &mut dst[y as usize * stride..(y as usize + 1) * stride];
            for x in r.x..(r.x + r.w).min(stride as u32) {
                let edge = x < r.x + bw || x >= r.x + r.w - bw || y < r.y + bw || y >= r.y + r.h - bw;
                row[x as usize] = if edge { border } else { fill };
            }
        }
        let label = b.label();
        let scale = ((r.w * 3 / 4) / (8 * label.len() as u32)).max(1);
        let tw = 8 * scale * label.len() as u32;
        let tx = r.x + r.w.saturating_sub(tw) / 2;
        let ty = r.y + r.h.saturating_sub(8 * scale) / 2;
        draw_text(dst, stride, rows, tx, ty, label, scale, rgb(240, 240, 240));
    }
}

fn draw_text(dst: &mut [u16], stride: usize, rows: usize, x: u32, y: u32, text: &str, scale: u32, color: u16) {
    for (i, ch) in text.chars().enumerate() {
        let code = ch as usize;
        let glyph = font8x8::legacy::BASIC_LEGACY[if code < 128 { code } else { b'?' as usize }];
        let cx = x + i as u32 * 8 * scale;
        for (gy, bits) in glyph.iter().enumerate() {
            for gx in 0..8u32 {
                if bits >> gx & 1 == 0 {
                    continue;
                }
                for sy in 0..scale {
                    for sx in 0..scale {
                        let px = (cx + gx * scale + sx) as usize;
                        let py = (y + gy as u32 * scale + sy) as usize;
                        if px < stride && py < rows {
                            dst[py * stride + px] = color;
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------- GPU presenter: window-pixel layout

/// Button layout for a [`s3_mods::display::Viewport`], in window pixels: a column centred in the right bar
/// (or a row in the bottom bar). Empty when the bars are too narrow (Fill / Stretch / Native wide).
pub fn layout_vp(vp: &s3_mods::display::Viewport) -> Vec<(Button, Rect)> {
    let short = vp.win_w.min(vp.win_h) as f32;
    let s = (short * SIZE_FRAC).round().max(24.0) as u32;
    let (_l, r, _t, b) = vp.bars();
    let (dx, dy, dw, dh) = vp.dst_rect_clipped();
    let n = BUTTONS.len() as u32;
    let mut out = Vec::new();
    if r >= s + 8 {
        let bar_x = (dx + dw) as u32;
        let x = bar_x + (r - s) / 2;
        for (i, bt) in BUTTONS.iter().enumerate() {
            let cy = vp.win_h * (2 * i as u32 + 1) / (2 * n);
            out.push((*bt, Rect { x, y: cy.saturating_sub(s / 2), w: s, h: s }));
        }
    } else if b >= s + 8 {
        let bar_y = (dy + dh) as u32;
        let y = bar_y + (b - s) / 2;
        for (i, bt) in BUTTONS.iter().enumerate() {
            let cx = vp.win_w * (2 * i as u32 + 1) / (2 * n);
            out.push((*bt, Rect { x: cx.saturating_sub(s / 2), y, w: s, h: s }));
        }
    }
    out
}

/// Button under a window pixel (viewport layout).
pub fn hit_vp(vp: &s3_mods::display::Viewport, wx: f32, wy: f32) -> Option<Button> {
    let rects = layout_vp(vp);
    let grow = rects.first().map(|r| r.1.w as f64 * 0.25).unwrap_or(0.0);
    rects.into_iter().find(|(_, r)| r.contains_grown(wx as f64, wy as f64, grow)).map(|(b, _)| b)
}

/// One button rendered as an opaque RGB565 `size` x `size` image (uploaded as a texture by the GPU presenter).
pub fn button_image(b: Button, size: u32, st: &OverlayState) -> Vec<u16> {
    let s = size.max(8);
    let mut buf = vec![0u16; (s * s) as usize];
    draw_one(b, Rect { x: 0, y: 0, w: s, h: s }, st, &mut buf, s as usize, s as usize);
    buf
}

fn draw_one(b: Button, r: Rect, st: &OverlayState, dst: &mut [u16], stride: usize, rows: usize) {
    let active = match b {
        Button::RightClick => st.right_armed,
        Button::Keyboard => st.keyboard,
        Button::Esc => false,
    };
    let pressed = st.pressed == Some(b);
    let fill = if pressed {
        rgb(110, 110, 130)
    } else if active {
        rgb(120, 90, 20)
    } else {
        rgb(36, 36, 44)
    };
    let border = if active { rgb(255, 210, 60) } else { rgb(150, 150, 165) };
    let bw = (r.w / 24).max(1);
    for y in r.y..(r.y + r.h).min(rows as u32) {
        let row = &mut dst[y as usize * stride..(y as usize + 1) * stride];
        for x in r.x..(r.x + r.w).min(stride as u32) {
            let edge = x < r.x + bw || x >= r.x + r.w - bw || y < r.y + bw || y >= r.y + r.h - bw;
            row[x as usize] = if edge { border } else { fill };
        }
    }
    let label = b.label();
    let scale = ((r.w * 3 / 4) / (8 * label.len() as u32)).max(1);
    let tw = 8 * scale * label.len() as u32;
    let tx = r.x + r.w.saturating_sub(tw) / 2;
    let ty = r.y + r.h.saturating_sub(8 * scale) / 2;
    draw_text(dst, stride, rows, tx, ty, label, scale, rgb(240, 240, 240));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_in_right_bar_on_phone() {
        // In game: 640x480 on 2400x1080 -> buffer 1066x480, frame at x 213.
        let lb = Letterbox::new(640, 480, 2400, 1080);
        let l = layout(&lb);
        assert_eq!(l.len(), 3);
        for (_, r) in &l {
            assert!(r.x >= lb.off_x + lb.game_w, "{r:?} must be right of the frame");
            assert!(r.x + r.w <= lb.buf_w);
            assert!(r.y + r.h <= lb.buf_h);
        }
        // ~130 window px -> ~58 buffer px.
        assert!((50..=64).contains(&l[0].1.w), "{:?}", l[0]);
        // Hit test in window coordinates: centre of the first button.
        let r = l[0].1;
        let sc = lb.scale();
        let (wx, wy) = ((r.x + r.w / 2) as f32 * sc, (r.y + r.h / 2) as f32 * sc);
        assert_eq!(hit(&lb, wx, wy), Some(Button::Esc));
        let r = l[2].1;
        assert_eq!(hit(&lb, (r.x + 2) as f32 * sc, (r.y + 2) as f32 * sc), Some(Button::RightClick));
        // The game area and the left bar are not buttons.
        assert_eq!(hit(&lb, 1200.0, 540.0), None);
        assert_eq!(hit(&lb, 100.0, 540.0), None);
    }

    #[test]
    fn row_in_bottom_bar_when_tall() {
        let lb = Letterbox::new(800, 600, 1080, 2400);
        let l = layout(&lb);
        assert_eq!(l.len(), 3);
        assert!(l.iter().all(|(_, r)| r.y >= lb.off_y + lb.game_h));
    }

    #[test]
    fn viewport_layout_in_right_bar() {
        use s3_mods::display::{DisplayMode, Viewport};
        let vp = Viewport::new(DisplayMode::Fit, 800, 600, 2229, 904);
        let l = layout_vp(&vp);
        assert_eq!(l.len(), 3);
        let (dx, _, dw, _) = vp.dst_rect_clipped();
        for (_, r) in &l {
            assert!(r.x as i32 >= dx + dw && r.x + r.w <= vp.win_w && r.y + r.h <= vp.win_h, "{r:?}");
        }
        let r = l[1].1;
        assert_eq!(hit_vp(&vp, (r.x + r.w / 2) as f32, (r.y + r.h / 2) as f32), Some(Button::Keyboard));
        assert_eq!(hit_vp(&vp, 1100.0, 450.0), None);
        assert!(layout_vp(&Viewport::new(DisplayMode::Fill, 800, 600, 2229, 904)).is_empty());
        let img = button_image(Button::Esc, 64, &OverlayState::default());
        assert_eq!(img.len(), 64 * 64);
        assert!(img.iter().any(|&p| p == rgb(240, 240, 240)));
    }

    #[test]
    fn no_buttons_without_bars() {
        let lb = Letterbox::new(800, 600, 1600, 1200);
        assert!(layout(&lb).is_empty());
        assert_eq!(hit(&lb, 10.0, 10.0), None);
    }

    #[test]
    fn draw_stays_in_the_bar() {
        let lb = Letterbox::new(800, 600, 2400, 1080);
        let stride = lb.buf_w as usize + 6;
        let mut buf = vec![0u16; stride * lb.buf_h as usize];
        draw(&lb, &OverlayState { right_armed: true, ..Default::default() }, &mut buf, stride, lb.buf_h as usize);
        for y in 0..lb.buf_h as usize {
            for x in 0..(lb.off_x + lb.game_w) as usize {
                assert_eq!(buf[y * stride + x], 0, "pixel drawn over the game at {x},{y}");
            }
        }
        assert!(buf.iter().any(|&p| p != 0));
    }
}
