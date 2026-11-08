//! Context-aware touch for the GOG S3.EXE (W3, docs/porting/touch.md), shared by both UI modes.
//!
//! A [`Mod`] that sits just above the touch->mouse fallback. In game it turns gestures into what a player means:
//!
//! | Gesture | In game |
//! | --- | --- |
//! | tap own unit / building | select (left click, enlarged hit box) |
//! | tap ground / enemy with units selected | move / attack (quick right click = the game's own command 9 path) |
//! | tap in build placement | place; tap ✕ / two-finger tap = cancel |
//! | tap panel / minimap | left click |
//! | double tap own soldier | select the same type nearby (Ctrl + click) |
//! | two-finger tap | cancel placement, deselect building / units |
//! | one- or two-finger drag on the map | scroll the camera with the finger, with inertia |
//! | long press, then drag | box selection (left drag) |
//! | long press | hover (tooltip); never a right click |
//! | pinch | zoom = the game's resolution (640x480 / 800x600 / 1024x768, F1..F3) |
//!
//! Outside a game (menus) only long press changes (hover instead of a right click) and long-press drags become
//! left drags; everything else goes to the fallback mapping. The in-game test is a hook on
//! `CLogicThread::HandleInput` (runs every logic tick) plus the layout globals matching the presented frame.

use s3_runtime::{FrameBuffer, HookFlow, HostEvent, Machine, MouseButton};

use crate::fallback::CLICK_MS;
use crate::gesture::{Phase, TouchEvent};
use crate::layer::Layer;
use crate::overlay::Rgba;
use crate::touch_fx::{self as fx, Fx};
use crate::touch_game as game;
use crate::touch_logic::{self as logic, CameraPan, DragKind, Layout, Mode, TapAction, View};
use crate::{Mod, ModEvent, ModsState};

/// Updated by the `HandleInput` hook.
#[derive(Default)]
pub struct TouchGame {
    pub last_logic_tick_ms: u64,
    pub ticks: u64,
}

/// A logic tick within this many ms means a game is running.
const IN_GAME_MS: u64 = 1500;
/// Long-press progress ring starts after this many ms.
const PRESS_SHOW_MS: u64 = 150;

/// Install the context-aware touch mod (GOG exe only; called by [`crate::install`]).
pub fn install(m: &mut Machine) -> Result<(), String> {
    if m.mem.read_bytes(game::HANDLE_INPUT, 4).iter().all(|&b| b == 0) {
        return Err(format!("touch-context: no code at {:#x}", game::HANDLE_INPUT));
    }
    crate::hooks::add_hook(m, game::HANDLE_INPUT, "touch-context:logic-tick", |c| {
        let logic = c.reg(s3_cpu::ECX);
        game::note_logic(c.m, logic);
        let now = c.m.real_ms();
        let st = c.m.state::<TouchGame>();
        let mut g = st.borrow_mut();
        g.last_logic_tick_ms = now;
        g.ticks += 1;
        HookFlow::Continue
    });
    crate::add_mod(m, Box::new(ContextTouch::default()));
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Drag {
    Camera,
    Left,
    /// Finger dragged over the minimap: the game reacts to a button press there (jump), not to moves with the
    /// button held, so the drag re-presses at the finger every [`MINIMAP_REPRESS_MS`] (last press time).
    Minimap(u64),
    /// Box selection from `a` to `b` (screen).
    Box { a: (f32, f32), b: (f32, f32) },
}

#[derive(Default)]
pub struct ContextTouch {
    now: u64,
    in_game: bool,
    /// Light view (layout, mode, selection; no hit list) refreshed every tick.
    view: View,
    fx: Vec<Fx>,
    cam: CameraPan,
    /// Camera value we last wrote (to notice the game moving the camera itself).
    cam_written: Option<(i32, i32)>,
    drag: Option<Drag>,
    /// Finger down on the map, waiting for a long press: (x, y, t0).
    press: Option<(f32, f32, u64)>,
    long_press_ms: u64,
    pinch_done: bool,
    /// Zoom = render resolution levels (res-zoom); used instead of F1-F3 when the custom resolution is on.
    zoom: crate::zoom::Pinch,
    draw_wanted: bool,
}

fn with_fallback<R>(m: &mut Machine, f: impl FnOnce(&mut crate::fallback::TouchMouse) -> R) -> R {
    let st = m.state::<ModsState>();
    let mut s = st.borrow_mut();
    f(&mut s.fallback)
}

fn click(m: &mut Machine, t: u64, x: i32, y: i32, b: MouseButton) {
    with_fallback(m, |f| f.click(t, (x, y), b));
}

fn key(m: &mut Machine, t: u64, vk: u32) {
    with_fallback(m, |f| {
        f.push(t, HostEvent::Key { vk, down: true });
        f.push(t + CLICK_MS, HostEvent::Key { vk, down: false });
    });
}

fn mouse(m: &mut Machine, t: u64, e: HostEvent) {
    with_fallback(m, |f| f.push(t, e));
}

/// Minimap drag: re-press interval (the game jumps on a press there, see [`Drag::Minimap`]).
const MINIMAP_REPRESS_MS: u64 = 50;

fn pt(x: f32, y: f32) -> (i32, i32) {
    (x.round() as i32, y.round() as i32)
}

impl ContextTouch {
    fn refresh(&mut self, m: &mut Machine) {
        let recent = {
            let st = m.state::<TouchGame>();
            let g = st.borrow_mut();
            g.ticks > 0 && self.now.saturating_sub(g.last_logic_tick_ms) < IN_GAME_MS
        };
        let frame = m.state::<ModsState>().borrow_mut().frame_size;
        self.view = if recent { game::view(m, frame, false) } else { View::default() };
        self.in_game = self.view.layout.is_some();
    }

    fn layout(&self) -> Layout {
        self.view.layout.unwrap_or(Layout::GOG_640)
    }

    fn camera_now(&self, m: &Machine) -> (f32, f32) {
        match game::gd(m) {
            Some(gd) => {
                let (x, y) = game::camera(m, gd);
                (x as f32, y as f32)
            }
            None => (0.0, 0.0),
        }
    }

    fn write_camera(&mut self, m: &mut Machine) {
        let Some(gd) = game::gd(m) else { return };
        let l = self.layout();
        let (x, y) = logic::clamp_camera(self.cam.pos.0, self.cam.pos.1, game::world_size(m, gd), l.map_w, l.map_h);
        if (x, y) != self.cam.pos {
            self.cam.set_pos((x, y));
        }
        let v = (x.round() as i32, y.round() as i32);
        if std::env::var_os("S3_TOUCH_TRACE").is_some() {
            eprintln!("touch: t={} cam mem={:?} last={:?} -> {:?}", self.now, game::camera(m, gd), self.cam_written, v);
        }
        game::set_camera(m, gd, v.0, v.1);
        self.cam_written = Some(v);
    }

    fn exec(&mut self, m: &mut Machine, a: TapAction, tap: (f32, f32)) {
        let now = self.now;
        let ripple = |c: Rgba| Fx::Ripple { x: tap.0, y: tap.1, t0: now, color: c };
        match a {
            TapAction::Click { x, y } => {
                click(m, now, x, y, MouseButton::Left);
                self.fx.push(ripple(fx::TAP));
            }
            TapAction::Command { x, y, attack } => {
                // Quick down+up: the game ignores right-ups held >= 15 game steps (0x53437d).
                click(m, now, x, y, MouseButton::Right);
                let cam = self.camera_now(m);
                self.fx.push(ripple(if attack { fx::ATTACK } else { fx::MOVE }));
                self.fx.push(Fx::Flag { wx: x as f32 + cam.0, wy: y as f32 + cam.1, t0: now, attack });
            }
            TapAction::RightClick { x, y } => {
                click(m, now, x, y, MouseButton::Right);
                self.fx.push(ripple(fx::CANCEL));
            }
            TapAction::CancelBuild => {
                if let Some(gd) = game::gd(m) {
                    game::cancel_build(m, gd);
                }
                self.fx.push(ripple(fx::CANCEL));
            }
            TapAction::Deselect => {
                if let Some(gd) = game::gd(m) {
                    game::deselect(m, gd);
                }
                self.fx.push(ripple(fx::CANCEL));
            }
            TapAction::SelectSameType { x, y } => {
                // After the second tap's click (its button-up is CLICK_MS later).
                let t = now + CLICK_MS + 20;
                mouse(m, t, HostEvent::Key { vk: 0x11, down: true });
                click(m, t + 10, x, y, MouseButton::Left);
                mouse(m, t + 10 + CLICK_MS + 10, HostEvent::Key { vk: 0x11, down: false });
                self.fx.push(ripple(fx::SELECT));
            }
            TapAction::Nothing => {}
        }
    }

    fn left_drag(&mut self, m: &mut Machine, phase: Phase, start: (f32, f32), cur: (f32, f32)) {
        let now = self.now;
        let (s, c) = (pt(start.0, start.1), pt(cur.0, cur.1));
        match phase {
            Phase::Begin => {
                mouse(m, now, HostEvent::MouseMove { x: s.0, y: s.1 });
                mouse(m, now, HostEvent::MouseButton { button: MouseButton::Left, down: true });
                mouse(m, now, HostEvent::MouseMove { x: c.0, y: c.1 });
            }
            Phase::Update => mouse(m, now, HostEvent::MouseMove { x: c.0, y: c.1 }),
            Phase::End => {
                mouse(m, now, HostEvent::MouseMove { x: c.0, y: c.1 });
                mouse(m, now, HostEvent::MouseButton { button: MouseButton::Left, down: false });
            }
        }
    }

    fn end_drag(&mut self, m: &mut Machine) {
        match self.drag.take() {
            Some(Drag::Left) | Some(Drag::Box { .. }) | Some(Drag::Minimap(_)) => {
                mouse(m, self.now, HostEvent::MouseButton { button: MouseButton::Left, down: false })
            }
            Some(Drag::Camera) => self.cam.release(self.now),
            None => {}
        }
    }

    fn pan(&mut self, m: &mut Machine, ev: &TouchEvent) -> ModEvent {
        let TouchEvent::Pan { phase, fingers, x, y, dx, dy, start_x, start_y, after_long_press } = *ev else {
            return ModEvent::Pass;
        };
        // A drag that starts on the minimap is always a left-button drag there (scroll the map), also after a long
        // press (holding the finger first used to start a box selection instead; user report 2026-11-08).
        let on_minimap = self.in_game && {
            let (sx, sy) = pt(start_x, start_y);
            self.layout().region(sx, sy) == logic::Region::Minimap
        };
        if fingers == 1 && (on_minimap || matches!(self.drag, Some(Drag::Minimap(_)))) {
            self.press = None;
            let now = self.now;
            let c = pt(x, y);
            match phase {
                Phase::Begin => {
                    self.left_drag(m, Phase::Begin, (start_x, start_y), (x, y));
                    self.drag = Some(Drag::Minimap(now));
                }
                Phase::Update => {
                    if let Some(Drag::Minimap(t)) = self.drag {
                        if now.saturating_sub(t) >= MINIMAP_REPRESS_MS {
                            mouse(m, now, HostEvent::MouseButton { button: MouseButton::Left, down: false });
                            mouse(m, now, HostEvent::MouseMove { x: c.0, y: c.1 });
                            mouse(m, now, HostEvent::MouseButton { button: MouseButton::Left, down: true });
                            self.drag = Some(Drag::Minimap(now));
                        } else {
                            mouse(m, now, HostEvent::MouseMove { x: c.0, y: c.1 });
                        }
                    }
                }
                Phase::End => {
                    mouse(m, now, HostEvent::MouseButton { button: MouseButton::Left, down: false });
                    mouse(m, now, HostEvent::MouseMove { x: c.0, y: c.1 });
                    mouse(m, now, HostEvent::MouseButton { button: MouseButton::Left, down: true });
                    mouse(m, now, HostEvent::MouseButton { button: MouseButton::Left, down: false });
                    self.drag = None;
                }
            }
            return ModEvent::Consumed;
        }
        if fingers == 1 && after_long_press {
            // Box selection in game, a plain left drag in menus.
            self.press = None;
            self.left_drag(m, phase, (start_x, start_y), (x, y));
            self.drag = match phase {
                Phase::End => None,
                _ if self.in_game => Some(Drag::Box { a: (start_x, start_y), b: (x, y) }),
                _ => Some(Drag::Left),
            };
            return ModEvent::Consumed;
        }
        if !self.in_game {
            return ModEvent::Pass;
        }
        match phase {
            Phase::Begin => {
                self.press = None;
                let (sx, sy) = pt(start_x, start_y);
                let kind = if fingers >= 2 { DragKind::Camera } else { logic::drag_kind(&self.layout(), sx, sy) };
                match kind {
                    DragKind::Camera => {
                        let p = self.camera_now(m);
                        self.cam.begin(p, self.now);
                        self.cam.drag(dx, dy, self.now);
                        self.write_camera(m);
                        self.drag = Some(Drag::Camera);
                    }
                    DragKind::LeftDrag => {
                        self.left_drag(m, phase, (start_x, start_y), (x, y));
                        self.drag = Some(Drag::Left);
                    }
                }
            }
            Phase::Update => match self.drag {
                Some(Drag::Camera) => {
                    self.cam.drag(dx, dy, self.now);
                    self.write_camera(m);
                }
                Some(Drag::Left) => self.left_drag(m, phase, (start_x, start_y), (x, y)),
                _ => {}
            },
            Phase::End => match self.drag {
                Some(Drag::Left) => {
                    self.left_drag(m, phase, (start_x, start_y), (x, y));
                    self.drag = None;
                }
                _ => self.end_drag(m),
            },
        }
        ModEvent::Consumed
    }
}

impl Mod for ContextTouch {
    fn name(&self) -> &str {
        "touch-context"
    }

    fn on_touch(&mut self, m: &mut Machine, ev: &TouchEvent) -> ModEvent {
        self.now = m.real_ms();
        let right_armed = m.state::<ModsState>().borrow_mut().fallback.right_next;
        match *ev {
            TouchEvent::Down { x, y, .. } => {
                self.cam.stop();
                self.refresh(m);
                let l = self.layout();
                let first = self.press.is_none() && self.drag.is_none();
                self.press = (first && self.in_game && l.region(x as i32, y as i32) == logic::Region::Map)
                    .then_some((x, y, self.now));
                ModEvent::Pass
            }
            TouchEvent::Move { x, y, .. } => {
                if let Some((px, py, _)) = self.press {
                    if ((x - px).powi(2) + (y - py).powi(2)).sqrt() > 8.0 {
                        self.press = None;
                    }
                }
                ModEvent::Pass
            }
            TouchEvent::Up { .. } => {
                self.press = None;
                ModEvent::Pass
            }
            TouchEvent::Cancel { .. } => {
                self.press = None;
                self.end_drag(m);
                ModEvent::Pass
            }
            // The legacy RMB button armed "next tap = right click": let the fallback do it.
            TouchEvent::Tap { .. } if right_armed => ModEvent::Pass,
            TouchEvent::Tap { x, y } => {
                self.press = None;
                let trace = std::env::var_os("S3_TOUCH_TRACE").is_some();
                if !self.in_game {
                    if trace {
                        let (ticks, last) = {
                            let st = m.state::<TouchGame>();
                            let g = st.borrow_mut();
                            (g.ticks, g.last_logic_tick_ms)
                        };
                        let frame = m.state::<ModsState>().borrow_mut().frame_size;
                        eprintln!(
                            "touch: tap ({x:.0},{y:.0}) not in game -> fallback (ticks {ticks} last {last} now {} frame {frame:?} gd {:?} layout {:?})",
                            self.now,
                            game::gd(m),
                            game::layout(m)
                        );
                    }
                    return ModEvent::Pass;
                }
                let frame = m.state::<ModsState>().borrow_mut().frame_size;
                let v = with_vis(m, game::view(m, frame, true));
                let (ix, iy) = pt(x, y);
                let a = logic::decide_tap(&v, ix, iy);
                if trace {
                    eprintln!("touch: tap ({ix},{iy}) mode {:?} -> {a:?}", v.mode);
                }
                self.exec(m, a, (x, y));
                ModEvent::Consumed
            }
            TouchEvent::DoubleTap { x, y } => {
                if !self.in_game {
                    return ModEvent::Pass;
                }
                let frame = m.state::<ModsState>().borrow_mut().frame_size;
                let v = with_vis(m, game::view(m, frame, true));
                let (ix, iy) = pt(x, y);
                let a = logic::decide_double_tap(&v, ix, iy);
                self.exec(m, a, (x, y));
                ModEvent::Consumed
            }
            TouchEvent::LongPress { x, y } => {
                if self.in_game {
                    crate::haptics::touch(m);
                }
                // Hover only (tooltips); a long press is never a right click (the game ignores long right clicks).
                // The press ring stays (full) while the finger rests: dragging now draws a selection box.
                let p = pt(x, y);
                mouse(m, self.now, HostEvent::MouseMove { x: p.0, y: p.1 });
                ModEvent::Consumed
            }
            TouchEvent::TwoFingerTap { x, y } => {
                if !self.in_game {
                    return ModEvent::Pass;
                }
                let (ix, iy) = pt(x, y);
                let a = logic::decide_two_finger_tap(&self.view, ix, iy);
                self.exec(m, a, (x, y));
                ModEvent::Consumed
            }
            TouchEvent::ThreeFingerTap { .. } => ModEvent::Pass,
            TouchEvent::Pan { .. } => self.pan(m, ev),
            TouchEvent::Pinch { phase, scale, cx, cy } => {
                // Continuous zoom through the render resolution levels (docs/porting/resolution-zoom.md). Checked
                // before `in_game`: while a zoom step settles the frame size differs from the layout for a moment.
                if crate::res::in_game(m) && crate::zoom::pinch(m, &mut self.zoom, phase, cx, cy, scale) {
                    self.cam.stop();
                    self.drag = None;
                    return ModEvent::Consumed;
                }
                if !self.in_game {
                    return ModEvent::Pass;
                }
                if phase == Phase::Begin {
                    self.pinch_done = false;
                }
                if std::env::var_os("S3_TOUCH_TRACE").is_some() {
                    eprintln!("touch: pinch {phase:?} scale {scale:.2} res {} done {}", self.layout().res_index, self.pinch_done);
                }
                if !self.pinch_done && phase != Phase::End {
                    if let Some(vk) = logic::pinch_zoom_key(scale, self.layout().res_index) {
                        self.pinch_done = true;
                        key(m, self.now, vk);
                        // The view size changes; do not glide afterwards.
                        self.cam.stop();
                        self.drag = None;
                    }
                }
                ModEvent::Consumed
            }
        }
    }

    fn tick(&mut self, m: &mut Machine, now: u64) {
        self.now = now;
        if self.long_press_ms == 0 {
            self.long_press_ms = m.state::<ModsState>().borrow_mut().settings.touch.long_press_ms.max(1);
        }
        self.refresh(m);
        if !self.in_game {
            self.cam.stop();
            self.drag = None;
            self.press = None;
        } else if self.cam.gliding() {
            let cur = game::gd(m).map(|gd| game::camera(m, gd));
            if cur.is_some() && cur != self.cam_written {
                // The game moved the camera (minimap, keys, resolution switch): stop gliding.
                self.cam.stop();
            } else if self.cam.step(now).is_some() {
                self.write_camera(m);
            }
        }
        self.fx.retain(|f| f.alive(now));
        self.draw_wanted = self.in_game
            && (!self.fx.is_empty()
                || self.drag.is_some()
                || self.press.is_some()
                || matches!(self.view.mode, Some(Mode::BuildPlacement))
                || !self.view.selected.is_empty());
    }

    fn wants_draw(&self) -> bool {
        self.draw_wanted
    }

    fn draw_window(&mut self, m: &mut Machine, c: &mut Layer, _frame: &FrameBuffer) {
        let frame = m.state::<ModsState>().borrow_mut().frame_size;
        let v = with_vis(m, game::view(m, frame, true));
        let Some(l) = v.layout else { return };
        let now = self.now;
        let vp = crate::draw_viewport(m);
        let u = c.ui();
        let to_win = |x: f32, y: f32| vp.game_to_window(x, y);
        // Map-only effects are clipped to the map view.
        let mx = to_win(l.panel_w as f32, 0.0).0.round() as i32;
        let (_, wy0, _, wh) = vp.dst_rect_clipped();
        c.set_clip(mx, wy0, c.width() as i32 - mx, wh);
        if !v.selected.is_empty() {
            let s = vp.scale().1;
            for h in v.hits.iter().filter(|h| h.own && !h.is_building() && v.selected.contains(&h.id)) {
                let (x, y) = to_win(h.x as f32, h.y as f32);
                fx::selection_marker(c, x, y, s);
            }
        }
        let cam = self.camera_now(m);
        for f in &self.fx {
            if matches!(f, Fx::Flag { .. }) {
                fx::draw(c, f, now, cam, &to_win, u);
            }
        }
        c.reset_clip();
        for f in &self.fx {
            if matches!(f, Fx::Ripple { .. }) {
                fx::draw(c, f, now, cam, &to_win, u);
            }
        }
        if let Some((x, y, t0)) = self.press {
            let age = now.saturating_sub(t0);
            if age >= PRESS_SHOW_MS {
                let k = (age - PRESS_SHOW_MS) as f32 / (self.long_press_ms.saturating_sub(PRESS_SHOW_MS).max(1)) as f32;
                let (x, y) = to_win(x, y);
                fx::press_progress(c, x, y, k.min(1.0), u);
            }
        }
        if let Some(Drag::Box { a, b }) = self.drag {
            fx::selection_box(c, to_win(a.0, a.1), to_win(b.0, b.1), u);
        }
        // Mode B (Mobile HUD) shows its own placement banner with Cancel; our ✕ would sit on its resource strip.
        let hud = m.state::<crate::ModsState>().borrow_mut().settings.ui_mode == crate::settings::UiMode::MobileHud;
        if v.mode == Some(Mode::BuildPlacement) && !hud {
            fx::cancel_button(c, cancel_window_rect(m), u, "Cancel");
        } else if matches!(v.mode, Some(Mode::UnitsSelected | Mode::BuildingSelected)) && !hud {
            fx::cancel_button(c, cancel_window_rect(m), u, "Deselect");
        }
    }
}

/// Window rectangle of the ✕ button (top right of the shown game area, below the FPS counter).
fn cancel_window_rect(m: &mut Machine) -> (i32, i32, i32, i32) {
    let vp = crate::draw_viewport(m);
    let top = match crate::display::display_settings(m).fps_counter {
        crate::display::FpsCounter::Off => 0.0,
        crate::display::FpsCounter::Fps => 22.0,
        crate::display::FpsCounter::Detailed => 62.0,
    };
    fx::cancel_rect(vp.dst_rect_clipped(), top, vp.win_h as f32 / 600.0)
}

/// Add the visible frame rect (display mode) and the ✕ button (drawn in the window layer, mapped back to game
/// pixels for the tap test) to a view.
fn with_vis(m: &mut Machine, mut v: View) -> View {
    let r = crate::display::visible_game_rect(m);
    v.vis = Some((r, 0));
    let vp = crate::draw_viewport(m);
    let (x, y, w, h) = cancel_window_rect(m);
    let (gx0, gy0) = vp.window_to_game_f(x as f32, y as f32);
    let (gx1, gy1) = vp.window_to_game_f((x + w) as f32, (y + h) as f32);
    v.cancel = Some((gx0.floor() as i32, gy0.floor() as i32, (gx1 - gx0).ceil() as i32, (gy1 - gy0).ceil() as i32));
    v.deselect_button = m.state::<crate::ModsState>().borrow_mut().settings.ui_mode == crate::settings::UiMode::ClassicTouch;
    v
}
