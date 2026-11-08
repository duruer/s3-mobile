//! The game's in-game texts (left panel labels, numbers, tooltips, clock, end banner) drawn crisp in the window
//! layer (docs/porting/display.md "Panel texts in the window layer", RE: docs/reverse-engineering/game-state-ui.md
//! section 6 "Text DIB composite").
//!
//! The game renders these texts with `DrawTextA` (call `0x4a8725` in `0x4a8090`) into a 1-bpp DIB the size of
//! the screen, then copies the set bits of a rectangle onto the frame (`0x4d6740(x, y, w, h)`, light grey
//! RGB(230, 230, 230) with a black pixel at (+1, +1)). `0x4d65e0(x, y, w, h)` clears a DIB rectangle first. With
//! the Classic panel shown 2-4x enlarged those 1-bit texts were blocky and hard to read. Here:
//!
//! - the `DrawTextA` call site records each text with its layout (`s3_win32::user32::draw_text_layout`: lines,
//!   positions, font size, the same font as the DIB) as a "DIB text"; a DIB clear drops the texts in its rectangle;
//! - the composite is skipped when DIB texts lie in its rectangle; they become "frame texts", clipped to the
//!   composite rectangle, and this mod draws them at window resolution (same positions, scaled with the viewport).
//!
//! Lifetime: the map part is redrawn every frame, so texts composited by the frame renderer (tooltip, clock,
//! banner) live for one frame (collected per frame: frame renderer entry `0x4b26c0` .. call `0x4b422f`). The panel
//! is only redrawn when dirty; texts composited by the panel renderer (`0x4b4d00`, call `0x4b2dc5`) are kept
//! until a new composite overlaps them or the frame pixels under them change (hash of the rectangle in the
//! presented frame; the text itself is not in the frame).
//!
//! Presentation only: the DIB still gets every text, no guest state changes except the skipped copy onto the
//! frame (render thread). Only while a frontend shows the window layer (else the frame keeps the game's texts).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use s3_runtime::Machine;
use s3_win32::gdi32::Rect;

use crate::layer::Layer;
use crate::overlay::{measure_text, Rgba, Weight};
use crate::{FrameBuffer, Mod};

/// `call [DrawTextA]` in the game's text renderer `0x4a8090` (GOG S3.EXE 1.60): `[esp]` hdc, `+4` string,
/// `+8` count, `+0xc` RECT*, `+0x10` format.
const SITE_DRAW_TEXT: u32 = 0x004a_8725;
/// thiscall gfx (x, y, w, h), `ret 0x10`: clear a DIB rectangle / copy its set bits onto the frame.
const FN_DIB_CLEAR: u32 = 0x004d_65e0;
const FN_DIB_COMPOSITE: u32 = 0x004d_6740;
/// Frame renderer entry and its last drawing call (messages composite, right before the flip).
const FN_FRAME: u32 = 0x004b_26c0;
const SITE_FRAME_END: u32 = 0x004b_422f;
/// Call of the panel renderer `0x4b4d00` in the frame renderer and the instruction after it.
const SITE_PANEL: u32 = 0x004b_2dc5;
const SITE_PANEL_END: u32 = 0x004b_2dca;
/// Panel renderer redraw conditions: `[snap+0x40] == 1` or `[gfx+0xf8] == 1` (see the `SITE_PANEL` hook).
const PANEL_DIRTY: u32 = 0x40;
const PANEL_AGAIN: u32 = 0xf8;

const DT_NOCLIP: u32 = 0x100;
const DT_CALCRECT: u32 = 0x400;
const DT_CENTER: u32 = 0x1;
const DT_RIGHT: u32 = 0x2;

/// Texts are dropped when the game has not finished a game frame for this long (it left the game).
const STALE_MS: u64 = 300;

/// Game text colour (`0x4d6950`: `0x4c7b00(0xe6, 0xe6, 0xe6)`) and its shadow.
const FG: Rgba = Rgba::rgb(230, 230, 230);
const SHADOW: Rgba = Rgba::rgba(0, 0, 0, 230);

#[derive(Debug, Clone, PartialEq)]
struct Line {
    text: String,
    /// Left x, top y and width in game pixels (the DIB's layout).
    x: i32,
    y: i32,
    w: i32,
}

/// One `DrawTextA` call.
#[derive(Debug, Clone, PartialEq)]
struct Text {
    lines: Vec<Line>,
    px: f32,
    bold: bool,
    line_h: i32,
    fmt: u32,
    /// The DrawText rectangle (also the clip unless DT_NOCLIP), then intersected with the composite rectangle.
    clip: Rect,
}

/// A text on the frame.
#[derive(Debug, Clone, PartialEq)]
struct FrameText {
    t: Text,
    /// Panel text (kept across frames) and the hash of the frame pixels under it (set on the first present).
    retained: bool,
    hash: Option<u64>,
}

#[derive(Default)]
struct State {
    /// Texts in the DIB now.
    dib: Vec<Text>,
    /// Frame texts of the frame being rendered (not panel texts).
    cur: Vec<FrameText>,
    /// Frame texts of the last finished frame.
    shown: Vec<FrameText>,
    /// Panel texts on the frame.
    retained: Vec<FrameText>,
    in_panel: bool,
}

#[derive(Default)]
pub(crate) struct Shared {
    st: Mutex<State>,
    last_ms: AtomicU64,
}

/// Machine state: the shared text state for other mods (the Classic magnifier draws the texts in its inset).
#[derive(Default)]
pub(crate) struct PanelTextState {
    shared: Option<Arc<Shared>>,
}

fn intersects(a: &Rect, b: &Rect) -> bool {
    a.l < b.r && b.l < a.r && a.t < b.b && b.t < a.b
}

fn intersect(a: &Rect, b: &Rect) -> Rect {
    Rect { l: a.l.max(b.l), t: a.t.max(b.t), r: a.r.min(b.r), b: a.b.min(b.b) }
}

fn stack_rect(c: &mut crate::hooks::HookCtx) -> Rect {
    let (x, y, w, h) = (c.arg(0) as i32, c.arg(1) as i32, c.arg(2) as i32, c.arg(3) as i32);
    Rect { l: x, t: y, r: x + w, b: y + h }
}

/// The window layer is shown by the frontend (only then the frame's own texts are left out).
fn layer_shown(m: &mut Machine) -> bool {
    m.state::<crate::ModsState>().borrow_mut().layer_sink.is_some()
}

/// Install the hooks and the mod (GOG patch set only).
pub fn install(m: &mut Machine) {
    // `S3_PANEL_TEXT=0`: keep the game's own texts in the frame (comparison screenshots).
    if std::env::var("S3_PANEL_TEXT").is_ok_and(|v| v == "0") {
        return;
    }
    let shared = Arc::new(Shared::default());
    m.state::<PanelTextState>().borrow_mut().shared = Some(shared.clone());

    let sh = shared.clone();
    crate::hooks::add_hook(m, SITE_DRAW_TEXT, "panel-text:draw", move |c| {
        let esp = c.esp();
        let rd = |c: &mut crate::hooks::HookCtx, o: u32| c.m.mem.read_u32(esp + o);
        let (hdc, sp, n, prc, fmt) = (rd(c, 0), rd(c, 4), rd(c, 8) as i32, rd(c, 0xc), rd(c, 0x10));
        if fmt & DT_CALCRECT != 0 || sp == 0 || prc == 0 {
            return s3_runtime::HookFlow::Continue;
        }
        let bytes = if n < 0 { c.m.mem.read_cstr_bytes(sp) } else { c.m.mem.read_bytes(sp, (n as usize).min(0x1000)) };
        let rect = Rect::read(c.m, prc);
        if let Some(lay) = s3_win32::user32::draw_text_layout(c.m, hdc, &bytes, rect, fmt) {
            let lines: Vec<Line> = lay
                .lines
                .iter()
                .map(|l| Line { text: l.text.iter().map(|&b| s3_win32::font::cp1252(b)).collect(), x: l.x, y: l.y, w: l.w })
                .filter(|l| !l.text.trim().is_empty())
                .collect();
            if !lines.is_empty() {
                let clip = if fmt & DT_NOCLIP != 0 { Rect { l: -100_000, t: -100_000, r: 100_000, b: 100_000 } } else { rect };
                let t = Text { lines, px: lay.px, bold: lay.bold, line_h: lay.line_h, fmt, clip };
                sh.st.lock().unwrap().dib.push(t);
            }
        }
        s3_runtime::HookFlow::Continue
    });

    let sh = shared.clone();
    crate::hooks::add_hook(m, FN_DIB_CLEAR, "panel-text:clear", move |c| {
        let r = stack_rect(c);
        sh.st.lock().unwrap().dib.retain(|t| !intersects(&t.clip, &r));
        s3_runtime::HookFlow::Continue
    });

    let sh = shared.clone();
    crate::hooks::add_hook(m, FN_DIB_COMPOSITE, "panel-text:composite", move |c| {
        if !layer_shown(c.m) {
            return s3_runtime::HookFlow::Continue;
        }
        let r = stack_rect(c);
        let mut st = sh.st.lock().unwrap();
        let texts: Vec<Text> = st
            .dib
            .iter()
            .filter(|t| intersects(&t.clip, &r))
            .map(|t| Text { clip: intersect(&t.clip, &r), ..t.clone() })
            .collect();
        if texts.is_empty() {
            return s3_runtime::HookFlow::Continue;
        }
        let retained = st.in_panel;
        if std::env::var("S3_PANEL_TEXT_LOG").is_ok() {
            let names: Vec<&str> = texts.iter().flat_map(|t| t.lines.iter().map(|l| l.text.as_str())).collect();
            let ret = c.return_address();
            c.m.log(s3_runtime::LogLevel::Info, &format!("panel-text: composite {r:?} from {ret:#x} panel={retained} {names:?}"));
        }
        if retained {
            st.retained.retain(|f| !intersects(&f.t.clip, &r));
        }
        let list = if retained { &mut st.retained } else { &mut st.cur };
        list.extend(texts.into_iter().map(|t| FrameText { t, retained, hash: None }));
        drop(st);
        c.return_from_function(0, 4)
    });

    let sh = shared.clone();
    crate::hooks::add_hook(m, FN_FRAME, "panel-text:frame", move |_c| {
        sh.st.lock().unwrap().cur.clear();
        s3_runtime::HookFlow::Continue
    });
    let sh = shared.clone();
    crate::hooks::add_hook(m, SITE_FRAME_END, "panel-text:frame-end", move |c| {
        let mut st = sh.st.lock().unwrap();
        st.shown = std::mem::take(&mut st.cur);
        sh.last_ms.store(c.m.real_ms(), Ordering::Relaxed);
        s3_runtime::HookFlow::Continue
    });
    let sh = shared.clone();
    crate::hooks::add_hook(m, SITE_PANEL, "panel-text:panel", move |c| {
        // `0x4b4d00` draws the whole panel when `[snap+0x40] == 1` (copy of the panel dirty flag `gd+0x2d27c`) and
        // once more in the next frame (`[gfx+0xf8] == 1`); else it returns at once. A full redraw drops the old
        // texts: the new page composites its own (a page change can leave the background under a text unchanged).
        // At the call instruction `[esp]` is the pushed snapshot, ECX = gfx.
        let (esp, gfx) = (c.esp(), c.reg(s3_cpu::ECX));
        let snap = c.m.mem.read_u32(esp);
        let full = snap != 0 && (c.m.mem.read_u32(snap + PANEL_DIRTY) == 1 || c.m.mem.read_u32(gfx + PANEL_AGAIN) == 1);
        let mut st = sh.st.lock().unwrap();
        st.in_panel = true;
        if full {
            st.retained.clear();
        }
        s3_runtime::HookFlow::Continue
    });
    let sh = shared.clone();
    crate::hooks::add_hook(m, SITE_PANEL_END, "panel-text:panel-end", move |_c| {
        sh.st.lock().unwrap().in_panel = false;
        s3_runtime::HookFlow::Continue
    });

    crate::add_mod(m, Box::new(PanelTextMod { shared }));
}

/// FNV-1a over the frame pixels of `r` (clamped to the frame).
fn rect_hash(f: &FrameBuffer, r: &Rect) -> u64 {
    let (w, h) = (f.width as i32, f.height as i32);
    let (x0, x1, y0, y1) = (r.l.clamp(0, w), r.r.clamp(0, w), r.t.clamp(0, h), r.b.clamp(0, h));
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for y in y0..y1 {
        let row = &f.pixels[(y * w + x0) as usize..(y * w + x1) as usize];
        for &p in row {
            hash = (hash ^ p as u64).wrapping_mul(0x100_0000_01b3);
        }
    }
    hash
}

struct PanelTextMod {
    shared: Arc<Shared>,
}

/// Map of game pixels to window pixels for one drawing: through the viewport, or linear into a magnifier inset.
#[derive(Clone, Copy)]
enum Map {
    View,
    /// Game rect (x, y, w, h) shown in window rect (x, y, w, h).
    Inset([f32; 4], [f32; 4]),
}

fn to_window(vp: &crate::display::Viewport, map: Map, gx: f32, gy: f32) -> (f32, f32) {
    match map {
        Map::View => vp.game_to_window(gx, gy),
        Map::Inset(s, d) => (d[0] + (gx - s[0]) * d[2] / s[2].max(1.0), d[1] + (gy - s[1]) * d[3] / s[3].max(1.0)),
    }
}

/// The map for a text on screen. With the Classic panel split the panel and the map part have different scales; a
/// text that starts in the panel (labels, also the 180 px tooltip box at the panel's bottom that reaches into the
/// map part) is drawn wholly at the panel's scale, else corners in different parts would not fit together.
fn view_map(vp: &crate::display::Viewport, t: &Text) -> Map {
    match (vp.panel, vp.panel_scale()) {
        (Some(p), Some(ps)) if t.clip.l < p.w as i32 => Map::Inset([0.0, 0.0, 1.0, 1.0], [0.0, 0.0, ps, ps]),
        _ => Map::View,
    }
}

/// Draw one text: the clip and each line through `map`, size from the mapped line height.
fn draw_text(l: &mut Layer, vp: &crate::display::Viewport, map: Map, t: &Text, outer_clip: Option<(i32, i32, i32, i32)>) {
    let (cx0, cy0) = to_window(vp, map, t.clip.l as f32, t.clip.t as f32);
    let (cx1, cy1) = to_window(vp, map, t.clip.r as f32, t.clip.b as f32);
    let (mut x0, mut y0, mut x1, mut y1) = (cx0.floor() as i32, cy0.floor() as i32, cx1.ceil() as i32, cy1.ceil() as i32);
    if let Some((ox, oy, ow, oh)) = outer_clip {
        (x0, y0, x1, y1) = (x0.max(ox), y0.max(oy), x1.min(ox + ow), y1.min(oy + oh));
    }
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    // Clip with a little room below / right for the shadow; the game clips the same way to its rectangle.
    l.set_clip(x0, y0, x1 - x0 + 1, y1 - y0 + 1);
    let weight = if t.bold { Weight::Bold } else { Weight::Regular };
    for line in &t.lines {
        let (wx, wy) = to_window(vp, map, line.x as f32, line.y as f32);
        let (wx1, wy1) = to_window(vp, map, (line.x + line.w) as f32, (line.y + t.line_h) as f32);
        let s = ((wy1 - wy) / t.line_h.max(1) as f32).max(0.1);
        let px = t.px * s;
        let (tw, _) = measure_text(px, weight, &line.text);
        // Keep the game's alignment (the window font measures a little differently than the scaled DIB).
        let x = if t.fmt & DT_CENTER != 0 {
            (wx + wx1) / 2.0 - tw as f32 / 2.0
        } else if t.fmt & DT_RIGHT != 0 {
            wx1 - tw as f32
        } else {
            wx
        };
        let (xi, yi) = (x.round() as i32, wy.round() as i32);
        let d = (s * 0.6).round().max(1.0) as i32;
        l.text(xi + d, yi + d, px, weight, SHADOW, &line.text);
        l.text(xi, yi, px, weight, FG, &line.text);
    }
    l.reset_clip();
}

impl PanelTextMod {
    fn texts(&self) -> Vec<FrameText> {
        let st = self.shared.st.lock().unwrap();
        st.retained.iter().chain(st.shown.iter()).cloned().collect()
    }
}

impl Mod for PanelTextMod {
    fn name(&self) -> &str {
        "panel-text"
    }

    fn wants_draw(&self) -> bool {
        let st = self.shared.st.lock().unwrap();
        !st.retained.is_empty() || !st.shown.is_empty()
    }

    fn tick(&mut self, m: &mut Machine, now: u64) {
        let last = self.shared.last_ms.load(Ordering::Relaxed);
        if now.saturating_sub(last) > STALE_MS {
            let mut st = self.shared.st.lock().unwrap();
            if !st.retained.is_empty() || !st.shown.is_empty() {
                st.retained.clear();
                st.shown.clear();
                drop(st);
                m.refresh_overlay();
            }
        }
    }

    fn draw_window(&mut self, m: &mut Machine, l: &mut Layer, frame: &FrameBuffer) {
        let vp = crate::draw_viewport(m);
        if vp.layout.is_some() {
            return;
        }
        // Panel texts whose background changed are gone (the game redrew that part of the panel).
        {
            let mut st = self.shared.st.lock().unwrap();
            st.retained.retain_mut(|f| {
                let h = rect_hash(frame, &f.t.clip);
                match f.hash {
                    None => {
                        f.hash = Some(h);
                        true
                    }
                    Some(old) => old == h,
                }
            });
        }
        for f in self.texts() {
            draw_text(l, &vp, view_map(&vp, &f.t), &f.t, None);
        }
    }
}

/// Draw the texts inside the frame rectangle `src` (game px x, y, w, h) into the window rectangle `dst` (the
/// Classic magnifier inset; the frame there has no texts while the window layer is shown).
pub fn draw_inset(m: &mut Machine, l: &mut Layer, src: [f32; 4], dst: (i32, i32, i32, i32)) {
    let Some(sh) = m.state::<PanelTextState>().borrow_mut().shared.clone() else { return };
    let texts: Vec<FrameText> = {
        let st = sh.st.lock().unwrap();
        st.retained.iter().chain(st.shown.iter()).cloned().collect()
    };
    let vp = crate::draw_viewport(m);
    let s = Rect { l: src[0].floor() as i32, t: src[1].floor() as i32, r: (src[0] + src[2]).ceil() as i32, b: (src[1] + src[3]).ceil() as i32 };
    let map = Map::Inset(src, [dst.0 as f32, dst.1 as f32, dst.2 as f32, dst.3 as f32]);
    for f in texts.iter().filter(|f| intersects(&f.t.clip, &s)) {
        draw_text(l, &vp, map, &f.t, Some(dst));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rects() {
        let a = Rect { l: 0, t: 0, r: 10, b: 10 };
        let b = Rect { l: 10, t: 0, r: 20, b: 10 };
        assert!(!intersects(&a, &b));
        let c = Rect { l: 5, t: 5, r: 15, b: 15 };
        assert!(intersects(&a, &c));
        assert_eq!(intersect(&a, &c), Rect { l: 5, t: 5, r: 10, b: 10 });
    }

    #[test]
    fn hash_sees_changes() {
        let mut f = FrameBuffer::new(8, 8);
        let r = Rect { l: 2, t: 2, r: 5, b: 5 };
        let h0 = rect_hash(&f, &r);
        f.pixels[0] = 7; // outside
        assert_eq!(rect_hash(&f, &r), h0);
        f.pixels[3 * 8 + 3] = 7;
        assert_ne!(rect_hash(&f, &r), h0);
    }
}
