//! Fallback touch->mouse mapping for touch events no mod consumed. Mirrors the legacy Android mapping
//! (`s3-android/src/touch.rs`) so behaviour stays the same until W3's context-aware input replaces it:
//! - Tap: left click (right click when "right-click next" is armed);
//! - LongPress: right click (if `long_press_right_click`), TwoFingerTap: right click;
//! - ThreeFingerTap: keyboard request flag for the frontend;
//! - one-finger Pan: drag with the left button, or in game on the map with the right button (S3 scrolls
//!   the map with a right drag; mirrored and halved so the map follows the finger);
//! - long-press drags, two-finger pans and pinches: nothing (yet).
//!
//! Button-up events are delayed by [`CLICK_MS`] so games polling the button state see the press.

use s3_runtime::{HostEvent, MouseButton};

use crate::gesture::{Phase, TouchEvent};

pub const CLICK_MS: u64 = 60;
/// Width of the in-game left panel (640x480 game pixels), as in s3-android.
pub const PANEL_W: i32 = 170;

/// Drag button for a one-finger drag starting at `p` (menus are 800x600, see s3-android `drag_button_for`).
pub fn drag_button_for(frame: (u32, u32), p: (i32, i32)) -> MouseButton {
    let in_menu = frame == (800, 600);
    if !in_menu && p.0 >= PANEL_W {
        MouseButton::Right
    } else {
        MouseButton::Left
    }
}

#[derive(Default)]
pub struct TouchMouse {
    queue: Vec<(u64, HostEvent)>,
    last_due: u64,
    drag: Option<(MouseButton, (i32, i32))>,
    pub right_next: bool,
    pub keyboard_request: bool,
    pub long_press_right_click: bool,
}

impl TouchMouse {
    fn push(&mut self, t: u64, e: HostEvent) {
        let t = t.max(self.last_due);
        self.last_due = t;
        self.queue.push((t, e));
    }

    fn click(&mut self, t: u64, p: (i32, i32), button: MouseButton) {
        self.push(t, HostEvent::MouseMove { x: p.0, y: p.1 });
        self.push(t, HostEvent::MouseButton { button, down: true });
        self.push(t + CLICK_MS, HostEvent::MouseButton { button, down: false });
    }

    /// Map one unconsumed touch event. `frame` = current game frame size.
    pub fn handle(&mut self, ev: &TouchEvent, now: u64, frame: (u32, u32)) {
        let pt = |x: f32, y: f32| (x.round() as i32, y.round() as i32);
        match *ev {
            TouchEvent::Tap { x, y } => {
                let b = if std::mem::take(&mut self.right_next) { MouseButton::Right } else { MouseButton::Left };
                self.click(now, pt(x, y), b);
            }
            TouchEvent::LongPress { x, y } if self.long_press_right_click => self.click(now, pt(x, y), MouseButton::Right),
            TouchEvent::TwoFingerTap { x, y } => self.click(now, pt(x, y), MouseButton::Right),
            TouchEvent::ThreeFingerTap { .. } => self.keyboard_request = true,
            TouchEvent::Pan { phase, fingers: 1, x, y, start_x, start_y, after_long_press: false, .. } => match phase {
                Phase::Begin => {
                    let o = pt(start_x, start_y);
                    let b = drag_button_for(frame, o);
                    self.drag = Some((b, o));
                    self.push(now, HostEvent::MouseMove { x: o.0, y: o.1 });
                    self.push(now, HostEvent::MouseButton { button: b, down: true });
                    let p = drag_pos(b, o, pt(x, y), frame);
                    self.push(now, HostEvent::MouseMove { x: p.0, y: p.1 });
                }
                Phase::Update => {
                    if let Some((b, o)) = self.drag {
                        let p = drag_pos(b, o, pt(x, y), frame);
                        self.push(now, HostEvent::MouseMove { x: p.0, y: p.1 });
                    }
                }
                Phase::End => {
                    if let Some((b, _)) = self.drag.take() {
                        self.push(now, HostEvent::MouseButton { button: b, down: false });
                    }
                }
            },
            TouchEvent::Cancel { .. } => {
                if let Some((b, _)) = self.drag.take() {
                    self.push(now, HostEvent::MouseButton { button: b, down: false });
                }
            }
            _ => {}
        }
    }

    /// Events due at `now`, in order.
    pub fn drain_due(&mut self, now: u64) -> Vec<HostEvent> {
        let mut out = Vec::new();
        self.queue.retain(|&(t, e)| {
            if t <= now {
                out.push(e);
                false
            } else {
                true
            }
        });
        out
    }

    pub fn pending(&self) -> bool {
        !self.queue.is_empty()
    }
}

/// Mouse position while dragging (right drag mirrored around the origin and halved = grab scrolling).
fn drag_pos(b: MouseButton, o: (i32, i32), cur: (i32, i32), frame: (u32, u32)) -> (i32, i32) {
    let p = match b {
        MouseButton::Right => (o.0 - (cur.0 - o.0) / 2, o.1 - (cur.1 - o.1) / 2),
        _ => cur,
    };
    let (w, h) = (frame.0.max(1) as i32, frame.1.max(1) as i32);
    (p.0.clamp(0, w - 1), p.1.clamp(0, h - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tap_click_and_right_next() {
        let mut t = TouchMouse::default();
        t.handle(&TouchEvent::Tap { x: 10.4, y: 20.6 }, 100, (640, 480));
        let ev = t.drain_due(100);
        assert_eq!(ev, vec![
            HostEvent::MouseMove { x: 10, y: 21 },
            HostEvent::MouseButton { button: MouseButton::Left, down: true }
        ]);
        assert_eq!(t.drain_due(160), vec![HostEvent::MouseButton { button: MouseButton::Left, down: false }]);
        t.right_next = true;
        t.handle(&TouchEvent::Tap { x: 1.0, y: 1.0 }, 200, (640, 480));
        assert!(t.drain_due(300).contains(&HostEvent::MouseButton { button: MouseButton::Right, down: true }));
        assert!(!t.right_next);
    }

    #[test]
    fn map_drag_uses_right_button_mirrored() {
        let mut t = TouchMouse::default();
        let pan = |phase, x| TouchEvent::Pan { phase, fingers: 1, x, y: 200.0, dx: 0.0, dy: 0.0, start_x: 300.0, start_y: 200.0, after_long_press: false };
        t.handle(&pan(Phase::Begin, 320.0), 0, (640, 480));
        t.handle(&pan(Phase::Update, 340.0), 10, (640, 480));
        t.handle(&pan(Phase::End, 340.0), 20, (640, 480));
        let ev = t.drain_due(100);
        assert_eq!(ev[1], HostEvent::MouseButton { button: MouseButton::Right, down: true });
        assert_eq!(ev[2], HostEvent::MouseMove { x: 290, y: 200 });
        assert_eq!(ev[3], HostEvent::MouseMove { x: 280, y: 200 });
        assert_eq!(ev[4], HostEvent::MouseButton { button: MouseButton::Right, down: false });
        // Menus (800x600): left drag, not mirrored.
        t.handle(&pan(Phase::Begin, 320.0), 200, (800, 600));
        let ev = t.drain_due(300);
        assert_eq!(ev[1], HostEvent::MouseButton { button: MouseButton::Left, down: true });
        assert_eq!(ev[2], HostEvent::MouseMove { x: 320, y: 200 });
    }
}
