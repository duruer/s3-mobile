//! Pure decision logic for context-aware touch (W3, docs/porting/touch.md). No machine access, unit tested.
//!
//! [`View`] is a snapshot of the game state that matters for touch (built by `touch_game` from GOG S3.EXE
//! memory); [`decide_tap`] / [`decide_two_finger_tap`] turn a gesture into a [`TapAction`]. Camera panning with
//! inertia is [`CameraPan`]; pinch -> resolution switch is [`pinch_zoom_key`].

/// Screen layout of the running game (globals `0x730f0c..0x730f34`, docs/reverse-engineering/game-state-ui.md §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub screen_w: i32,
    pub screen_h: i32,
    /// x < panel_w is the left panel (or the minimap).
    pub panel_w: i32,
    /// Minimap origin x (sheared parallelogram), bottom y.
    pub mm_x: i32,
    pub mm_bottom: i32,
    /// Map view width/height (`screen - panel`).
    pub map_w: i32,
    pub map_h: i32,
    /// Resolution index + 1 (1 = 640x480, 2 = 800x600, 3 = 1024x768).
    pub res_index: i32,
}

impl Layout {
    pub const GOG_640: Layout =
        Layout { screen_w: 640, screen_h: 480, panel_w: 136, mm_x: 5, mm_bottom: 142, map_w: 504, map_h: 480, res_index: 1 };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    Panel,
    Minimap,
    Map,
}

impl Layout {
    /// Where a screen point is. The minimap is a parallelogram whose right edge leans right by
    /// `(mm_bottom - y)/2` (the game's own hit test, `0x52537b`).
    pub fn region(&self, x: i32, y: i32) -> Region {
        if y < self.mm_bottom {
            let d = (self.mm_bottom - y) / 2;
            if x < self.panel_w + d && x > self.mm_x + d {
                return Region::Minimap;
            }
        }
        if x < self.panel_w {
            Region::Panel
        } else {
            Region::Map
        }
    }
}

/// Interaction mode derived from the panel page and selections (game-state-ui.md §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    /// Page 1 with a building chosen: the next map click places a site.
    BuildPlacement,
    /// Page 4: a building is selected.
    BuildingSelected,
    /// Units in the UI selection.
    UnitsSelected,
    /// Page 9.
    Paused,
}

/// Something selectable drawn on screen (entry of the hit list `0x7ade08`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    /// 100 = building, 1..9 / 40 / 50.. = soldiers and other selectable settlers.
    pub kind: u16,
    /// Screen position (the unit's feet).
    pub x: i32,
    pub y: i32,
    pub id: u16,
    /// Owned by the local player.
    pub own: bool,
}

impl Hit {
    pub fn is_building(&self) -> bool {
        self.kind == 100
    }
}

/// Touch-relevant game state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct View {
    pub layout: Option<Layout>,
    pub mode: Option<Mode>,
    pub hits: Vec<Hit>,
    /// UI selection (settler ids).
    pub selected: Vec<u16>,
    /// Visible part of the frame and extra top margin (FPS counter) for the ✕ button (docs/porting/display.md:
    /// Fill mode crops the frame). `None` = whole frame.
    pub vis: Option<((i32, i32, i32, i32), i32)>,
}

/// What a touch does in game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TapAction {
    /// Left click at a point (select, place, panel button, minimap).
    Click { x: i32, y: i32 },
    /// Quick right click at a point with units selected: the game's move / attack command.
    Command { x: i32, y: i32, attack: bool },
    /// Quick right click (deselects a building).
    RightClick { x: i32, y: i32 },
    /// Leave build placement (slot := 0).
    CancelBuild,
    /// Clear the unit selection.
    Deselect,
    /// Ctrl + left click: select units of the same type around a unit.
    SelectSameType { x: i32, y: i32 },
    /// Do nothing (but the gesture is handled).
    Nothing,
}

/// Game hit box around a click (left-button-up handler `0x5339a1`): x ±10, feet `y-5 ..= y+32`.
pub const GAME_HIT: (i32, i32, i32) = (10, 5, 32);
/// Touch hit box (bigger fingers): x ±18, feet `y-12 ..= y+44`.
pub const TOUCH_HIT: (i32, i32, i32) = (18, 12, 44);
/// A click this far above a unit's feet is inside the game's own hit box.
pub const CLICK_ABOVE_FEET: i32 = 12;

/// Best hit-list entry under a finger at (x, y): own entries first, then nearest to the body centre.
pub fn find_hit(hits: &[Hit], x: i32, y: i32) -> Option<Hit> {
    let (hx, up, down) = TOUCH_HIT;
    hits.iter()
        .filter(|h| (h.x - x).abs() <= hx && h.y >= y - up && h.y <= y + down)
        .min_by_key(|h| {
            let (dx, dy) = (h.x - x, h.y - CLICK_ABOVE_FEET - 4 - y);
            (!h.own, dx * dx + dy * dy)
        })
        .copied()
}

/// Cancel button (✕) shown in build placement mode: top-right corner of the map view.
pub fn cancel_button(l: &Layout) -> (i32, i32, i32, i32) {
    cancel_button_in(l, None)
}

/// [`cancel_button`] inside the visible part of the frame (`View::vis`).
pub fn cancel_button_in(l: &Layout, vis: Option<((i32, i32, i32, i32), i32)>) -> (i32, i32, i32, i32) {
    let s = 40;
    match vis {
        Some(((vx, vy, vw, _), top)) => ((vx + vw).min(l.screen_w) - s - 8, vy + 8 + top, s, s),
        None => (l.screen_w - s - 8, 8, s, s),
    }
}

fn inside(r: (i32, i32, i32, i32), x: i32, y: i32) -> bool {
    x >= r.0 && y >= r.1 && x < r.0 + r.2 && y < r.1 + r.3
}

/// One-finger tap.
pub fn decide_tap(v: &View, x: i32, y: i32) -> TapAction {
    let (Some(l), Some(mode)) = (v.layout, v.mode) else { return TapAction::Click { x, y } };
    if mode == Mode::BuildPlacement && inside(cancel_button_in(&l, v.vis), x, y) {
        return TapAction::CancelBuild;
    }
    if l.region(x, y) != Region::Map {
        // Panel buttons / minimap: the game handles them (choosing another slot or tab also ends placement).
        return TapAction::Click { x, y };
    }
    match mode {
        Mode::Paused | Mode::BuildPlacement => TapAction::Click { x, y },
        _ => {
            let hit = find_hit(&v.hits, x, y);
            let at = |h: Hit| (h.x, h.y - CLICK_ABOVE_FEET);
            match hit {
                Some(h) if h.own => {
                    let (cx, cy) = at(h);
                    TapAction::Click { x: cx, y: cy }
                }
                Some(h) if mode == Mode::UnitsSelected => {
                    let (cx, cy) = at(h);
                    TapAction::Command { x: cx, y: cy, attack: true }
                }
                None if mode == Mode::UnitsSelected => TapAction::Command { x, y, attack: false },
                _ => TapAction::Click { x, y },
            }
        }
    }
}

/// Second tap of a double tap (the first two taps were already handled by [`decide_tap`]).
pub fn decide_double_tap(v: &View, x: i32, y: i32) -> TapAction {
    let (Some(l), Some(mode)) = (v.layout, v.mode) else { return TapAction::Nothing };
    if l.region(x, y) != Region::Map || matches!(mode, Mode::Paused | Mode::BuildPlacement) {
        return TapAction::Nothing;
    }
    match find_hit(&v.hits, x, y) {
        Some(h) if h.own && !h.is_building() => TapAction::SelectSameType { x: h.x, y: h.y - CLICK_ABOVE_FEET },
        _ => TapAction::Nothing,
    }
}

/// Two-finger tap = "cancel / back".
pub fn decide_two_finger_tap(v: &View, x: i32, y: i32) -> TapAction {
    match v.mode {
        Some(Mode::BuildPlacement) => TapAction::CancelBuild,
        Some(Mode::BuildingSelected) => {
            let l = v.layout.unwrap_or(Layout::GOG_640);
            // A right click on the map deselects the building (right-up handler, page 4).
            let (x, y) = if l.region(x, y) == Region::Map { (x, y) } else { (l.panel_w + l.map_w / 2, l.screen_h / 2) };
            TapAction::RightClick { x, y }
        }
        Some(Mode::UnitsSelected) => TapAction::Deselect,
        _ => TapAction::Nothing,
    }
}

/// What a one-finger drag starting at (x, y) does in game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragKind {
    /// Scroll the map with the finger.
    Camera,
    /// Left-button drag (panel controls, minimap).
    LeftDrag,
}

pub fn drag_kind(l: &Layout, x: i32, y: i32) -> DragKind {
    if l.region(x, y) == Region::Map {
        DragKind::Camera
    } else {
        DragKind::LeftDrag
    }
}

/// Pinch thresholds: closing the fingers to this scale zooms out one step (more map), opening zooms in.
pub const PINCH_OUT_SCALE: f32 = 0.75;
pub const PINCH_IN_SCALE: f32 = 1.33;

/// Key (F1/F2/F3 = VK 0x70..0x72) that switches the game resolution for a pinch of `scale`, if any.
/// Zoom levels are the game's resolutions: 640x480 (closest) .. 1024x768 (most map; the frontend scales the
/// frame to the screen).
pub fn pinch_zoom_key(scale: f32, res_index: i32) -> Option<u32> {
    let target = if scale <= PINCH_OUT_SCALE {
        res_index + 1
    } else if scale >= PINCH_IN_SCALE {
        res_index - 1
    } else {
        return None;
    };
    (1..=3).contains(&target).then(|| 0x70 + (target - 1) as u32)
}

/// Camera bounds (clamp `0x51b4d0`): `x <= 24*W + 0x167 - mapW`, `y <= 9*W + 0x176 - mapW` (sic: the game
/// uses the view width for both).
pub fn clamp_camera(x: f32, y: f32, world: i32, map_w: i32) -> (f32, f32) {
    let mx = (24 * world + 0x167 - map_w).max(0) as f32;
    let my = (9 * world + 0x176 - map_w).max(0) as f32;
    (x.clamp(0.0, mx), y.clamp(0.0, my))
}

/// Finger-driven camera with inertia (world pixels; pure, the caller writes the result to the game).
#[derive(Debug, Clone, Default)]
pub struct CameraPan {
    pub pos: (f32, f32),
    /// px/ms, camera direction.
    pub vel: (f32, f32),
    pub dragging: bool,
    last_t: u64,
    /// Recent (time, position) samples for the release velocity (several events can share a timestamp, e.g.
    /// both fingers of a two-finger pan, so the velocity is measured over a window, not per event).
    samples: Vec<(u64, (f32, f32))>,
}

/// Inertia decays with this time constant (ms).
pub const INERTIA_TAU_MS: f32 = 300.0;
/// Below this speed (px/ms) inertia stops.
pub const INERTIA_MIN: f32 = 0.02;
/// A release slower than this (px/ms) does not glide (a controlled drag stops where the finger lifts).
pub const FLING_MIN: f32 = 0.25;
/// Max fling speed (px/ms).
pub const INERTIA_MAX: f32 = 4.0;
/// Velocity window (ms).
pub const VELOCITY_WINDOW_MS: u64 = 80;

impl CameraPan {
    pub fn begin(&mut self, pos: (f32, f32), t: u64) {
        self.pos = pos;
        self.vel = (0.0, 0.0);
        self.dragging = true;
        self.last_t = t;
        self.samples.clear();
        self.samples.push((t, pos));
    }

    /// The finger moved by (dx, dy) screen pixels: the camera moves the other way (content follows the finger).
    pub fn drag(&mut self, dx: f32, dy: f32, t: u64) {
        self.pos.0 -= dx;
        self.pos.1 -= dy;
        self.last_t = t;
        self.samples.push((t, self.pos));
        let keep = t.saturating_sub(VELOCITY_WINDOW_MS * 2);
        if self.samples.len() > 2 && self.samples[0].0 < keep {
            self.samples.retain(|s| s.0 >= keep);
        }
    }

    /// The camera position was clamped (or changed) by the caller.
    pub fn set_pos(&mut self, p: (f32, f32)) {
        if let Some(s) = self.samples.last_mut() {
            s.1 = p;
        }
        self.pos = p;
    }

    /// Finger lifted: keep gliding if it was flung.
    pub fn release(&mut self, t: u64) {
        self.dragging = false;
        self.vel = (0.0, 0.0);
        self.last_t = t;
        let Some(&(t1, p1)) = self.samples.last() else { return };
        if t.saturating_sub(t1) > VELOCITY_WINDOW_MS {
            return; // the finger rested before lifting
        }
        // Position at the start of the window (last sample at or before it, else the oldest one).
        let from = t1.saturating_sub(VELOCITY_WINDOW_MS);
        let Some(&(t0, p0)) = self.samples.iter().rev().find(|s| s.0 <= from).or(self.samples.first()) else { return };
        if t0 >= t1 {
            return;
        }
        let dt = (t1 - t0).max(8) as f32;
        let (mut vx, mut vy) = ((p1.0 - p0.0) / dt, (p1.1 - p0.1) / dt);
        let s = (vx * vx + vy * vy).sqrt();
        if s < FLING_MIN {
            return;
        }
        if s > INERTIA_MAX {
            vx *= INERTIA_MAX / s;
            vy *= INERTIA_MAX / s;
        }
        self.vel = (vx, vy);
    }

    pub fn gliding(&self) -> bool {
        !self.dragging && (self.vel.0 != 0.0 || self.vel.1 != 0.0)
    }

    /// Stop gliding (the game moved the camera itself, a new touch...).
    pub fn stop(&mut self) {
        self.vel = (0.0, 0.0);
        self.dragging = false;
    }

    /// Advance inertia to `t`; returns the new position while gliding.
    pub fn step(&mut self, t: u64) -> Option<(f32, f32)> {
        if !self.gliding() {
            return None;
        }
        let dt = t.saturating_sub(self.last_t) as f32;
        if dt <= 0.0 {
            return None;
        }
        self.last_t = t;
        // Exact integral of v*e^(-t/tau) over dt.
        let decay = (-dt / INERTIA_TAU_MS).exp();
        let f = INERTIA_TAU_MS * (1.0 - decay);
        self.pos.0 += self.vel.0 * f;
        self.pos.1 += self.vel.1 * f;
        self.vel = (self.vel.0 * decay, self.vel.1 * decay);
        if (self.vel.0.powi(2) + self.vel.1.powi(2)).sqrt() < INERTIA_MIN {
            self.vel = (0.0, 0.0);
        }
        Some(self.pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(mode: Mode, hits: Vec<Hit>) -> View {
        View { layout: Some(Layout::GOG_640), mode: Some(mode), hits, selected: vec![], vis: None }
    }
    const OWN_SOLDIER: Hit = Hit { kind: 1, x: 300, y: 300, id: 41, own: true };
    const ENEMY: Hit = Hit { kind: 2, x: 500, y: 200, id: 90, own: false };
    const OWN_TOWER: Hit = Hit { kind: 100, x: 400, y: 150, id: 1, own: true };

    #[test]
    fn regions_640() {
        let l = Layout::GOG_640;
        assert_eq!(l.region(50, 300), Region::Panel);
        assert_eq!(l.region(60, 60), Region::Minimap);
        // Sheared right edge of the minimap: at y=10 it reaches x = 136 + 66.
        assert_eq!(l.region(190, 10), Region::Minimap);
        assert_eq!(l.region(190, 140), Region::Map);
        assert_eq!(l.region(400, 300), Region::Map);
    }

    #[test]
    fn tap_without_game_state_is_a_click() {
        assert_eq!(decide_tap(&View::default(), 10, 20), TapAction::Click { x: 10, y: 20 });
    }

    #[test]
    fn tap_on_own_unit_selects_inside_game_hit_box() {
        let v = view(Mode::Normal, vec![OWN_SOLDIER, ENEMY]);
        // Finger 15 px left of and 30 px above the feet: outside the game's own box, inside ours.
        let a = decide_tap(&v, 285, 270);
        assert_eq!(a, TapAction::Click { x: 300, y: 288 });
        let TapAction::Click { x, y } = a else { unreachable!() };
        let (hx, up, down) = GAME_HIT;
        assert!((OWN_SOLDIER.x - x).abs() <= hx && OWN_SOLDIER.y >= y - up && OWN_SOLDIER.y <= y + down);
    }

    #[test]
    fn units_selected_tap_ground_moves_tap_enemy_attacks_tap_own_selects() {
        let v = view(Mode::UnitsSelected, vec![OWN_SOLDIER, ENEMY, OWN_TOWER]);
        assert_eq!(decide_tap(&v, 450, 400), TapAction::Command { x: 450, y: 400, attack: false });
        assert_eq!(decide_tap(&v, 505, 180), TapAction::Command { x: 500, y: 188, attack: true });
        assert_eq!(decide_tap(&v, 300, 290), TapAction::Click { x: 300, y: 288 });
        assert_eq!(decide_tap(&v, 400, 130), TapAction::Click { x: 400, y: 138 });
        // Panel and minimap stay plain clicks.
        assert_eq!(decide_tap(&v, 40, 300), TapAction::Click { x: 40, y: 300 });
        assert_eq!(decide_tap(&v, 60, 60), TapAction::Click { x: 60, y: 60 });
    }

    #[test]
    fn own_unit_wins_over_nearer_enemy() {
        let near_enemy = Hit { kind: 1, x: 302, y: 300, id: 7, own: false };
        let v = view(Mode::UnitsSelected, vec![near_enemy, Hit { x: 315, ..OWN_SOLDIER }]);
        assert_eq!(decide_tap(&v, 302, 285), TapAction::Click { x: 315, y: 288 });
    }

    #[test]
    fn nothing_selected_tap_on_enemy_or_ground_is_click() {
        let v = view(Mode::Normal, vec![ENEMY]);
        assert_eq!(decide_tap(&v, 500, 190), TapAction::Click { x: 500, y: 190 });
        assert_eq!(decide_tap(&v, 450, 400), TapAction::Click { x: 450, y: 400 });
    }

    #[test]
    fn full_screen_hud_layout_is_all_map() {
        // UI mode B patches the panel width to 0 (ui-hud.md): every point is map (the HUD handles the minimap).
        let l = Layout { panel_w: 0, map_w: 640, ..Layout::GOG_640 };
        for (x, y) in [(0, 0), (5, 100), (60, 60), (135, 300), (639, 479)] {
            assert_eq!(l.region(x, y), Region::Map, "({x},{y})");
        }
    }

    #[test]
    fn build_placement_tap_places_cancel_button_and_two_finger_tap_cancel() {
        let v = view(Mode::BuildPlacement, vec![OWN_SOLDIER]);
        // Even on a unit, a tap places (the game decides whether the site is valid).
        assert_eq!(decide_tap(&v, 300, 290), TapAction::Click { x: 300, y: 290 });
        let (bx, by, bw, bh) = cancel_button(&Layout::GOG_640);
        assert_eq!(decide_tap(&v, bx + bw / 2, by + bh / 2), TapAction::CancelBuild);
        assert_eq!(decide_two_finger_tap(&v, 300, 300), TapAction::CancelBuild);
        // The ✕ only exists in placement mode.
        let n = view(Mode::Normal, vec![]);
        assert_eq!(decide_tap(&n, bx + 5, by + 5), TapAction::Click { x: bx + 5, y: by + 5 });
    }

    #[test]
    fn two_finger_tap_backs_out() {
        assert_eq!(decide_two_finger_tap(&view(Mode::UnitsSelected, vec![]), 300, 300), TapAction::Deselect);
        assert_eq!(decide_two_finger_tap(&view(Mode::BuildingSelected, vec![]), 300, 300), TapAction::RightClick { x: 300, y: 300 });
        assert_eq!(decide_two_finger_tap(&view(Mode::BuildingSelected, vec![]), 30, 300), TapAction::RightClick { x: 388, y: 240 });
        assert_eq!(decide_two_finger_tap(&view(Mode::Normal, vec![]), 300, 300), TapAction::Nothing);
        assert_eq!(decide_two_finger_tap(&View::default(), 300, 300), TapAction::Nothing);
    }

    #[test]
    fn double_tap_on_own_soldier_selects_same_type() {
        let v = view(Mode::UnitsSelected, vec![OWN_SOLDIER, OWN_TOWER, ENEMY]);
        assert_eq!(decide_double_tap(&v, 300, 290), TapAction::SelectSameType { x: 300, y: 288 });
        assert_eq!(decide_double_tap(&v, 400, 140), TapAction::Nothing);
        assert_eq!(decide_double_tap(&v, 500, 190), TapAction::Nothing);
    }

    #[test]
    fn drag_kinds() {
        let l = Layout::GOG_640;
        assert_eq!(drag_kind(&l, 400, 300), DragKind::Camera);
        assert_eq!(drag_kind(&l, 40, 300), DragKind::LeftDrag);
        assert_eq!(drag_kind(&l, 60, 60), DragKind::LeftDrag);
    }

    #[test]
    fn pinch_zoom_steps() {
        assert_eq!(pinch_zoom_key(0.6, 1), Some(0x71)); // 640 -> 800
        assert_eq!(pinch_zoom_key(0.6, 2), Some(0x72)); // 800 -> 1024
        assert_eq!(pinch_zoom_key(0.6, 3), None);
        assert_eq!(pinch_zoom_key(1.5, 3), Some(0x71));
        assert_eq!(pinch_zoom_key(1.5, 1), None);
        assert_eq!(pinch_zoom_key(1.0, 2), None);
    }

    #[test]
    fn camera_clamp() {
        assert_eq!(clamp_camera(-5.0, -1.0, 384, 504), (0.0, 0.0));
        assert_eq!(clamp_camera(1e6, 1e6, 384, 504), ((24 * 384 + 0x167 - 504) as f32, (9 * 384 + 0x176 - 504) as f32));
        assert_eq!(clamp_camera(100.0, 200.0, 384, 504), (100.0, 200.0));
    }

    #[test]
    fn camera_follows_finger_and_glides() {
        let mut c = CameraPan::default();
        c.begin((1000.0, 500.0), 0);
        for i in 1..=10 {
            c.drag(5.0, -2.0, i * 16);
        }
        assert_eq!(c.pos, (950.0, 520.0));
        c.release(165);
        assert!(c.gliding());
        let mut last = c.pos;
        let mut t = 165;
        while c.gliding() && t < 10_000 {
            t += 16;
            let p = c.step(t).unwrap();
            assert!(p.0 < last.0 && p.1 > last.1, "keeps moving the same way");
            last = p;
        }
        assert!(!c.gliding(), "inertia stops");
        // Total glide ~ v*tau = (5/16)*300 ~ 94 px.
        assert!((950.0 - last.0) > 60.0 && (950.0 - last.0) < 100.0, "{last:?}");
    }

    #[test]
    fn two_finger_samples_with_equal_timestamps_give_the_real_speed() {
        // Each finger's move arrives as its own half-step Pan update at the same time.
        let mut c = CameraPan::default();
        c.begin((0.0, 0.0), 0);
        for i in 1..=10u64 {
            c.drag(2.5, 0.0, i * 16);
            c.drag(2.5, 0.0, i * 16);
        }
        c.release(161);
        let v = c.vel.0;
        assert!((v + 5.0 / 16.0).abs() < 0.02, "{v}");
        // A slow drag does not glide.
        let mut c = CameraPan::default();
        c.begin((0.0, 0.0), 0);
        for i in 1..=10u64 {
            c.drag(1.0, 0.0, i * 16);
        }
        c.release(161);
        assert!(!c.gliding());
    }

    #[test]
    fn camera_no_glide_after_resting_finger() {
        let mut c = CameraPan::default();
        c.begin((0.0, 0.0), 0);
        c.drag(10.0, 0.0, 16);
        c.release(300);
        assert!(!c.gliding());
        assert_eq!(c.step(400), None);
    }
}
