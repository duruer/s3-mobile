//! Built-in debug overlay (settings `debug_overlay` or `S3_MODS_DEBUG=1`): fingers as circles, the last
//! recognized gesture as a label (window layer, window pixels). Never consumes events.

use s3_runtime::{FrameBuffer, Machine};

use crate::gesture::TouchEvent;
use crate::layer::Layer;
use crate::overlay::{Rgba, Weight};
use crate::{Mod, ModsState};

#[derive(Default)]
pub(crate) struct DebugTouch {
    enabled: Option<bool>,
    fingers: Vec<(u32, f32, f32)>,
    label: String,
    label_at: (f32, f32),
    last_event_ms: u64,
    now: u64,
    dirty: bool,
}

const SHOW_MS: u64 = 1500;

impl Mod for DebugTouch {
    fn name(&self) -> &str {
        "debug-touch"
    }

    fn observe(&mut self, m: &mut Machine, ev: &TouchEvent) {
        if self.enabled.is_none() {
            self.enabled = Some(m.state::<ModsState>().borrow_mut().settings.debug_overlay);
        }
        if self.enabled != Some(true) {
            return;
        }
        self.last_event_ms = m.now_ms();
        self.dirty = true;
        // Window position of the event (the layer draws in window pixels).
        let (x, y) = crate::touch_window_pos(m, ev);
        let ev = &match *ev {
            TouchEvent::Down { id, .. } => TouchEvent::Down { id, x, y },
            TouchEvent::Move { id, .. } => TouchEvent::Move { id, x, y },
            TouchEvent::Tap { .. } => TouchEvent::Tap { x, y },
            TouchEvent::DoubleTap { .. } => TouchEvent::DoubleTap { x, y },
            TouchEvent::LongPress { .. } => TouchEvent::LongPress { x, y },
            TouchEvent::TwoFingerTap { .. } => TouchEvent::TwoFingerTap { x, y },
            TouchEvent::ThreeFingerTap { .. } => TouchEvent::ThreeFingerTap { x, y },
            TouchEvent::Pan { phase, fingers, dx, dy, start_x, start_y, after_long_press, .. } => {
                TouchEvent::Pan { phase, fingers, x, y, dx, dy, start_x, start_y, after_long_press }
            }
            TouchEvent::Pinch { phase, scale, .. } => TouchEvent::Pinch { phase, cx: x, cy: y, scale },
            e => e,
        };
        match *ev {
            TouchEvent::Down { id, x, y } | TouchEvent::Move { id, x, y } => {
                match self.fingers.iter_mut().find(|f| f.0 == id) {
                    Some(f) => *f = (id, x, y),
                    None => self.fingers.push((id, x, y)),
                }
            }
            TouchEvent::Up { id, .. } | TouchEvent::Cancel { id } => self.fingers.retain(|f| f.0 != id),
            TouchEvent::Tap { x, y } => self.set_label("Tap", x, y),
            TouchEvent::DoubleTap { x, y } => self.set_label("DoubleTap", x, y),
            TouchEvent::LongPress { x, y } => self.set_label("LongPress", x, y),
            TouchEvent::TwoFingerTap { x, y } => self.set_label("TwoFingerTap", x, y),
            TouchEvent::ThreeFingerTap { x, y } => self.set_label("ThreeFingerTap", x, y),
            // Two-finger pans come with a Pinch; show the pinch label.
            TouchEvent::Pan { fingers: 2, .. } => {}
            TouchEvent::Pan { phase, fingers, x, y, after_long_press, .. } => {
                let l = format!("Pan{fingers} {phase:?}{}", if after_long_press { " (held)" } else { "" });
                self.set_label(&l, x, y)
            }
            TouchEvent::Pinch { phase, cx, cy, scale } => self.set_label(&format!("Pinch {phase:?} x{scale:.2}"), cx, cy),
        }
    }

    fn wants_draw(&self) -> bool {
        self.enabled == Some(true) && (!self.fingers.is_empty() || self.now.saturating_sub(self.last_event_ms) < SHOW_MS)
    }

    fn needs_redraw(&self) -> bool {
        self.dirty || (self.enabled == Some(true) && self.now.saturating_sub(self.last_event_ms) < SHOW_MS + 100)
    }

    fn tick(&mut self, m: &mut Machine, now: u64) {
        self.now = now;
        self.enabled = Some(m.state::<ModsState>().borrow_mut().settings.debug_overlay);
    }

    fn draw_window(&mut self, _m: &mut Machine, c: &mut Layer, _frame: &FrameBuffer) {
        self.dirty = false;
        let u = c.ui();
        for &(_, x, y) in &self.fingers {
            c.fill_circle(x, y, 14.0 * u, Rgba::rgba(255, 220, 0, 110));
            c.fill_circle(x, y, 3.0 * u, Rgba::rgba(255, 60, 0, 230));
        }
        if !self.label.is_empty() {
            let k = 1.0 - (self.now.saturating_sub(self.last_event_ms) as f32 / SHOW_MS as f32).clamp(0.0, 1.0);
            let k = if self.fingers.is_empty() { k } else { 1.0 };
            let px = 14.0 * u;
            let (tw, th) = crate::overlay::measure_text(px, Weight::Bold, &self.label);
            let p = |v: f32| (v * u).round() as i32;
            let x = (self.label_at.0 as i32 + p(16.0)).min(c.width() as i32 - tw - p(12.0)).max(0);
            let y = (self.label_at.1 as i32 - th - p(16.0)).max(0);
            c.fill_rounded_rect(x, y, tw + p(12.0), th + p(8.0), 6.0 * u, Rgba::rgba(0, 0, 0, 170).fade(k));
            c.text(x + p(6.0), y + p(4.0), px, Weight::Bold, Rgba::WHITE.fade(k), &self.label);
        }
    }
}

impl DebugTouch {
    fn set_label(&mut self, l: &str, x: f32, y: f32) {
        self.label = l.to_string();
        self.label_at = (x, y);
    }
}
