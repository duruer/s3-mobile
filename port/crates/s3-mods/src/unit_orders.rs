//! Unit orders for touch, shared by both UI modes (docs/porting/touch.md "Unit orders, paths, groups"; RE in
//! docs/reverse-engineering/game-state-units.md "Orders, waypoints, patrol" and "Control groups"). GOG S3.EXE only.
//!
//! - **Orders** the PC game gives with a modifier + right click on the map while units are selected:
//!   Shift = add a waypoint (up to three after the normal move), Alt = patrol between the current position and the
//!   target, Ctrl = move ignoring enemies on the way. [`arm`] makes the next map tap a right click with that key
//!   held (the touch->mouse fallback sends key down, click, key up), so the game builds its own command
//!   (0x0d / 0x0e / 0x0f) exactly like on a PC. "Waypoint" stays armed for the following taps (up to four: the
//!   first tap of an idle group is a normal move) until its button is tapped again.
//!   Mode B (Mobile HUD) shows the buttons in its selection card (`ui_hud.rs`), mode A (Classic Touch) gets a
//!   column below the Deselect ✕ drawn here.
//! - **Paths**: dotted, marching lines from each unit to its target and on through its queued waypoints (patrol
//!   routes as a loop), for the selected units, and for every own unit under a move order when the setting
//!   `show_unit_paths` is on. Read from the settler table (+0x24 target, +0x30..+0x3b waypoint queue), drawn in
//!   the window layer with the game's own map -> screen transform.
//! - **Control groups** (setting `group_shortcuts`): buttons 1..6. Tap = the game's "key n" (`0x542660`: select
//!   group n; tapping it again while it is selected centres the camera on it), long press = "Alt + n"
//!   (`0x542820`: assign the current selection to group n). Both are called on the logic thread at
//!   `HandleInput` entry, like the key handler does; non-empty groups are highlighted.

use std::sync::atomic::{AtomicU32, Ordering};

use s3_runtime::{FrameBuffer, HookFlow, LogLevel, Machine};

use crate::gesture::TouchEvent;
use crate::layer::Layer;
use crate::overlay::{measure_text, Rgba, Weight};
use crate::settings::UiMode;
use crate::touch_game as game;
use crate::{Mod, ModEvent, ModsState};

// ---------------------------------------------------------------- addresses (GOG exe only)

/// `CLogicThread::HandleInput` (thiscall logic), logic thread, every tick in game.
const HANDLE_INPUT: u32 = 0x0052_4fa0;
/// `CLogicThread::SelectGroup(n)` (thiscall logic, ret 4): digit key without Alt (`0x525cfa`).
pub const FN_SELECT_GROUP: u32 = 0x0054_2660;
/// `CLogicThread::AssignGroup(n)` (thiscall logic, ret 4): Alt + digit (`0x525cf0`).
pub const FN_ASSIGN_GROUP: u32 = 0x0054_2820;
const SEL_SLOT: u32 = 0x007d_fd68;
const SEL_COUNT: u32 = 0x007a_d620;
/// Resolution index 1..3 (`0x4c6d70` layout table).
const L_RES: u32 = 0x0073_0f08;
const GD_CAM_X: u32 = 0x2d254;
const GD_CAM_Y: u32 = 0x2d258;
const GD_PAGE: u32 = 0x2d284;
const GD_WORLD: u32 = 0x2d2d0;
const GD_LOCAL_PLAYER: u32 = 0x2d2d4;
const GD_TILES: u32 = 0x67444;
const GD_SETTLERS: u32 = 0x12a5e60;
const GD_SETTLERS_MAX: u32 = 0x12a5e5c;

/// Number of group buttons (the game has 1..9).
pub const GROUPS: u32 = 6;
/// A logic tick within this many ms means a game is running.
const IN_GAME_MS: u64 = 1500;
/// Settler job "walking on a player's order" (`+0x0e`; 0x37 = idle soldier). VERIFIED 2026-11-08.
const JOB_ORDERED_WALK: u16 = 0x35;

// ---------------------------------------------------------------- orders

/// A modifier order (PC: modifier + right click).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    /// Shift: queue a waypoint (command 0x0d).
    Waypoint,
    /// Alt: patrol between here and the target (command 0x0e).
    Patrol,
    /// Ctrl: move, ignoring enemies on the way (command 0x0f).
    NoFight,
}

impl Order {
    pub const ALL: [Order; 3] = [Order::Waypoint, Order::Patrol, Order::NoFight];

    /// Windows VK of the modifier the game reads (`0x45b370`: Shift 1, Alt 2, Ctrl 4 in the event's modifier byte).
    pub fn vk(self) -> u32 {
        match self {
            Order::Waypoint => 0x10,
            Order::Patrol => 0x12,
            Order::NoFight => 0x11,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Order::Waypoint => "Waypoint",
            Order::Patrol => "Patrol",
            Order::NoFight => "No fight",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Order::Waypoint => "Tap the next waypoint",
            Order::Patrol => "Tap the far end of the patrol",
            Order::NoFight => "Tap the target (no fighting on the way)",
        }
    }

    fn from_vk(vk: u32) -> Option<Order> {
        Order::ALL.into_iter().find(|o| o.vk() == vk)
    }
}

/// Taps "Waypoint" stays armed for: the first move of an idle group plus the game's three waypoints.
const WAYPOINT_TAPS: u32 = 4;

/// Arm `o` for the next map tap (or disarm if it is armed already). Returns whether it is armed now.
pub fn arm(m: &mut Machine, o: Order) -> bool {
    let on = armed(m) != Some(o);
    m.state::<OrdersShared>().borrow_mut().sticky = (on && o == Order::Waypoint).then_some(WAYPOINT_TAPS - 1);
    if on {
        crate::set_order_next(m, o.vk());
    } else {
        crate::set_right_next(m, false);
    }
    on
}

/// The order armed for the next map tap, if any.
pub fn armed(m: &mut Machine) -> Option<Order> {
    let (on, vk) = crate::order_next(m);
    if on {
        Order::from_vk(vk)
    } else {
        None
    }
}

/// Disarm (selection gone, UI mode changed).
pub fn disarm(m: &mut Machine) {
    if armed(m).is_some() {
        crate::set_right_next(m, false);
    }
    m.state::<OrdersShared>().borrow_mut().sticky = None;
}

// ---------------------------------------------------------------- shared state (hook <-> mod)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GroupAction {
    Select(u32),
    Assign(u32),
}

#[derive(Default)]
pub struct OrdersShared {
    last_tick_ms: u64,
    pending: Vec<GroupAction>,
    /// Remaining extra taps "Waypoint" is re-armed for.
    sticky: Option<u32>,
    hooked: bool,
}

/// Install the hook (GOG exe with its patch set) and the mod. Before the UI modes: they get touches first and
/// draw on top (HUD sheets cover the group buttons).
pub fn install(m: &mut Machine, gog: bool) {
    if gog {
        crate::hooks::add_hook(m, HANDLE_INPUT, "unit-orders:logic", |c| {
            let logic = c.reg(s3_cpu::ECX);
            on_logic_tick(c.m, logic);
            HookFlow::Continue
        });
        m.state::<OrdersShared>().borrow_mut().hooked = true;
    }
    crate::add_mod(m, Box::new(UnitOrders::default()));
}

fn on_logic_tick(m: &mut Machine, logic: u32) {
    let now = m.real_ms();
    let st = m.state::<OrdersShared>();
    let pending = {
        let mut s = st.borrow_mut();
        s.last_tick_ms = now;
        if s.pending.is_empty() {
            return;
        }
        std::mem::take(&mut s.pending)
    };
    for a in pending {
        match a {
            GroupAction::Select(n) => {
                crate::call::call_thiscall(m, FN_SELECT_GROUP, logic, &[n]);
            }
            GroupAction::Assign(n) => {
                // The game copies the current slot's ids; only with a unit selection (slot 0..9, count > 0).
                let slot = m.mem.read_i32(SEL_SLOT);
                if (0..10).contains(&slot) && m.mem.read_u16(SEL_COUNT + slot as u32 * 2) > 0 {
                    crate::call::call_thiscall(m, FN_ASSIGN_GROUP, logic, &[n]);
                }
            }
        }
        m.log(LogLevel::Debug, &format!("unit-orders: {a:?}"));
    }
}

/// Units in group `n` (1..9) as the game keeps them (`word [0x7ad620 + n*2]`; may include fallen soldiers).
pub fn group_count(m: &Machine, n: u32) -> u32 {
    if !(1..10).contains(&n) {
        return 0;
    }
    m.mem.read_u16(SEL_COUNT + n * 2) as u32
}

/// Current selection slot (-1 none, 0 plain selection, 1..9 a group).
pub fn current_slot(m: &Machine) -> i32 {
    m.mem.read_i32(SEL_SLOT)
}

// ---------------------------------------------------------------- paths (pure parts, unit tested)

/// One settler's order route in tiles.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Route {
    pub pos: (u16, u16),
    /// Target, then queued waypoints (in order).
    pub points: Vec<(u16, u16)>,
    /// Patrol between these two tiles (after reaching `points`).
    pub patrol: Option<((u16, u16), (u16, u16))>,
}

/// Waypoint slot value: empty.
const WP_EMPTY: u16 = 0xffff;
/// Third waypoint slot = this marks a patrol (`0x536c01`).
const WP_PATROL: u16 = 0xfffe;

/// Route from a settler entry (0x40 bytes). `force` = show even without a player's order (selected units).
pub fn route_from_entry(e: &[u8], world: u16, force: bool) -> Option<Route> {
    if e.len() < 0x40 {
        return None;
    }
    let w = |o: usize| u16::from_le_bytes([e[o], e[o + 1]]);
    let ok = |x: u16, y: u16| x < world && y < world && (x, y) != (0, 0);
    let pos = (w(4), w(6));
    if !ok(pos.0, pos.1) {
        return None;
    }
    let job = w(0x0e);
    let tgt = (w(0x24), w(0x26));
    let wp = [(w(0x30), w(0x32)), (w(0x34), w(0x36)), (w(0x38), w(0x3a))];
    let mut r = Route { pos, ..Default::default() };
    let ordered = job == JOB_ORDERED_WALK;
    if ordered || force {
        if ok(tgt.0, tgt.1) && tgt != pos {
            r.points.push(tgt);
        }
    }
    if wp[2] == (WP_PATROL, WP_PATROL) {
        if ok(wp[0].0, wp[0].1) && ok(wp[1].0, wp[1].1) {
            r.patrol = Some((wp[0], wp[1]));
        }
    } else if ordered || force {
        for &(x, y) in &wp {
            if x == WP_EMPTY || !ok(x, y) {
                break;
            }
            r.points.push((x, y));
        }
    }
    (!r.points.is_empty() || r.patrol.is_some()).then_some(r)
}

/// Map tile -> game screen pixel (the unit's feet), the inverse of `gd::ScreenToMap` (`0x4ff170`) for the
/// original layout: `sx = 16x - 8y - camX + 8W + 280 + 32 res`, `sy = 9y - h - camY + 182` (`h` = tile height,
/// `camX/Y` = `[gd+0x2d254/8]`, `W` = world size, `res` = `[0x730f08]`). VERIFIED against the hit list: exact for
/// 14 of 14 units (2026-11-08).
pub fn tile_to_screen(x: i32, y: i32, h: i32, cam: (i32, i32), world: i32, res: i32) -> (i32, i32) {
    (16 * x - 8 * y - cam.0 + 8 * world + 280 + 32 * res, 9 * y - h - cam.1 + 182)
}

/// Points every `step` px along a polyline, shifted by `phase` (0..step): the marching dots.
pub fn dots_along(pts: &[(f32, f32)], step: f32, phase: f32) -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    if step <= 0.5 {
        return out;
    }
    let mut carry = phase.rem_euclid(step);
    for s in pts.windows(2) {
        let (a, b) = (s[0], s[1]);
        let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        let mut t = carry;
        while t < len {
            let k = t / len;
            out.push((a.0 + (b.0 - a.0) * k, a.1 + (b.1 - a.1) * k));
            if out.len() > 4000 {
                return out;
            }
            t += step;
        }
        carry = t - len;
    }
    out
}

// ---------------------------------------------------------------- the mod

/// One group button touch: (pointer, group, down ms, long press fired).
#[derive(Debug, Clone, Copy)]
struct GroupPress {
    id: u32,
    n: u32,
    t0: u64,
    fired: bool,
}

#[derive(Default)]
pub struct UnitOrders {
    in_game: bool,
    now: u64,
    mode: UiMode,
    show_paths: bool,
    groups_on: bool,
    long_press_ms: u64,
    /// Units selected (page 5) right now.
    units_selected: bool,
    press: Option<GroupPress>,
    /// Order button pressed (pointer id, order).
    order_press: Option<(u32, Order)>,
    toast: Option<(String, u64)>,
    /// Map x offset of the hit list against [`tile_to_screen`] (the HUD's full-screen layout moves the map), from
    /// the last frame with units on screen.
    x_off: Option<i32>,
    dirty: bool,
    /// Last armed order seen (to notice the fallback taking it).
    last_armed: Option<Order>,
}

static DRAWN_PATHS: AtomicU32 = AtomicU32::new(0);

/// Routes drawn in the last frame (tests / debugging).
pub fn drawn_paths() -> u32 {
    DRAWN_PATHS.load(Ordering::Relaxed)
}

impl UnitOrders {
    fn q(&self, m: &mut Machine, v: f32) -> i32 {
        let vp = crate::draw_viewport(m);
        (v * vp.win_h as f32 / 600.0).round() as i32
    }

    /// Group buttons in window pixels.
    fn group_rects(&self, m: &mut Machine) -> Vec<(u32, (i32, i32, i32, i32))> {
        if !self.groups_on || !self.in_game {
            return Vec::new();
        }
        let vp = crate::draw_viewport(m);
        let (vx, vy, vw, vh) = vp.dst_rect_clipped();
        if vw < 160 || vh < 120 {
            return Vec::new();
        }
        let (x0, y, bw, bh, gap) = match self.mode {
            UiMode::MobileHud => {
                // Same scale as the HUD (`ui_hud::MobileHud::k`): row right above the bottom bar, right of the
                // minimap card; above the minimap card when the selection card would be in the way.
                let (bar, _) = crate::ui_hud::bar_and_strip_h(m, vw, vh);
                let k = bar as f32 / 54.0;
                let u = |v: f32| (v * k).round() as i32;
                let (bw, bh, gap) = (u(40.0), u(34.0), u(5.0));
                let row_w = GROUPS as i32 * (bw + gap);
                let mini_w = u(96.0) * 3 / 2 + u(10.0);
                let x = vx + u(6.0) + mini_w + u(8.0);
                let card_left = vx + vw - u(256.0);
                let bottom = vy + vh - bar - u(6.0);
                // Same place with and without a selection (the card appears with one).
                if x + row_w <= card_left {
                    (x, bottom - bh, bw, bh, gap)
                } else {
                    (vx + u(6.0), bottom - u(106.0) - u(6.0) - bh, bw, bh, gap)
                }
            }
            UiMode::ClassicTouch => {
                // Bottom of the map, right of the panel; the Menu / speed buttons sit bottom right.
                let q = |v: f32| (v * vp.win_h as f32 / 600.0).round() as i32;
                let (bw, bh, gap) = (q(46.0), q(40.0), q(8.0));
                let pw = game::layout(m).map_or(0, |l| l.panel_w);
                let gh = vp.game_h as f32;
                let x = vp.game_to_window(pw as f32, gh - 1.0).0.round() as i32 + q(10.0);
                (x.max(vx + q(8.0)), vy + vh - bh - q(14.0), bw, bh, gap)
            }
        };
        (1..=GROUPS).map(|n| (n, (x0 + (n as i32 - 1) * (bw + gap), y, bw, bh))).collect()
    }

    /// Order buttons of mode A (a column below the Deselect ✕, top right), window pixels.
    fn order_rects(&self, m: &mut Machine) -> Vec<(Order, (i32, i32, i32, i32))> {
        if !self.in_game || self.mode != UiMode::ClassicTouch || !self.units_selected {
            return Vec::new();
        }
        let vp = crate::draw_viewport(m);
        let u = vp.win_h as f32 / 600.0;
        let top = match crate::display::display_settings(m).fps_counter {
            crate::display::FpsCounter::Off => 0.0,
            crate::display::FpsCounter::Fps => 22.0,
            crate::display::FpsCounter::Detailed => 62.0,
        };
        let (cx, cy, cw, ch) = crate::touch_fx::cancel_rect(vp.dst_rect_clipped(), top, u);
        let q = |v: f32| (v * u).round() as i32;
        let (bw, bh) = (q(104.0), q(38.0));
        let right = cx + cw;
        // Below the ✕ and its "Deselect" caption.
        let mut y = cy + ch + q(26.0);
        let mut v = Vec::new();
        for o in Order::ALL {
            v.push((o, (right - bw, y, bw, bh)));
            y += bh + q(8.0);
        }
        v
    }

    fn group_at(&self, m: &mut Machine, x: f32, y: f32) -> Option<u32> {
        let e = self.q(m, 3.0);
        self.group_rects(m).into_iter().find_map(|(n, r)| inside(r, x, y, e).then_some(n))
    }

    fn order_at(&self, m: &mut Machine, x: f32, y: f32) -> Option<Order> {
        let e = self.q(m, 4.0);
        self.order_rects(m).into_iter().find_map(|(o, r)| inside(r, x, y, e).then_some(o))
    }

    fn set_toast(&mut self, t: String) {
        self.toast = Some((t, self.now + 1800));
        self.dirty = true;
    }

    fn queue(&mut self, m: &mut Machine, a: GroupAction) {
        m.state::<OrdersShared>().borrow_mut().pending.push(a);
    }

    fn refresh(&mut self, m: &mut Machine) {
        let (last, hooked) = {
            let st = m.state::<OrdersShared>();
            let s = st.borrow_mut();
            (s.last_tick_ms, s.hooked)
        };
        let (frame, settings) = {
            let st = m.state::<ModsState>();
            let s = st.borrow_mut();
            (s.frame_size, s.settings.clone())
        };
        self.mode = settings.ui_mode;
        self.show_paths = settings.show_unit_paths;
        self.groups_on = settings.group_shortcuts;
        self.long_press_ms = settings.touch.long_press_ms.max(150);
        let gd = game::gd(m);
        let lay = game::layout(m).filter(|l| (l.screen_w as u32, l.screen_h as u32) == frame);
        self.in_game = hooked && gd.is_some() && lay.is_some() && self.now.saturating_sub(last) < IN_GAME_MS;
        self.units_selected = self.in_game
            && gd.is_some_and(|g| m.mem.read_u32(g + GD_PAGE) == 5)
            && game::selection_count(m) > 0;
    }

    /// Routes to draw: selected units (always) plus every own unit walking to a target with `show_paths`.
    fn routes(&self, m: &Machine, gd: u32) -> Vec<(Route, bool)> {
        let world = m.mem.read_u32(gd + GD_WORLD).min(1024) as u16;
        let me = m.mem.read_u32(gd + GD_LOCAL_PLAYER) as u8;
        let max = m.mem.read_u32(gd + GD_SETTLERS_MAX).min(60_000);
        let sel = game::selection(m);
        let entry = |id: u32| m.mem.read_bytes(gd + GD_SETTLERS + id * 0x40, 0x40);
        let mut out = Vec::new();
        for &id in &sel {
            if (id as u32) == 0 || id as u32 > max {
                continue;
            }
            let e = entry(id as u32);
            if e[0] == me {
                if let Some(r) = route_from_entry(&e, world, true) {
                    out.push((r, true));
                }
            }
        }
        if self.show_paths {
            for id in 1..=max {
                let a = gd + GD_SETTLERS + id * 0x40;
                if m.mem.read_u8(a) != me || sel.contains(&(id as u16)) {
                    continue;
                }
                // Every own unit on its way somewhere (carriers and workers too, not only player orders; user report
                // 2026-11-08). Cheap pre-check before copying the entry: a target other than the position, or a
                // patrol marker.
                let (px, py, tx, ty) = (m.mem.read_u16(a + 4), m.mem.read_u16(a + 6), m.mem.read_u16(a + 0x24), m.mem.read_u16(a + 0x26));
                let moving = (tx, ty) != (px, py) && (tx, ty) != (0, 0) && tx < world && ty < world;
                if !moving && m.mem.read_u16(a + 0x38) != WP_PATROL {
                    continue;
                }
                if let Some(r) = route_from_entry(&entry(id), world, true) {
                    out.push((r, false));
                }
                if out.len() > 400 {
                    break;
                }
            }
        }
        out
    }

    fn draw_paths(&mut self, m: &mut Machine, c: &mut Layer) {
        DRAWN_PATHS.store(0, Ordering::Relaxed);
        let Some(gd) = game::gd(m) else { return };
        let Some(l) = game::layout(m) else { return };
        let routes = self.routes(m, gd);
        if routes.is_empty() {
            return;
        }
        let world = m.mem.read_u32(gd + GD_WORLD) as i32;
        let cam = (m.mem.read_u32(gd + GD_CAM_X) as i32, m.mem.read_u32(gd + GD_CAM_Y) as i32);
        let res = m.mem.read_u32(L_RES) as i32;
        // Calibrate x against the hit list (units on screen); the HUD's full-screen map shifts the view.
        let hits = game::hits(m, gd);
        let mut dxs: Vec<i32> = hits
            .iter()
            .filter(|h| !h.is_building())
            .filter_map(|h| {
                let a = gd + GD_SETTLERS + h.id as u32 * 0x40;
                let (tx, ty) = (m.mem.read_u16(a + 4) as i32, m.mem.read_u16(a + 6) as i32);
                (tx < world && ty < world).then(|| h.x - tile_to_screen(tx, ty, 0, cam, world, res).0)
            })
            .take(40)
            .collect();
        if !dxs.is_empty() {
            dxs.sort_unstable();
            self.x_off = Some(dxs[dxs.len() / 2]);
        }
        let x_off = self.x_off.unwrap_or_else(|| {
            if crate::ui_hud_game::full_layout_active(m) {
                -(crate::ui_hud_game::current_original_layout(m).map_or(0, |(pw, _)| pw as i32))
            } else {
                0
            }
        });
        let vp = crate::draw_viewport(m);
        let height = |x: u16, y: u16| m.mem.read_u8(gd + GD_TILES + (x as u32 * 768 + y as u32) * 0x18 + 4) as i32;
        let to_win = |p: (u16, u16)| {
            let (sx, sy) = tile_to_screen(p.0 as i32, p.1 as i32, height(p.0, p.1), cam, world, res);
            vp.game_to_window((sx + x_off) as f32, sy as f32)
        };
        let u = vp.win_h as f32 / 600.0;
        let mx = vp.game_to_window(l.panel_w as f32, 0.0).0.round() as i32;
        let (_, wy0, _, wh) = vp.dst_rect_clipped();
        c.set_clip(mx, wy0, c.width() as i32 - mx, wh);
        let step = 11.0 * u;
        let phase = (self.now % 1000) as f32 / 1000.0 * step;
        let mut ends: Vec<((u16, u16), bool, bool)> = Vec::new();
        for (r, sel) in &routes {
            let mut pts = vec![to_win(r.pos)];
            pts.extend(r.points.iter().map(|&p| to_win(p)));
            let (col, rad) = if *sel { (Rgba::rgba(255, 255, 255, 235), 2.4 * u) } else { (Rgba::rgba(235, 235, 200, 170), 1.8 * u) };
            for (x, y) in dots_along(&pts, step, phase) {
                c.fill_circle(x, y, rad + 1.2 * u, Rgba::rgba(0, 0, 0, 120));
                c.fill_circle(x, y, rad, col);
            }
            for (i, &p) in r.points.iter().enumerate() {
                ends.push((p, *sel, i + 1 == r.points.len() && r.patrol.is_none()));
            }
            if let Some((a, b)) = r.patrol {
                let (pa, pb) = (to_win(a), to_win(b));
                let pcol = Rgba::rgba(255, 196, 70, if *sel { 235 } else { 170 });
                let loop_pts = [pa, pb, pa];
                for (x, y) in dots_along(&loop_pts, step, phase) {
                    c.fill_circle(x, y, rad + 1.2 * u, Rgba::rgba(0, 0, 0, 120));
                    c.fill_circle(x, y, rad, pcol);
                }
                for p in [pa, pb] {
                    c.ring(p.0, p.1, 6.0 * u, 2.0 * u, pcol);
                }
            }
        }
        // Target / waypoint markers once per tile (a group's units share nearby tiles).
        ends.sort_by_key(|e| (e.0, !e.2));
        ends.dedup_by_key(|e| e.0);
        for (p, sel, last) in ends {
            let (x, y) = to_win(p);
            let a = if sel { 235 } else { 160 };
            if last {
                c.ring(x, y, 7.0 * u, 2.4 * u, Rgba::rgba(0, 0, 0, 120));
                c.ring(x, y, 6.0 * u, 2.0 * u, Rgba::rgba(120, 255, 120, a));
            } else {
                c.ring(x, y, 4.5 * u, 1.6 * u, Rgba::rgba(255, 255, 255, a));
            }
        }
        c.reset_clip();
        DRAWN_PATHS.store(routes.len() as u32, Ordering::Relaxed);
    }

    fn draw_groups(&mut self, m: &mut Machine, c: &mut Layer) {
        let rects = self.group_rects(m);
        if rects.is_empty() {
            return;
        }
        let vp = crate::draw_viewport(m);
        let u = vp.win_h as f32 / 600.0;
        let cur = current_slot(m);
        let gold = Rgba::rgb(236, 196, 92);
        for (n, (x, y, w, h)) in rects {
            let count = group_count(m, n);
            let pressed = self.press.is_some_and(|p| p.n == n);
            let selected = cur == n as i32 && count > 0;
            let bg = if pressed {
                gold.fade(0.85)
            } else if selected {
                Rgba::rgba(120, 90, 30, 230)
            } else if count > 0 {
                Rgba::rgba(30, 26, 18, 215)
            } else {
                Rgba::rgba(20, 16, 10, 140)
            };
            let r = 8.0 * u;
            c.fill_rounded_rect(x, y, w, h, r, bg);
            c.stroke_rounded_rect(x, y, w, h, r, if selected { 2.5 * u } else { 1.5 * u }, if count > 0 { gold } else { gold.fade(0.45) });
            let fg = if pressed { Rgba::BLACK } else if count > 0 { Rgba::WHITE } else { Rgba::rgba(255, 255, 255, 120) };
            let px = (h as f32 * 0.5).max(10.0);
            if count > 0 {
                c.text_centered(x, y - (h as f32 * 0.16) as i32, w, h, px * 0.85, Weight::Bold, fg, &n.to_string());
                let small = (h as f32 * 0.25).max(8.0);
                c.text_centered(x, y + (h as f32 * 0.42) as i32, w, h - (h as f32 * 0.5) as i32, small, Weight::Regular, fg, &count.to_string());
            } else {
                c.text_centered(x, y, w, h, px, Weight::Bold, fg, &n.to_string());
            }
            // Long-press progress.
            if let Some(p) = self.press.filter(|p| p.n == n && !p.fired) {
                let k = (self.now.saturating_sub(p.t0) as f32 / self.long_press_ms as f32).clamp(0.0, 1.0);
                c.fill_rect(x + (4.0 * u) as i32, y + h - (5.0 * u) as i32, ((w as f32 - 8.0 * u) * k) as i32, (3.0 * u).max(1.0) as i32, Rgba::BLACK);
            }
        }
    }

    fn draw_orders(&mut self, m: &mut Machine, c: &mut Layer) {
        let rects = self.order_rects(m);
        if rects.is_empty() {
            return;
        }
        let vp = crate::draw_viewport(m);
        let u = vp.win_h as f32 / 600.0;
        let armed = armed(m);
        let gold = Rgba::rgb(236, 196, 92);
        for (o, (x, y, w, h)) in rects {
            let on = armed == Some(o) || self.order_press.is_some_and(|p| p.1 == o);
            c.fill_rounded_rect(x, y, w, h, 9.0 * u, if on { gold.fade(0.9) } else { Rgba::rgba(20, 16, 10, 200) });
            c.stroke_rounded_rect(x, y, w, h, 9.0 * u, 2.0 * u, gold);
            c.text_centered(x, y, w, h, 16.0 * u, Weight::Bold, if on { Rgba::BLACK } else { Rgba::WHITE }, o.label());
        }
    }

    fn draw_hint(&mut self, m: &mut Machine, c: &mut Layer) {
        let vp = crate::draw_viewport(m);
        let u = vp.win_h as f32 / 600.0;
        let text = match (&self.toast, armed(m)) {
            (Some((t, until)), _) if self.now < *until => t.clone(),
            // Mode B shows its own toast for the order; mode A shows the hint while armed.
            (_, Some(o)) if self.mode == UiMode::ClassicTouch && self.units_selected => {
                let left = m.state::<OrdersShared>().borrow_mut().sticky.unwrap_or(0);
                if o == Order::Waypoint {
                    format!("{} ({} left)", o.hint(), left + 1)
                } else {
                    o.hint().to_string()
                }
            }
            _ => return,
        };
        let px = 15.0 * u;
        let (tw, th) = measure_text(px, Weight::Bold, &text);
        let (vx, vy, vw, vh) = vp.dst_rect_clipped();
        let (w, h) = (tw + (24.0 * u) as i32, th + (14.0 * u) as i32);
        let x = vx + (vw - w) / 2;
        let y = match self.group_rects(m).first() {
            Some(&(_, (_, gy, _, _))) => gy - h - (8.0 * u) as i32,
            None => vy + vh - h - (70.0 * u) as i32,
        };
        c.fill_rounded_rect(x, y, w, h, 8.0 * u, Rgba::rgba(20, 16, 10, 215));
        c.text(x + (12.0 * u) as i32, y + (7.0 * u) as i32, px, Weight::Bold, Rgba::rgb(236, 196, 92), &text);
    }
}

fn inside(r: (i32, i32, i32, i32), x: f32, y: f32, e: i32) -> bool {
    x >= (r.0 - e) as f32 && x < (r.0 + r.2 + e) as f32 && y >= (r.1 - e) as f32 && y < (r.1 + r.3 + e) as f32
}

impl Mod for UnitOrders {
    fn name(&self) -> &str {
        "unit-orders"
    }

    fn on_touch(&mut self, m: &mut Machine, ev: &TouchEvent) -> ModEvent {
        self.now = m.real_ms();
        match *ev {
            TouchEvent::Down { id, .. } => {
                self.refresh(m);
                if !self.in_game {
                    return ModEvent::Pass;
                }
                let (wx, wy) = crate::touch_window_pos(m, ev);
                if let Some(n) = self.group_at(m, wx, wy) {
                    self.press = Some(GroupPress { id, n, t0: self.now, fired: false });
                    self.dirty = true;
                    return ModEvent::Consumed;
                }
                if let Some(o) = self.order_at(m, wx, wy) {
                    self.order_press = Some((id, o));
                    self.dirty = true;
                    return ModEvent::Consumed;
                }
                ModEvent::Pass
            }
            TouchEvent::Move { id, .. } => {
                if self.press.is_some_and(|p| p.id == id) || self.order_press.is_some_and(|p| p.0 == id) {
                    return ModEvent::Consumed;
                }
                ModEvent::Pass
            }
            TouchEvent::Up { id, .. } => {
                let (wx, wy) = crate::touch_window_pos(m, ev);
                if let Some(p) = self.press.filter(|p| p.id == id) {
                    self.press = None;
                    self.dirty = true;
                    if !p.fired && self.group_at(m, wx, wy) == Some(p.n) {
                        if group_count(m, p.n) == 0 {
                            self.set_toast(format!("Group {} is empty: hold the button to assign the selection", p.n));
                        } else {
                            let again = current_slot(m) == p.n as i32;
                            self.queue(m, GroupAction::Select(p.n));
                            if again {
                                self.set_toast(format!("Group {}: camera centred", p.n));
                            }
                        }
                    }
                    return ModEvent::Consumed;
                }
                if let Some((pid, o)) = self.order_press.filter(|p| p.0 == id) {
                    self.order_press = None;
                    self.dirty = true;
                    let _ = pid;
                    if self.order_at(m, wx, wy) == Some(o) {
                        arm(m, o);
                    }
                    return ModEvent::Consumed;
                }
                ModEvent::Pass
            }
            TouchEvent::Cancel { id } => {
                if self.press.is_some_and(|p| p.id == id) {
                    self.press = None;
                }
                if self.order_press.is_some_and(|p| p.0 == id) {
                    self.order_press = None;
                }
                ModEvent::Pass
            }
            _ => ModEvent::Pass,
        }
    }

    fn tick(&mut self, m: &mut Machine, now: u64) {
        self.now = now;
        self.refresh(m);
        if !self.in_game {
            self.press = None;
            self.order_press = None;
            if self.last_armed.take().is_some() {
                disarm(m);
            }
            return;
        }
        // Long press on a group button = assign.
        if let Some(p) = self.press.as_mut() {
            if !p.fired && now.saturating_sub(p.t0) >= self.long_press_ms {
                p.fired = true;
                let n = p.n;
                let sel = game::selection_count(m);
                if self.units_selected && sel > 0 {
                    self.queue(m, GroupAction::Assign(n));
                    self.set_toast(format!("Group {n}: {sel} units assigned"));
                } else {
                    self.set_toast("Select units first, then hold a group button".to_string());
                }
            }
            self.dirty = true;
        }
        // Order taps: the fallback clears "right click next" when it sends the order; re-arm a waypoint run.
        let a = armed(m);
        if a.is_none() {
            if let Some(Order::Waypoint) = self.last_armed {
                let st = m.state::<OrdersShared>();
                let left = st.borrow_mut().sticky;
                if let Some(left) = left.filter(|&l| l > 0 && self.units_selected) {
                    st.borrow_mut().sticky = Some(left - 1);
                    crate::set_order_next(m, Order::Waypoint.vk());
                } else {
                    st.borrow_mut().sticky = None;
                }
            }
        }
        if !self.units_selected && a.is_some() {
            disarm(m);
        }
        let a = armed(m);
        if a != self.last_armed {
            self.dirty = true;
        }
        self.last_armed = a;
    }

    fn wants_draw(&self) -> bool {
        self.in_game
    }

    fn needs_redraw(&self) -> bool {
        self.dirty || self.press.is_some()
    }

    fn draw_window(&mut self, m: &mut Machine, c: &mut Layer, _frame: &FrameBuffer) {
        self.dirty = false;
        if !self.in_game {
            return;
        }
        self.draw_paths(m, c);
        self.draw_groups(m, c);
        self.draw_orders(m, c);
        self.draw_hint(m, c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(pos: (u16, u16), job: u16, tgt: (u16, u16), wp: [(u16, u16); 3]) -> Vec<u8> {
        let mut e = vec![0u8; 0x40];
        let mut put = |o: usize, v: u16| e[o..o + 2].copy_from_slice(&v.to_le_bytes());
        put(4, pos.0);
        put(6, pos.1);
        put(0x0e, job);
        put(0x24, tgt.0);
        put(0x26, tgt.1);
        for (i, (x, y)) in wp.into_iter().enumerate() {
            put(0x30 + i * 4, x);
            put(0x32 + i * 4, y);
        }
        e
    }

    const E: (u16, u16) = (0xffff, 0xffff);

    #[test]
    fn routes_from_runtime_dumps() {
        // Move + two Shift waypoints (s3run 2026-11-08, settler 0x35).
        let e = entry((264, 257), 0x35, (278, 248), [(293, 253), (298, 276), E]);
        let r = route_from_entry(&e, 384, false).unwrap();
        assert_eq!(r.points, vec![(278, 248), (293, 253), (298, 276)]);
        assert!(r.patrol.is_none());
        // Patrol (Alt): start, target, 0xfffe marker.
        let e = entry((226, 182), 0x35, (237, 159), [(223, 182), (237, 159), (0xfffe, 0xfffe)]);
        let r = route_from_entry(&e, 384, false).unwrap();
        assert_eq!(r.points, vec![(237, 159)]);
        assert_eq!(r.patrol, Some(((223, 182), (237, 159))));
        // Idle soldier: nothing, even when selected (target = position).
        let e = entry((235, 188), 0x37, (235, 188), [E, E, E]);
        assert!(route_from_entry(&e, 384, true).is_none());
        // Never commanded (zeros): nothing.
        let e = entry((264, 262), 0x37, (0, 0), [(0, 0), (0, 0), (0, 0)]);
        assert!(route_from_entry(&e, 384, true).is_none());
        // Walking without a player's order: only when selected.
        let e = entry((10, 10), 0x17, (20, 20), [E, E, E]);
        assert!(route_from_entry(&e, 384, false).is_none());
        assert_eq!(route_from_entry(&e, 384, true).unwrap().points, vec![(20, 20)]);
    }

    #[test]
    fn tile_to_screen_matches_hit_list() {
        // s3run 2026-11-08: cam (x 0x2d254, y 0x2d258), W 384, res 1; hit list (sx, sy) vs tile / height.
        let cam = (4928, 1166);
        // Unit 46 at tile (234, 185): hit (720, 630). Height unknown in that dump: check x only.
        assert_eq!(tile_to_screen(234, 185, 0, cam, 384, 1).0, 720);
        // Second run: unit 54 at (225, 246), h 68, dy + h = 182 with cam y from that run.
        let (_, sy) = tile_to_screen(225, 246, 68, (0, 0), 384, 1);
        assert_eq!(sy, 9 * 246 - 68 + 182);
    }

    #[test]
    fn dots_are_evenly_spaced() {
        let d = dots_along(&[(0.0, 0.0), (100.0, 0.0)], 10.0, 0.0);
        assert_eq!(d.len(), 10);
        assert!((d[1].0 - d[0].0 - 10.0).abs() < 1e-3);
        // Spacing continues over a corner.
        let d = dots_along(&[(0.0, 0.0), (15.0, 0.0), (15.0, 15.0)], 10.0, 0.0);
        assert_eq!(d.len(), 3);
        assert!((d[2].0 - 15.0).abs() < 1e-3 && (d[2].1 - 5.0).abs() < 1e-3);
        assert!(dots_along(&[(0.0, 0.0)], 10.0, 0.0).is_empty());
    }

    #[test]
    fn order_keys() {
        assert_eq!(Order::Waypoint.vk(), 0x10);
        assert_eq!(Order::Patrol.vk(), 0x12);
        assert_eq!(Order::NoFight.vk(), 0x11);
        assert_eq!(Order::from_vk(0x12), Some(Order::Patrol));
    }
}
