//! `gdi32.dll` HLE. Device contexts, fonts, regions (rectangles), pens/brushes, text and simple drawing into
//! guest pixel memory: DirectDraw surface DCs (via `s3_runtime::gdi::create_dc`, installed here), DIB sections and
//! compatible bitmaps. Window DCs (`user32!GetDC`) have no pixels; drawing on them is a no-op.
//!
//! Text uses `font.rs` (fontdue + bundled Liberation Sans). Coverage is alpha-blended into 16-bit targets.
//! GDI objects use their own handle space (`0x00C0_0000 + 4n`, stock objects `0x00BF_00xx`).

use std::collections::HashMap;

use s3_runtime::gdi::{DcHooks, DcTarget};
use s3_runtime::{LogLevel, Machine, Registry, ThunkResult};

use crate::font::{self, FontCache, FontSpec, Metrics};

/// DLL name as registered.
pub const DLL: &str = "gdi32.dll";

/// Known exports (static imports of S3.EXE first).
pub const NAMES: &[&str] = &[
    "LineTo", "SelectObject", "GetTextMetricsA", "GetTextExtentPoint32A", "GetStockObject",
    "CreateFontIndirectA", "GetDeviceCaps", "CreateRectRgn", "SelectClipRgn", "CreatePen",
    "SetBkMode", "SetTextColor", "DeleteObject", "CreateDIBSection", "DeleteDC", "TextOutA",
    "CreateCompatibleDC", "MoveToEx", "GetTextExtentExPointA", "SetBkColor", "CreateFontA",
    "GdiFlush", "GetCurrentObject", "SetTextAlign", "ExtTextOutA", "CreateSolidBrush", "Rectangle",
    "FillRect", "GetObjectA", "BitBlt", "StretchBlt", "SetPixel", "GetPixel", "RealizePalette",
    "SelectPalette", "CreatePalette", "GetSystemPaletteEntries", "SetDIBitsToDevice",
    "StretchDIBits", "CreateCompatibleBitmap", "Polyline", "Ellipse", "PatBlt",
    "GetTextColor", "GetBkColor", "GetBkMode", "GetTextAlign", "CreateRectRgnIndirect",
    "GetTextExtentPointA", "GetTextFaceA", "SaveDC", "RestoreDC", "GetClipBox", "IntersectClipRect",
    "SetROP2", "SetMapMode", "SetStretchBltMode", "CreateBrushIndirect", "SetDCBrushColor",
];

/// Declare all names and register implemented handlers.
pub fn register(reg: &mut Registry) {
    reg.declare_all(DLL, NAMES);
    s3_runtime::gdi::install(DcHooks { create: create_surface_dc, release: release_surface_dc });
    let h: &[(&str, s3_runtime::Handler)] = &[
        ("GetStockObject", get_stock_object),
        ("CreateFontIndirectA", create_font_indirect_a),
        ("CreateFontA", create_font_a),
        ("CreateRectRgn", create_rect_rgn),
        ("CreateRectRgnIndirect", create_rect_rgn_indirect),
        ("SelectClipRgn", select_clip_rgn),
        ("IntersectClipRect", intersect_clip_rect),
        ("GetClipBox", get_clip_box),
        ("CreatePen", create_pen),
        ("CreateSolidBrush", create_solid_brush),
        ("CreateBrushIndirect", create_brush_indirect),
        ("CreatePalette", create_palette),
        ("SelectObject", select_object),
        ("DeleteObject", delete_object),
        ("GetCurrentObject", get_current_object),
        ("GetObjectA", get_object_a),
        ("SetBkMode", set_bk_mode),
        ("GetBkMode", get_bk_mode),
        ("SetTextColor", set_text_color),
        ("GetTextColor", get_text_color),
        ("SetBkColor", set_bk_color),
        ("GetBkColor", get_bk_color),
        ("SetTextAlign", set_text_align),
        ("GetTextAlign", get_text_align),
        ("SetROP2", set_rop2),
        ("SetMapMode", set_map_mode),
        ("SetStretchBltMode", set_stretch_blt_mode),
        ("SetDCBrushColor", set_dc_brush_color),
        ("GetDeviceCaps", get_device_caps),
        ("GetTextMetricsA", get_text_metrics_a),
        ("GetTextExtentPoint32A", get_text_extent_point32_a),
        ("GetTextExtentPointA", get_text_extent_point32_a),
        ("GetTextExtentExPointA", get_text_extent_ex_point_a),
        ("GetTextFaceA", get_text_face_a),
        ("TextOutA", text_out_a),
        ("ExtTextOutA", ext_text_out_a),
        ("MoveToEx", move_to_ex),
        ("LineTo", line_to),
        ("Polyline", polyline),
        ("Rectangle", rectangle),
        ("Ellipse", ellipse),
        ("FillRect", fill_rect),
        ("PatBlt", pat_blt),
        ("SetPixel", set_pixel),
        ("GetPixel", get_pixel),
        ("CreateCompatibleDC", create_compatible_dc),
        ("DeleteDC", delete_dc),
        ("CreateDIBSection", create_dib_section),
        ("CreateCompatibleBitmap", create_compatible_bitmap),
        ("BitBlt", bit_blt),
        ("StretchBlt", stretch_blt),
        ("GdiFlush", gdi_flush),
        ("SelectPalette", select_palette),
        ("RealizePalette", realize_palette),
        ("GetSystemPaletteEntries", get_system_palette_entries),
        ("SaveDC", save_dc),
        ("RestoreDC", restore_dc),
    ];
    for (n, f) in h {
        reg.register(DLL, n, *f);
    }
}

// ---------------------------------------------------------------------------------------------- data model

/// Integer rectangle (left, top, right, bottom), right/bottom exclusive.
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
    pub fn read(m: &Machine, p: u32) -> Rect {
        Rect {
            l: m.mem.read_u32(p) as i32,
            t: m.mem.read_u32(p + 4) as i32,
            r: m.mem.read_u32(p + 8) as i32,
            b: m.mem.read_u32(p + 12) as i32,
        }
    }
    pub fn write(&self, m: &mut Machine, p: u32) {
        m.mem.write_u32(p, self.l as u32);
        m.mem.write_u32(p + 4, self.t as u32);
        m.mem.write_u32(p + 8, self.r as u32);
        m.mem.write_u32(p + 12, self.b as u32);
    }
    pub fn is_empty(&self) -> bool {
        self.r <= self.l || self.b <= self.t
    }
    pub fn intersect(&self, o: &Rect) -> Rect {
        let r = Rect { l: self.l.max(o.l), t: self.t.max(o.t), r: self.r.min(o.r), b: self.b.min(o.b) };
        if r.is_empty() {
            Rect::default()
        } else {
            r
        }
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.l && x < self.r && y >= self.t && y < self.b
    }
}

/// Pixel memory (top row address, signed pitch for bottom-up DIBs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pixels {
    pub addr: u32,
    pub pitch: i32,
    pub width: i32,
    pub height: i32,
    pub bpp: u32,
    pub rgb565: bool,
}

impl Pixels {
    fn from_target(t: &DcTarget) -> Pixels {
        Pixels { addr: t.addr, pitch: t.pitch as i32, width: t.width as i32, height: t.height as i32, bpp: t.bpp, rgb565: t.rgb565 }
    }
    fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }
    fn at(&self, x: i32, y: i32) -> u32 {
        (self.addr as i64 + y as i64 * self.pitch as i64 + x as i64 * (self.bpp as i64 / 8)) as u32
    }
    /// Encode a COLORREF (0x00BBGGRR).
    pub fn encode(&self, c: u32) -> u32 {
        let (r, g, b) = (c & 0xFF, (c >> 8) & 0xFF, (c >> 16) & 0xFF);
        match self.bpp {
            16 if self.rgb565 => ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3),
            16 => ((r >> 3) << 10) | ((g >> 3) << 5) | (b >> 3),
            24 | 32 => (r << 16) | (g << 8) | b,
            _ => (r * 30 + g * 59 + b * 11) / 100, // 8-bit: grey index (palettes unused by S3)
        }
    }
    /// Decode a pixel to COLORREF.
    pub fn decode(&self, p: u32) -> u32 {
        let (r, g, b) = match self.bpp {
            16 if self.rgb565 => {
                let (r, g, b) = ((p >> 11) & 0x1F, (p >> 5) & 0x3F, p & 0x1F);
                ((r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2))
            }
            16 => {
                let (r, g, b) = ((p >> 10) & 0x1F, (p >> 5) & 0x1F, p & 0x1F);
                ((r << 3) | (r >> 2), (g << 3) | (g >> 2), (b << 3) | (b >> 2))
            }
            24 | 32 => ((p >> 16) & 0xFF, (p >> 8) & 0xFF, p & 0xFF),
            _ => (p & 0xFF, p & 0xFF, p & 0xFF),
        };
        r | (g << 8) | (b << 16)
    }
    fn read_raw(&self, m: &Machine, x: i32, y: i32) -> u32 {
        let a = self.at(x, y);
        match self.bpp {
            16 => m.mem.read_u16(a) as u32,
            24 => m.mem.read_u16(a) as u32 | (m.mem.read_u8(a + 2) as u32) << 16,
            32 => m.mem.read_u32(a),
            _ => m.mem.read_u8(a) as u32,
        }
    }
    fn write_raw(&self, m: &mut Machine, x: i32, y: i32, v: u32) {
        let a = self.at(x, y);
        match self.bpp {
            16 => m.mem.write_u16(a, v as u16),
            24 => {
                m.mem.write_u16(a, v as u16);
                m.mem.write_u8(a + 2, (v >> 16) as u8);
            }
            32 => m.mem.write_u32(a, v),
            _ => m.mem.write_u8(a, v as u8),
        }
    }
    /// Fill a rectangle (already clipped) with a COLORREF.
    fn fill(&self, m: &mut Machine, r: &Rect, color: u32) {
        let v = self.encode(color);
        for y in r.t..r.b {
            if self.bpp == 16 {
                let a = self.at(r.l, y);
                let n = (r.r - r.l) as usize;
                let s = m.mem.slice_mut(a, n * 2);
                for px in s.chunks_exact_mut(2) {
                    px.copy_from_slice(&(v as u16).to_le_bytes());
                }
            } else {
                for x in r.l..r.r {
                    self.write_raw(m, x, y, v);
                }
            }
        }
    }
    /// Blend `color` with coverage `a` (0..255) into pixel (x, y).
    fn blend(&self, m: &mut Machine, x: i32, y: i32, color: u32, a: u8) {
        if a >= 250 || self.bpp == 8 {
            if a >= 128 {
                let v = self.encode(color);
                self.write_raw(m, x, y, v);
            }
            return;
        }
        let old = self.decode(self.read_raw(m, x, y));
        let a = a as u32;
        let mix = |s: u32, d: u32| (s * a + d * (255 - a)) / 255;
        let c = mix(color & 0xFF, old & 0xFF) | mix((color >> 8) & 0xFF, (old >> 8) & 0xFF) << 8 | mix((color >> 16) & 0xFF, (old >> 16) & 0xFF) << 16;
        let v = self.encode(c);
        self.write_raw(m, x, y, v);
    }
}

#[derive(Debug, Clone)]
pub enum GdiObj {
    Font(FontSpec),
    Brush { color: u32, null: bool },
    Pen { style: u32, width: u32, color: u32 },
    Region(Rect),
    Bitmap { pixels: Pixels, dib: bool, bits_owned: bool },
    Palette,
    Dc(Box<Dc>),
}

/// A device context.
#[derive(Debug, Clone)]
pub struct Dc {
    /// Drawing target (None: window/screen DC without pixels).
    pub pixels: Option<Pixels>,
    /// True for DCs created by `create_surface_dc` (released by ddraw).
    pub surface: bool,
    /// Memory DC (CreateCompatibleDC): target follows the selected bitmap.
    pub memory: bool,
    pub font: u32,
    pub brush: u32,
    pub pen: u32,
    pub bitmap: u32,
    pub palette: u32,
    pub text_color: u32,
    pub bk_color: u32,
    pub bk_mode: u32,
    pub text_align: u32,
    pub rop2: u32,
    pub clip: Option<Rect>,
    pub pos: (i32, i32),
    pub saved: Vec<Box<Dc>>,
}

impl Dc {
    fn new(pixels: Option<Pixels>) -> Dc {
        Dc {
            pixels,
            surface: false,
            memory: false,
            font: STOCK_BASE + SYSTEM_FONT,
            brush: STOCK_BASE + WHITE_BRUSH,
            pen: STOCK_BASE + BLACK_PEN,
            bitmap: STOCK_BASE + DEFAULT_BITMAP,
            palette: STOCK_BASE + DEFAULT_PALETTE,
            text_color: 0,
            bk_color: 0xFF_FFFF,
            bk_mode: OPAQUE,
            text_align: 0,
            rop2: 13,
            clip: None,
            pos: (0, 0),
            saved: Vec::new(),
        }
    }
    /// Effective clip rectangle (target bounds ∩ clip region).
    fn clip_rect(&self) -> Option<Rect> {
        let p = self.pixels?;
        let b = p.bounds();
        Some(match self.clip {
            Some(c) => b.intersect(&c),
            None => b,
        })
    }
}

const STOCK_BASE: u32 = 0x00BF_0000;
const WHITE_BRUSH: u32 = 0;
const NULL_BRUSH: u32 = 5;
const WHITE_PEN: u32 = 6;
const BLACK_PEN: u32 = 7;
const NULL_PEN: u32 = 8;
const SYSTEM_FONT: u32 = 13;
const DEFAULT_PALETTE: u32 = 15;
/// Stock 1x1 monochrome bitmap selected into new DCs (Windows' DEFAULT_BITMAP); SelectObject must return it.
const DEFAULT_BITMAP: u32 = 21;
const OBJ_BASE: u32 = 0x00C0_0000;

const OPAQUE: u32 = 2;

/// gdi32 state.
pub struct GdiState {
    pub objects: HashMap<u32, GdiObj>,
    next: u32,
    pub fonts: FontCache,
    metrics: HashMap<u32, Metrics>,
}

impl Default for GdiState {
    fn default() -> Self {
        let mut objects = HashMap::new();
        for (i, c) in [(0u32, 0xFF_FFFFu32), (1, 0xC0C0C0), (2, 0x808080), (3, 0x404040), (4, 0)] {
            objects.insert(STOCK_BASE + i, GdiObj::Brush { color: c, null: false });
        }
        objects.insert(STOCK_BASE + NULL_BRUSH, GdiObj::Brush { color: 0, null: true });
        objects.insert(STOCK_BASE + WHITE_PEN, GdiObj::Pen { style: 0, width: 1, color: 0xFF_FFFF });
        objects.insert(STOCK_BASE + BLACK_PEN, GdiObj::Pen { style: 0, width: 1, color: 0 });
        objects.insert(STOCK_BASE + NULL_PEN, GdiObj::Pen { style: 5, width: 1, color: 0 });
        for i in [10u32, 11, 12, 13, 14, 16, 17] {
            let spec = FontSpec { height: if i == 13 { 16 } else { 13 }, weight: if i == 13 { 700 } else { 400 }, ..Default::default() };
            objects.insert(STOCK_BASE + i, GdiObj::Font(spec));
        }
        objects.insert(STOCK_BASE + DEFAULT_PALETTE, GdiObj::Palette);
        GdiState { objects, next: OBJ_BASE, fonts: FontCache::new(), metrics: HashMap::new() }
    }
}

impl GdiState {
    pub fn add(&mut self, o: GdiObj) -> u32 {
        self.next += 4;
        self.objects.insert(self.next, o);
        self.next
    }
    pub fn dc(&mut self, h: u32) -> Option<&mut Dc> {
        match self.objects.get_mut(&h) {
            Some(GdiObj::Dc(d)) => Some(d),
            _ => None,
        }
    }
    fn font_of(&self, dc: &Dc) -> FontSpec {
        match self.objects.get(&dc.font) {
            Some(GdiObj::Font(f)) => f.clone(),
            _ => FontSpec::default(),
        }
    }
    /// Metrics of the DC's current font (cached per font handle).
    pub fn dc_metrics(&mut self, hdc: u32) -> Option<Metrics> {
        let dc = self.dc(hdc)?.clone();
        if let Some(m) = self.metrics.get(&dc.font) {
            return Some(*m);
        }
        let mt = font::metrics(&self.font_of(&dc));
        self.metrics.insert(dc.font, mt);
        Some(mt)
    }
}

fn gdi(m: &mut Machine) -> s3_runtime::Shared<GdiState> {
    m.state::<GdiState>()
}

/// Create a DC drawing into raw pixels (used by user32/tests).
pub fn create_dc_for(m: &mut Machine, pixels: Option<Pixels>) -> u32 {
    gdi(m).borrow_mut().add(GdiObj::Dc(Box::new(Dc::new(pixels))))
}

fn create_surface_dc(m: &mut Machine, t: DcTarget) -> u32 {
    let mut dc = Dc::new(Some(Pixels::from_target(&t)));
    dc.surface = true;
    gdi(m).borrow_mut().add(GdiObj::Dc(Box::new(dc)))
}

fn release_surface_dc(m: &mut Machine, hdc: u32) {
    gdi(m).borrow_mut().objects.remove(&hdc);
}

/// Delete a DC (user32 ReleaseDC).
pub fn delete_dc_handle(m: &mut Machine, hdc: u32) {
    let st = gdi(m);
    let mut g = st.borrow_mut();
    if matches!(g.objects.get(&hdc), Some(GdiObj::Dc(_))) {
        g.objects.remove(&hdc);
    }
}

// ---------------------------------------------------------------------------------------------- text helpers

/// Draw text into a DC at (x, y) honouring text align, bk mode and clip. `opaque_rect` (ETO_OPAQUE) is filled
/// with the bk colour first. Returns the advance width.
pub fn draw_text_at(m: &mut Machine, hdc: u32, x: i32, y: i32, text: &[u8], opaque_rect: Option<Rect>, extra_clip: Option<Rect>) -> i32 {
    let st = gdi(m);
    let mut g = st.borrow_mut();
    let Some(mt) = g.dc_metrics(hdc) else { return 0 };
    let dc = g.dc(hdc).unwrap().clone();
    let width = g.fonts.text_width(&mt, text);
    let mut x0 = x;
    let mut y0 = y;
    let (cx, cy) = if dc.text_align & 1 != 0 { dc.pos } else { (x, y) };
    if dc.text_align & 1 != 0 {
        x0 = cx;
        y0 = cy;
    }
    match dc.text_align & 6 {
        2 => x0 -= width,     // TA_RIGHT
        6 => x0 -= width / 2, // TA_CENTER
        _ => {}
    }
    match dc.text_align & 24 {
        8 => y0 -= mt.height,  // TA_BOTTOM
        24 => y0 -= mt.ascent, // TA_BASELINE
        _ => {}
    }
    if dc.text_align & 1 != 0 {
        if let Some(d) = g.dc(hdc) {
            d.pos.0 += width;
        }
    }
    let (Some(px), Some(mut clip)) = (dc.pixels, dc.clip_rect()) else { return width };
    if let Some(e) = extra_clip {
        clip = clip.intersect(&e);
    }
    if let Some(o) = opaque_rect {
        let r = o.intersect(&clip);
        if !r.is_empty() {
            px.fill(m, &r, dc.bk_color);
        }
    } else if dc.bk_mode == OPAQUE {
        let r = Rect::new(x0, y0, x0 + width, y0 + mt.height).intersect(&clip);
        if !r.is_empty() {
            px.fill(m, &r, dc.bk_color);
        }
    }
    let color = dc.text_color & 0xFF_FFFF;
    let mut plots: Vec<(i32, i32, u8)> = Vec::with_capacity(text.len() * 40);
    g.fonts.draw(&mt, text, x0, y0, |x, y, a| {
        if clip.contains(x, y) {
            plots.push((x, y, a));
        }
    });
    let spec_underline = matches!(g.objects.get(&dc.font), Some(GdiObj::Font(f)) if f.underline);
    drop(g);
    for (x, y, a) in plots {
        px.blend(m, x, y, color, a);
    }
    if spec_underline {
        let uy = y0 + mt.ascent + 1;
        let r = Rect::new(x0, uy, x0 + width, uy + 1).intersect(&clip);
        if !r.is_empty() {
            px.fill(m, &r, color);
        }
    }
    width
}

/// Text extent of `text` in the DC's font: (width, height).
pub fn text_extent(m: &mut Machine, hdc: u32, text: &[u8]) -> Option<(i32, i32)> {
    let st = gdi(m);
    let mut g = st.borrow_mut();
    let mt = g.dc_metrics(hdc)?;
    let w = g.fonts.text_width(&mt, text);
    Some((w, mt.height))
}

/// Per-character advances in the DC's font.
pub fn char_advances(m: &mut Machine, hdc: u32, text: &[u8]) -> Option<(Vec<i32>, i32)> {
    let st = gdi(m);
    let mut g = st.borrow_mut();
    let mt = g.dc_metrics(hdc)?;
    let v = text.iter().map(|&b| g.fonts.advance(&mt, font::cp1252(b))).collect();
    Some((v, mt.height))
}

/// Fill a rectangle on a DC with a brush handle (user32 FillRect).
pub fn fill_rect_brush(m: &mut Machine, hdc: u32, r: Rect, brush: u32) -> bool {
    let st = gdi(m);
    let mut g = st.borrow_mut();
    let color = if brush != 0 && brush <= 31 {
        // (COLOR_x + 1): system colours.
        sys_color(brush - 1)
    } else {
        match g.objects.get(&brush) {
            Some(GdiObj::Brush { null: true, .. }) => return true,
            Some(GdiObj::Brush { color, .. }) => *color,
            _ => return false,
        }
    };
    let Some(dc) = g.dc(hdc).cloned() else { return false };
    drop(g);
    if let (Some(px), Some(clip)) = (dc.pixels, dc.clip_rect()) {
        let r = r.intersect(&clip);
        if !r.is_empty() {
            px.fill(m, &r, color);
        }
    }
    true
}

/// GetSysColor values (classic Windows scheme).
pub fn sys_color(i: u32) -> u32 {
    match i {
        1 => 0x808000,  // COLOR_BACKGROUND
        5 => 0xFF_FFFF, // COLOR_WINDOW
        6 => 0,         // COLOR_WINDOWFRAME
        8 => 0,         // COLOR_WINDOWTEXT
        15 => 0xC0C0C0, // COLOR_BTNFACE
        16 => 0x808080, // COLOR_BTNSHADOW
        18 => 0,        // COLOR_BTNTEXT
        _ => 0xC0C0C0,
    }
}

// ---------------------------------------------------------------------------------------------- handlers

fn get_stock_object(_m: &mut Machine) -> ThunkResult {
    let i = _m.arg_u32(0);
    let h = if i <= 17 && i != 9 { STOCK_BASE + i } else { 0 };
    ThunkResult::stdcall(h, 1)
}

fn read_logfont(m: &Machine, p: u32) -> FontSpec {
    let face_bytes = m.mem.read_bytes(p + 28, 32);
    let end = face_bytes.iter().position(|&b| b == 0).unwrap_or(32);
    FontSpec {
        height: m.mem.read_u32(p) as i32,
        width: m.mem.read_u32(p + 4) as i32,
        weight: m.mem.read_u32(p + 16) as i32,
        italic: m.mem.read_u8(p + 20) != 0,
        underline: m.mem.read_u8(p + 21) != 0,
        strikeout: m.mem.read_u8(p + 22) != 0,
        charset: m.mem.read_u8(p + 23),
        pitch_family: m.mem.read_u8(p + 27),
        face: String::from_utf8_lossy(&face_bytes[..end]).into_owned(),
    }
}

fn write_logfont(m: &mut Machine, p: u32, f: &FontSpec) {
    m.mem.fill(p, 60, 0);
    m.mem.write_u32(p, f.height as u32);
    m.mem.write_u32(p + 4, f.width as u32);
    m.mem.write_u32(p + 16, f.weight as u32);
    m.mem.write_u8(p + 20, f.italic as u8);
    m.mem.write_u8(p + 21, f.underline as u8);
    m.mem.write_u8(p + 22, f.strikeout as u8);
    m.mem.write_u8(p + 23, f.charset);
    m.mem.write_u8(p + 27, f.pitch_family);
    let face: Vec<u8> = f.face.bytes().take(31).collect();
    m.mem.write_bytes(p + 28, &face);
}

fn create_font_indirect_a(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    if p == 0 {
        return ThunkResult::stdcall(0, 1);
    }
    let spec = read_logfont(m, p);
    m.log(LogLevel::Debug, &format!("CreateFontIndirectA {spec:?}"));
    let h = gdi(m).borrow_mut().add(GdiObj::Font(spec));
    ThunkResult::stdcall(h, 1)
}

/// CreateFontA(h, w, esc, orient, weight, italic, underline, strike, charset, outprec, clipprec, quality, pitch, face)
fn create_font_a(m: &mut Machine) -> ThunkResult {
    let spec = FontSpec {
        height: m.arg_i32(0),
        width: m.arg_i32(1),
        weight: m.arg_i32(4),
        italic: m.arg_u32(5) != 0,
        underline: m.arg_u32(6) != 0,
        strikeout: m.arg_u32(7) != 0,
        charset: m.arg_u32(8) as u8,
        pitch_family: m.arg_u32(12) as u8,
        face: m.arg_str(13).unwrap_or_default(),
    };
    let h = gdi(m).borrow_mut().add(GdiObj::Font(spec));
    ThunkResult::stdcall(h, 14)
}

fn create_rect_rgn(m: &mut Machine) -> ThunkResult {
    let r = Rect::new(m.arg_i32(0), m.arg_i32(1), m.arg_i32(2), m.arg_i32(3));
    let h = gdi(m).borrow_mut().add(GdiObj::Region(r));
    ThunkResult::stdcall(h, 4)
}

fn create_rect_rgn_indirect(m: &mut Machine) -> ThunkResult {
    let r = Rect::read(m, m.arg_u32(0));
    let h = gdi(m).borrow_mut().add(GdiObj::Region(r));
    ThunkResult::stdcall(h, 1)
}

const NULLREGION: u32 = 1;
const SIMPLEREGION: u32 = 2;

/// SelectClipRgn(hdc, hrgn): copies the region (NULL removes clipping).
fn select_clip_rgn(m: &mut Machine) -> ThunkResult {
    let (hdc, hrgn) = (m.arg_u32(0), m.arg_u32(1));
    let st = gdi(m);
    let mut g = st.borrow_mut();
    let rgn = match g.objects.get(&hrgn) {
        Some(GdiObj::Region(r)) => Some(*r),
        _ => None,
    };
    let Some(dc) = g.dc(hdc) else { return ThunkResult::stdcall(0, 2) };
    dc.clip = rgn;
    let r = match rgn {
        Some(r) if r.is_empty() => NULLREGION,
        _ => SIMPLEREGION,
    };
    ThunkResult::stdcall(r, 2)
}

fn intersect_clip_rect(m: &mut Machine) -> ThunkResult {
    let hdc = m.arg_u32(0);
    let r = Rect::new(m.arg_i32(1), m.arg_i32(2), m.arg_i32(3), m.arg_i32(4));
    let st = gdi(m);
    let mut g = st.borrow_mut();
    let Some(dc) = g.dc(hdc) else { return ThunkResult::stdcall(0, 5) };
    let base = dc.clip.or(dc.pixels.map(|p| p.bounds())).unwrap_or(r);
    let n = base.intersect(&r);
    dc.clip = Some(n);
    ThunkResult::stdcall(if n.is_empty() { NULLREGION } else { SIMPLEREGION }, 5)
}

fn get_clip_box(m: &mut Machine) -> ThunkResult {
    let (hdc, out) = (m.arg_u32(0), m.arg_u32(1));
    let r = {
        let st = gdi(m);
        let mut g = st.borrow_mut();
        g.dc(hdc).map(|d| d.clip_rect().or(d.clip).unwrap_or(Rect::new(0, 0, 800, 600)))
    };
    let Some(r) = r else { return ThunkResult::stdcall(0, 2) };
    r.write(m, out);
    ThunkResult::stdcall(if r.is_empty() { NULLREGION } else { SIMPLEREGION }, 2)
}

fn create_pen(m: &mut Machine) -> ThunkResult {
    let (style, width, color) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    let h = gdi(m).borrow_mut().add(GdiObj::Pen { style, width, color });
    ThunkResult::stdcall(h, 3)
}

fn create_solid_brush(m: &mut Machine) -> ThunkResult {
    let color = m.arg_u32(0);
    let h = gdi(m).borrow_mut().add(GdiObj::Brush { color, null: false });
    ThunkResult::stdcall(h, 1)
}

fn create_brush_indirect(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    let (style, color) = (m.mem.read_u32(p), m.mem.read_u32(p + 4));
    let h = gdi(m).borrow_mut().add(GdiObj::Brush { color, null: style == 1 });
    ThunkResult::stdcall(h, 1)
}

fn create_palette(m: &mut Machine) -> ThunkResult {
    let h = gdi(m).borrow_mut().add(GdiObj::Palette);
    ThunkResult::stdcall(h, 1)
}

fn select_object(m: &mut Machine) -> ThunkResult {
    let (hdc, h) = (m.arg_u32(0), m.arg_u32(1));
    let st = gdi(m);
    let mut g = st.borrow_mut();
    let obj = g.objects.get(&h).cloned();
    let Some(dc) = g.dc(hdc) else { return ThunkResult::stdcall(0, 2) };
    let prev = match obj {
        Some(GdiObj::Font(_)) => std::mem::replace(&mut dc.font, h),
        Some(GdiObj::Brush { .. }) => std::mem::replace(&mut dc.brush, h),
        Some(GdiObj::Pen { .. }) => std::mem::replace(&mut dc.pen, h),
        Some(GdiObj::Region(r)) => {
            dc.clip = Some(r);
            SIMPLEREGION
        }
        Some(GdiObj::Bitmap { pixels, .. }) => {
            if dc.memory {
                dc.pixels = Some(pixels);
                dc.clip = None;
            }
            std::mem::replace(&mut dc.bitmap, h)
        }
        Some(GdiObj::Palette) => std::mem::replace(&mut dc.palette, h),
        None if h == STOCK_BASE + DEFAULT_BITMAP => {
            if dc.memory {
                dc.pixels = None;
            }
            std::mem::replace(&mut dc.bitmap, h)
        }
        _ => 0,
    };
    ThunkResult::stdcall(prev, 2)
}

fn delete_object(m: &mut Machine) -> ThunkResult {
    let h = m.arg_u32(0);
    if h >= STOCK_BASE && h < OBJ_BASE {
        return ThunkResult::stdcall(1, 1);
    }
    let st = gdi(m);
    let mut g = st.borrow_mut();
    let ok = match g.objects.get(&h) {
        Some(GdiObj::Dc(_)) | None => false,
        Some(_) => {
            g.objects.remove(&h);
            g.metrics.remove(&h);
            true
        }
    };
    ThunkResult::stdcall(ok as u32, 1)
}

fn get_current_object(m: &mut Machine) -> ThunkResult {
    let (hdc, ty) = (m.arg_u32(0), m.arg_u32(1));
    let st = gdi(m);
    let mut g = st.borrow_mut();
    let Some(dc) = g.dc(hdc) else { return ThunkResult::stdcall(0, 2) };
    let h = match ty {
        1 => dc.pen,
        2 => dc.brush,
        5 => dc.palette,
        6 => dc.font,
        7 => dc.bitmap,
        _ => 0,
    };
    ThunkResult::stdcall(h, 2)
}

/// GetObjectA(h, cb, buf)
fn get_object_a(m: &mut Machine) -> ThunkResult {
    let (h, cb, buf) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    let obj = gdi(m).borrow_mut().objects.get(&h).cloned();
    let r = match obj {
        Some(GdiObj::Font(f)) => {
            if buf != 0 && cb >= 60 {
                write_logfont(m, buf, &f);
            }
            60
        }
        Some(GdiObj::Bitmap { pixels: p, dib, .. }) => {
            let size = if dib && cb >= 84 { 84 } else { 24 };
            if buf != 0 && cb >= 24 {
                m.mem.fill(buf, size as usize, 0);
                m.mem.write_u32(buf + 4, p.width as u32);
                m.mem.write_u32(buf + 8, p.height as u32);
                m.mem.write_u32(buf + 12, p.pitch.unsigned_abs());
                m.mem.write_u16(buf + 16, 1);
                m.mem.write_u16(buf + 18, p.bpp as u16);
                m.mem.write_u32(buf + 20, if dib { p.addr.min(if p.pitch < 0 { (p.addr as i64 + (p.height as i64 - 1) * p.pitch as i64) as u32 } else { p.addr }) } else { 0 });
                if size == 84 {
                    // BITMAPINFOHEADER dsBmih at +24
                    m.mem.write_u32(buf + 24, 40);
                    m.mem.write_u32(buf + 28, p.width as u32);
                    m.mem.write_u32(buf + 32, if p.pitch < 0 { p.height } else { -p.height } as u32);
                    m.mem.write_u16(buf + 36, 1);
                    m.mem.write_u16(buf + 38, p.bpp as u16);
                }
            }
            size
        }
        Some(GdiObj::Pen { style, width, color }) => {
            if buf != 0 && cb >= 16 {
                m.mem.write_u32(buf, style);
                m.mem.write_u32(buf + 4, width);
                m.mem.write_u32(buf + 8, 0);
                m.mem.write_u32(buf + 12, color);
            }
            16
        }
        Some(GdiObj::Brush { color, null }) => {
            if buf != 0 && cb >= 12 {
                m.mem.write_u32(buf, null as u32);
                m.mem.write_u32(buf + 4, color);
                m.mem.write_u32(buf + 8, 0);
            }
            12
        }
        Some(GdiObj::Palette) => {
            if buf != 0 && cb >= 2 {
                m.mem.write_u16(buf, 0);
            }
            2
        }
        _ => 0,
    };
    ThunkResult::stdcall(r, 3)
}

macro_rules! dc_setter {
    ($name:ident, $field:ident, $mask:expr) => {
        fn $name(m: &mut Machine) -> ThunkResult {
            let (hdc, v) = (m.arg_u32(0), m.arg_u32(1));
            let st = gdi(m);
            let mut g = st.borrow_mut();
            let r = match g.dc(hdc) {
                Some(dc) => std::mem::replace(&mut dc.$field, v & $mask),
                None => 0,
            };
            ThunkResult::stdcall(r, 2)
        }
    };
}
macro_rules! dc_getter {
    ($name:ident, $field:ident) => {
        fn $name(m: &mut Machine) -> ThunkResult {
            let hdc = m.arg_u32(0);
            let st = gdi(m);
            let mut g = st.borrow_mut();
            let r = g.dc(hdc).map(|d| d.$field).unwrap_or(0);
            ThunkResult::stdcall(r, 1)
        }
    };
}
dc_setter!(set_bk_mode, bk_mode, 0xFFFF_FFFF);
dc_setter!(set_text_color, text_color, 0x03FF_FFFF);
dc_setter!(set_bk_color, bk_color, 0x03FF_FFFF);
dc_setter!(set_text_align, text_align, 0xFFFF_FFFF);
dc_setter!(set_rop2, rop2, 0xFFFF_FFFF);
dc_getter!(get_bk_mode, bk_mode);
dc_getter!(get_text_color, text_color);
dc_getter!(get_bk_color, bk_color);
dc_getter!(get_text_align, text_align);

fn set_map_mode(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(1, 2) // MM_TEXT
}

fn set_stretch_blt_mode(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(1, 2)
}

fn set_dc_brush_color(m: &mut Machine) -> ThunkResult {
    let c = m.arg_u32(1);
    ThunkResult::stdcall(c, 2)
}

/// GetDeviceCaps(hdc, index). Reports a 16-bit true colour display at 96 dpi.
fn get_device_caps(m: &mut Machine) -> ThunkResult {
    let (hdc, idx) = (m.arg_u32(0), m.arg_u32(1));
    let (sw, sh) = crate::user32::screen_size(m);
    let (w, h) = {
        let st = gdi(m);
        let mut g = st.borrow_mut();
        g.dc(hdc).and_then(|d| d.pixels).map(|p| (p.width as u32, p.height as u32)).unwrap_or((sw, sh))
    };
    let v = match idx {
        0 => 0x400,       // DRIVERVERSION
        2 => 1,           // TECHNOLOGY: DT_RASDISPLAY
        4 => w * 254 / 960, // HORZSIZE mm
        6 => h * 254 / 960, // VERTSIZE mm
        8 => w,           // HORZRES
        10 => h,          // VERTRES
        12 => 16,         // BITSPIXEL
        14 => 1,          // PLANES
        24 => u32::MAX,   // NUMCOLORS (-1: > 8 bpp)
        38 => 0x7E99,     // RASTERCAPS (no RC_PALETTE)
        88 | 90 => 96,    // LOGPIXELSX / LOGPIXELSY
        104 => 0,         // SIZEPALETTE
        108 => 16,        // COLORRES
        116 => 60,        // VREFRESH
        117 => sw,        // DESKTOPVERTRES (approx)
        118 => sh,
        _ => 0,
    };
    ThunkResult::stdcall(v, 2)
}

/// GetTextMetricsA(hdc, lptm): TEXTMETRICA (56 bytes).
fn get_text_metrics_a(m: &mut Machine) -> ThunkResult {
    let (hdc, p) = (m.arg_u32(0), m.arg_u32(1));
    let st = gdi(m);
    let (mt, spec) = {
        let mut g = st.borrow_mut();
        let Some(mt) = g.dc_metrics(hdc) else { return ThunkResult::stdcall(0, 2) };
        let dc = g.dc(hdc).unwrap().clone();
        (mt, g.font_of(&dc))
    };
    m.mem.fill(p, 56, 0);
    let vals = [
        mt.height,
        mt.ascent,
        mt.descent,
        mt.internal_leading,
        0,
        mt.ave_char_width,
        mt.max_char_width,
        if mt.bold { 700 } else { 400 },
        0,
        96,
        96,
    ];
    for (i, v) in vals.iter().enumerate() {
        m.mem.write_u32(p + 4 * i as u32, *v as u32);
    }
    m.mem.write_bytes(p + 44, &[0x20, 0xFF, 0x1F, 0x20, spec.italic as u8, spec.underline as u8, spec.strikeout as u8, 0x26, 0]);
    ThunkResult::stdcall(1, 2)
}

/// GetTextExtentPoint32A(hdc, str, len, lpSize)
fn get_text_extent_point32_a(m: &mut Machine) -> ThunkResult {
    let (hdc, s, n, out) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_u32(3));
    let text = m.mem.read_bytes(s, n.min(0x10000) as usize);
    let Some((w, h)) = text_extent(m, hdc, &text) else { return ThunkResult::stdcall(0, 4) };
    m.mem.write_u32(out, w as u32);
    m.mem.write_u32(out + 4, h as u32);
    ThunkResult::stdcall(1, 4)
}

/// GetTextExtentExPointA(hdc, str, len, nMaxExtent, lpnFit, alpDx, lpSize)
fn get_text_extent_ex_point_a(m: &mut Machine) -> ThunkResult {
    let (hdc, s, n, max) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_i32(3));
    let (lp_fit, lp_dx, lp_size) = (m.arg_u32(4), m.arg_u32(5), m.arg_u32(6));
    let text = m.mem.read_bytes(s, n.min(0x10000) as usize);
    let Some((adv, h)) = char_advances(m, hdc, &text) else { return ThunkResult::stdcall(0, 7) };
    let mut acc = 0;
    let mut fit = 0;
    for (i, a) in adv.iter().enumerate() {
        acc += a;
        if acc <= max {
            fit = i + 1;
        }
        if lp_dx != 0 {
            m.mem.write_u32(lp_dx + 4 * i as u32, acc as u32);
        }
    }
    if lp_fit != 0 {
        m.mem.write_u32(lp_fit, fit as u32);
    }
    if lp_size != 0 {
        m.mem.write_u32(lp_size, acc as u32);
        m.mem.write_u32(lp_size + 4, h as u32);
    }
    ThunkResult::stdcall(1, 7)
}

fn get_text_face_a(m: &mut Machine) -> ThunkResult {
    let (hdc, n, buf) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    let face = {
        let st = gdi(m);
        let mut g = st.borrow_mut();
        let Some(dc) = g.dc(hdc).cloned() else { return ThunkResult::stdcall(0, 3) };
        g.font_of(&dc).face
    };
    if buf != 0 && n > 0 {
        let b: Vec<u8> = face.bytes().take(n as usize - 1).collect();
        m.mem.write_cstr_bytes(buf, &b);
    }
    ThunkResult::stdcall(face.len() as u32 + 1, 3)
}

/// TextOutA(hdc, x, y, str, len)
fn text_out_a(m: &mut Machine) -> ThunkResult {
    let (hdc, x, y, s, n) = (m.arg_u32(0), m.arg_i32(1), m.arg_i32(2), m.arg_u32(3), m.arg_u32(4));
    let text = m.mem.read_bytes(s, n.min(0x10000) as usize);
    draw_text_at(m, hdc, x, y, &text, None, None);
    ThunkResult::stdcall(1, 5)
}

/// ExtTextOutA(hdc, x, y, options, lprect, str, len, lpDx)
fn ext_text_out_a(m: &mut Machine) -> ThunkResult {
    let (hdc, x, y, opt, prc, s, n) = (m.arg_u32(0), m.arg_i32(1), m.arg_i32(2), m.arg_u32(3), m.arg_u32(4), m.arg_u32(5), m.arg_u32(6));
    let rect = if prc != 0 { Some(Rect::read(m, prc)) } else { None };
    let text = if s != 0 { m.mem.read_bytes(s, n.min(0x10000) as usize) } else { Vec::new() };
    let opaque = if opt & 2 != 0 { rect } else { None };
    let clip = if opt & 4 != 0 { rect } else { None };
    if text.is_empty() {
        if let Some(r) = opaque {
            let st = gdi(m);
            let dc = st.borrow_mut().dc(hdc).cloned();
            if let Some(dc) = dc {
                if let (Some(px), Some(c)) = (dc.pixels, dc.clip_rect()) {
                    let r = r.intersect(&c);
                    if !r.is_empty() {
                        px.fill(m, &r, dc.bk_color);
                    }
                }
            }
        }
    } else {
        draw_text_at(m, hdc, x, y, &text, opaque, clip);
    }
    ThunkResult::stdcall(1, 8)
}

fn move_to_ex(m: &mut Machine) -> ThunkResult {
    let (hdc, x, y, prev) = (m.arg_u32(0), m.arg_i32(1), m.arg_i32(2), m.arg_u32(3));
    let old = {
        let st = gdi(m);
        let mut g = st.borrow_mut();
        let Some(dc) = g.dc(hdc) else { return ThunkResult::stdcall(0, 4) };
        std::mem::replace(&mut dc.pos, (x, y))
    };
    if prev != 0 {
        m.mem.write_u32(prev, old.0 as u32);
        m.mem.write_u32(prev + 4, old.1 as u32);
    }
    ThunkResult::stdcall(1, 4)
}

/// Draw a line with the DC's pen (endpoint excluded, like GDI).
fn draw_line(m: &mut Machine, hdc: u32, x1: i32, y1: i32) -> bool {
    let st = gdi(m);
    let mut g = st.borrow_mut();
    let Some(dc) = g.dc(hdc).cloned() else { return false };
    if let Some(d) = g.dc(hdc) {
        d.pos = (x1, y1);
    }
    let (style, color) = match g.objects.get(&dc.pen) {
        Some(GdiObj::Pen { style, color, .. }) => (*style, *color),
        _ => (0, 0),
    };
    drop(g);
    if style == 5 {
        return true; // PS_NULL
    }
    let (Some(px), Some(clip)) = (dc.pixels, dc.clip_rect()) else { return true };
    let v = px.encode(color);
    let (mut x, mut y) = dc.pos;
    let dx = (x1 - x).abs();
    let dy = -(y1 - y).abs();
    let sx = if x < x1 { 1 } else { -1 };
    let sy = if y < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    while (x, y) != (x1, y1) {
        if clip.contains(x, y) {
            px.write_raw(m, x, y, v);
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
    true
}

fn line_to(m: &mut Machine) -> ThunkResult {
    let (hdc, x, y) = (m.arg_u32(0), m.arg_i32(1), m.arg_i32(2));
    let ok = draw_line(m, hdc, x, y);
    ThunkResult::stdcall(ok as u32, 3)
}

fn polyline(m: &mut Machine) -> ThunkResult {
    let (hdc, pts, n) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    if n == 0 {
        return ThunkResult::stdcall(0, 3);
    }
    let saved = gdi(m).borrow_mut().dc(hdc).map(|d| d.pos);
    let Some(saved) = saved else { return ThunkResult::stdcall(0, 3) };
    let p0 = (m.mem.read_u32(pts) as i32, m.mem.read_u32(pts + 4) as i32);
    gdi(m).borrow_mut().dc(hdc).unwrap().pos = p0;
    for i in 1..n.min(10000) {
        let (x, y) = (m.mem.read_u32(pts + 8 * i) as i32, m.mem.read_u32(pts + 8 * i + 4) as i32);
        draw_line(m, hdc, x, y);
    }
    gdi(m).borrow_mut().dc(hdc).unwrap().pos = saved;
    ThunkResult::stdcall(1, 3)
}

/// Rectangle(hdc, l, t, r, b): border with the pen, interior with the brush.
fn rectangle(m: &mut Machine) -> ThunkResult {
    let hdc = m.arg_u32(0);
    let r = Rect::new(m.arg_i32(1), m.arg_i32(2), m.arg_i32(3), m.arg_i32(4));
    let brush = gdi(m).borrow_mut().dc(hdc).map(|d| d.brush);
    let Some(brush) = brush else { return ThunkResult::stdcall(0, 5) };
    fill_rect_brush(m, hdc, Rect::new(r.l + 1, r.t + 1, r.r - 1, r.b - 1), brush);
    let saved = gdi(m).borrow_mut().dc(hdc).unwrap().pos;
    gdi(m).borrow_mut().dc(hdc).unwrap().pos = (r.l, r.t);
    draw_line(m, hdc, r.r - 1, r.t);
    draw_line(m, hdc, r.r - 1, r.b - 1);
    draw_line(m, hdc, r.l, r.b - 1);
    draw_line(m, hdc, r.l, r.t);
    gdi(m).borrow_mut().dc(hdc).unwrap().pos = saved;
    ThunkResult::stdcall(1, 5)
}

/// Ellipse: approximated by its bounding rectangle fill (HYPOTHESIS: unused by S3).
fn ellipse(m: &mut Machine) -> ThunkResult {
    let hdc = m.arg_u32(0);
    let r = Rect::new(m.arg_i32(1), m.arg_i32(2), m.arg_i32(3), m.arg_i32(4));
    let brush = gdi(m).borrow_mut().dc(hdc).map(|d| d.brush).unwrap_or(0);
    fill_rect_brush(m, hdc, r, brush);
    ThunkResult::stdcall(1, 5)
}

/// FillRect(hdc, lprc, hbr) (really user32; also registered there).
pub fn fill_rect(m: &mut Machine) -> ThunkResult {
    let (hdc, prc, br) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    let r = Rect::read(m, prc);
    let ok = fill_rect_brush(m, hdc, r, br);
    ThunkResult::stdcall(ok as u32, 3)
}

/// PatBlt(hdc, x, y, w, h, rop): PATCOPY with the brush, BLACKNESS/WHITENESS.
fn pat_blt(m: &mut Machine) -> ThunkResult {
    let hdc = m.arg_u32(0);
    let (x, y, w, h, rop) = (m.arg_i32(1), m.arg_i32(2), m.arg_i32(3), m.arg_i32(4), m.arg_u32(5));
    let r = Rect::new(x, y, x + w, y + h);
    match rop {
        0x0000_0042 => {
            let b = gdi(m).borrow_mut().add(GdiObj::Brush { color: 0, null: false });
            fill_rect_brush(m, hdc, r, b);
            gdi(m).borrow_mut().objects.remove(&b);
        }
        0x00FF_0062 => {
            let b = gdi(m).borrow_mut().add(GdiObj::Brush { color: 0xFF_FFFF, null: false });
            fill_rect_brush(m, hdc, r, b);
            gdi(m).borrow_mut().objects.remove(&b);
        }
        _ => {
            let brush = gdi(m).borrow_mut().dc(hdc).map(|d| d.brush).unwrap_or(0);
            fill_rect_brush(m, hdc, r, brush);
        }
    }
    ThunkResult::stdcall(1, 6)
}

fn set_pixel(m: &mut Machine) -> ThunkResult {
    let (hdc, x, y, c) = (m.arg_u32(0), m.arg_i32(1), m.arg_i32(2), m.arg_u32(3));
    let dc = gdi(m).borrow_mut().dc(hdc).cloned();
    if let Some(dc) = dc {
        if let (Some(px), Some(clip)) = (dc.pixels, dc.clip_rect()) {
            if clip.contains(x, y) {
                let v = px.encode(c);
                px.write_raw(m, x, y, v);
                return ThunkResult::stdcall(px.decode(v), 4);
            }
        }
    }
    ThunkResult::stdcall(u32::MAX, 4)
}

fn get_pixel(m: &mut Machine) -> ThunkResult {
    let (hdc, x, y) = (m.arg_u32(0), m.arg_i32(1), m.arg_i32(2));
    let dc = gdi(m).borrow_mut().dc(hdc).cloned();
    if let Some(Some(px)) = dc.map(|d| d.pixels) {
        if px.bounds().contains(x, y) {
            return ThunkResult::stdcall(px.decode(px.read_raw(m, x, y)), 3);
        }
    }
    ThunkResult::stdcall(u32::MAX, 3) // CLR_INVALID
}

fn create_compatible_dc(m: &mut Machine) -> ThunkResult {
    let mut dc = Dc::new(None);
    dc.memory = true;
    let h = gdi(m).borrow_mut().add(GdiObj::Dc(Box::new(dc)));
    ThunkResult::stdcall(h, 1)
}

fn delete_dc(m: &mut Machine) -> ThunkResult {
    let h = m.arg_u32(0);
    let st = gdi(m);
    let mut g = st.borrow_mut();
    let ok = matches!(g.objects.get(&h), Some(GdiObj::Dc(_)));
    if ok {
        g.objects.remove(&h);
    }
    ThunkResult::stdcall(ok as u32, 1)
}

fn alloc_bitmap(m: &mut Machine, w: i32, h: i32, bpp: u32, top_down: bool, rgb565: bool) -> Option<(Pixels, u32)> {
    if w <= 0 || h <= 0 || w > 8192 || h > 8192 {
        return None;
    }
    let stride = ((w as u32 * bpp + 31) / 32) * 4;
    let size = stride * h as u32;
    let bits = m.mem.alloc(size);
    if bits == 0 {
        return None;
    }
    let (addr, pitch) = if top_down { (bits, stride as i32) } else { (bits + stride * (h as u32 - 1), -(stride as i32)) };
    Some((Pixels { addr, pitch, width: w, height: h, bpp, rgb565 }, bits))
}

/// CreateDIBSection(hdc, pbmi, usage, ppvBits, hSection, offset)
fn create_dib_section(m: &mut Machine) -> ThunkResult {
    let (bmi, ppv) = (m.arg_u32(1), m.arg_u32(3));
    let w = m.mem.read_u32(bmi + 4) as i32;
    let h = m.mem.read_u32(bmi + 8) as i32;
    let bpp = m.mem.read_u16(bmi + 14) as u32;
    let compression = m.mem.read_u32(bmi + 16);
    // BI_BITFIELDS (3): masks follow the header; 565 if the green mask is 0x7E0.
    let rgb565 = compression == 3 && m.mem.read_u32(bmi + 40 + 4) == 0x07E0;
    let Some((px, bits)) = alloc_bitmap(m, w, h.abs(), bpp, h < 0, rgb565) else {
        if ppv != 0 {
            m.mem.write_u32(ppv, 0);
        }
        return ThunkResult::stdcall(0, 6);
    };
    if ppv != 0 {
        m.mem.write_u32(ppv, bits);
    }
    m.log(LogLevel::Debug, &format!("CreateDIBSection {w}x{h}x{bpp} comp {compression} -> bits {bits:#x}"));
    let hb = gdi(m).borrow_mut().add(GdiObj::Bitmap { pixels: px, dib: true, bits_owned: true });
    ThunkResult::stdcall(hb, 6)
}

fn create_compatible_bitmap(m: &mut Machine) -> ThunkResult {
    let (w, h) = (m.arg_i32(1), m.arg_i32(2));
    let Some((px, _)) = alloc_bitmap(m, w, h, 16, true, true) else { return ThunkResult::stdcall(0, 3) };
    let hb = gdi(m).borrow_mut().add(GdiObj::Bitmap { pixels: px, dib: false, bits_owned: true });
    ThunkResult::stdcall(hb, 3)
}

fn blit(m: &mut Machine, dst: u32, dr: Rect, src: u32, sx: i32, sy: i32, sw: i32, sh: i32, rop: u32) -> bool {
    let (d, s) = {
        let st = gdi(m);
        let mut g = st.borrow_mut();
        (g.dc(dst).cloned(), if src != 0 { g.dc(src).cloned() } else { None })
    };
    let Some(d) = d else { return false };
    let (Some(dp), Some(clip)) = (d.pixels, d.clip_rect()) else { return true };
    let dw = dr.r - dr.l;
    let dh = dr.b - dr.t;
    if dw <= 0 || dh <= 0 {
        return true;
    }
    match rop {
        0x00CC_0020 => {} // SRCCOPY
        0x0000_0042 => {
            let r = dr.intersect(&clip);
            if !r.is_empty() {
                dp.fill(m, &r, 0);
            }
            return true;
        }
        0x00FF_0062 => {
            let r = dr.intersect(&clip);
            if !r.is_empty() {
                dp.fill(m, &r, 0xFF_FFFF);
            }
            return true;
        }
        other => m.log(LogLevel::Debug, &format!("BitBlt rop {other:#x} treated as SRCCOPY")),
    }
    let Some(Some(sp)) = s.map(|s| s.pixels) else { return true };
    let same = sp.bpp == dp.bpp && sp.rgb565 == dp.rgb565;
    for y in dr.t.max(clip.t)..dr.b.min(clip.b) {
        let syy = sy + ((y - dr.t) as i64 * sh as i64 / dh as i64) as i32;
        if syy < 0 || syy >= sp.height {
            continue;
        }
        for x in dr.l.max(clip.l)..dr.r.min(clip.r) {
            let sxx = sx + ((x - dr.l) as i64 * sw as i64 / dw as i64) as i32;
            if sxx < 0 || sxx >= sp.width {
                continue;
            }
            let raw = sp.read_raw(m, sxx, syy);
            let v = if same { raw } else { dp.encode(sp.decode(raw)) };
            dp.write_raw(m, x, y, v);
        }
    }
    true
}

/// BitBlt(hdcDest, x, y, w, h, hdcSrc, x1, y1, rop)
fn bit_blt(m: &mut Machine) -> ThunkResult {
    let (dst, x, y, w, h) = (m.arg_u32(0), m.arg_i32(1), m.arg_i32(2), m.arg_i32(3), m.arg_i32(4));
    let (src, sx, sy, rop) = (m.arg_u32(5), m.arg_i32(6), m.arg_i32(7), m.arg_u32(8));
    let ok = blit(m, dst, Rect::new(x, y, x + w, y + h), src, sx, sy, w, h, rop);
    ThunkResult::stdcall(ok as u32, 9)
}

/// StretchBlt(hdcDest, x, y, w, h, hdcSrc, xs, ys, ws, hs, rop)
fn stretch_blt(m: &mut Machine) -> ThunkResult {
    let (dst, x, y, w, h) = (m.arg_u32(0), m.arg_i32(1), m.arg_i32(2), m.arg_i32(3), m.arg_i32(4));
    let (src, sx, sy, sw, sh, rop) = (m.arg_u32(5), m.arg_i32(6), m.arg_i32(7), m.arg_i32(8), m.arg_i32(9), m.arg_u32(10));
    let ok = blit(m, dst, Rect::new(x, y, x + w, y + h), src, sx, sy, sw, sh, rop);
    ThunkResult::stdcall(ok as u32, 11)
}

fn gdi_flush(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(1, 0)
}

fn select_palette(m: &mut Machine) -> ThunkResult {
    let (hdc, hpal) = (m.arg_u32(0), m.arg_u32(1));
    let st = gdi(m);
    let mut g = st.borrow_mut();
    let prev = g.dc(hdc).map(|d| std::mem::replace(&mut d.palette, hpal)).unwrap_or(0);
    ThunkResult::stdcall(prev, 3)
}

fn realize_palette(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(0, 1)
}

fn get_system_palette_entries(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(0, 4)
}

fn save_dc(m: &mut Machine) -> ThunkResult {
    let hdc = m.arg_u32(0);
    let st = gdi(m);
    let mut g = st.borrow_mut();
    let Some(dc) = g.dc(hdc) else { return ThunkResult::stdcall(0, 1) };
    let mut copy = dc.clone();
    copy.saved.clear();
    dc.saved.push(Box::new(copy));
    ThunkResult::stdcall(dc.saved.len() as u32, 1)
}

fn restore_dc(m: &mut Machine) -> ThunkResult {
    let (hdc, n) = (m.arg_u32(0), m.arg_i32(1));
    let st = gdi(m);
    let mut g = st.borrow_mut();
    let Some(dc) = g.dc(hdc) else { return ThunkResult::stdcall(0, 2) };
    let len = dc.saved.len() as i32;
    let idx = if n < 0 { len + n } else { n - 1 };
    if idx < 0 || idx >= len {
        return ThunkResult::stdcall(0, 2);
    }
    let mut saved = dc.saved.drain(idx as usize..).collect::<Vec<_>>();
    let mut restored = *saved.remove(0);
    restored.saved = std::mem::take(&mut dc.saved);
    *dc = restored;
    ThunkResult::stdcall(1, 2)
}
