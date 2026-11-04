//! Pixel operations on surfaces in guest memory: colour fill, blit (clipping, nearest-neighbour stretching,
//! mirroring, source/destination colour keys), conversion of a surface to an RGB565 [`FrameBuffer`].
//! Independent of COM so it can be unit-tested directly.

use s3_mem::GuestMemory;
use s3_runtime::FrameBuffer;

/// Pixel memory of a surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Surf {
    /// Guest address of pixel (0,0).
    pub bits: u32,
    pub width: u32,
    pub height: u32,
    /// Bytes per row.
    pub pitch: u32,
    /// 8, 16, 24 or 32.
    pub bpp: u32,
}

impl Surf {
    #[inline]
    pub fn bypp(&self) -> u32 {
        (self.bpp / 8).max(1)
    }
    pub fn full(&self) -> Rect {
        Rect { l: 0, t: 0, r: self.width as i32, b: self.height as i32 }
    }
    #[inline]
    fn row(&self, y: i32) -> u32 {
        self.bits.wrapping_add(y as u32 * self.pitch)
    }
}

/// Win32 RECT (right/bottom exclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub l: i32,
    pub t: i32,
    pub r: i32,
    pub b: i32,
}

impl Rect {
    pub fn new(l: i32, t: i32, r: i32, b: i32) -> Rect {
        Rect { l, t, r, b }
    }
    /// Read a RECT from guest memory.
    pub fn read(mem: &GuestMemory, p: u32) -> Rect {
        Rect {
            l: mem.read_u32(p) as i32,
            t: mem.read_u32(p + 4) as i32,
            r: mem.read_u32(p + 8) as i32,
            b: mem.read_u32(p + 12) as i32,
        }
    }
    pub fn w(&self) -> i32 {
        self.r - self.l
    }
    pub fn h(&self) -> i32 {
        self.b - self.t
    }
    pub fn is_empty(&self) -> bool {
        self.r <= self.l || self.b <= self.t
    }
    pub fn intersect(&self, o: &Rect) -> Rect {
        Rect { l: self.l.max(o.l), t: self.t.max(o.t), r: self.r.min(o.r), b: self.b.min(o.b) }
    }
}

/// Colour key range (inclusive). For a single colour `low == high`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ColorKey {
    pub low: u32,
    pub high: u32,
}

impl ColorKey {
    #[inline]
    pub fn matches(&self, p: u32) -> bool {
        p >= self.low && p <= self.high
    }
}

/// Options for [`blt`].
#[derive(Debug, Clone, Copy, Default)]
pub struct BltOpts {
    /// Skip source pixels in this range (DDBLT_KEYSRC / DDBLTFAST_SRCCOLORKEY).
    pub src_key: Option<ColorKey>,
    /// Only write where the destination is in this range (DDBLT_KEYDEST).
    pub dst_key: Option<ColorKey>,
    pub mirror_lr: bool,
    pub mirror_ud: bool,
}

#[inline]
fn get_px(row: &[u8], x: usize, bypp: usize) -> u32 {
    match bypp {
        1 => row[x] as u32,
        2 => u16::from_le_bytes([row[2 * x], row[2 * x + 1]]) as u32,
        3 => row[3 * x] as u32 | (row[3 * x + 1] as u32) << 8 | (row[3 * x + 2] as u32) << 16,
        _ => u32::from_le_bytes([row[4 * x], row[4 * x + 1], row[4 * x + 2], row[4 * x + 3]]),
    }
}

#[inline]
fn put_px(row: &mut [u8], x: usize, bypp: usize, v: u32) {
    match bypp {
        1 => row[x] = v as u8,
        2 => row[2 * x..2 * x + 2].copy_from_slice(&(v as u16).to_le_bytes()),
        3 => row[3 * x..3 * x + 3].copy_from_slice(&v.to_le_bytes()[..3]),
        _ => row[4 * x..4 * x + 4].copy_from_slice(&v.to_le_bytes()),
    }
}

/// Fill `rect` (clipped to the surface) with `color` (raw pixel value).
pub fn fill(mem: &mut GuestMemory, dst: &Surf, rect: Rect, color: u32) {
    let r = rect.intersect(&dst.full());
    if r.is_empty() {
        return;
    }
    let bypp = dst.bypp() as usize;
    let w = r.w() as usize;
    // Build one row once, then copy it.
    let mut line = vec![0u8; w * bypp];
    for x in 0..w {
        put_px(&mut line, x, bypp, color);
    }
    for y in r.t..r.b {
        let a = dst.row(y) + r.l as u32 * bypp as u32;
        mem.write_bytes(a, &line);
    }
}

/// Blit `srect` of `src` to `drect` of `dst` (same pixel depth expected). Stretches with nearest neighbour when
/// the sizes differ. The destination is clipped to the surface; source pixels outside the source surface are
/// skipped. Overlapping blits within one surface behave like memmove.
pub fn blt(mem: &mut GuestMemory, dst: &Surf, drect: Rect, src: &Surf, srect: Rect, opts: &BltOpts) {
    if drect.is_empty() || srect.is_empty() {
        return;
    }
    let bypp = dst.bypp() as usize;
    let sbypp = src.bypp() as usize;
    let clip = drect.intersect(&dst.full());
    if clip.is_empty() {
        return;
    }
    let (dw, dh, sw, sh) = (drect.w() as i64, drect.h() as i64, srect.w() as i64, srect.h() as i64);
    let simple = dw == sw && dh == sh && opts.src_key.is_none() && opts.dst_key.is_none() && !opts.mirror_lr
        && !opts.mirror_ud && bypp == sbypp;
    if simple {
        // Clip against both surfaces with a 1:1 mapping.
        let (ox, oy) = (srect.l - drect.l, srect.t - drect.t);
        let sclip = srect.intersect(&src.full());
        let c = clip.intersect(&Rect::new(sclip.l - ox, sclip.t - oy, sclip.r - ox, sclip.b - oy));
        if c.is_empty() {
            return;
        }
        let len = c.w() as usize * bypp;
        let same = src.bits == dst.bits;
        let rows: Box<dyn Iterator<Item = i32>> =
            if same && oy < 0 { Box::new((c.t..c.b).rev()) } else { Box::new(c.t..c.b) };
        for y in rows {
            let s = src.row(y + oy) + (c.l + ox) as u32 * bypp as u32;
            let d = dst.row(y) + c.l as u32 * bypp as u32;
            mem.copy_within(s, d, len);
        }
        return;
    }
    // General path: snapshot the needed source rows (handles overlap), map each destination pixel.
    let sfull = src.full();
    let src_rows: Vec<Option<Vec<u8>>> = (srect.t..srect.b)
        .map(|sy| {
            if sy < 0 || sy >= sfull.b {
                None
            } else {
                Some(mem.read_bytes(src.row(sy), (src.width as usize) * sbypp))
            }
        })
        .collect();
    let mut drow = vec![0u8; clip.w() as usize * bypp];
    for dy in clip.t..clip.b {
        let mut ry = ((dy - drect.t) as i64 * sh / dh) as i32;
        if opts.mirror_ud {
            ry = sh as i32 - 1 - ry;
        }
        let Some(Some(srow)) = src_rows.get(ry as usize) else { continue };
        let daddr = dst.row(dy) + clip.l as u32 * bypp as u32;
        let dl = drow.len();
        drow.copy_from_slice(mem.slice(daddr, dl));
        let mut changed = false;
        for dx in clip.l..clip.r {
            let mut rx = ((dx - drect.l) as i64 * sw / dw) as i32;
            if opts.mirror_lr {
                rx = sw as i32 - 1 - rx;
            }
            let sx = srect.l + rx;
            if sx < 0 || sx >= src.width as i32 {
                continue;
            }
            let p = get_px(srow, sx as usize, sbypp);
            if let Some(k) = opts.src_key {
                if k.matches(p) {
                    continue;
                }
            }
            let xi = (dx - clip.l) as usize;
            if let Some(k) = opts.dst_key {
                if !k.matches(get_px(&drow, xi, bypp)) {
                    continue;
                }
            }
            put_px(&mut drow, xi, bypp, p);
            changed = true;
        }
        if changed {
            mem.write_bytes(daddr, &drow);
        }
    }
}

/// Convert an 8-bit palette entry (`0x00BBGGRR` like PALETTEENTRY peRed/peGreen/peBlue) to RGB565.
#[inline]
pub fn pe_to_565(pe: u32) -> u16 {
    let r = pe & 0xFF;
    let g = (pe >> 8) & 0xFF;
    let b = (pe >> 16) & 0xFF;
    ((r >> 3) << 11 | (g >> 2) << 5 | (b >> 3)) as u16
}

/// Copy a surface into an RGB565 frame buffer. `rgb555` converts 16-bit X1R5G5B5 data; `palette` is used for
/// 8-bit surfaces (PALETTEENTRY dwords; grey ramp if `None`).
pub fn to_framebuffer(mem: &GuestMemory, s: &Surf, rgb555: bool, palette: Option<&[u32]>) -> FrameBuffer {
    let mut fb = FrameBuffer::new(s.width, s.height);
    to_framebuffer_into(mem, s, rgb555, palette, &mut fb);
    fb
}

/// Like [`to_framebuffer`] but reuses `fb` (resized if needed).
pub fn to_framebuffer_into(mem: &GuestMemory, s: &Surf, rgb555: bool, palette: Option<&[u32]>, fb: &mut FrameBuffer) {
    let (w, h) = (s.width as usize, s.height as usize);
    if fb.width != s.width || fb.height != s.height || fb.pixels.len() != w * h {
        *fb = FrameBuffer::new(s.width, s.height);
    }
    let bypp = s.bypp() as usize;
    for y in 0..h {
        let row = mem.slice(s.row(y as i32), w * bypp);
        let out = &mut fb.pixels[y * w..(y + 1) * w];
        match s.bpp {
            16 if !rgb555 => {
                for (x, o) in out.iter_mut().enumerate() {
                    *o = u16::from_le_bytes([row[2 * x], row[2 * x + 1]]);
                }
            }
            16 => {
                for (x, o) in out.iter_mut().enumerate() {
                    let p = u16::from_le_bytes([row[2 * x], row[2 * x + 1]]);
                    *o = ((p & 0x7FE0) << 1) | ((p >> 4) & 0x20) | (p & 0x1F);
                }
            }
            8 => {
                for (x, o) in out.iter_mut().enumerate() {
                    let i = row[x] as usize;
                    *o = match palette {
                        Some(p) => pe_to_565(p[i]),
                        None => pe_to_565((i as u32) * 0x010101),
                    };
                }
            }
            24 | 32 => {
                for (x, o) in out.iter_mut().enumerate() {
                    let (b, g, r) = (row[bypp * x] as u16, row[bypp * x + 1] as u16, row[bypp * x + 2] as u16);
                    *o = (r >> 3) << 11 | (g >> 2) << 5 | (b >> 3);
                }
            }
            _ => {}
        }
    }
}
