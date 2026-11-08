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
//!
//! Everything is drawn into the window layer ([`crate::layer`], window pixels, sizes in units of a 600 px high
//! window), so the floating buttons, chip and bubble keep their on-screen size at every zoom level; the panel
//! itself is shown at a constant scale by the frontend ([`crate::display::PanelSplit`]).

#[path = "ui_classic_panel.rs"]
pub mod panel;

use s3_cpu::ECX;
use s3_runtime::{FrameBuffer, HostEvent, LogLevel, Machine, MouseButton};

use crate::gesture::TouchEvent;
use crate::layer::Layer;
use crate::overlay::{measure_text, Rgba, Weight};
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
    let now = m.real_ms();
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
    /// Shown while the menu is open: the game's own Save / Quit page (panel page 7).
    GameMenu,
    /// Shown while the menu is open: switch to UI mode B (the HUD's menu has the way back).
    HudMode,
    /// Shown while the menu is open: cycle the FPS counter (docs/porting/display.md).
    Fps,
    /// Shown while the menu is open: cycle the game frame limit (30 / 60 / 120 / 144 / display rate).
    FpsLimit,
    /// Shown while the menu is open: move the FPS counter to the next corner / edge.
    FpsPos,
    /// Shown while the menu is open: cycle the display filter (Pixel / Smooth / Sharp / xBR / AI).
    Filter,
    /// Always (single-player game): normal <-> chosen fast speed, left of Menu.
    Speed,
    /// Shown while the menu is open: choose the fast speed (2x..8x).
    SpeedChoice,
    /// Shown while the menu is open: game time display on / off.
    GameTime,
    /// Shown while the menu is open: cycle the display mode (Fit / Fill / Stretch / Native wide).
    Screen,
    /// Shown while the menu is open: zoom in / out one level (render resolution, docs/porting/resolution-zoom.md).
    ZoomIn,
    ZoomOut,
    /// Shown while the menu is open: paths of all ordered units / only the selected ones (`crate::unit_orders`).
    UnitPaths,
    /// Shown while the menu is open: group buttons 1..6 on / off.
    Groups,
    /// Shown while the menu is open: vibration all / touch / messages / off.
    Vibration,
}

/// The mode A mod.
pub struct ClassicTouch {
    env_mode: Option<bool>,
    active: bool,
    in_game: bool,
    view: GameView,
    touch: Option<PanelTouch>,
    pressed: Option<(u32, FloatBtn, bool)>,
    /// Our floating Menu is open: the column of extra buttons (Save / Quit, UI mode, FPS, filter, zoom...).
    /// Independent of the game's Save / Quit page: Menu no longer opens that page by itself, "Save / Quit" in the
    /// column does; the panel's own computer icon opens just the game's page (user reports 2026-11-08).
    float_open: bool,
    /// Speed button state (chosen fast speed label, running fast) while it can be changed, refreshed each tick.
    speed: Option<(String, bool)>,
    /// Delayed host events (click button-up).
    queue: Vec<(u64, HostEvent)>,
    texts: std::collections::HashMap<u32, Option<String>>,
    dirty: bool,
    logged_active: bool,
    /// Window rectangle that shows the game (`Viewport::dst_rect_clipped`), the UI scale (window height / 600) and
    /// the display settings, for the floating buttons (window layer).
    vis: Option<(i32, i32, i32, i32)>,
    ui: f32,
    disp: crate::display::DisplaySettings,
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
            float_open: false,
            speed: None,
            queue: Vec::new(),
            texts: Default::default(),
            dirty: false,
            logged_active: false,
            vis: None,
            ui: 1.0,
            disp: crate::display::DisplaySettings::default(),
        }
    }

    fn read_view(&mut self, m: &mut Machine) {
        let (gd, logic, last, hooked) = {
            let st = m.state::<ClassicShared>();
            let s = st.borrow_mut();
            (s.gd, s.logic, s.last_input_ms, s.hooked)
        };
        let frame = m.last_frame_size();
        let vp = crate::draw_viewport(m);
        self.vis = Some(vp.dst_rect_clipped());
        self.ui = vp.win_h as f32 / 600.0;
        self.disp = crate::display::display_settings(m);
        let layout = Layout::read(&m.mem).filter(|l| (l.screen_w as u32, l.screen_h as u32) == frame);
        self.in_game = hooked
            && layout.is_some()
            && valid_gd(m, gd)
            && m.real_ms().saturating_sub(last) < IN_GAME_MS;
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

    /// Window pixels for `v` units (1 unit = 1 px of a 600 px high window).
    fn q(&self, v: f32) -> i32 {
        (v * self.ui).round() as i32
    }

    /// Floating buttons: (kind, window rect).
    fn float_buttons(&self) -> Vec<(FloatBtn, (i32, i32, i32, i32))> {
        if self.view.layout.is_none() {
            return Vec::new();
        }
        // Bottom right of the map: the top right corner holds touch.rs' build-placement ✕ button.
        let (w, h) = (self.q(76.0), self.q(38.0));
        // Right / bottom edge of the part of the window showing the game (Fill mode crops the frame).
        let Some((vx, vy, vw, vh)) = self.vis.filter(|r| r.2 > 0 && r.3 > 0) else { return Vec::new() };
        let (right, bottom) = (vx + vw, vy + vh);
        let menu = (right - w - self.q(8.0), bottom - h - self.q(14.0), w, h);
        let mut v = vec![(FloatBtn::Menu, menu)];
        if self.speed.is_some() {
            let sw = self.q(60.0);
            v.push((FloatBtn::Speed, (menu.0 - sw - self.q(10.0), menu.1, sw, h)));
        }
        if self.float_open {
            let hw = self.q(150.0);
            let mut y = menu.1;
            let mut x = right - hw - self.q(8.0);
            let mut ks = vec![FloatBtn::GameMenu, FloatBtn::GameTime, FloatBtn::HudMode, FloatBtn::Fps, FloatBtn::FpsPos, FloatBtn::FpsLimit, FloatBtn::Filter, FloatBtn::Screen, FloatBtn::ZoomIn, FloatBtn::ZoomOut, FloatBtn::UnitPaths, FloatBtn::Groups, FloatBtn::Vibration];
            if self.speed.is_some() {
                ks.insert(1, FloatBtn::SpeedChoice);
            }
            for k in ks {
                y -= h + self.q(12.0);
                if y < vy + self.q(4.0) {
                    // Next column to the left (short windows).
                    y = menu.1 - h - self.q(12.0);
                    x -= hw + self.q(10.0);
                    if x < vx + self.q(4.0) {
                        break;
                    }
                }
                v.push((k, (x, y, hw, h)));
            }
        }
        v
    }

    fn placing(&self) -> bool {
        self.view.panel.page == 1 && self.view.slot != 0
    }

    /// Floating button under a window point.
    fn float_at(&self, x: f32, y: f32) -> Option<FloatBtn> {
        // A few pixels of extra margin around each button.
        let e = self.q(6.0);
        self.float_buttons().into_iter().find_map(|(k, (bx, by, bw, bh))| {
            let inside = x >= (bx - e) as f32 && x < (bx + bw + e) as f32 && y >= (by - e) as f32 && y < (by + bh + e) as f32;
            inside.then_some(k)
        })
    }

    fn queue_click(&mut self, m: &mut Machine, p: (i32, i32)) {
        let now = m.real_ms();
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

    fn draw_panel_touch(&mut self, m: &mut Machine, c: &mut Layer, frame: &FrameBuffer) {
        let (Some(l), Some(t)) = (self.view.layout, self.touch.clone()) else { return };
        let vp = crate::draw_viewport(m);
        let ps = vp.panel_scale().unwrap_or(vp.scale().1);
        let to_win = |x: i32, y: i32| {
            let (wx, wy) = vp.game_to_window(x as f32, y as f32);
            (wx.round() as i32, wy.round() as i32)
        };
        // Magnified source region, centred on the chosen button (or the finger).
        let (cx, cy) = self.touch_point().unwrap_or((t.x as i32, t.y as i32));
        let (sw, sh) = (64, 40);
        // 3x the panel's on-screen size.
        let k = 3.0 * ps;
        let sx = (cx - sw / 2).clamp(0, l.panel_w - sw).max(0);
        let sy = (cy - sh / 2).clamp(0, l.screen_h - sh).max(0);
        let _ = frame;
        let label = match t.target {
            Some(i) => self.label(m, t.buttons[i].label),
            None => match self.view.tooltip {
                Some(id) => self.text(m, id).unwrap_or_default(),
                None => String::new(),
            },
        };
        let ui = self.ui;
        let q = move |v: f32| (v * ui).round() as i32;
        // Outline the chosen button in place.
        if let Some(i) = t.target {
            let (bx, by, bw, bh) = l.rect(&t.buttons[i]);
            let (x0, y0) = to_win(bx, by);
            let (x1, y1) = to_win(bx + bw, by + bh);
            c.stroke_rounded_rect(x0 - q(2.0), y0 - q(2.0), x1 - x0 + q(4.0), y1 - y0 + q(4.0), 3.0 * self.ui, 2.0 * self.ui, GOLD);
        }
        // Bubble right of the panel, vertically next to the finger (never under it).
        let (mw, mh) = ((sw as f32 * k).round() as i32, (sh as f32 * k).round() as i32);
        let px = 18.0 * self.ui;
        let (tw, th) = measure_text(px, Weight::Bold, &label);
        let bw = mw.max(tw + q(20.0)) + q(12.0);
        let bh = mh + q(12.0) + if label.is_empty() { 0 } else { th + q(10.0) };
        let (panel_right, fy) = to_win(l.panel_w, cy);
        let bx = panel_right + q(10.0);
        let (wtop, wbottom) = self.vis.map(|v| (v.1, v.1 + v.3)).unwrap_or((0, c.height() as i32));
        let by = (fy - bh / 2 - q(20.0)).clamp(wtop + q(4.0), (wbottom - bh - q(4.0)).max(wtop + q(4.0)));
        c.fill_rounded_rect(bx, by, bw, bh, 10.0 * self.ui, BG);
        c.stroke_rounded_rect(bx, by, bw, bh, 10.0 * self.ui, 2.0 * self.ui, GOLD);
        // Pointer triangle towards the finger.
        let ty = fy.clamp(by + q(12.0), by + bh - q(12.0));
        for i in 0..q(10.0) {
            c.fill_rect(bx - i, ty - (q(10.0) - i), 1, 2 * (q(10.0) - i), BG);
        }
        let ix = bx + (bw - mw) / 2;
        let iy = by + q(6.0);
        // The frontend draws the frame region there with the display filter (xBR / AI like the panel itself).
        c.frame_inset([sx as f32, sy as f32, sw as f32, sh as f32], ix, iy, mw, mh);
        // The panel texts are not in the frame while the window layer is shown (`panel_text`): draw them too.
        crate::panel_text::draw_inset(m, c, [sx as f32, sy as f32, sw as f32, sh as f32], (ix, iy, mw, mh));
        // Outline of the chosen button inside the magnified view.
        if let Some(i) = t.target {
            let (rx, ry, rw, rh) = l.rect(&t.buttons[i]);
            let kk = |v: i32| (v as f32 * k).round() as i32;
            c.set_clip(ix, iy, mw, mh);
            c.stroke_rect(ix + kk(rx - sx) - q(2.0), iy + kk(ry - sy) - q(2.0), kk(rw) + q(4.0), kk(rh) + q(4.0), q(2.0).max(1), GOLD);
            c.reset_clip();
        }
        if !label.is_empty() {
            c.text(bx + (bw - tw) / 2, iy + mh + q(6.0), px, Weight::Bold, Rgba::WHITE, &label);
        }
    }

    fn draw_float(&mut self, m: &mut Machine, c: &mut Layer) {
        let Some(l) = self.view.layout else { return };
        let ui = self.ui;
        let q = move |v: f32| (v * ui).round() as i32;
        let u = self.ui;
        let menu_open = self.float_open;
        for (k, (x, y, w, h)) in self.float_buttons() {
            let pressed = self.pressed.is_some_and(|(_, pk, inside)| pk == k && inside);
            let (bg, text) = match k {
                FloatBtn::Menu => (BG, if menu_open { "Close" } else { "Menu" }),
                FloatBtn::GameMenu => (BG, "Save / Quit"),
                FloatBtn::GameTime => (BG, if crate::game_time::enabled(m) { "Game time: on" } else { "Game time: off" }),
                FloatBtn::HudMode => (BG, "UI: Mobile HUD"),
                FloatBtn::Fps => (BG, match self.disp.fps_counter {
                    crate::display::FpsCounter::Off => "FPS: off",
                    crate::display::FpsCounter::Fps => "FPS: on",
                    crate::display::FpsCounter::Detailed => "FPS: details",
                }),
                FloatBtn::Screen => (BG, match self.disp.mode {
                    crate::display::DisplayMode::Fit => "Screen: Fit",
                    crate::display::DisplayMode::Fill => "Screen: Fill",
                    crate::display::DisplayMode::Stretch => "Screen: Stretch",
                    crate::display::DisplayMode::NativeWide => "Screen: Wide",
                }),
                FloatBtn::Speed if self.speed.as_ref().is_some_and(|s| s.1) => (GOLD.fade(0.55), ""),
                FloatBtn::UnitPaths => (BG, match crate::settings(m).unit_paths {
                    crate::settings::PathMode::Selected => "Paths: selected",
                    crate::settings::PathMode::Units => "Paths: units",
                    crate::settings::PathMode::All => "Paths: all units",
                }),
                FloatBtn::Vibration => (BG, crate::settings::vibration_label(&crate::settings(m))),
                FloatBtn::Groups => (BG, if crate::settings(m).group_shortcuts { "Groups: on" } else { "Groups: off" }),
                FloatBtn::ZoomIn | FloatBtn::ZoomOut | FloatBtn::FpsLimit | FloatBtn::FpsPos | FloatBtn::Filter | FloatBtn::Speed | FloatBtn::SpeedChoice => (BG, ""),
            };
            let zoom_label;
            let text = match k {
                FloatBtn::ZoomIn | FloatBtn::ZoomOut => {
                    let (lv, n) = (crate::zoom::level(m) + 1, crate::zoom::level_count());
                    let sign = if k == FloatBtn::ZoomIn { "+" } else { "-" };
                    let word = if k == FloatBtn::ZoomIn { "in" } else { "out" };
                    let _ = sign;
                    zoom_label = if crate::zoom::enabled(m) { format!("Zoom {word} ({lv}/{n})") } else { format!("Zoom {word} (off)") };
                    zoom_label.as_str()
                }
                FloatBtn::FpsLimit => {
                    zoom_label = crate::display::fps_limit_label(m);
                    zoom_label.as_str()
                }
                FloatBtn::FpsPos => {
                    zoom_label = format!("FPS pos: {}", self.disp.fps_pos.label());
                    zoom_label.as_str()
                }
                FloatBtn::Filter => {
                    zoom_label = format!("Filter: {}", self.disp.filter.label());
                    zoom_label.as_str()
                }
                FloatBtn::Speed => {
                    zoom_label = format!(">> {}", self.speed.as_ref().map(|s| s.0.clone()).unwrap_or_default());
                    zoom_label.as_str()
                }
                FloatBtn::SpeedChoice => {
                    zoom_label = format!("Fast speed: {}", crate::speed::choice_label(m));
                    zoom_label.as_str()
                }
                _ => text,
            };
            let bg = if pressed { GOLD.fade(0.85) } else { bg };
            c.fill_rounded_rect(x, y, w, h, 9.0 * u, bg);
            c.stroke_rounded_rect(x, y, w, h, 9.0 * u, 2.0 * u, GOLD);
            let fg = if pressed { Rgba::BLACK } else { Rgba::WHITE };
            c.text_centered(x, y, w, h, 17.0 * u, Weight::Bold, fg, text);
        }
        if let Some(chip) = self.context_chip(m) {
            let px = 15.0 * u;
            let (tw, th) = measure_text(px, Weight::Bold, &chip);
            let vp = crate::draw_viewport(m);
            // Right of the panel art incl. the minimap frame's diagonal edge at the top (`PanelSplit::overhang`).
            let right = vp.panel.map_or(l.panel_w as f32, |p| p.art_right(0.0).max(l.panel_w as f32));
            // (`right - 1` is the last art pixel, mapped at the panel's scale; `right` itself is map.)
            let x = vp.game_to_window(right - 1.0, 0.0).0.round() as i32 + q(10.0);
            let top = self.vis.map(|v| v.1).unwrap_or(0) + q(8.0);
            let w = tw + q(20.0);
            if x + w < c.width() as i32 - q(60.0) {
                c.fill_rounded_rect(x, top, w, th + q(12.0), 8.0 * u, BG);
                c.text(x + q(10.0), top + q(6.0), px, Weight::Bold, GOLD, &chip);
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
                let (wx, wy) = crate::touch_window_pos(m, ev);
                if let Some(k) = self.float_at(wx, wy) {
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
                        let (wx, wy) = crate::touch_window_pos(m, ev);
                        let inside = self.float_at(wx, wy) == Some(k);
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
                        let (wx, wy) = crate::touch_window_pos(m, ev);
                        if self.float_at(wx, wy) == Some(k) {
                            let action = match k {
                                FloatBtn::Menu => {
                                    // Only our button column; the game's page is opened by "Save / Quit".
                                    self.float_open = !self.float_open;
                                    return ModEvent::Consumed;
                                }
                                FloatBtn::GameMenu => {
                                    self.float_open = false;
                                    if self.view.panel.page == 7 {
                                        return ModEvent::Consumed;
                                    }
                                    Action::OpenMenu
                                }
                                FloatBtn::HudMode => {
                                    // Close the menu page, then hand over to mode B (settings are saved).
                                    let mut s = crate::settings(m);
                                    s.ui_mode = UiMode::MobileHud;
                                    m.log(LogLevel::Info, "ui-classic: switching to UI mode B (Mobile HUD)");
                                    let _ = crate::set_settings(m, s);
                                    self.float_open = false;
                                    if self.view.panel.page != 7 {
                                        return ModEvent::Consumed;
                                    }
                                    Action::CloseMenu
                                }
                                FloatBtn::ZoomIn | FloatBtn::ZoomOut => {
                                    // Render resolution only (res.rs switches at the next frame).
                                    if k == FloatBtn::ZoomIn {
                                        crate::zoom::zoom_in(m);
                                    } else {
                                        crate::zoom::zoom_out(m);
                                    }
                                    self.dirty = true;
                                    return ModEvent::Consumed;
                                }
                                FloatBtn::GameTime => {
                                    crate::game_time::toggle(m);
                                    self.dirty = true;
                                    return ModEvent::Consumed;
                                }
                                FloatBtn::UnitPaths | FloatBtn::Groups | FloatBtn::Vibration => {
                                    let mut s = crate::settings(m);
                                    if k == FloatBtn::UnitPaths {
                                        s.unit_paths = s.unit_paths.next();
                                    } else if k == FloatBtn::Vibration {
                                        crate::settings::cycle_vibration(&mut s);
                                    } else {
                                        s.group_shortcuts = !s.group_shortcuts;
                                    }
                                    let _ = crate::set_settings(m, s);
                                    self.dirty = true;
                                    return ModEvent::Consumed;
                                }
                                FloatBtn::Speed | FloatBtn::SpeedChoice => {
                                    if k == FloatBtn::Speed {
                                        crate::speed::toggle(m);
                                    } else {
                                        crate::speed::cycle_choice(m);
                                    }
                                    self.speed = crate::speed::button_state(m);
                                    self.dirty = true;
                                    return ModEvent::Consumed;
                                }
                                FloatBtn::Fps | FloatBtn::Screen | FloatBtn::FpsLimit | FloatBtn::FpsPos | FloatBtn::Filter => {
                                    // Presentation settings only: no game action.
                                    if k == FloatBtn::Fps {
                                        crate::display::cycle_fps(m);
                                    } else if k == FloatBtn::FpsLimit {
                                        crate::display::cycle_fps_limit(m);
                                    } else if k == FloatBtn::FpsPos {
                                        crate::display::cycle_fps_pos(m);
                                    } else if k == FloatBtn::Filter {
                                        crate::display::cycle_filter(m);
                                    } else {
                                        crate::display::cycle_mode(m);
                                    }
                                    self.disp = crate::display::display_settings(m);
                                    return ModEvent::Consumed;
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
        let speed = crate::speed::button_state(m);
        if speed != self.speed {
            self.speed = speed;
            self.dirty = true;
        }
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

    fn draw_window(&mut self, m: &mut Machine, c: &mut Layer, frame: &FrameBuffer) {
        self.dirty = false;
        if !self.in_game {
            return;
        }
        let vp = crate::draw_viewport(m);
        self.vis = Some(vp.dst_rect_clipped());
        self.ui = vp.win_h as f32 / 600.0;
        if self.touch.is_some() {
            self.draw_panel_touch(m, c, frame);
        }
        self.draw_float(m, c);
    }
}
