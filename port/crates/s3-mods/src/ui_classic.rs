//! UI mode A "Classic Touch" (docs/porting/ui-classic.md): the original panel and screens, made
//! finger-friendly without replacing them.
//!
//! - **Panel taps with a magnifier**: a finger on the left panel (below the minimap) is captured. While it is
//!   down, a bubble right of the panel shows the area under the finger enlarged 3x plus the button's name in
//!   large text; the button that will be pressed is outlined. The finger may slide to correct the choice.
//!   On lift, a left click is sent to the **centre** of that button (taps in the gaps between buttons snap
//!   to the nearest button on pages whose layout we know). Sliding far out of the panel cancels.
//! - **Floating Menu button** (top right of the map): opens/closes the in-game Save/Quit menu (panel page 7)
//!   by writing the panel state at the logic thread's `HandleInput` entry (ESC is not a hotkey in S3).
//! - **Context chip**: top left of the map, the current context ("Place: Woodcutter's Hut", "14 selected",
//!   the selected building's name). The build-placement ✕ Cancel button is drawn by the context-aware touch
//!   mod (`touch.rs`, top right), shared by both UI modes, so the Menu button sits bottom right.
//!
//! Active when `settings.ui_mode == ClassicTouch` (override with `S3_UI_MODE=A|classic|B|hud|off`), in game,
//! and only for the GOG exe (all addresses are for GOG S3.EXE `00a925cf…`). Map touches are not touched
//! here: they go on to the context-aware input / fallback mapping.

#[path = "ui_classic_panel.rs"]
pub mod panel;

use s3_cpu::ECX;
use s3_runtime::{HostEvent, LogLevel, Machine, MouseButton};

use crate::gesture::TouchEvent;
use crate::overlay::{measure_text, Canvas, Image, Rgba, Weight};
use crate::{settings::UiMode, Mod, ModEvent, ModsState};
use panel::{Button, Label, Layout, PanelState};

/// GOG addresses (game-state-ui.md / game-state-units.md).
pub mod gog {
    /// `CLogicThread::HandleInput`, thiscall logic; runs on the logic thread every tick in game.
    pub const HANDLE_INPUT: u32 = 0x0052_4fa0;
    /// Render screen object ctor (thiscall gfx); `[gfx+0x5f6b8]` = string table.
    pub const GFX_CTOR: u32 = 0x0042_c820;
    pub const GFX_STRTAB: u32 = 0x5f6b8;
    /// `[logic+0x3a0]` = game data `gd`.
    pub const LOGIC_GD: u32 = 0x3a0;
    /// `[logic+0x3d0]` = render snapshot; `snap+0x270` = tooltip text id (-1 none).
    pub const LOGIC_SNAP: u32 = 0x3d0;
    pub const SNAP_TOOLTIP: u32 = 0x270;
    /// CGameData vtable (sanity check).
    pub const GD_VTABLE: u32 = 0x0065_8c18;
    pub const GD_PANEL_DIRTY: u32 = 0x2d27c;
    pub const GD_PAGE: u32 = 0x2d284;
    pub const GD_CATEGORY: u32 = 0x2d288;
    pub const GD_SLOT: u32 = 0x2d28c;
    pub const GD_SUBPAGE: u32 = 0x2d298;
    pub const GD_BUILD_TYPE: u32 = 0x2d2f0;
    pub const GD_LOCAL_PLAYER: u32 = 0x2d2d4;
    pub const GD_PLAYERS: u32 = 0x11943e0;
    pub const PLAYER_SIZE: u32 = 0x148;
    pub const GD_BUILDINGS: u32 = 0x1499e64;
    pub const BUILDING_SIZE: u32 = 0x3a;
    /// UI selection: slot `[SEL_SLOT]` (-1 none), count `u16 [SEL_COUNT + slot*2]`.
    pub const SEL_SLOT: u32 = 0x7dfd68;
    pub const SEL_COUNT: u32 = 0x7ad620;
    /// Selected building id (0 none).
    pub const SEL_BUILDING: u32 = 0x7dfd6c;
    /// `CLogicThread` click sound `0x53f560(id)` thiscall logic; 0x3d = panel click (STATIC).
    pub const PLAY_SOUND: u32 = 0x0053_f560;
}

/// Hook / state name.
pub const NAME: &str = "ui-classic";

/// UI changes queued by the mod and applied on the logic thread at `HandleInput` entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Open the Save/Quit menu (page 7, sub-page 0).
    OpenMenu,
    /// Close it again (back to the previous page, or none).
    CloseMenu,
}

/// State shared between the hooks (logic thread) and the mod (input/overlay).
#[derive(Default)]
pub struct ClassicShared {
    pub logic: u32,
    pub gd: u32,
    pub gfx: u32,
    /// Host ms of the last `HandleInput` call (in-game detection).
    pub last_input_ms: u64,
    pub handle_input_hits: u64,
    pub pending: Vec<Action>,
    /// Page before the menu was opened.
    prev_page: u32,
    /// Hooks installed (GOG exe).
    pub hooked: bool,
}

/// Install mode A: hooks (GOG exe only) and the mod. Called by [`crate::install`] before the hook commit.
pub fn install(m: &mut Machine, gog: bool) {
    if gog {
        crate::add_hook(m, gog::GFX_CTOR, NAME, |c| {
            let gfx = c.reg(ECX);
            c.m.state::<ClassicShared>().borrow_mut().gfx = gfx;
            s3_runtime::HookFlow::Continue
        });
        crate::add_hook(m, gog::HANDLE_INPUT, NAME, |c| {
            let logic = c.reg(ECX);
            on_handle_input(c.m, logic);
            s3_runtime::HookFlow::Continue
        });
        m.state::<ClassicShared>().borrow_mut().hooked = true;
    }
    crate::add_mod(m, Box::new(ClassicTouch::new()));
}

fn valid_gd(m: &Machine, gd: u32) -> bool {
    gd >= 0x1_0000 && m.mem.read_u32(gd) == gog::GD_VTABLE
}

fn on_handle_input(m: &mut Machine, logic: u32) {
    let now = m.now_ms();
    let st = m.state::<ClassicShared>();
    let gd = m.mem.read_u32(logic.wrapping_add(gog::LOGIC_GD));
    let pending = {
        let mut s = st.borrow_mut();
        s.logic = logic;
        s.gd = gd;
        s.last_input_ms = now;
        s.handle_input_hits += 1;
        if s.pending.is_empty() {
            return;
        }
        std::mem::take(&mut s.pending)
    };
    if !valid_gd(m, gd) {
        return;
    }
    for a in pending {
        let page = m.mem.read_u32(gd + gog::GD_PAGE);
        match a {
            Action::OpenMenu => {
                if page != 7 {
                    st.borrow_mut().prev_page = page;
                }
                m.mem.write_u32(gd + gog::GD_PAGE, 7);
                m.mem.write_u32(gd + gog::GD_SUBPAGE, 0);
            }
            Action::CloseMenu => {
                let prev = std::mem::take(&mut st.borrow_mut().prev_page);
                let back = if matches!(prev, 1..=3) { prev } else { 0 };
                m.mem.write_u32(gd + gog::GD_PAGE, back);
                m.mem.write_u32(gd + gog::GD_SUBPAGE, 0);
            }
        }
        m.mem.write_u32(gd + gog::GD_PANEL_DIRTY, 1);
        // Click feedback like the original panel buttons (thiscall logic, (sound id)).
        crate::call::call_thiscall(m, gog::PLAY_SOUND, logic, &[0x3d]);
        m.log(LogLevel::Debug, &format!("ui-classic: {a:?} (page {page})"));
    }
}

/// Snapshot of the game UI read per event/frame.
#[derive(Debug, Clone, Copy, Default)]
struct GameView {
    layout: Option<Layout>,
    panel: PanelState,
    slot: u32,
    build_type: u8,
    sel_units: u32,
    sel_building_type: Option<u8>,
    tooltip: Option<u32>,
}

/// One finger on the panel.
#[derive(Debug, Clone)]
struct PanelTouch {
    id: u32,
    x: f32,
    y: f32,
    /// Chosen button (index into `buttons`).
    target: Option<usize>,
    buttons: Vec<Button>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FloatBtn {
    Menu,
    /// Shown while the menu is open: switch to UI mode B (the HUD's menu has the way back).
    HudMode,
}

/// The mode A mod.
pub struct ClassicTouch {
    env_mode: Option<bool>,
    active: bool,
    in_game: bool,
    view: GameView,
    touch: Option<PanelTouch>,
    pressed: Option<(u32, FloatBtn, bool)>,
    /// Delayed host events (click button-up).
    queue: Vec<(u64, HostEvent)>,
    texts: std::collections::HashMap<u32, Option<String>>,
    dirty: bool,
    logged_active: bool,
}

/// Snap radius in panel grid cells (3 px per cell at 640x480 -> 12 px).
const SNAP_CELLS: f32 = 4.0;
/// Sliding this far right of the panel cancels the panel tap.
const CANCEL_MARGIN: f32 = 48.0;
const CLICK_MS: u64 = 60;
/// In game = `HandleInput` ran within this time.
const IN_GAME_MS: u64 = 1500;

const BG: Rgba = Rgba::rgba(20, 16, 10, 200);
const GOLD: Rgba = Rgba::rgb(236, 196, 92);

impl ClassicTouch {
    pub fn new() -> Self {
        let env_mode = std::env::var("S3_UI_MODE").ok().map(|v| {
            let v = v.to_ascii_lowercase();
            matches!(v.as_str(), "a" | "classic" | "classictouch" | "classic-touch")
        });
        ClassicTouch {
            env_mode,
            active: false,
            in_game: false,
            view: GameView::default(),
            touch: None,
            pressed: None,
            queue: Vec::new(),
            texts: Default::default(),
            dirty: false,
            logged_active: false,
        }
    }

    fn read_view(&mut self, m: &mut Machine) {
        let (gd, logic, last, hooked) = {
            let st = m.state::<ClassicShared>();
            let s = st.borrow_mut();
            (s.gd, s.logic, s.last_input_ms, s.hooked)
        };
        let frame = m.last_frame_size();
        let layout = Layout::read(&m.mem).filter(|l| (l.screen_w as u32, l.screen_h as u32) == frame);
        self.in_game = hooked
            && layout.is_some()
            && valid_gd(m, gd)
            && m.now_ms().saturating_sub(last) < IN_GAME_MS;
        if !self.in_game {
            self.view = GameView::default();
            return;
        }
        let mem = &m.mem;
        let rd = |o: u32| mem.read_u32(gd + o);
        let player = rd(gog::GD_LOCAL_PLAYER).min(7);
        let race = mem.read_u8(gd + gog::GD_PLAYERS + player * gog::PLAYER_SIZE).min(3);
        let page = rd(gog::GD_PAGE);
        let slot_sel = mem.read_i32(gog::SEL_SLOT);
        let sel_units = if page == 5 && (0..10).contains(&slot_sel) {
            mem.read_u16(gog::SEL_COUNT + slot_sel as u32 * 2) as u32
        } else {
            0
        };
        let bid = mem.read_u32(gog::SEL_BUILDING);
        let sel_building_type = (page == 4 && (1..20000).contains(&bid))
            .then(|| mem.read_u8(gd + gog::GD_BUILDINGS + bid * gog::BUILDING_SIZE + 6));
        let snap = mem.read_u32(logic.wrapping_add(gog::LOGIC_SNAP));
        let tip = if snap >= 0x1_0000 { mem.read_u32(snap + gog::SNAP_TOOLTIP) } else { u32::MAX };
        self.view = GameView {
            layout,
            panel: PanelState { page, sub: rd(gog::GD_SUBPAGE), category: rd(gog::GD_CATEGORY), race },
            slot: rd(gog::GD_SLOT),
            build_type: rd(gog::GD_BUILD_TYPE) as u8,
            sel_units,
            sel_building_type,
            tooltip: (tip >> 16 == 8).then_some(tip),
        };
    }

    fn text(&mut self, m: &mut Machine, id: u32) -> Option<String> {
        if id == 0 {
            return None;
        }
        if let Some(t) = self.texts.get(&id) {
            return t.clone();
        }
        let gfx = m.state::<ClassicShared>().borrow_mut().gfx;
        if gfx < 0x1_0000 {
            return None;
        }
        let strtab = m.mem.read_u32(gfx + gog::GFX_STRTAB);
        let t = panel::read_text(&m.mem, strtab, id);
        if t.is_some() {
            self.texts.insert(id, t.clone());
        }
        t
    }

    fn label(&mut self, m: &mut Machine, l: Label) -> String {
        match l {
            Label::Text(id, en) => self.text(m, id).unwrap_or_else(|| en.to_string()),
            Label::Building(t) => self.building_name(m, t),
        }
    }

    fn building_name(&mut self, m: &mut Machine, t: u8) -> String {
        self.text(m, panel::building_text_id(t)).unwrap_or_else(|| format!("Building {t}"))
    }

    /// Floating buttons: (kind, rect).
    fn float_buttons(&self) -> Vec<(FloatBtn, (i32, i32, i32, i32))> {
        let Some(l) = self.view.layout else { return Vec::new() };
        // Bottom right of the map: the top right corner holds touch.rs' build-placement ✕ button.
        let (w, h) = (76, 38);
        let menu = (l.screen_w - w - 8, l.screen_h - h - 14, w, h);
        let mut v = vec![(FloatBtn::Menu, menu)];
        if self.view.panel.page == 7 {
            let hw = 150;
            v.push((FloatBtn::HudMode, (l.screen_w - hw - 8, menu.1 - h - 12, hw, h)));
        }
        v
    }

    fn placing(&self) -> bool {
        self.view.panel.page == 1 && self.view.slot != 0
    }

    fn float_at(&self, x: f32, y: f32) -> Option<FloatBtn> {
        // A few pixels of extra margin around each button.
        self.float_buttons().into_iter().find_map(|(k, (bx, by, bw, bh))| {
            let inside = x >= (bx - 6) as f32 && x < (bx + bw + 6) as f32 && y >= (by - 6) as f32 && y < (by + bh + 6) as f32;
            inside.then_some(k)
        })
    }

    fn queue_click(&mut self, m: &mut Machine, p: (i32, i32)) {
        let now = m.now_ms();
        m.log(LogLevel::Debug, &format!("ui-classic: panel click at {},{} (page {})", p.0, p.1, self.view.panel.page));
        m.input.push(HostEvent::MouseMove { x: p.0, y: p.1 });
        m.input.push(HostEvent::MouseButton { button: MouseButton::Left, down: true });
        self.queue.push((now + CLICK_MS, HostEvent::MouseButton { button: MouseButton::Left, down: false }));
    }

    fn update_target(&mut self) {
        let (Some(l), Some(t)) = (self.view.layout, self.touch.as_mut()) else { return };
        let snap = self.view.panel.fully_known();
        t.target = panel::pick(&l, &t.buttons, t.x, t.y, snap, SNAP_CELLS * l.cell as f32);
    }

    /// Where the click for the current panel touch goes.
    fn touch_point(&self) -> Option<(i32, i32)> {
        let t = self.touch.as_ref()?;
        let l = self.view.layout?;
        Some(match t.target {
            Some(i) => panel::center(&l, &t.buttons[i]),
            None => (t.x.round() as i32, t.y.round() as i32),
        })
    }

    fn context_chip(&mut self, m: &mut Machine) -> Option<String> {
        let v = self.view;
        if self.placing() {
            let name = self.building_name(m, v.build_type);
            return Some(format!("Place: {name}"));
        }
        if v.sel_units > 0 {
            return Some(format!("{} selected", v.sel_units));
        }
        if let Some(t) = v.sel_building_type {
            return Some(self.building_name(m, t));
        }
        None
    }

    fn draw_panel_touch(&mut self, m: &mut Machine, c: &mut Canvas) {
        let (Some(l), Some(t)) = (self.view.layout, self.touch.clone()) else { return };
        // Magnified source region, centred on the chosen button (or the finger).
        let (cx, cy) = self.touch_point().unwrap_or((t.x as i32, t.y as i32));
        let (sw, sh, k) = (64, 40, 3);
        let sx = (cx - sw / 2).clamp(0, l.panel_w - sw).max(0);
        let sy = (cy - sh / 2).clamp(0, l.screen_h - sh).max(0);
        let mut img = Image::new(sw as u32, sh as u32);
        for yy in 0..sh {
            for xx in 0..sw {
                let p = c.fb.pixels.get(((sy + yy) as u32 * c.fb.width + (sx + xx) as u32) as usize).copied().unwrap_or(0);
                let (r, g, b) = s3_runtime::host::rgb565_to_rgb888(p);
                let i = ((yy * sw + xx) * 4) as usize;
                img.rgba[i..i + 4].copy_from_slice(&[r, g, b, 255]);
            }
        }
        let label = match t.target {
            Some(i) => self.label(m, t.buttons[i].label),
            None => match self.view.tooltip {
                Some(id) => self.text(m, id).unwrap_or_default(),
                None => String::new(),
            },
        };
        // Outline the chosen button in place.
        if let Some(i) = t.target {
            let (bx, by, bw, bh) = l.rect(&t.buttons[i]);
            c.stroke_rounded_rect(bx - 2, by - 2, bw + 4, bh + 4, 3.0, 2.0, GOLD);
        }
        // Bubble right of the panel, vertically next to the finger (never under it).
        let (mw, mh) = (sw * k, sh * k);
        let px = 18.0;
        let (tw, th) = measure_text(px, Weight::Bold, &label);
        let bw = mw.max(tw + 20) + 12;
        let bh = mh + 12 + if label.is_empty() { 0 } else { th + 10 };
        let bx = l.panel_w + 10;
        let by = (cy - bh / 2 - 20).clamp(4, l.screen_h - bh - 4);
        c.fill_rounded_rect(bx, by, bw, bh, 10.0, BG);
        c.stroke_rounded_rect(bx, by, bw, bh, 10.0, 2.0, GOLD);
        // Pointer triangle towards the finger.
        let ty = cy.clamp(by + 12, by + bh - 12);
        for i in 0..10 {
            c.fill_rect(bx - i, ty - (10 - i), 1, 2 * (10 - i), BG);
        }
        let ix = bx + (bw - mw) / 2;
        c.blit_scaled(&img, ix, by + 6, mw, mh, 1.0);
        // Outline of the chosen button inside the magnified view.
        if let Some(i) = t.target {
            let (rx, ry, rw, rh) = l.rect(&t.buttons[i]);
            c.set_clip(ix, by + 6, mw, mh);
            c.stroke_rect(ix + (rx - sx) * k - 2, by + 6 + (ry - sy) * k - 2, rw * k + 4, rh * k + 4, 2, GOLD);
            c.reset_clip();
        }
        if !label.is_empty() {
            c.text(bx + (bw - tw) / 2, by + 6 + mh + 6, px, Weight::Bold, Rgba::WHITE, &label);
        }
    }

    fn draw_float(&mut self, m: &mut Machine, c: &mut Canvas) {
        let Some(l) = self.view.layout else { return };
        let menu_open = self.view.panel.page == 7;
        for (k, (x, y, w, h)) in self.float_buttons() {
            let pressed = self.pressed.is_some_and(|(_, pk, inside)| pk == k && inside);
            let (bg, text) = match k {
                FloatBtn::Menu => (BG, if menu_open { "Close" } else { "Menu" }),
                FloatBtn::HudMode => (BG, "UI: Mobile HUD"),
            };
            let bg = if pressed { GOLD.fade(0.85) } else { bg };
            c.fill_rounded_rect(x, y, w, h, 9.0, bg);
            c.stroke_rounded_rect(x, y, w, h, 9.0, 2.0, GOLD);
            let fg = if pressed { Rgba::BLACK } else { Rgba::WHITE };
            c.text_centered(x, y, w, h, 17.0, Weight::Bold, fg, text);
        }
        if let Some(chip) = self.context_chip(m) {
            let px = 15.0;
            let (tw, th) = measure_text(px, Weight::Bold, &chip);
            let x = l.panel_w + 8;
            let w = tw + 20;
            if x + w < l.screen_w - 60 {
                c.fill_rounded_rect(x, 8, w, th + 12, 8.0, BG);
                c.text(x + 10, 14, px, Weight::Bold, GOLD, &chip);
            }
        }
    }
}

impl Default for ClassicTouch {
    fn default() -> Self {
        Self::new()
    }
}

impl Mod for ClassicTouch {
    fn name(&self) -> &str {
        NAME
    }

    fn on_touch(&mut self, m: &mut Machine, ev: &TouchEvent) -> ModEvent {
        if !self.active {
            return ModEvent::Pass;
        }
        match *ev {
            TouchEvent::Down { id, x, y } => {
                self.read_view(m);
                if !self.in_game {
                    return ModEvent::Pass;
                }
                if let Some(k) = self.float_at(x, y) {
                    self.pressed = Some((id, k, true));
                    self.dirty = true;
                    return ModEvent::Consumed;
                }
                let Some(l) = self.view.layout else { return ModEvent::Pass };
                if self.touch.is_none() && l.in_panel(x, y) && self.view.panel.page != 9 {
                    self.touch = Some(PanelTouch { id, x, y, target: None, buttons: panel::buttons(&self.view.panel) });
                    self.update_target();
                    // Hover there so the game computes its own tooltip for buttons we do not know.
                    m.input.push(HostEvent::MouseMove { x: x as i32, y: y as i32 });
                    self.dirty = true;
                    return ModEvent::Consumed;
                }
                ModEvent::Pass
            }
            TouchEvent::Move { id, x, y } => {
                if let Some((pid, k, _)) = self.pressed {
                    if pid == id {
                        let inside = self.float_at(x, y) == Some(k);
                        self.pressed = Some((pid, k, inside));
                        self.dirty = true;
                        return ModEvent::Consumed;
                    }
                }
                if self.touch.as_ref().is_some_and(|t| t.id == id) {
                    if let Some(t) = self.touch.as_mut() {
                        t.x = x;
                        t.y = y;
                    }
                    self.update_target();
                    m.input.push(HostEvent::MouseMove { x: x as i32, y: y as i32 });
                    self.dirty = true;
                    return ModEvent::Consumed;
                }
                ModEvent::Pass
            }
            TouchEvent::Up { id, x, y } => {
                if let Some((pid, k, _)) = self.pressed {
                    if pid == id {
                        self.pressed = None;
                        self.dirty = true;
                        if self.float_at(x, y) == Some(k) {
                            let action = match k {
                                FloatBtn::Menu if self.view.panel.page == 7 => Action::CloseMenu,
                                FloatBtn::Menu => Action::OpenMenu,
                                FloatBtn::HudMode => {
                                    // Close the menu page, then hand over to mode B (settings are saved).
                                    let mut s = crate::settings(m);
                                    s.ui_mode = UiMode::MobileHud;
                                    m.log(LogLevel::Info, "ui-classic: switching to UI mode B (Mobile HUD)");
                                    let _ = crate::set_settings(m, s);
                                    Action::CloseMenu
                                }
                            };
                            m.state::<ClassicShared>().borrow_mut().pending.push(action);
                        }
                        return ModEvent::Consumed;
                    }
                }
                if self.touch.as_ref().is_some_and(|t| t.id == id) {
                    if let Some(t) = self.touch.as_mut() {
                        t.x = x;
                        t.y = y;
                    }
                    self.read_view(m);
                    self.update_target();
                    let cancel = self.view.layout.is_none_or(|l| x > l.panel_w as f32 + CANCEL_MARGIN);
                    if !cancel {
                        if let Some(p) = self.touch_point() {
                            self.queue_click(m, p);
                        }
                    }
                    self.touch = None;
                    self.dirty = true;
                    return ModEvent::Consumed;
                }
                ModEvent::Pass
            }
            TouchEvent::Cancel { id } => {
                if self.pressed.is_some_and(|p| p.0 == id) {
                    self.pressed = None;
                }
                if self.touch.as_ref().is_some_and(|t| t.id == id) {
                    self.touch = None;
                }
                self.dirty = true;
                ModEvent::Pass
            }
            _ => ModEvent::Pass,
        }
    }

    fn wants_draw(&self) -> bool {
        self.active && self.in_game
    }

    fn needs_redraw(&self) -> bool {
        self.dirty
    }

    fn tick(&mut self, m: &mut Machine, now: u64) {
        let mode = m.state::<ModsState>().borrow_mut().settings.ui_mode;
        self.active = self.env_mode.unwrap_or(mode == UiMode::ClassicTouch);
        if !self.queue.is_empty() {
            let mut due = Vec::new();
            self.queue.retain(|&(t, e)| {
                if t <= now {
                    due.push(e);
                    false
                } else {
                    true
                }
            });
            for e in due {
                m.input.push(e);
            }
        }
        if self.active {
            let was = self.in_game;
            self.read_view(m);
            if was != self.in_game {
                self.dirty = true;
                if self.in_game && !self.logged_active {
                    self.logged_active = true;
                    m.log(LogLevel::Info, "ui-classic: in game, Classic Touch overlay active");
                }
            }
        } else {
            self.in_game = false;
            self.touch = None;
            self.pressed = None;
        }
    }

    fn draw(&mut self, m: &mut Machine, c: &mut Canvas) {
        self.dirty = false;
        if !self.in_game {
            return;
        }
        if self.touch.is_some() {
            self.draw_panel_touch(m, c);
        }
        self.draw_float(m, c);
    }
}
