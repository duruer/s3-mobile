//! The game's mouse cursor in the window layer (docs/porting/display.md "Cursor in the window layer").
//!
//! `s3_runtime::cursor` draws the Windows cursor the game set into the presented frame, so with the zoom
//! (= render size) it grew, shrank and moved on screen together with the map. When a frontend shows the window
//! layer, this mod turns that off (`CursorState::disabled`) and draws the same image here: at the window position
//! of the emulated mouse (through the shown viewport, so it stays where the finger / mouse is) and at a constant
//! size (the cursor's size in a 600 px high frame shown full height, `Layer::ui`).

use std::collections::HashMap;
use std::sync::Arc;

use s3_runtime::cursor::{CursorImage, CursorPixel, CursorState};
use s3_runtime::Machine;

use crate::layer::{ImageRef, Layer};
use crate::overlay::Image;
use crate::{FrameBuffer, Mod};

/// What the cursor looks like now: window-independent part (game position, HCURSOR).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Shown {
    x: i32,
    y: i32,
    handle: u32,
}

#[derive(Default)]
pub struct CursorMod {
    /// RGBA images by HCURSOR.
    images: HashMap<u32, (ImageRef, i32, i32)>,
    /// Cursor to draw (from the last tick), `None` = hidden.
    want: Option<Shown>,
    /// Cursor in the last recorded layer.
    drawn: Option<Shown>,
    /// The frontend shows the window layer (the frame cursor is turned off).
    active: bool,
}

fn rgb565_to_rgba(c: u16) -> [u8; 4] {
    let r = ((c >> 11) & 31) as u32 * 255 / 31;
    let g = ((c >> 5) & 63) as u32 * 255 / 63;
    let b = (c & 31) as u32 * 255 / 31;
    [r as u8, g as u8, b as u8, 255]
}

/// RGBA copy of a cursor (XOR pixels: their colour, usually white = the inverted outline on dark ground).
fn to_rgba(img: &CursorImage) -> Image {
    let mut out = Image::new(img.width, img.height);
    for (i, p) in img.pixels.iter().enumerate() {
        let px = match *p {
            CursorPixel::Clear => [0, 0, 0, 0],
            CursorPixel::Color(c) | CursorPixel::Xor(c) => rgb565_to_rgba(c),
            CursorPixel::Blend(c, a) => {
                let [r, g, b, _] = rgb565_to_rgba(c);
                [r, g, b, a]
            }
        };
        out.rgba[i * 4..i * 4 + 4].copy_from_slice(&px);
    }
    out
}

impl CursorMod {
    /// Read the cursor state; turn the frame cursor off while the window layer is shown.
    fn update(&mut self, m: &mut Machine) {
        self.active = m.state::<crate::ModsState>().borrow_mut().layer_sink.is_some();
        let (mx, my) = (m.input.mouse_x, m.input.mouse_y);
        let cs = m.state::<CursorState>();
        let mut c = cs.borrow_mut();
        c.disabled = self.active;
        if !self.active || c.show_count < 0 || c.current == 0 {
            self.want = None;
            return;
        }
        let Some(img) = c.images.get(&c.current).cloned() else {
            self.want = None;
            return;
        };
        let (x, y) = match c.clip {
            Some((l, t, r, b)) => (mx.clamp(l, (r - 1).max(l)), my.clamp(t, (b - 1).max(t))),
            None => (mx, my),
        };
        let handle = c.current;
        drop(c);
        self.images.entry(handle).or_insert_with(|| (Arc::new(to_rgba(&img)), img.hot_x, img.hot_y));
        self.want = Some(Shown { x, y, handle });
    }
}

impl Mod for CursorMod {
    fn name(&self) -> &str {
        "cursor"
    }

    fn wants_draw(&self) -> bool {
        self.want.is_some()
    }

    fn needs_redraw(&self) -> bool {
        self.want != self.drawn
    }

    fn tick(&mut self, m: &mut Machine, _now: u64) {
        self.update(m);
    }

    fn draw_window(&mut self, m: &mut Machine, l: &mut Layer, _frame: &FrameBuffer) {
        self.update(m);
        let Some(s) = self.want else { return };
        let Some((img, hx, hy)) = self.images.get(&s.handle).cloned() else { return };
        let vp = crate::draw_viewport(m);
        let (wx, wy) = vp.game_to_window(s.x as f32, s.y as f32);
        let k = l.ui();
        let (w, h) = ((img.width as f32 * k).round() as i32, (img.height as f32 * k).round() as i32);
        let (x, y) = ((wx - hx as f32 * k).round() as i32, (wy - hy as f32 * k).round() as i32);
        l.blit_scaled(&img, x, y, w, h, 1.0);
        self.drawn = Some(s);
    }
}

/// Add the mod (last, so the cursor is drawn on top of every other window-layer UI).
pub fn install(m: &mut Machine) {
    crate::add_mod(m, Box::new(CursorMod::default()));
}
