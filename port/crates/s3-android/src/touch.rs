//! Touch -> mouse mapping (docs/porting/android.md "Controls").
//!
//! - tap (down + up, little movement, < 500 ms): left click at the finger;
//! - long press (>= 500 ms without moving): right click at the finger;
//! - one-finger drag (moved beyond the slop): mouse moves with the left button held, released on lift;
//! - two-finger tap (second finger lands while the first has not started dragging): right click at the first finger.
//!
//! Button-up events are delayed by [`CLICK_MS`] so games that poll the button state (GetAsyncKeyState,
//! DirectInput GetDeviceState) see the press. All events carry a due time; call [`TouchMapper::drain_due`].

use s3_runtime::{HostEvent, MouseButton};

pub const LONG_PRESS_MS: u64 = 500;

/// Width of the in-game control panel (minimap, menus) at the left edge, in game pixels (640x480: ~135 px).
pub const PANEL_W: i32 = 170;

/// Which mouse button a one-finger drag starting at `game` holds.
/// Menus run at 800x600 (left drag: scroll bars). In game (any other mode; default 640x480) a drag that starts on
/// the map holds the right button, which scrolls the map in S3; drags on the left panel stay left.
/// HYPOTHESIS: a user who sets the in-game resolution to 800x600 gets left drags on the map (arrow keys still scroll).
pub fn drag_button_for(game_w: u32, game_h: u32, game: (i32, i32)) -> MouseButton {
    let in_menu = game_w == 800 && game_h == 600;
    if !in_menu && game.0 >= PANEL_W { MouseButton::Right } else { MouseButton::Left }
}
pub const CLICK_MS: u64 = 60;

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    Idle,
    /// Finger down, not yet classified.
    Pending { id: i32, win: (f32, f32), game: (i32, i32), t0: u64, drag: MouseButton },
    /// `drag` button held, following the finger.
    Dragging { id: i32, drag: MouseButton, origin: (i32, i32) },
    /// Gesture finished (long press / two-finger tap fired); ignore until all fingers are up.
    Consumed,
}

/// Mouse position sent while dragging. A right drag scrolls the S3 map *in* the mouse direction (measured on the
/// emulator: +40 game px moved the view ~+53 px, +89 px moved it ~+187 px; the game seems to accelerate), so it is
/// mirrored around the start point and halved: the map then follows the finger roughly 1:1 ("grab" scrolling).
/// The frontend clamps the result to the game screen.
fn drag_pos(drag: MouseButton, origin: (i32, i32), cur: (i32, i32)) -> (i32, i32) {
    match drag {
        MouseButton::Right => (origin.0 - (cur.0 - origin.0) / 2, origin.1 - (cur.1 - origin.1) / 2),
        _ => cur,
    }
}

pub struct TouchMapper {
    state: State,
    /// Movement threshold in window pixels before a press becomes a drag.
    slop: f32,
    fingers: u32,
    queue: Vec<(u64, HostEvent)>,
    /// Last time an event was scheduled (keeps output ordered when delays overlap).
    last_due: u64,
}

impl TouchMapper {
    pub fn new(slop_px: f32) -> Self {
        TouchMapper { state: State::Idle, slop: slop_px, fingers: 0, queue: Vec::new(), last_due: 0 }
    }

    pub fn set_slop(&mut self, slop_px: f32) {
        self.slop = slop_px;
    }

    fn push(&mut self, t: u64, ev: HostEvent) {
        let t = t.max(self.last_due);
        self.last_due = t;
        self.queue.push((t, ev));
    }

    fn click(&mut self, t: u64, game: (i32, i32), button: MouseButton) {
        self.push(t, HostEvent::MouseMove { x: game.0, y: game.1 });
        self.push(t, HostEvent::MouseButton { button, down: true });
        self.push(t + CLICK_MS, HostEvent::MouseButton { button, down: false });
    }

    /// First finger (ACTION_DOWN).
    pub fn down(&mut self, id: i32, win: (f32, f32), game: (i32, i32), t: u64) {
        self.down_with_drag(id, win, game, t, MouseButton::Left)
    }

    /// First finger; a drag that starts here holds `drag` (Right = scroll the map in game).
    pub fn down_with_drag(&mut self, id: i32, win: (f32, f32), game: (i32, i32), t: u64, drag: MouseButton) {
        self.fingers = 1;
        self.state = State::Pending { id, win, game, t0: t, drag };
        // Hover the cursor there right away (menus highlight under the cursor).
        self.push(t, HostEvent::MouseMove { x: game.0, y: game.1 });
    }

    /// Additional finger (ACTION_POINTER_DOWN).
    pub fn pointer_down(&mut self, t: u64) {
        self.fingers += 1;
        if let State::Pending { game, .. } = self.state {
            self.click(t, game, MouseButton::Right);
            self.state = State::Consumed;
        }
    }

    /// A finger other than the last one lifted (ACTION_POINTER_UP).
    pub fn pointer_up(&mut self, id: i32, t: u64) {
        self.fingers = self.fingers.saturating_sub(1);
        if let State::Dragging { id: d, drag, .. } = self.state {
            if d == id {
                self.push(t, HostEvent::MouseButton { button: drag, down: false });
                self.state = State::Consumed;
            }
        }
    }

    /// Finger moved (ACTION_MOVE), for the pointer with `id`.
    pub fn moved(&mut self, id: i32, win: (f32, f32), game: (i32, i32), t: u64) {
        match self.state {
            State::Pending { id: p, win: w0, game: g0, drag, .. } if p == id => {
                let (dx, dy) = (win.0 - w0.0, win.1 - w0.1);
                if dx * dx + dy * dy > self.slop * self.slop {
                    self.push(t, HostEvent::MouseMove { x: g0.0, y: g0.1 });
                    self.push(t, HostEvent::MouseButton { button: drag, down: true });
                    let p = drag_pos(drag, g0, game);
                    self.push(t, HostEvent::MouseMove { x: p.0, y: p.1 });
                    self.state = State::Dragging { id, drag, origin: g0 };
                }
            }
            State::Dragging { id: d, drag, origin } if d == id => {
                let p = drag_pos(drag, origin, game);
                self.push(t, HostEvent::MouseMove { x: p.0, y: p.1 })
            }
            _ => {}
        }
    }

    /// Last finger lifted (ACTION_UP).
    pub fn up(&mut self, t: u64) {
        match self.state {
            State::Pending { game, .. } => self.click(t, game, MouseButton::Left),
            State::Dragging { drag, .. } => self.push(t, HostEvent::MouseButton { button: drag, down: false }),
            _ => {}
        }
        self.fingers = 0;
        self.state = State::Idle;
    }

    /// Gesture cancelled by the system (ACTION_CANCEL).
    pub fn cancel(&mut self, t: u64) {
        if let State::Dragging { drag, .. } = self.state {
            self.push(t, HostEvent::MouseButton { button: drag, down: false });
        }
        self.fingers = 0;
        self.state = State::Idle;
    }

    /// Time passes: fires the long press.
    pub fn tick(&mut self, t: u64) {
        if let State::Pending { game, t0, .. } = self.state {
            if t >= t0 + LONG_PRESS_MS {
                self.click(t, game, MouseButton::Right);
                self.state = State::Consumed;
            }
        }
    }

    /// Milliseconds until the next timer-driven event (long press or a delayed button-up), if any.
    pub fn next_deadline(&self, t: u64) -> Option<u64> {
        let mut d: Option<u64> = None;
        if let State::Pending { t0, .. } = self.state {
            d = Some((t0 + LONG_PRESS_MS).saturating_sub(t));
        }
        if let Some(q) = self.queue.iter().map(|e| e.0).min() {
            let w = q.saturating_sub(t);
            d = Some(d.map_or(w, |x| x.min(w)));
        }
        d
    }

    /// Events whose due time has come, in order.
    pub fn drain_due(&mut self, t: u64) -> Vec<HostEvent> {
        self.tick(t);
        let mut out = Vec::new();
        let mut rest = Vec::new();
        for (due, ev) in self.queue.drain(..) {
            if due <= t {
                out.push(ev);
            } else {
                rest.push((due, ev));
            }
        }
        self.queue = rest;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use HostEvent::MouseMove;

    const L: MouseButton = MouseButton::Left;
    const R: MouseButton = MouseButton::Right;

    #[test]
    fn tap_is_left_click_with_delayed_up() {
        let mut t = TouchMapper::new(20.0);
        t.down(0, (100.0, 100.0), (50, 50), 1000);
        t.up(1100);
        assert_eq!(
            t.drain_due(1100),
            vec![MouseMove { x: 50, y: 50 }, MouseMove { x: 50, y: 50 }, HostEvent::MouseButton { button: L, down: true }]
        );
        assert_eq!(t.drain_due(1100 + CLICK_MS), vec![HostEvent::MouseButton { button: L, down: false }]);
        assert!(t.drain_due(5000).is_empty());
    }

    #[test]
    fn long_press_is_right_click() {
        let mut t = TouchMapper::new(20.0);
        t.down(0, (100.0, 100.0), (50, 50), 0);
        t.moved(0, (105.0, 100.0), (52, 50), 100); // within slop
        t.drain_due(100);
        assert_eq!(t.next_deadline(100), Some(400));
        let ev = t.drain_due(500);
        assert!(ev.contains(&HostEvent::MouseButton { button: R, down: true }));
        assert!(!ev.iter().any(|e| matches!(e, HostEvent::MouseButton { button: MouseButton::Left, .. })));
        t.up(800);
        assert_eq!(t.drain_due(900), vec![HostEvent::MouseButton { button: R, down: false }]);
    }

    #[test]
    fn drag_holds_left() {
        let mut t = TouchMapper::new(20.0);
        t.down(0, (100.0, 100.0), (50, 50), 0);
        t.moved(0, (200.0, 100.0), (100, 50), 50);
        t.moved(0, (300.0, 100.0), (150, 50), 60);
        t.up(70);
        let ev = t.drain_due(70);
        assert_eq!(
            ev,
            vec![
                MouseMove { x: 50, y: 50 },
                MouseMove { x: 50, y: 50 },
                HostEvent::MouseButton { button: L, down: true },
                MouseMove { x: 100, y: 50 },
                MouseMove { x: 150, y: 50 },
                HostEvent::MouseButton { button: L, down: false },
            ]
        );
        // No long press after a drag.
        assert!(t.drain_due(2000).is_empty());
    }

    #[test]
    fn map_drag_holds_right() {
        let mut t = TouchMapper::new(20.0);
        t.down_with_drag(0, (100.0, 100.0), (50, 50), 0, R);
        t.moved(0, (200.0, 100.0), (100, 50), 50);
        t.up(70);
        let ev = t.drain_due(70);
        assert!(ev.contains(&MouseMove { x: 25, y: 50 }), "mirrored: {ev:?}");
        assert!(ev.contains(&HostEvent::MouseButton { button: R, down: true }));
        assert!(ev.contains(&HostEvent::MouseButton { button: R, down: false }));
        assert!(!ev.iter().any(|e| matches!(e, HostEvent::MouseButton { button: MouseButton::Left, .. })));
    }

    #[test]
    fn two_finger_tap_is_right_click() {
        let mut t = TouchMapper::new(20.0);
        t.down(0, (100.0, 100.0), (50, 50), 0);
        t.pointer_down(30);
        t.pointer_up(1, 80);
        t.up(90);
        let ev = t.drain_due(1000);
        assert!(ev.contains(&HostEvent::MouseButton { button: R, down: true }));
        assert!(ev.contains(&HostEvent::MouseButton { button: R, down: false }));
        assert!(!ev.iter().any(|e| matches!(e, HostEvent::MouseButton { button: MouseButton::Left, .. })));
    }
}
