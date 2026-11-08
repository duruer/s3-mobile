//! In-game message lines (tutorial text, chat, notices) drawn in the window layer at a constant on-screen size
//! (docs/reverse-engineering/game-state-ui.md section 7, docs/porting/mods.md "Window layer").
//!
//! The game draws its 5 message lines and the chat input line into the frame (text DIB `0x4d82b0`, icons +
//! composite `0x4d8400`, render thread, both thiscall `gfx` with the snapshot as the only stack argument, `ret 4`).
//! With the zoom (= render size) they grew and shrank on screen. Here both functions are skipped; the second hook
//! reads the lines from the snapshot (exactly what the game would have drawn) and this mod draws them in window
//! pixels. The tutorial arrow (icon `0xfc`) becomes a "Next" button that sets the tutorial wait counter
//! `[gd+0x2d304] = 1`, which advances an arrow step like a click in the game's arrow corner (VERIFIED in RE).
//! Presentation only for the frame; the logic side (ring, timers, tutorial) is untouched.

use std::sync::{Arc, Mutex};

use s3_runtime::Machine;

use crate::gesture::TouchEvent;
use crate::layer::Layer;
use crate::overlay::{measure_text, Rgba, Weight};
use crate::{FrameBuffer, Mod, ModEvent};

/// Render-thread functions (GOG S3.EXE 1.60).
const FN_DRAW_TEXT: u32 = 0x004d_82b0;
const FN_COMPOSITE: u32 = 0x004d_8400;
/// Snapshot copy of the message ring (+ `res::snap_delta()`), 0x1c bytes per entry, and the chat input line.
const SNAP_MSGS: u32 = 0xc66d8;
const SNAP_CHAT: u32 = 0xc66c0;
const ENTRY: u32 = 0x1c;
const SLOTS: u32 = 5;
/// Tutorial: `[0x7dfdb0] == 0x63`; wait counter `gd+0x2d304` (1 = advance an arrow step).
const MISSION_KIND: u32 = 0x007d_fdb0;
const TUTORIAL: u32 = 0x63;
const GD_TUTORIAL_WAIT: u32 = 0x2d304;

const ICON_ARROW: u32 = 0xfc;
const ICON_INFO: u32 = 0xfd;

#[derive(Debug, Clone, PartialEq, Default)]
struct Line {
    slot: u32,
    text: String,
    icon: u32,
}

#[derive(Debug, Clone, PartialEq, Default)]
struct Shown {
    lines: Vec<Line>,
    chat: Option<String>,
}

/// Lines read by the render-thread hook, consumed by the mod (same machine, shared through an Arc).
#[derive(Default)]
struct Shared {
    shown: Mutex<Shown>,
    /// `m.now_ms()` of the last composite call (the game renders a game frame).
    last_ms: std::sync::atomic::AtomicU64,
}

/// Lines are dropped when the game has not drawn a game frame for this long (it left the game: quit, statistics,
/// main menu; the hook is not called there and the last lines stayed on screen, user report 2026-11-08).
const STALE_MS: u64 = 300;

fn read_wstr(m: &Machine, obj: u32) -> Option<String> {
    let ptr = m.mem.read_u32(obj + 4);
    let len = m.mem.read_u32(obj + 8).min(400);
    if ptr == 0 || len == 0 {
        return None;
    }
    let b = m.mem.read_bytes(ptr, len as usize * 2);
    let units: Vec<u16> = b.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let s: String = String::from_utf16_lossy(&units).chars().filter(|c| !c.is_control()).collect();
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn read_snapshot(m: &Machine, snap: u32) -> Shown {
    let d = crate::res::snap_delta();
    let mut lines = Vec::new();
    for i in 0..SLOTS {
        let e = snap + SNAP_MSGS + d + i * ENTRY;
        let timer = m.mem.read_u32(e + 0x10);
        if timer == 0 {
            continue;
        }
        if let Some(text) = read_wstr(m, e) {
            lines.push(Line { slot: i, text, icon: m.mem.read_u32(e + 0x14) & 0xff });
        }
    }
    let chat = read_wstr(m, snap + SNAP_CHAT + d);
    Shown { lines, chat }
}

/// Install the hooks and the mod (GOG patch set only).
pub fn install(m: &mut Machine) {
    let shared = Arc::new(Shared::default());
    crate::hooks::add_hook(m, FN_DRAW_TEXT, "messages:skip-text", |c| c.return_from_function(0, 1));
    let sh = shared.clone();
    crate::hooks::add_hook(m, FN_COMPOSITE, "messages:read", move |c| {
        let snap = c.arg(0);
        if snap != 0 {
            let s = read_snapshot(c.m, snap);
            *sh.shown.lock().unwrap() = s;
            sh.last_ms.store(c.m.now_ms(), std::sync::atomic::Ordering::Relaxed);
        }
        c.return_from_function(0, 1)
    });
    crate::add_mod(m, Box::new(MessagesMod { shared, next_rect: None }));
}

struct MessagesMod {
    shared: Arc<Shared>,
    /// Window rectangle of the tutorial "Next" button while it is shown.
    next_rect: Option<(f32, f32, f32, f32)>,
}

impl Mod for MessagesMod {
    fn name(&self) -> &str {
        "messages"
    }

    fn wants_draw(&self) -> bool {
        let s = self.shared.shown.lock().unwrap();
        !s.lines.is_empty() || s.chat.is_some()
    }

    fn tick(&mut self, m: &mut Machine, now: u64) {
        let last = self.shared.last_ms.load(std::sync::atomic::Ordering::Relaxed);
        if now.saturating_sub(last) > STALE_MS {
            let mut s = self.shared.shown.lock().unwrap();
            if !s.lines.is_empty() || s.chat.is_some() {
                *s = Shown::default();
                drop(s);
                self.next_rect = None;
                m.refresh_overlay();
            }
        }
    }

    fn draw_window(&mut self, m: &mut Machine, l: &mut Layer, _frame: &FrameBuffer) {
        let shown = self.shared.shown.lock().unwrap().clone();
        self.next_rect = None;
        // Not over menu screens (menu layouts) or a frame the game did not draw as a game frame.
        if crate::draw_viewport(m).layout.is_some() {
            return;
        }
        // Constant physical size: 13 dp when the frontend reports the density (Android), else the window-height
        // scale. With the height scale the text was huge on a foldable's tall inner screen and small on the
        // cover screen (user report 2026-11-08). `ui` = px per "reference px" (text 14 reference px).
        let wh = l.height() as f32;
        let ui = crate::display::dp(m).map_or(l.ui(), |d| d * 13.0 / 14.0);
        let vp = crate::draw_viewport(m);
        // Left edge: right of the Classic panel; in the Mobile HUD right of its minimap (like the game's 50c).
        let panel_w = m.mem.read_u32(crate::res::L_PANEL_W) as f32;
        let panel_x = if panel_w > 0.0 { vp.game_to_window(panel_w, 0.0).0.max(0.0) } else { 0.0 };
        let x0 = panel_x + if panel_w > 0.0 { 0.05 * wh } else { 0.33 * wh };
        let px = 14.0 * ui;
        let lh = (measure_text(px, Weight::Bold, "Ag").1 as f32 * 1.35).max(1.0);
        let bottom = 0.9 * wh;
        let pad = (5.0 * ui).round() as i32;
        let tutorial = m.mem.read_u32(MISSION_KIND) == TUTORIAL;
        for line in &shown.lines {
            let y = (bottom - (line.slot as f32 + 1.0) * lh).round() as i32;
            let mut x = x0.round() as i32;
            if line.icon == ICON_INFO {
                let r = px * 0.5;
                l.fill_circle(x as f32 + r, y as f32 + lh * 0.5, r, Rgba::rgba(40, 90, 200, 230));
                l.text_centered(x, y, (2.0 * r) as i32, lh as i32, px * 0.8, Weight::Bold, Rgba::WHITE, "i");
                x += (2.0 * r) as i32 + pad;
            }
            let (_, _, tw, th) = l.text_box(x, y, px, Weight::Bold, Rgba::WHITE, Rgba::rgba(0, 0, 0, 150), &line.text);
            if line.icon == ICON_ARROW && tutorial {
                let label = "Next >";
                let (bw, bh) = measure_text(px, Weight::Bold, label);
                let (bx, by) = (x + tw + pad * 2, y);
                let (bw, bh) = (bw + pad * 4, bh.max(th));
                l.fill_rounded_rect(bx, by, bw, bh, 6.0 * ui, Rgba::rgba(220, 170, 40, 240));
                l.text_centered(bx, by, bw, bh, px, Weight::Bold, Rgba::rgb(30, 20, 0), label);
                self.next_rect = Some((bx as f32, by as f32, bw as f32, bh as f32));
            }
        }
        if let Some(chat) = &shown.chat {
            let y = (bottom + 0.2 * lh).round() as i32;
            l.text_box(x0.round() as i32, y, px, Weight::Bold, Rgba::rgb(255, 230, 140), Rgba::rgba(0, 0, 0, 170), chat);
        }
    }

    fn on_touch(&mut self, m: &mut Machine, ev: &TouchEvent) -> ModEvent {
        let TouchEvent::Tap { .. } = ev else { return ModEvent::Pass };
        let Some((x, y, w, h)) = self.next_rect else { return ModEvent::Pass };
        let (wx, wy) = crate::touch_window_pos(m, ev);
        // A little extra margin around the button for fingers.
        let g = 0.3 * h;
        if wx < x - g || wx > x + w + g || wy < y - g || wy > y + h + g {
            return ModEvent::Pass;
        }
        if m.mem.read_u32(MISSION_KIND) == TUTORIAL {
            if let Some(gd) = crate::ui_hud_game::gd(m) {
                m.mem.write_u32(gd + GD_TUTORIAL_WAIT, 1);
                m.log(s3_runtime::LogLevel::Info, "messages: tutorial Next");
            }
        }
        ModEvent::Consumed
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn layout_constants() {
        assert_eq!(super::SNAP_MSGS - super::SNAP_CHAT, 0x18);
        // 20 entries of 0x1c bytes follow the chat line (only the first 5 are used).
        assert_eq!(super::ENTRY * 20 + super::SNAP_MSGS, 0xc6908);
    }
}
