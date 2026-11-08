//! Touch gesture recognizer (pure Rust, no machine access) shared by every frontend: frontends deliver raw
//! pointer events only ([`s3_runtime::HostEvent::Pointer`]); this turns them into [`TouchEvent`]s.
//!
//! Recognized (coordinates are game-screen pixels, `f32`):
//! - raw `Down` / `Move` / `Up` / `Cancel` per pointer id (always emitted first);
//! - `Tap` (one finger, < `tap_max_ms`, moved < `tap_slop`), plus `DoubleTap` when a second tap lands within
//!   `double_tap_ms` and `double_tap_slop` of the first (the second `Tap` is emitted too, so taps never wait);
//! - `LongPress` (one finger held >= `long_press_ms` without moving; fired from [`GestureRecognizer::tick`]);
//! - `Pan` with phases (one finger after it moved beyond the slop; `after_long_press` marks a long-press-drag;
//!   two fingers move together = `fingers: 2`);
//! - `Pinch` with phases (two fingers; `scale` = current distance / start distance);
//! - `TwoFingerTap` / `ThreeFingerTap` (all fingers lifted without moving, quickly).

/// Gesture phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Begin,
    Update,
    End,
}

/// High-level touch event delivered to mods (and then to the touch->mouse fallback).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TouchEvent {
    Down { id: u32, x: f32, y: f32 },
    Move { id: u32, x: f32, y: f32 },
    Up { id: u32, x: f32, y: f32 },
    Cancel { id: u32 },
    Tap { x: f32, y: f32 },
    DoubleTap { x: f32, y: f32 },
    LongPress { x: f32, y: f32 },
    TwoFingerTap { x: f32, y: f32 },
    ThreeFingerTap { x: f32, y: f32 },
    /// `x, y` current position (centroid for two fingers), `dx, dy` delta since the previous Pan event,
    /// `start_x, start_y` where the gesture started.
    Pan { phase: Phase, fingers: u8, x: f32, y: f32, dx: f32, dy: f32, start_x: f32, start_y: f32, after_long_press: bool },
    /// `cx, cy` centroid, `scale` relative to the distance when the pinch began.
    Pinch { phase: Phase, cx: f32, cy: f32, scale: f32 },
}

impl TouchEvent {
    /// Is this a raw per-pointer event (Down/Move/Up/Cancel)?
    pub fn is_raw(&self) -> bool {
        matches!(self, TouchEvent::Down { .. } | TouchEvent::Move { .. } | TouchEvent::Up { .. } | TouchEvent::Cancel { .. })
    }
}

/// Thresholds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GestureConfig {
    pub long_press_ms: u64,
    pub tap_max_ms: u64,
    pub tap_slop: f32,
    pub double_tap_ms: u64,
    pub double_tap_slop: f32,
    /// Max duration of a two/three-finger tap.
    pub multi_tap_max_ms: u64,
}

impl Default for GestureConfig {
    fn default() -> Self {
        GestureConfig { long_press_ms: crate::settings::LONG_PRESS_MS, tap_max_ms: 500, tap_slop: 8.0, double_tap_ms: 300, double_tap_slop: 24.0, multi_tap_max_ms: 500 }
    }
}

impl From<&crate::settings::TouchSettings> for GestureConfig {
    fn from(t: &crate::settings::TouchSettings) -> Self {
        GestureConfig {
            long_press_ms: t.long_press_ms,
            tap_max_ms: t.tap_max_ms,
            tap_slop: t.tap_slop,
            double_tap_ms: t.double_tap_ms,
            double_tap_slop: t.double_tap_slop,
            ..GestureConfig::default()
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Finger {
    id: u32,
    x: f32,
    y: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    Idle,
    Single { x0: f32, y0: f32, t0: u64, panning: bool, long_fired: bool, last: (f32, f32) },
    Multi { max: u8, t0: u64, c0: (f32, f32), d0: f32, transforming: bool, last_c: (f32, f32) },
    /// Gesture over (or cancelled); ignore until all fingers are up.
    Ended,
}

pub struct GestureRecognizer {
    pub cfg: GestureConfig,
    fingers: Vec<Finger>,
    mode: Mode,
    /// Time and place of the last single tap (for DoubleTap).
    last_tap: Option<(u64, f32, f32)>,
    /// The current one-finger touch went down quickly after a tap near it (tap, then press-and-drag): dragging
    /// it starts a box selection at once, like a drag after a long press (user request 2026-11-08).
    double_drag: bool,
}

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

impl GestureRecognizer {
    pub fn new(cfg: GestureConfig) -> Self {
        GestureRecognizer { cfg, fingers: Vec::new(), mode: Mode::Idle, last_tap: None, double_drag: false }
    }

    /// Number of fingers down.
    pub fn fingers(&self) -> usize {
        self.fingers.len()
    }

    fn centroid(&self) -> (f32, f32) {
        let n = self.fingers.len().max(1) as f32;
        let (sx, sy) = self.fingers.iter().fold((0.0, 0.0), |a, f| (a.0 + f.x, a.1 + f.y));
        (sx / n, sy / n)
    }

    fn spread(&self) -> f32 {
        if self.fingers.len() < 2 {
            return 0.0;
        }
        dist((self.fingers[0].x, self.fingers[0].y), (self.fingers[1].x, self.fingers[1].y))
    }

    /// End a running transform (pan/pinch) and push the End events.
    fn end_active(&mut self, out: &mut Vec<TouchEvent>) {
        match self.mode {
            Mode::Single { x0, y0, panning: true, long_fired, last, .. } => out.push(TouchEvent::Pan {
                phase: Phase::End,
                fingers: 1,
                x: last.0,
                y: last.1,
                dx: 0.0,
                dy: 0.0,
                start_x: x0,
                start_y: y0,
                after_long_press: long_fired,
            }),
            Mode::Multi { transforming: true, c0, last_c, .. } => {
                let scale = self.pinch_scale();
                out.push(TouchEvent::Pinch { phase: Phase::End, cx: last_c.0, cy: last_c.1, scale });
                out.push(TouchEvent::Pan {
                    phase: Phase::End,
                    fingers: 2,
                    x: last_c.0,
                    y: last_c.1,
                    dx: 0.0,
                    dy: 0.0,
                    start_x: c0.0,
                    start_y: c0.1,
                    after_long_press: false,
                });
            }
            _ => {}
        }
    }

    fn pinch_scale(&self) -> f32 {
        match self.mode {
            Mode::Multi { d0, .. } if d0 > 0.0 && self.fingers.len() >= 2 => self.spread() / d0,
            _ => 1.0,
        }
    }

    fn enter_multi(&mut self, t: u64, max: u8) {
        let t0 = match self.mode {
            Mode::Single { t0, .. } | Mode::Multi { t0, .. } => t0,
            _ => t,
        };
        let c = self.centroid();
        self.mode = Mode::Multi { max, t0, c0: c, d0: self.spread(), transforming: false, last_c: c };
    }

    /// A finger touched down.
    pub fn down(&mut self, id: u32, x: f32, y: f32, t: u64, out: &mut Vec<TouchEvent>) {
        self.fingers.retain(|f| f.id != id);
        self.fingers.push(Finger { id, x, y });
        out.push(TouchEvent::Down { id, x, y });
        match (self.mode, self.fingers.len()) {
            (Mode::Idle, 1) => {
                self.double_drag = self.last_tap.is_some_and(|(lt, lx, ly)| {
                    t.saturating_sub(lt) <= self.cfg.double_tap_ms && dist((lx, ly), (x, y)) <= self.cfg.double_tap_slop * 2.0
                });
                self.mode = Mode::Single { x0: x, y0: y, t0: t, panning: false, long_fired: false, last: (x, y) }
            }
            (Mode::Single { panning, long_fired, .. }, 2) => {
                if panning || long_fired {
                    // A finger joined a running one-finger gesture: end it, then ignore the rest.
                    self.end_active(out);
                    self.mode = Mode::Ended;
                } else {
                    self.enter_multi(t, 2);
                }
            }
            (Mode::Multi { transforming: false, .. }, n) if n >= 2 => self.enter_multi(t, n.min(3) as u8),
            (Mode::Multi { transforming: true, .. }, _) => {
                self.end_active(out);
                self.mode = Mode::Ended;
            }
            (Mode::Ended, _) | (Mode::Idle, _) => {
                if self.mode == Mode::Idle {
                    self.mode = Mode::Ended;
                }
            }
            _ => {}
        }
    }

    /// A finger moved.
    pub fn moved(&mut self, id: u32, x: f32, y: f32, _t: u64, out: &mut Vec<TouchEvent>) {
        let Some(f) = self.fingers.iter_mut().find(|f| f.id == id) else { return };
        if f.x == x && f.y == y {
            return;
        }
        f.x = x;
        f.y = y;
        out.push(TouchEvent::Move { id, x, y });
        let slop = self.cfg.tap_slop;
        match self.mode {
            Mode::Single { x0, y0, t0, panning, long_fired, last } => {
                if !panning {
                    if dist((x, y), (x0, y0)) > slop {
                        // Tap, then press-and-drag: a box selection like after a long press.
                        let long_fired = if !long_fired && self.double_drag {
                            self.last_tap = None;
                            out.push(TouchEvent::LongPress { x: x0, y: y0 });
                            true
                        } else {
                            long_fired
                        };
                        out.push(TouchEvent::Pan {
                            phase: Phase::Begin,
                            fingers: 1,
                            x,
                            y,
                            dx: x - x0,
                            dy: y - y0,
                            start_x: x0,
                            start_y: y0,
                            after_long_press: long_fired,
                        });
                        self.mode = Mode::Single { x0, y0, t0, panning: true, long_fired, last: (x, y) };
                    }
                } else {
                    out.push(TouchEvent::Pan {
                        phase: Phase::Update,
                        fingers: 1,
                        x,
                        y,
                        dx: x - last.0,
                        dy: y - last.1,
                        start_x: x0,
                        start_y: y0,
                        after_long_press: long_fired,
                    });
                    self.mode = Mode::Single { x0, y0, t0, panning, long_fired, last: (x, y) };
                }
            }
            Mode::Multi { max: 2, t0, c0, d0, transforming, last_c } if self.fingers.len() == 2 => {
                let c = self.centroid();
                let d = self.spread();
                let phase = if transforming {
                    Phase::Update
                } else if dist(c, c0) > slop || (d - d0).abs() > slop {
                    Phase::Begin
                } else {
                    return;
                };
                let scale = if d0 > 0.0 { d / d0 } else { 1.0 };
                out.push(TouchEvent::Pinch { phase, cx: c.0, cy: c.1, scale });
                let (dx, dy) = if phase == Phase::Begin { (c.0 - c0.0, c.1 - c0.1) } else { (c.0 - last_c.0, c.1 - last_c.1) };
                out.push(TouchEvent::Pan { phase, fingers: 2, x: c.0, y: c.1, dx, dy, start_x: c0.0, start_y: c0.1, after_long_press: false });
                self.mode = Mode::Multi { max: 2, t0, c0, d0, transforming: true, last_c: c };
            }
            Mode::Multi { max: 3, c0, .. } => {
                // Three fingers moving: not a tap any more.
                if dist(self.centroid(), c0) > slop {
                    self.mode = Mode::Ended;
                }
            }
            _ => {}
        }
    }

    /// A finger lifted.
    pub fn up(&mut self, id: u32, x: f32, y: f32, t: u64, out: &mut Vec<TouchEvent>) {
        if !self.fingers.iter().any(|f| f.id == id) {
            return;
        }
        self.moved(id, x, y, t, out);
        out.push(TouchEvent::Up { id, x, y });
        match self.mode {
            Mode::Single { x0, y0, t0, panning, long_fired, .. } => {
                if panning {
                    self.end_active(out);
                } else if !long_fired && t.saturating_sub(t0) <= self.cfg.tap_max_ms {
                    out.push(TouchEvent::Tap { x: x0, y: y0 });
                    match self.last_tap {
                        Some((lt, lx, ly))
                            if t.saturating_sub(lt) <= self.cfg.double_tap_ms
                                && dist((lx, ly), (x0, y0)) <= self.cfg.double_tap_slop =>
                        {
                            out.push(TouchEvent::DoubleTap { x: x0, y: y0 });
                            self.last_tap = None;
                        }
                        _ => self.last_tap = Some((t, x0, y0)),
                    }
                }
                self.mode = Mode::Ended;
            }
            Mode::Multi { max, t0, transforming, .. } => {
                if transforming {
                    self.end_active(out);
                } else if t.saturating_sub(t0) <= self.cfg.multi_tap_max_ms {
                    let c = self.centroid();
                    out.push(if max >= 3 { TouchEvent::ThreeFingerTap { x: c.0, y: c.1 } } else { TouchEvent::TwoFingerTap { x: c.0, y: c.1 } });
                }
                self.mode = Mode::Ended;
            }
            _ => {}
        }
        self.fingers.retain(|f| f.id != id);
        if self.fingers.is_empty() {
            self.mode = Mode::Idle;
        }
    }

    /// The platform cancelled a pointer.
    pub fn cancel(&mut self, id: u32, out: &mut Vec<TouchEvent>) {
        if !self.fingers.iter().any(|f| f.id == id) {
            return;
        }
        out.push(TouchEvent::Cancel { id });
        self.end_active(out);
        self.mode = Mode::Ended;
        self.fingers.retain(|f| f.id != id);
        if self.fingers.is_empty() {
            self.mode = Mode::Idle;
        }
    }

    /// Time-based gestures (long press). Call regularly (each scheduler slice).
    pub fn tick(&mut self, t: u64, out: &mut Vec<TouchEvent>) {
        if let Mode::Single { x0, y0, t0, panning: false, long_fired: false, last } = self.mode {
            if t.saturating_sub(t0) >= self.cfg.long_press_ms {
                out.push(TouchEvent::LongPress { x: x0, y: y0 });
                self.mode = Mode::Single { x0, y0, t0, panning: false, long_fired: true, last };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec() -> GestureRecognizer {
        GestureRecognizer::new(GestureConfig::default())
    }

    fn high(v: &[TouchEvent]) -> Vec<TouchEvent> {
        v.iter().copied().filter(|e| !e.is_raw()).collect()
    }

    #[test]
    fn tap_then_drag_is_box_select() {
        let mut g = GestureRecognizer::new(GestureConfig::default());
        let mut o = Vec::new();
        g.down(1, 100.0, 100.0, 0, &mut o);
        g.up(1, 100.0, 100.0, 80, &mut o);
        o.clear();
        g.down(2, 104.0, 100.0, 200, &mut o);
        g.moved(2, 160.0, 140.0, 260, &mut o);
        assert!(o.iter().any(|e| matches!(e, TouchEvent::LongPress { .. })));
        assert!(o.iter().any(|e| matches!(e, TouchEvent::Pan { phase: Phase::Begin, after_long_press: true, .. })));
        // A plain drag (no tap before) stays a camera pan.
        let mut g = GestureRecognizer::new(GestureConfig::default());
        let mut o = Vec::new();
        g.down(1, 100.0, 100.0, 0, &mut o);
        g.moved(1, 160.0, 140.0, 60, &mut o);
        assert!(o.iter().any(|e| matches!(e, TouchEvent::Pan { phase: Phase::Begin, after_long_press: false, .. })));
    }

    #[test]
    fn tap_and_double_tap() {
        let mut g = rec();
        let mut o = Vec::new();
        g.down(1, 100.0, 100.0, 0, &mut o);
        g.tick(50, &mut o);
        g.up(1, 102.0, 101.0, 80, &mut o);
        assert_eq!(high(&o), vec![TouchEvent::Tap { x: 100.0, y: 100.0 }]);
        o.clear();
        g.down(2, 105.0, 100.0, 200, &mut o);
        g.up(2, 105.0, 100.0, 260, &mut o);
        assert_eq!(high(&o), vec![TouchEvent::Tap { x: 105.0, y: 100.0 }, TouchEvent::DoubleTap { x: 105.0, y: 100.0 }]);
        // Third tap starts a new pair.
        o.clear();
        g.down(3, 105.0, 100.0, 400, &mut o);
        g.up(3, 105.0, 100.0, 450, &mut o);
        assert_eq!(high(&o), vec![TouchEvent::Tap { x: 105.0, y: 100.0 }]);
        // Too slow for a tap.
        o.clear();
        g.down(4, 10.0, 10.0, 2000, &mut o);
        g.up(4, 10.0, 10.0, 2000 + 501, &mut o);
        assert!(high(&o).is_empty(), "{o:?}");
    }

    #[test]
    fn long_press_then_drag() {
        let mut g = rec();
        let mut o = Vec::new();
        g.down(1, 50.0, 60.0, 1000, &mut o);
        g.tick(1000 + crate::settings::LONG_PRESS_MS - 1, &mut o);
        assert!(high(&o).is_empty());
        g.tick(1000 + crate::settings::LONG_PRESS_MS, &mut o);
        g.tick(1600, &mut o);
        assert_eq!(high(&o), vec![TouchEvent::LongPress { x: 50.0, y: 60.0 }]);
        o.clear();
        g.moved(1, 80.0, 60.0, 1700, &mut o);
        g.moved(1, 90.0, 70.0, 1716, &mut o);
        g.up(1, 90.0, 70.0, 1730, &mut o);
        let h = high(&o);
        assert_eq!(h.len(), 3, "{h:?}");
        assert!(matches!(h[0], TouchEvent::Pan { phase: Phase::Begin, fingers: 1, after_long_press: true, dx, .. } if dx == 30.0));
        assert!(matches!(h[1], TouchEvent::Pan { phase: Phase::Update, dx, dy, .. } if dx == 10.0 && dy == 10.0));
        assert!(matches!(h[2], TouchEvent::Pan { phase: Phase::End, .. }));
    }

    #[test]
    fn pan_within_slop_is_tap() {
        let mut g = rec();
        let mut o = Vec::new();
        g.down(1, 0.0, 0.0, 0, &mut o);
        g.moved(1, 5.0, 5.0, 20, &mut o); // 7.07 < 8
        g.up(1, 5.0, 5.0, 40, &mut o);
        assert_eq!(high(&o), vec![TouchEvent::Tap { x: 0.0, y: 0.0 }]);
        // Raw events are all there.
        assert!(matches!(o[0], TouchEvent::Down { id: 1, .. }));
        assert!(matches!(o[1], TouchEvent::Move { id: 1, .. }));
    }

    #[test]
    fn pinch_and_two_finger_pan() {
        let mut g = rec();
        let mut o = Vec::new();
        g.down(1, 100.0, 100.0, 0, &mut o);
        g.down(2, 200.0, 100.0, 10, &mut o);
        g.moved(1, 50.0, 100.0, 30, &mut o);
        g.moved(2, 250.0, 100.0, 30, &mut o);
        g.up(1, 50.0, 100.0, 60, &mut o);
        g.up(2, 250.0, 100.0, 70, &mut o);
        let h = high(&o);
        let pinches: Vec<_> = h.iter().filter_map(|e| if let TouchEvent::Pinch { phase, scale, .. } = e { Some((*phase, *scale)) } else { None }).collect();
        assert_eq!(pinches.first().unwrap().0, Phase::Begin);
        assert_eq!(pinches.last().unwrap(), &(Phase::End, 2.0), "{pinches:?}");
        assert!(pinches.iter().any(|(_, s)| (*s - 2.0).abs() < 1e-4), "{pinches:?}");
        assert!(!h.iter().any(|e| matches!(e, TouchEvent::Tap { .. } | TouchEvent::TwoFingerTap { .. })));
        assert!(h.iter().any(|e| matches!(e, TouchEvent::Pan { fingers: 2, phase: Phase::End, .. })));
    }

    #[test]
    fn two_and_three_finger_taps() {
        let mut g = rec();
        let mut o = Vec::new();
        g.down(1, 100.0, 100.0, 0, &mut o);
        g.down(2, 140.0, 100.0, 20, &mut o);
        g.up(2, 140.0, 100.0, 100, &mut o);
        g.up(1, 100.0, 100.0, 110, &mut o);
        assert_eq!(high(&o), vec![TouchEvent::TwoFingerTap { x: 120.0, y: 100.0 }]);
        o.clear();
        g.down(1, 0.0, 0.0, 1000, &mut o);
        g.down(2, 30.0, 0.0, 1010, &mut o);
        g.down(3, 60.0, 0.0, 1020, &mut o);
        g.up(3, 60.0, 0.0, 1100, &mut o);
        g.up(2, 30.0, 0.0, 1100, &mut o);
        g.up(1, 0.0, 0.0, 1100, &mut o);
        assert_eq!(high(&o), vec![TouchEvent::ThreeFingerTap { x: 30.0, y: 0.0 }]);
        assert_eq!(g.fingers(), 0);
    }

    #[test]
    fn second_finger_ends_one_finger_pan_and_cancel() {
        let mut g = rec();
        let mut o = Vec::new();
        g.down(1, 0.0, 0.0, 0, &mut o);
        g.moved(1, 40.0, 0.0, 20, &mut o);
        g.down(2, 100.0, 0.0, 30, &mut o);
        g.moved(2, 150.0, 0.0, 40, &mut o);
        g.up(2, 150.0, 0.0, 50, &mut o);
        g.up(1, 40.0, 0.0, 60, &mut o);
        let h = high(&o);
        assert_eq!(h.len(), 2, "{h:?}");
        assert!(matches!(h[1], TouchEvent::Pan { phase: Phase::End, fingers: 1, .. }));
        o.clear();
        g.down(5, 0.0, 0.0, 100, &mut o);
        g.moved(5, 30.0, 0.0, 120, &mut o);
        g.cancel(5, &mut o);
        let h = high(&o);
        assert!(matches!(h.last(), Some(TouchEvent::Pan { phase: Phase::End, .. })));
        assert_eq!(g.fingers(), 0);
        // Next touch works normally.
        o.clear();
        g.down(6, 1.0, 1.0, 1000, &mut o);
        g.up(6, 1.0, 1.0, 1010, &mut o);
        assert_eq!(high(&o), vec![TouchEvent::Tap { x: 1.0, y: 1.0 }]);
    }
}
