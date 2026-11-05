//! Touch input: Android MotionEvents -> raw [`HostEvent::Pointer`] events (docs/porting/android.md "Controls",
//! docs/porting/touch.md).
//!
//! The frontend only reports fingers. Gesture recognition (tap, long press, pans, pinch, multi-finger taps) and
//! what a gesture does in the game (context-aware touch, UI modes, touch->mouse fallback) live in `s3-mods`,
//! shared with the desktop `s3run --inject` touch commands, so both behave the same.

use s3_runtime::{HostEvent, PointerPhase};

/// MotionEvent action kinds we care about (android-activity `MotionAction`, platform independent for tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// First finger down (ACTION_DOWN).
    Down,
    /// Another finger down (ACTION_POINTER_DOWN).
    PointerDown,
    /// One or more fingers moved (ACTION_MOVE): every pointer of the event is reported.
    Move,
    /// A finger other than the last lifted (ACTION_POINTER_UP).
    PointerUp,
    /// Last finger lifted (ACTION_UP).
    Up,
    /// The system took the gesture (ACTION_CANCEL): every pointer is cancelled.
    Cancel,
}

/// Raw pointer events for one MotionEvent. `acting` = (pointer id, game position) of the pointer the action is
/// about (`pointer_index()`); `all` = every pointer of the event. Positions are game pixels.
pub fn pointer_events(action: Action, acting: (i32, (i32, i32)), all: &[(i32, (i32, i32))]) -> Vec<HostEvent> {
    let ev = |id: i32, phase, p: (i32, i32)| HostEvent::Pointer { id: id as u32, phase, x: p.0, y: p.1 };
    match action {
        Action::Down | Action::PointerDown => vec![ev(acting.0, PointerPhase::Down, acting.1)],
        Action::PointerUp | Action::Up => vec![ev(acting.0, PointerPhase::Up, acting.1)],
        Action::Move => all.iter().map(|&(id, p)| ev(id, PointerPhase::Move, p)).collect(),
        Action::Cancel => all.iter().map(|&(id, p)| ev(id, PointerPhase::Cancel, p)).collect(),
    }
}

/// Drops Move events that do not change a finger's position (MotionEvents batch all pointers).
#[derive(Default)]
pub struct PointerFilter {
    last: Vec<(u32, i32, i32)>,
}

impl PointerFilter {
    pub fn keep(&mut self, e: &HostEvent) -> bool {
        let HostEvent::Pointer { id, phase, x, y } = *e else { return true };
        let i = self.last.iter().position(|l| l.0 == id);
        match phase {
            PointerPhase::Move => {
                if let Some(i) = i {
                    if (self.last[i].1, self.last[i].2) == (x, y) {
                        return false;
                    }
                    self.last[i] = (id, x, y);
                }
                true
            }
            PointerPhase::Down => {
                match i {
                    Some(i) => self.last[i] = (id, x, y),
                    None => self.last.push((id, x, y)),
                }
                true
            }
            PointerPhase::Up | PointerPhase::Cancel => {
                if let Some(i) = i {
                    self.last.swap_remove(i);
                }
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use PointerPhase::*;

    fn p(id: u32, phase: PointerPhase, x: i32, y: i32) -> HostEvent {
        HostEvent::Pointer { id, phase, x, y }
    }

    #[test]
    fn motion_actions_map_to_pointer_phases() {
        assert_eq!(pointer_events(Action::Down, (0, (10, 20)), &[(0, (10, 20))]), vec![p(0, Down, 10, 20)]);
        let all = [(0, (10, 20)), (1, (50, 60))];
        assert_eq!(pointer_events(Action::PointerDown, (1, (50, 60)), &all), vec![p(1, Down, 50, 60)]);
        assert_eq!(pointer_events(Action::Move, (0, (10, 20)), &all), vec![p(0, Move, 10, 20), p(1, Move, 50, 60)]);
        assert_eq!(pointer_events(Action::PointerUp, (0, (11, 21)), &all), vec![p(0, Up, 11, 21)]);
        assert_eq!(pointer_events(Action::Up, (1, (50, 61)), &[(1, (50, 61))]), vec![p(1, Up, 50, 61)]);
        assert_eq!(pointer_events(Action::Cancel, (0, (0, 0)), &all), vec![p(0, Cancel, 10, 20), p(1, Cancel, 50, 60)]);
    }

    #[test]
    fn filter_drops_unchanged_moves() {
        let mut f = PointerFilter::default();
        assert!(f.keep(&p(0, Down, 1, 1)));
        assert!(f.keep(&p(1, Down, 5, 5)));
        assert!(!f.keep(&p(0, Move, 1, 1)));
        assert!(f.keep(&p(1, Move, 6, 5)));
        assert!(!f.keep(&p(1, Move, 6, 5)));
        assert!(f.keep(&p(1, Up, 6, 5)));
        assert!(f.keep(&HostEvent::MouseMove { x: 0, y: 0 }));
    }
}
