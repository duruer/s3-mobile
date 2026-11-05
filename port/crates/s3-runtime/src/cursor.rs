//! Mouse cursor emulation (the game runs with registry `GDIMouse`=1, i.e. it uses the Windows cursor).
//!
//! - Decoding of Win32 cursor resources (`RT_GROUP_CURSOR` -> `RT_CURSOR`): monochrome AND/XOR cursors and
//!   colour DIB cursors (1/4/8/24/32 bpp, 32 bpp with per-pixel alpha).
//! - A built-in arrow for system cursors (`LoadCursorA(NULL, IDC_*)`), drawn from code here (not copied from
//!   Windows).
//! - [`CursorState`] (machine state, written by user32 `LoadCursorA`/`SetCursor`/`ShowCursor`/`ClipCursor`).
//! - [`CursorState::compose`]: called by [`Machine::present`](crate::Machine::present) to draw the current
//!   cursor at the emulated mouse position into a host-side copy of the frame. Guest surfaces are never written,
//!   so every frontend gets the cursor for free.

use std::collections::HashMap;
use std::sync::Arc;

use crate::host::FrameBuffer;
use crate::resources::{find_resource, rt, ResId};
use crate::Machine;

/// One cursor pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorPixel {
    /// Screen shows through (AND=1, XOR=0).
    Clear,
    /// Opaque RGB565 colour (AND=0).
    Color(u16),
    /// Screen XOR RGB565 colour (AND=1, XOR!=0; white = invert).
    Xor(u16),
    /// RGB565 colour blended with alpha 1..254 (32 bpp alpha cursors).
    Blend(u16, u8),
}

/// A decoded cursor image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorImage {
    pub width: u32,
    pub height: u32,
    pub hot_x: i32,
    pub hot_y: i32,
    /// Row-major, top row first.
    pub pixels: Vec<CursorPixel>,
}

#[inline]
fn rgb565(r: u8, g: u8, b: u8) -> u16 {
    ((r as u16 >> 3) << 11) | ((g as u16 >> 2) << 5) | (b as u16 >> 3)
}

fn rd16(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}

fn rd32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

/// Pick the `RT_CURSOR` id from `RT_GROUP_CURSOR` data (`NEWHEADER` + 14-byte `CURSORDIRENTRY`s):
/// the entry whose width is closest to `want` (ties: higher bit count).
pub fn pick_group_entry(grp: &[u8], want: u32) -> Option<u16> {
    let count = rd16(grp, 4)? as usize;
    let mut best: Option<(u32, u16, u16)> = None; // (distance, bitcount, id)
    for i in 0..count {
        let e = 6 + i * 14;
        let w = rd16(grp, e)? as u32;
        let bits = rd16(grp, e + 6)?;
        let id = rd16(grp, e + 12)?;
        let w = if w == 0 { 256 } else { w };
        let d = w.abs_diff(want);
        let better = match best {
            None => true,
            Some((bd, bb, _)) => d < bd || (d == bd && bits > bb),
        };
        if better {
            best = Some((d, bits, id));
        }
    }
    best.map(|b| b.2)
}

/// Decode `RT_CURSOR` resource data: `u16 hot_x, u16 hot_y`, then a DIB (`BITMAPINFOHEADER`, colour table,
/// XOR bitmap, AND mask; `biHeight` counts both bitmaps).
pub fn decode_cursor_resource(data: &[u8]) -> Option<CursorImage> {
    let hx = rd16(data, 0)? as i32;
    let hy = rd16(data, 2)? as i32;
    decode_dib_cursor(data.get(4..)?, hx, hy)
}

/// Decode a cursor/icon DIB (XOR image + AND mask) with the given hot spot.
pub fn decode_dib_cursor(dib: &[u8], hot_x: i32, hot_y: i32) -> Option<CursorImage> {
    let hsize = rd32(dib, 0)? as usize;
    if hsize < 40 {
        return None;
    }
    let w = rd32(dib, 4)? as i32;
    let h2 = rd32(dib, 8)? as i32;
    let bpp = rd16(dib, 14)? as u32;
    let compression = rd32(dib, 16)?;
    let clr_used = rd32(dib, 32)? as usize;
    if w <= 0 || w > 256 || h2 == 0 || compression != 0 {
        return None;
    }
    let (w, h) = (w as u32, (h2.unsigned_abs() / 2).max(1));
    if h > 256 {
        return None;
    }
    let top_down = h2 < 0;
    let ncolors = if bpp <= 8 { if clr_used != 0 { clr_used } else { 1usize << bpp } } else { 0 };
    let pal_off = hsize;
    let palette: Vec<(u8, u8, u8)> = (0..ncolors)
        .map(|i| {
            let o = pal_off + i * 4;
            dib.get(o..o + 3).map(|c| (c[2], c[1], c[0]))
        })
        .collect::<Option<_>>()?;
    let xor_off = pal_off + ncolors * 4;
    let xor_stride = ((w * bpp + 31) / 32 * 4) as usize;
    let and_off = xor_off + xor_stride * h as usize;
    let and_stride = ((w + 31) / 32 * 4) as usize;
    let has_and = dib.len() >= and_off + and_stride * h as usize;
    if dib.len() < and_off {
        return None;
    }
    let row_of = |y: u32| if top_down { y } else { h - 1 - y } as usize;
    // 32 bpp with any non-zero alpha: alpha channel is authoritative.
    let use_alpha = bpp == 32
        && (0..h).any(|y| (0..w).any(|x| dib[xor_off + row_of(y) * xor_stride + x as usize * 4 + 3] != 0));
    let mut pixels = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        let xr = xor_off + row_of(y) * xor_stride;
        let ar = and_off + row_of(y) * and_stride;
        for x in 0..w {
            let (r, g, b, a) = match bpp {
                1 | 4 | 8 => {
                    let bit = (x * bpp) as usize;
                    let byte = dib[xr + bit / 8];
                    let idx = (byte >> (8 - bpp as usize - bit % 8)) & ((1u16 << bpp) - 1) as u8;
                    let (r, g, b) = palette.get(idx as usize).copied().unwrap_or((0, 0, 0));
                    (r, g, b, 255)
                }
                24 => {
                    let o = xr + x as usize * 3;
                    (dib[o + 2], dib[o + 1], dib[o], 255)
                }
                32 => {
                    let o = xr + x as usize * 4;
                    (dib[o + 2], dib[o + 1], dib[o], dib[o + 3])
                }
                _ => return None,
            };
            let and = has_and && (dib[ar + x as usize / 8] >> (7 - x % 8)) & 1 != 0;
            let c = rgb565(r, g, b);
            let px = if use_alpha {
                match a {
                    0 => CursorPixel::Clear,
                    255 => CursorPixel::Color(c),
                    a => CursorPixel::Blend(c, a),
                }
            } else if !and {
                CursorPixel::Color(c)
            } else if c == 0 {
                CursorPixel::Clear
            } else {
                CursorPixel::Xor(c)
            };
            pixels.push(px);
        }
    }
    Some(CursorImage { width: w, height: h, hot_x, hot_y, pixels })
}

/// Our own arrow (12x19, black outline, white fill, hot spot at the tip), used for every system cursor.
pub fn builtin_arrow() -> CursorImage {
    const ART: [&str; 19] = [
        "B...........",
        "BB..........",
        "BWB.........",
        "BWWB........",
        "BWWWB.......",
        "BWWWWB......",
        "BWWWWWB.....",
        "BWWWWWWB....",
        "BWWWWWWWB...",
        "BWWWWWWWWB..",
        "BWWWWWWWWWB.",
        "BWWWWWWBBBBB",
        "BWWWBWWB....",
        "BWWBBWWB....",
        "BWB..BWWB...",
        "BB...BWWB...",
        "B.....BWWB..",
        "......BWWB..",
        ".......BB...",
    ];
    let pixels = ART
        .iter()
        .flat_map(|row| {
            row.bytes().map(|c| match c {
                b'B' => CursorPixel::Color(0x0000),
                b'W' => CursorPixel::Color(0xFFFF),
                _ => CursorPixel::Clear,
            })
        })
        .collect();
    CursorImage { width: 12, height: 19, hot_x: 0, hot_y: 0, pixels }
}

/// Load a cursor from the `.rsrc` of the module mapped at `module_base` (`RT_GROUP_CURSOR` `name`, best
/// entry for 32x32). Falls back to a direct `RT_CURSOR` lookup.
pub fn load_cursor_resource(m: &Machine, module_base: u32, name: &ResId) -> Option<CursorImage> {
    let cur_id = match find_resource(m, module_base, &ResId::Id(rt::GROUP_CURSOR), name, None) {
        Some((addr, size)) => ResId::Id(pick_group_entry(m.mem.slice(addr, size as usize), 32)?),
        None => name.clone(),
    };
    let (addr, size) = find_resource(m, module_base, &ResId::Id(rt::CURSOR), &cur_id, None)?;
    decode_cursor_resource(m.mem.slice(addr, size as usize))
}

/// Cursor state shared between user32 (writer) and [`Machine::present`](crate::Machine::present) (reader).
#[derive(Debug, Default)]
pub struct CursorState {
    /// Decoded images by HCURSOR value.
    pub images: HashMap<u32, Arc<CursorImage>>,
    /// Current HCURSOR (`SetCursor`); 0 = no cursor.
    pub current: u32,
    /// `ShowCursor` display counter; drawn when >= 0.
    pub show_count: i32,
    /// `ClipCursor` rectangle (left, top, right, bottom) the drawn position is clamped to.
    pub clip: Option<(i32, i32, i32, i32)>,
    /// Frontend switch: set to true to never draw the cursor.
    pub disabled: bool,
    /// Copy of the last frame the guest presented (without cursor), for cursor-only refreshes.
    src: Option<FrameBuffer>,
    /// Frame with the cursor composed in.
    scratch: Option<FrameBuffer>,
    /// What the host shows now: (x, y, HCURSOR) of the drawn cursor, `None` = no cursor drawn.
    drawn: Option<(i32, i32, u32)>,
}

impl CursorState {
    /// The image to draw now, if the cursor is visible.
    pub fn visible_image(&self) -> Option<&Arc<CursorImage>> {
        if self.disabled || self.show_count < 0 || self.current == 0 {
            return None;
        }
        self.images.get(&self.current)
    }

    fn clamp(&self, x: i32, y: i32) -> (i32, i32) {
        match self.clip {
            Some((l, t, r, b)) => (x.clamp(l, (r - 1).max(l)), y.clamp(t, (b - 1).max(t))),
            None => (x, y),
        }
    }

    /// Wanted cursor at mouse (`x`, `y`): (x, y, HCURSOR) or `None` if hidden.
    fn wanted(&self, x: i32, y: i32) -> Option<(i32, i32, u32)> {
        self.visible_image()?;
        let (x, y) = self.clamp(x, y);
        Some((x, y, self.current))
    }

    /// A guest frame is presented: remember it and compose the visible cursor at mouse position (`x`, `y`)
    /// over a copy. Returns `None` when no cursor is visible (present `fb` itself then).
    pub fn compose(&mut self, fb: &FrameBuffer, x: i32, y: i32) -> Option<&FrameBuffer> {
        let mut src = self.src.take().unwrap_or_else(|| FrameBuffer::new(0, 0));
        copy_frame(&mut src, fb);
        self.src = Some(src);
        self.drawn = self.wanted(x, y);
        self.drawn?;
        self.recompose()
    }

    /// Cursor-only update between guest presents (the game redraws nothing when only the mouse moves; a
    /// real Windows cursor is a hardware overlay). Returns the frame to show if the cursor moved, changed
    /// shape or visibility since the last frame handed to the host.
    pub fn refresh(&mut self, x: i32, y: i32) -> Option<&FrameBuffer> {
        let want = self.wanted(x, y);
        if want == self.drawn || self.src.is_none() {
            return None;
        }
        self.drawn = want;
        if want.is_none() {
            return self.src.as_ref();
        }
        self.recompose()
    }

    fn recompose(&mut self) -> Option<&FrameBuffer> {
        let (x, y, _) = self.drawn?;
        let img = self.visible_image()?.clone();
        let mut out = self.scratch.take().unwrap_or_else(|| FrameBuffer::new(0, 0));
        copy_frame(&mut out, self.src.as_ref()?);
        draw_cursor(&mut out, &img, x, y);
        self.scratch = Some(out);
        self.scratch.as_ref()
    }
}

fn copy_frame(dst: &mut FrameBuffer, src: &FrameBuffer) {
    dst.width = src.width;
    dst.height = src.height;
    dst.pixels.clear();
    dst.pixels.extend_from_slice(&src.pixels);
}

/// Draw `img` with its hot spot at (`x`, `y`) into `fb` (clipped to the frame).
pub fn draw_cursor(fb: &mut FrameBuffer, img: &CursorImage, x: i32, y: i32) {
    let (fw, fh) = (fb.width as i32, fb.height as i32);
    if fb.pixels.len() < (fw * fh) as usize {
        return;
    }
    let (ox, oy) = (x - img.hot_x, y - img.hot_y);
    for cy in 0..img.height as i32 {
        let sy = oy + cy;
        if sy < 0 || sy >= fh {
            continue;
        }
        for cx in 0..img.width as i32 {
            let sx = ox + cx;
            if sx < 0 || sx >= fw {
                continue;
            }
            let d = &mut fb.pixels[(sy * fw + sx) as usize];
            match img.pixels[(cy * img.width as i32 + cx) as usize] {
                CursorPixel::Clear => {}
                CursorPixel::Color(c) => *d = c,
                CursorPixel::Xor(c) => *d ^= c,
                CursorPixel::Blend(c, a) => *d = blend565(*d, c, a),
            }
        }
    }
}

fn blend565(dst: u16, src: u16, a: u8) -> u16 {
    let a = a as u32;
    let mix = |s: u32, d: u32| (s * a + d * (255 - a)) / 255;
    let (s, d) = (src as u32, dst as u32);
    let r = mix(s >> 11, d >> 11);
    let g = mix((s >> 5) & 0x3F, (d >> 5) & 0x3F);
    let b = mix(s & 0x1F, d & 0x1F);
    ((r << 11) | (g << 5) | b) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic 4x4 monochrome cursor (hot spot 1,2), stored as in a PE `RT_CURSOR` (bottom-up rows).
    /// Top row (y=0): AND 0, XOR 1 -> white; y=1: AND 0, XOR 0 -> black; y=2: AND 1, XOR 0 -> clear;
    /// y=3: AND 1, XOR 1 -> invert. Column 3 of row 0 is black (XOR bit clear).
    fn synthetic_mono() -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&2u16.to_le_bytes());
        // BITMAPINFOHEADER
        v.extend_from_slice(&40u32.to_le_bytes());
        v.extend_from_slice(&4i32.to_le_bytes());
        v.extend_from_slice(&8i32.to_le_bytes()); // 2 * height
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes()); // 1 bpp
        v.extend_from_slice(&[0u8; 24]);
        // palette: black, white
        v.extend_from_slice(&[0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0]);
        // XOR rows, bottom-up: y=3, y=2, y=1, y=0 (each 4-byte stride)
        for row in [0xF0u8, 0x00, 0x00, 0xE0] {
            v.extend_from_slice(&[row, 0, 0, 0]);
        }
        // AND rows, bottom-up
        for row in [0xF0u8, 0xF0, 0x00, 0x00] {
            v.extend_from_slice(&[row, 0, 0, 0]);
        }
        v
    }

    #[test]
    fn decodes_monochrome_cursor() {
        let img = decode_cursor_resource(&synthetic_mono()).expect("decode");
        assert_eq!((img.width, img.height, img.hot_x, img.hot_y), (4, 4, 1, 2));
        use CursorPixel::*;
        assert_eq!(&img.pixels[0..4], &[Color(0xFFFF), Color(0xFFFF), Color(0xFFFF), Color(0)]);
        assert_eq!(&img.pixels[4..8], &[Color(0); 4]);
        assert_eq!(&img.pixels[8..12], &[Clear; 4]);
        assert_eq!(&img.pixels[12..16], &[Xor(0xFFFF); 4]);
    }

    #[test]
    fn decodes_32bpp_alpha_cursor() {
        let mut v = vec![0u8; 4];
        v.extend_from_slice(&40u32.to_le_bytes());
        v.extend_from_slice(&2i32.to_le_bytes());
        v.extend_from_slice(&2i32.to_le_bytes()); // 1 row
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&32u16.to_le_bytes());
        v.extend_from_slice(&[0u8; 24]);
        v.extend_from_slice(&[0xFF, 0, 0, 0xFF, 0, 0xFF, 0, 0x80]); // blue opaque, green half
        v.extend_from_slice(&[0u8; 4]); // AND row
        let img = decode_dib_cursor(&v[4..], 0, 0).unwrap();
        assert_eq!(img.pixels, vec![CursorPixel::Color(0x001F), CursorPixel::Blend(0x07E0, 0x80)]);
    }

    #[test]
    fn picks_group_entry_and_composes_with_hotspot_and_show_count() {
        // NEWHEADER (reserved, type 2, count 2) + 2 entries: 16px id 7, 32px id 9
        let mut g = vec![0, 0, 2, 0, 2, 0];
        for (w, id) in [(16u16, 7u16), (32, 9)] {
            g.extend_from_slice(&w.to_le_bytes());
            g.extend_from_slice(&(w * 2).to_le_bytes());
            g.extend_from_slice(&1u16.to_le_bytes());
            g.extend_from_slice(&1u16.to_le_bytes());
            g.extend_from_slice(&308u32.to_le_bytes());
            g.extend_from_slice(&id.to_le_bytes());
        }
        assert_eq!(pick_group_entry(&g, 32), Some(9));

        let img = decode_cursor_resource(&synthetic_mono()).unwrap();
        let mut st = CursorState::default();
        st.images.insert(0x55, Arc::new(img));
        let fb = FrameBuffer { width: 8, height: 8, pixels: vec![0x1234; 64] };
        assert!(st.compose(&fb, 4, 4).is_none(), "no SetCursor yet");
        st.current = 0x55;
        let out = st.compose(&fb, 4, 4).unwrap().clone();
        // hot spot (1,2) at (4,4): image origin (3,2)
        assert_eq!(out.pixels[2 * 8 + 3], 0xFFFF);
        assert_eq!(out.pixels[2 * 8 + 6], 0x0000);
        assert_eq!(out.pixels[4 * 8 + 3], 0x1234); // clear row
        assert_eq!(out.pixels[5 * 8 + 3], 0x1234 ^ 0xFFFF); // invert row
        assert_eq!(fb.pixels[2 * 8 + 3], 0x1234, "source frame untouched");
        st.show_count = -1;
        assert!(st.compose(&fb, 4, 4).is_none(), "hidden by ShowCursor(FALSE)");
        // clipped at the frame edge: no panic
        st.show_count = 0;
        assert!(st.compose(&fb, 7, 7).is_some());
        assert!(st.compose(&fb, -10, 100).is_some());
        // refresh: unchanged -> nothing; moved -> redrawn from the remembered frame; hidden -> plain frame
        assert!(st.refresh(-10, 100).is_none());
        let moved = st.refresh(1, 2).unwrap().clone();
        assert_eq!(moved.pixels[0], 0xFFFF);
        st.show_count = -1;
        assert_eq!(st.refresh(1, 2).unwrap().pixels, fb.pixels);
        assert!(st.refresh(1, 2).is_none());
    }

    #[test]
    fn builtin_arrow_is_well_formed() {
        let a = builtin_arrow();
        assert_eq!(a.pixels.len(), (a.width * a.height) as usize);
        assert_eq!(a.pixels[0], CursorPixel::Color(0));
    }
}
