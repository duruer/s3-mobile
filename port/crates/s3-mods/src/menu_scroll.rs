//! Finger scrolling of the game's menu lists (user report 2026-11-08: the multiplayer map list could only be
//! scrolled with its tiny scroll-bar arrows, hard to hit with a finger). A one-finger vertical drag that starts on a
//! known list clicks the list's own arrow buttons, one click per row of finger movement (finger up = later rows,
//! like a touch list), so the game scrolls exactly as with the mouse. Taps still select rows.

use s3_runtime::{Machine, MouseButton};

use crate::gesture::{Phase, TouchEvent};
use crate::{Mod, ModEvent, ModsState};

/// A scrollable list on an 800x600 menu screen (game pixels).
struct List {
    /// Menu layouts (name prefix) the list belongs to.
    layout: &'static str,
    /// List incl. its scroll bar: x, y, w, h.
    rect: (i32, i32, i32, i32),
    up: (i32, i32),
    down: (i32, i32),
    /// Row height.
    row: f32,
}

/// Measured on the Game Setup map picker (single player scenario and multiplayer host): list x 120..226,
/// scroll bar x 228..238 with arrows at y 250 / 435, rows 15 px.
const LISTS: &[List] = &[List { layout: "map-picker", rect: (118, 242, 124, 200), up: (233, 250), down: (233, 435), row: 15.0 }];

#[derive(Default)]
pub struct MenuScroll {
    /// Active drag: list index, accumulated finger movement (game px) not yet turned into clicks.
    drag: Option<(usize, f32)>,
}

fn layout_name(m: &mut Machine) -> Option<&'static str> {
    crate::draw_viewport(m).layout.map(|l| l.name)
}

impl Mod for MenuScroll {
    fn name(&self) -> &str {
        "menu-scroll"
    }

    fn on_touch(&mut self, m: &mut Machine, ev: &TouchEvent) -> ModEvent {
        let TouchEvent::Pan { phase, fingers: 1, y, dy, start_x, start_y, after_long_press: false, .. } = *ev else {
            return ModEvent::Pass;
        };
        match phase {
            Phase::Begin => {
                let Some(name) = layout_name(m) else { return ModEvent::Pass };
                let (sx, sy) = (start_x.round() as i32, start_y.round() as i32);
                let hit = LISTS.iter().position(|l| {
                    name.starts_with(l.layout)
                        && !name.contains('+')
                        && sx >= l.rect.0
                        && sx < l.rect.0 + l.rect.2
                        && sy >= l.rect.1
                        && sy < l.rect.1 + l.rect.3
                });
                let Some(i) = hit else { return ModEvent::Pass };
                self.drag = Some((i, y - start_y));
            }
            Phase::Update | Phase::End => {
                let Some((_, acc)) = self.drag.as_mut() else { return ModEvent::Pass };
                *acc += dy;
            }
        }
        let Some((i, mut acc)) = self.drag else { return ModEvent::Pass };
        let l = &LISTS[i];
        let now = m.real_ms();
        let mut clicks = Vec::new();
        while acc <= -l.row {
            clicks.push(l.down);
            acc += l.row;
        }
        while acc >= l.row {
            clicks.push(l.up);
            acc -= l.row;
        }
        {
            let st = m.state::<ModsState>();
            let mut s = st.borrow_mut();
            for p in clicks.into_iter().take(12) {
                s.fallback.click(now, p, MouseButton::Left);
            }
        }
        self.drag = if phase == Phase::End { None } else { Some((i, acc)) };
        ModEvent::Consumed
    }
}
