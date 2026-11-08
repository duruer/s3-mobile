//! UI mode B "Mobile HUD" (workstream W4, docs/porting/ui-hud.md): a native large-button overlay UI.
//!
//! - The original left panel is hidden and the map uses the full screen (layout patch, see
//!   [`ui_hud_game`](crate::ui_hud_game)); the game's own minimap, its frame and camera rectangle are not drawn.
//! - Own minimap widget (bottom left, [`ui_hud_minimap`](crate::ui_hud_minimap)) from the game's minimap image:
//!   drag on it pans the camera, a tap opens the full-screen map (tap a point = jump there and close).
//! - Settings sheet: display mode / filter / FPS counter (`crate::display`), zoom, HUD size, UI mode.
//! - Bottom bar: Basic / Food / Military / Special (build categories), Settlers, Stats, Menu.
//! - Bottom sheet: the buildings of a category as large cards (icon + the game's own name). Icons are sampled
//!   at runtime from the game's own panel (the HUD lets the game draw each build page once and copies the slot
//!   pixels from the presented frame); nothing is shipped. Choosing a card enters the game's build placement
//!   mode (panel state, game-state-ui.md); the next map tap places the site.
//! - Selection card (units / building), building sheet (stop / resume, site priority, work area, garrison),
//!   resource summary strip, settlers sheet (with recruitment) and goods sheet, in-game menu (save sheet with the
//!   game's slot list, quit, pause, settings incl. the UI mode switch). The original panel is never shown
//!   (docs/porting/ui-hud.md "No original panel").
//!
//! Active only while `settings.ui_mode == MobileHud`; switching modes at runtime restores the original layout.
//!
//! Drawn into the window layer ([`crate::layer`]) in window pixels, sized from the window height (not the game's
//! render size), so the HUD keeps its size and sharpness at every zoom level; touches are hit-tested with their
//! window position ([`crate::touch_window_pos`]).

use std::sync::Arc;

use s3_runtime::{FrameBuffer, HostEvent, Machine};

use crate::gesture::TouchEvent;
use crate::layer::{ImageRef, Layer};
use crate::overlay::{measure_text, Image, Rgba, Weight};
use crate::settings::UiMode;
use crate::ui_hud_data::{
    build_type, building_text_id, good_name, inventory_icon_rect, CATEGORIES, GARRISON_ACTIONS, GARRISON_TYPES, GOODS,
    RECRUIT_ACTIONS, STOP_WORK_TYPES, WORK_AREA_TYPES,
};

/// Harvest step that reads the goods icons (after the four build categories).
const GOODS_PAGE: u32 = 5;
use crate::ui_hud_game::{self as game, GameSnapshot, HudShared, LogicAction};
use crate::ui_hud_minimap::{self as minimap, MapGeom, MiniData};
use crate::{Mod, ModEvent, ModsState};

// ---------------------------------------------------------------- palette

const BAR_BG: Rgba = Rgba::rgba(18, 22, 30, 225);
const SHEET_BG: Rgba = Rgba::rgba(24, 29, 40, 238);
const CARD_BG: Rgba = Rgba::rgba(44, 52, 68, 255);
const CARD_HI: Rgba = Rgba::rgba(232, 176, 64, 255);
const BTN_BG: Rgba = Rgba::rgba(52, 61, 80, 255);
const BTN_ACTIVE: Rgba = Rgba::rgba(196, 138, 36, 255);
const BTN_DANGER: Rgba = Rgba::rgba(170, 52, 44, 255);
const TEXT: Rgba = Rgba::rgb(240, 236, 226);
const TEXT_DIM: Rgba = Rgba::rgb(168, 174, 188);
const ACCENT: Rgba = Rgba::rgb(240, 190, 80);

/// Height of the bottom bar in HUD units (1 unit = 1 px of a 480 px high window, times `hud_scale`).
const BAR_H: f32 = 54.0;

/// Window px of the HUD's bottom bar and top resource strip for a view of `vw` x `vh` (same scale as `HudMod::k`),
/// for other window-layer UI that must not cover them (FPS counter).
pub(crate) fn bar_and_strip_h(m: &mut Machine, vw: i32, vh: i32) -> (i32, i32) {
    let pref = crate::settings(m).hud_scale;
    let k = (vh.max(1) as f32 / 480.0).min(vw.max(1) as f32 / 640.0).max(0.5) * pref.clamp(0.6, 1.6);
    ((BAR_H * k).round() as i32, (34.0 * k).round() as i32)
}

// ---------------------------------------------------------------- geometry

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }
    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x as f32 && y >= self.y as f32 && x < (self.x + self.w) as f32 && y < (self.y + self.h) as f32
    }
    fn inset(&self, d: i32) -> Rect {
        Rect::new(self.x + d, self.y + d, self.w - 2 * d, self.h - 2 * d)
    }
}

/// Something the user can tap.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Action {
    Category(u32),
    Settlers,
    Stats,
    Menu,
    CloseSheet,
    Build { cat: u32, slot: u32, btype: u8 },
    CancelBuild,
    Deselect,
    MoveUnits,
    Destroy,
    /// Open the building sheet (replaces the original panel's page 4).
    BuildingDetails,
    /// Building sheet: Stop / Resume Working (or Building on a site), command 0x04.
    ToggleWork,
    /// Building sheet (construction site): Highest Priority on / off, command 0xf5 / 0xf6.
    SitePriority,
    /// Building sheet: Set new working area (next map tap).
    WorkArea,
    /// Building sheet: garrison action of command 0x0c (1..8, `ui_hud_data::GARRISON_ACTIONS`).
    Garrison(u8),
    /// Settlers sheet: recruit `count` specialists of `kind` (command 0x10).
    Recruit { kind: u8, count: u8 },
    MenuResume,
    /// Open the save sheet (slot list read with the game's own reader).
    MenuSave,
    /// Save into slot 1..8 (or the multiplayer save).
    SaveSlot(u32),
    MenuQuit,
    /// Confirmation: really end the game.
    MenuQuitYes,
    MenuPause,
    MenuToggleMode,
    MenuToggleFullMap,
    MenuScale(i32),
    /// The minimap widget (drag = pan, tap = full-screen map).
    Minimap,
    /// Game speed: normal <-> chosen fast speed (single player).
    Speed,
    /// Settings: choose the fast speed (2x..8x).
    CycleFastSpeed,
    /// Settings: game time display on / off.
    ToggleGameTime,
    /// The full-screen map image (tap = jump there).
    MapPoint,
    MenuSettings,
    SettingsBack,
    CycleDisplayMode,
    CycleFilter,
    CycleFps,
    /// Next game frame limit (30 / 60 / 120 / 144 / display rate).
    CycleFpsLimit,
    /// Next FPS counter position.
    CycleFpsPos,
    Zoom(i32),
    /// Swallow the tap (sheet background).
    Nothing,
}

/// Which sheet is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sheet {
    None,
    Build(u32),
    Settlers,
    Stats,
    Menu,
    Settings,
    /// "Quit this game?" confirmation (replaces the original panel's page 7 sub-page 1).
    QuitConfirm,
    /// Full-screen map overlay.
    Map,
    /// Selected building: state and the original page 4's actions (stop/resume, priority, work area, garrison).
    Building,
    /// Save game: the 8 slots (or one multiplayer save).
    Save,
}

/// Icon harvesting state (see module docs).
#[derive(Debug, Clone, PartialEq)]
enum Harvest {
    /// Not started (waiting for a game).
    Idle,
    /// Showing build category `cat`; capture once `panel_frames >= wait_until`.
    Cat { cat: u32, wait_until: u64, started_ms: u64 },
    Done,
}

/// The Mobile HUD mod.
pub struct MobileHud {
    active: bool,
    scale_pref: f32,
    fullscreen_map: bool,
    sheet: Sheet,
    /// A save is running (slot) since this time: the save sheet shows "Saving..." until the result arrives.
    saving: Option<(u32, u64)>,
    /// Occupied save slot tapped once: (slot, until ms); a second tap overwrites it.
    overwrite: Option<(u32, u64)>,
    /// The last drawn save sheet showed the slot list (redraw once the logic thread has read it).
    save_slots_drawn: bool,
    /// Speed button state (chosen fast speed label, running fast) while it can be changed.
    speed: Option<(String, bool)>,
    snap: Option<GameSnapshot>,
    in_game: bool,
    frame: (u32, u32),
    /// Hit targets of the last draw (top-most last).
    targets: Vec<(Rect, Action)>,
    /// Pointer captured by a HUD element: (id, action at Down).
    pressed: Option<(u32, Action)>,
    /// Icons per (cat, slot) for the current race.
    icons: Vec<((u32, u32), ImageRef)>,
    icon_race: Option<u32>,
    /// Goods icons from the Inventory page, by good id.
    good_icons: Vec<(u8, ImageRef)>,
    harvest: Harvest,
    /// Saved panel state before harvesting (page, cat).
    saved_page: Option<(u32, u32)>,
    names: Vec<(u32, String)>,
    destroy_armed_ms: u64,
    toast: Option<(String, u64)>,
    now: u64,
    dirty: bool,
    last_snap_ms: u64,
    text_dumped: bool,
    /// Part of the window that shows the game (window pixels; display `Fit` leaves bars, HUD elements stay inside).
    view: Rect,
    /// Minimap image + camera rectangle (refreshed every `MINI_REFRESH_MS`) and the image as RGBA for the layer.
    mini: Option<MiniData>,
    mini_img: Option<ImageRef>,
    last_mini_ms: u64,
    /// Geometry of the minimap widget / of the full-screen map from the last draw (hit tests).
    mini_geom: Option<MapGeom>,
    map_geom: Option<MapGeom>,
    /// Finger on the minimap widget: start point and whether it became a drag.
    mini_drag: Option<(f32, f32, bool)>,
    /// Last tile the camera was centred on by a minimap drag (avoid queueing the same jump again).
    mini_last_tile: Option<(u32, u32)>,
}

/// Minimap image refresh period (the game updates its minimap incrementally, a few times a second).
const MINI_REFRESH_MS: u64 = 250;

impl Default for MobileHud {
    fn default() -> Self {
        MobileHud {
            active: false,
            scale_pref: 1.0,
            fullscreen_map: true,
            sheet: Sheet::None,
            saving: None,
            save_slots_drawn: false,
            overwrite: None,
            speed: None,
            snap: None,
            in_game: false,
            frame: (640, 480),
            targets: Vec::new(),
            pressed: None,
            icons: Vec::new(),
            icon_race: None,
            good_icons: Vec::new(),
            harvest: Harvest::Idle,
            saved_page: None,
            names: Vec::new(),
            destroy_armed_ms: 0,
            toast: None,
            now: 0,
            dirty: true,
            last_snap_ms: 0,
            text_dumped: false,
            view: Rect::new(0, 0, 640, 480),
            mini: None,
            mini_img: None,
            last_mini_ms: 0,
            mini_geom: None,
            map_geom: None,
            mini_drag: None,
            mini_last_tile: None,
        }
    }
}

/// Add the HUD mod (always; it is inactive unless `ui_mode == MobileHud`) and its game hooks for the GOG exe.
/// Call before `hooks::commit`.
pub fn install(m: &mut Machine, gog_patches: bool) {
    if gog_patches {
        game::install_hooks(m);
    }
    crate::add_mod(m, Box::new(MobileHud::default()));
}

impl MobileHud {
    /// HUD scale factor (window px per HUD unit): window height relative to 480, times the user's `hud_scale`.
    fn k(&self) -> f32 {
        let (w, h) = (self.view.w.max(1) as f32, self.view.h.max(1) as f32);
        (h / 480.0).min(w / 640.0).max(0.5) * self.scale_pref.clamp(0.6, 1.6)
    }
    fn u(&self, v: f32) -> i32 {
        (v * self.k()).round() as i32
    }

    fn name(&mut self, m: &mut Machine, id: u32) -> String {
        if let Some((_, s)) = self.names.iter().find(|(i, _)| *i == id) {
            return s.clone();
        }
        match game::text(m, id) {
            Some(s) if !s.is_empty() => {
                self.names.push((id, s.clone()));
                s
            }
            _ => format!("#{:x}", id & 0xffff),
        }
    }

    fn building_name(&mut self, m: &mut Machine, t: u8) -> String {
        self.name(m, building_text_id(t))
    }

    fn icon(&self, cat: u32, slot: u32) -> Option<&ImageRef> {
        self.icons.iter().find(|(k, _)| *k == (cat, slot)).map(|(_, i)| i)
    }

    fn set_toast(&mut self, s: &str) {
        self.toast = Some((s.to_string(), self.now + 2500));
        self.dirty = true;
    }

    fn sync_layout(&mut self, m: &mut Machine) {
        // The original panel is never shown in this mode (except behind the "Preparing" cover while the icons are
        // read): every former panel page has a native sheet.
        let want = self.active && self.fullscreen_map && self.harvest_done_or_idle();
        let st = m.state::<HudShared>();
        let mut s = st.borrow_mut();
        if s.fullscreen_map != want {
            s.fullscreen_map = want;
            drop(s);
            if !want {
                // Restore immediately (the panel hook also restores on the next frame).
                game::apply_layout(m, false);
            }
        }
    }

    fn harvest_done_or_idle(&self) -> bool {
        matches!(self.harvest, Harvest::Done | Harvest::Idle)
    }

    // ------------------------------------------------------------ icon harvesting

    fn harvest_tick(&mut self, m: &mut Machine) {
        let harvesting = matches!(self.harvest, Harvest::Cat { .. });
        m.state::<HudShared>().borrow_mut().harvesting = harvesting;
        let Some(snap) = self.snap.clone() else { return };
        if self.icon_race != Some(snap.race) && self.harvest == Harvest::Done {
            self.harvest = Harvest::Idle;
            self.icons.clear();
        }
        let hooked = m.state::<HudShared>().borrow_mut().hooked;
        if !hooked || !self.in_game {
            return;
        }
        match self.harvest {
            Harvest::Idle => {
                // Only from a calm state (no placement / menu in progress).
                if snap.page == 9 || snap.page == 7 || (snap.page == 1 && snap.slot != 0) {
                    return;
                }
                self.saved_page = Some((snap.page, snap.cat));
                self.icon_race = Some(snap.race);
                self.start_harvest_cat(m, 1);
            }
            Harvest::Cat { started_ms, cat, .. } => {
                if self.now.saturating_sub(started_ms) > 8000 {
                    // Give up on this category (game not rendering?); continue with the next.
                    self.next_harvest_cat(m, cat);
                }
            }
            Harvest::Done => {}
        }
    }

    fn start_harvest_cat(&mut self, m: &mut Machine, cat: u32) {
        self.sync_layout_force_panel(m);
        if cat == GOODS_PAGE {
            game::queue(m, LogicAction::GoodsTab(1));
        } else {
            game::queue(m, LogicAction::Page { page: 1, sub: None });
            game::queue(m, LogicAction::StartBuild { cat, slot: 0, btype: 0 });
        }
        let frames = m.state::<HudShared>().borrow_mut().panel_frames;
        self.harvest = Harvest::Cat { cat, wait_until: frames + 3, started_ms: self.now };
        self.dirty = true;
    }

    fn sync_layout_force_panel(&mut self, m: &mut Machine) {
        m.state::<HudShared>().borrow_mut().fullscreen_map = false;
        game::apply_layout(m, false);
    }

    fn next_harvest_cat(&mut self, m: &mut Machine, cat: u32) {
        if cat < 5 {
            self.start_harvest_cat(m, cat + 1);
        } else {
            let (page, c) = self.saved_page.take().unwrap_or((0, 1));
            game::queue(m, LogicAction::StartBuild { cat: c.max(1), slot: 0, btype: 0 });
            game::queue(m, LogicAction::Page { page: if page == 1 { 0 } else { page }, sub: None });
            self.harvest = Harvest::Done;
            m.state::<HudShared>().borrow_mut().harvesting = false;
            self.sync_layout(m);
            self.dirty = true;
        }
    }

    /// Called in `draw` with the guest frame (before the HUD draws): copy the build slots of the shown page.
    fn harvest_capture(&mut self, m: &mut Machine, c: &FrameBuffer) {
        let Harvest::Cat { cat, wait_until, .. } = self.harvest else { return };
        let frames = m.state::<HudShared>().borrow_mut().panel_frames;
        if frames < wait_until {
            return;
        }
        let Some(g) = game::gd(m) else { return };
        let shown = if cat == GOODS_PAGE {
            m.mem.read_u32(g + game::GD_PAGE) == 2 && m.mem.read_u32(g + game::GD_GOODS_TAB) == 1
        } else {
            m.mem.read_u32(g + game::GD_PAGE) == 1 && m.mem.read_u32(g + game::GD_BUILD_CAT) == cat
        };
        // The panel art needs a frame at least as high as the 640x480 layout (the zoom switches up for the harvest).
        if !shown || c.height < game::HARVEST_MIN_H {
            return;
        }
        if m.mem.read_u32(game::L_PANEL_W) == 0 {
            return;
        }
        let cell = m.mem.read_u32(game::L_CELL) as i32;
        let (gx, gy) = (m.mem.read_u32(game::L_GRID_X) as i32, m.mem.read_u32(game::L_GRID_Y) as i32);
        if !(1..=8).contains(&cell) {
            return;
        }
        if cat == GOODS_PAGE {
            for &(good, row, col, _) in GOODS.iter() {
                let (x, y, w, h) = inventory_icon_rect(row, col, cell, gx, gy);
                let img = Arc::new(copy_rect(c, Rect::new(x, y, w, h)));
                self.good_icons.retain(|(k, _)| *k != good);
                self.good_icons.push((good, img));
            }
            self.next_harvest_cat(m, cat);
            return;
        }
        let race = self.icon_race.unwrap_or(0);
        for slot in 1..=12u32 {
            if build_type(race, cat, slot).is_none() {
                continue;
            }
            let r = slot_rect(slot, cell, gx, gy);
            let img = Arc::new(copy_rect(c, r));
            self.icons.retain(|(k, _)| *k != (cat, slot));
            self.icons.push(((cat, slot), img));
        }
        self.next_harvest_cat(m, cat);
    }

    // ------------------------------------------------------------ actions

    fn run(&mut self, m: &mut Machine, a: Action) {
        self.dirty = true;
        match a {
            Action::Category(c) => {
                self.sheet = if self.sheet == Sheet::Build(c) { Sheet::None } else { Sheet::Build(c) };
            }
            Action::Settlers => self.sheet = if self.sheet == Sheet::Settlers { Sheet::None } else { Sheet::Settlers },
            Action::Stats => self.sheet = if self.sheet == Sheet::Stats { Sheet::None } else { Sheet::Stats },
            Action::Menu => self.sheet = if self.sheet == Sheet::Menu { Sheet::None } else { Sheet::Menu },
            Action::CloseSheet | Action::MenuResume => self.sheet = Sheet::None,
            Action::Build { cat, slot, btype } => {
                game::queue(m, LogicAction::StartBuild { cat, slot, btype: btype as u32 });
                self.sheet = Sheet::None;
                let n = self.building_name(m, btype);
                self.set_toast(&format!("Tap the map to place: {n}"));
            }
            Action::CancelBuild => game::queue(m, LogicAction::CancelBuild),
            Action::Deselect => game::queue(m, LogicAction::ClearSelection),
            Action::MoveUnits => {
                crate::set_right_next(m, true);
                self.set_toast("Tap the target on the map");
            }
            Action::Destroy => {
                if self.now < self.destroy_armed_ms {
                    if let Some(b) = self.snap.as_ref().map(|s| s.building).filter(|&b| b != 0) {
                        game::queue(m, LogicAction::Destroy { building: b });
                        self.set_toast("Building destroyed");
                    }
                    self.destroy_armed_ms = 0;
                } else {
                    self.destroy_armed_ms = self.now + 3000;
                }
            }
            Action::BuildingDetails => self.sheet = Sheet::Building,
            Action::ToggleWork | Action::SitePriority | Action::Garrison(_) => {
                if std::env::var("S3_HUD_DEBUG").is_ok() {
                    if let Some(s) = self.snap.as_ref().filter(|s| s.building != 0) {
                        let e = s.gd + game::GD_BUILDINGS + s.building as u32 * 0x3a;
                        let b: Vec<String> = (0..0x3a).map(|i| format!("{:02x}", m.mem.read_u8(e + i))).collect();
                        eprintln!("ui-hud: {a:?} building {} entry {}", s.building, b.join(" "));
                    }
                }
                if let Some(req) = self.snap.as_ref().and_then(|s| building_request(s, &a)) {
                    game::queue(m, req);
                }
            }
            Action::WorkArea => {
                if let Some(b) = self.snap.as_ref().map(|s| s.building).filter(|&b| b != 0) {
                    game::queue(m, LogicAction::WorkArea { building: b });
                    self.sheet = Sheet::None;
                    self.set_toast("Tap the map to set the work area");
                }
            }
            Action::Recruit { kind, count } => {
                game::queue(m, LogicAction::Recruit { kind, count });
                self.set_toast(&format!("Ordered {count}"));
            }
            Action::MenuSave => {
                m.state::<HudShared>().borrow_mut().save_slots = None;
                game::queue(m, LogicAction::ReadSaveSlots);
                self.sheet = Sheet::Save;
            }
            Action::SaveSlot(slot) => {
                // An occupied slot needs a second tap within 3 s (the original list overwrites at once).
                let used = m.state::<HudShared>().borrow_mut().save_slots.as_ref().is_some_and(|s| {
                    !s.multiplayer && s.slots.get(slot.wrapping_sub(1) as usize).is_some_and(|x| x.is_some())
                });
                if used && !self.overwrite_armed(slot) {
                    self.overwrite = Some((slot, self.now + 3000));
                } else if self.saving.is_none() {
                    self.overwrite = None;
                    game::queue(m, LogicAction::SaveGame { slot });
                    self.saving = Some((slot, self.now));
                }
            }
            Action::MenuQuit => self.sheet = Sheet::QuitConfirm,
            Action::MenuQuitYes => {
                game::queue(m, LogicAction::QuitGame);
                self.sheet = Sheet::None;
            }
            Action::MenuPause => {
                // P toggles the pause page (game-state-ui.md: hotkey table).
                m.input.push(HostEvent::Key { vk: 0x50, down: true });
                m.input.push(HostEvent::Key { vk: 0x50, down: false });
                self.sheet = Sheet::None;
            }
            Action::MenuToggleMode => {
                let mut s = crate::settings(m);
                s.ui_mode = if s.ui_mode == UiMode::MobileHud { UiMode::ClassicTouch } else { UiMode::MobileHud };
                let _ = crate::set_settings(m, s);
                self.sheet = Sheet::None;
            }
            Action::MenuToggleFullMap => {
                self.fullscreen_map = !self.fullscreen_map;
                m.state::<HudShared>().borrow_mut().no_full = !self.fullscreen_map;
                let ok = m.state::<HudShared>().borrow_mut().buffer_ok;
                if self.fullscreen_map && !ok {
                    self.set_toast("Full-screen map after the next resolution change (F1-F3) or game start");
                }
            }
            Action::MenuScale(d) => {
                let mut s = crate::settings(m);
                s.hud_scale = ((s.hud_scale * 10.0).round() / 10.0 + d as f32 * 0.1).clamp(0.7, 1.5);
                self.scale_pref = s.hud_scale;
                let _ = crate::set_settings(m, s);
            }
            Action::MenuSettings => self.sheet = Sheet::Settings,
            Action::SettingsBack => self.sheet = Sheet::Menu,
            Action::CycleDisplayMode => {
                let v = crate::display::cycle_mode(m);
                self.set_toast(&format!("Screen: {}", v.label()));
            }
            Action::CycleFilter => {
                crate::display::cycle_filter(m);
            }
            Action::CycleFps => {
                crate::display::cycle_fps(m);
            }
            Action::CycleFpsLimit => {
                crate::display::cycle_fps_limit(m);
            }
            Action::CycleFpsPos => {
                crate::display::cycle_fps_pos(m);
            }
            Action::Zoom(d) => {
                if let Some(t) = zoom_step(m, d) {
                    self.set_toast(&t);
                }
            }
            Action::Minimap => self.sheet = Sheet::Map,
            Action::Speed => {
                let s = crate::speed::toggle(m);
                self.speed = crate::speed::button_state(m);
                self.set_toast(&format!("Game speed {s}x"));
            }
            Action::ToggleGameTime => {
                crate::game_time::toggle(m);
            }
            Action::CycleFastSpeed => {
                let s = crate::speed::cycle_choice(m);
                self.speed = crate::speed::button_state(m);
                self.set_toast(&format!("Fast speed {s}x"));
            }
            Action::MapPoint | Action::Nothing => {}
        }
        self.sync_layout(m);
    }

    /// Centre the camera on the tile under a point of a map geometry (replaces a still queued jump).
    fn center_at(&mut self, m: &mut Machine, g: &MapGeom, x: f32, y: f32) {
        let Some(world) = self.mini.as_ref().map(|d| d.world) else { return };
        let (tx, ty) = g.to_tile_clamped(x, y, world);
        if self.mini_last_tile == Some((tx, ty)) {
            return;
        }
        self.mini_last_tile = Some((tx, ty));
        let st = m.state::<HudShared>();
        st.borrow_mut().actions.retain(|a| !matches!(a, LogicAction::CenterOn { .. }));
        game::queue(m, LogicAction::CenterOn { tx, ty });
        // Show the new camera rectangle soon.
        self.last_mini_ms = 0;
    }

    /// Our minimap widget is shown (full-screen layout active: the game's own minimap is hidden).
    fn widget_active(&self, m: &mut Machine) -> bool {
        self.active && self.in_game && game::full_layout_active(m)
    }

    // ------------------------------------------------------------ drawing

    fn add(&mut self, r: Rect, a: Action) {
        self.targets.push((r, a));
    }

    fn button(&mut self, c: &mut Layer, r: Rect, label: &str, bg: Rgba, a: Action) {
        let pressed = self.pressed.as_ref().is_some_and(|(_, p)| *p == a);
        let bg = if pressed { CARD_HI } else { bg };
        c.fill_rounded_rect(r.x, r.y, r.w, r.h, self.u(8.0) as f32, bg);
        let px = self.k() * 14.0;
        c.text_centered(r.x, r.y, r.w, r.h, px, Weight::Bold, TEXT, label);
        self.add(r, a);
    }

    fn draw_bar(&mut self, c: &mut Layer) {
        let v = self.view;
        let w = v.w;
        let bh = self.u(BAR_H);
        let y0 = v.y + v.h - bh;
        c.fill_rect(v.x, y0, w, bh, BAR_BG);
        c.fill_rect(v.x, y0, w, 1.max(self.u(1.0)), Rgba::rgba(255, 255, 255, 40));
        let items: [(String, Action); 7] = [
            (CATEGORIES[0].0.into(), Action::Category(1)),
            (CATEGORIES[1].0.into(), Action::Category(2)),
            (CATEGORIES[2].0.into(), Action::Category(3)),
            (CATEGORIES[3].0.into(), Action::Category(4)),
            ("Settlers".into(), Action::Settlers),
            ("Stats".into(), Action::Stats),
            ("Menu".into(), Action::Menu),
        ];
        let gap = self.u(4.0);
        let bw = (w - gap) / items.len() as i32;
        for (i, (label, a)) in items.into_iter().enumerate() {
            let r = Rect::new(v.x + gap + i as i32 * bw, y0 + gap, bw - gap, bh - 2 * gap);
            let active = match (&a, self.sheet) {
                (Action::Category(cat), Sheet::Build(s)) => *cat == s,
                (Action::Settlers, Sheet::Settlers) | (Action::Stats, Sheet::Stats) | (Action::Menu, Sheet::Menu) => true,
                _ => false,
            };
            let pressed = self.pressed.as_ref().is_some_and(|(_, p)| *p == a);
            let bg = if pressed { CARD_HI } else if active { BTN_ACTIVE } else { BTN_BG };
            c.fill_rounded_rect(r.x, r.y, r.w, r.h, self.u(8.0) as f32, bg);
            // Icon area (top) + label (bottom).
            let icon_h = r.h - self.u(17.0);
            let ir = Rect::new(r.x + self.u(4.0), r.y + self.u(3.0), r.w - self.u(8.0), icon_h - self.u(3.0));
            match a {
                Action::Category(cat) => {
                    if let Some(img) = self.icon(cat, 1).cloned() {
                        blit_fit(c, &img, ir);
                    } else {
                        draw_house(c, ir, self.k());
                    }
                }
                Action::Settlers => draw_person(c, ir, self.k()),
                Action::Stats => draw_bars(c, ir, self.k()),
                Action::Menu => draw_burger(c, ir, self.k()),
                _ => {}
            }
            c.text_centered(r.x, r.y + r.h - self.u(17.0), r.w, self.u(15.0), self.k() * 12.0, Weight::Bold, TEXT, &label);
            self.add(r, a);
        }
    }

    fn sheet_rect(&self, _c: &Layer, rows_h: i32) -> Rect {
        let v = self.view;
        let bh = self.u(BAR_H);
        let sh = rows_h.min(v.h - bh - self.u(8.0));
        let mx = self.u(6.0);
        Rect::new(v.x + mx, v.y + v.h - bh - sh - self.u(4.0), v.w - 2 * mx, sh)
    }

    fn draw_sheet_frame(&mut self, c: &mut Layer, r: Rect, title: &str) -> Rect {
        c.fill_rounded_rect(r.x, r.y, r.w, r.h, self.u(12.0) as f32, SHEET_BG);
        c.stroke_rounded_rect(r.x, r.y, r.w, r.h, self.u(12.0) as f32, 1.0, Rgba::rgba(255, 255, 255, 50));
        self.add(r, Action::Nothing);
        let th = self.u(26.0);
        c.text(r.x + self.u(12.0), r.y + self.u(6.0), self.k() * 16.0, Weight::Bold, ACCENT, title);
        let close = Rect::new(r.x + r.w - self.u(44.0), r.y + self.u(3.0), self.u(40.0), self.u(26.0));
        self.button(c, close, "X", BTN_BG, Action::CloseSheet);
        Rect::new(r.x + self.u(8.0), r.y + th + self.u(6.0), r.w - self.u(16.0), r.h - th - self.u(12.0))
    }

    fn draw_build_sheet(&mut self, m: &mut Machine, c: &mut Layer, cat: u32) {
        let race = self.snap.as_ref().map(|s| s.race).unwrap_or(0);
        let slots: Vec<(u32, u8)> = (1..=12).filter_map(|s| build_type(race, cat, s).map(|t| (s, t))).collect();
        let cols = 4;
        let rows = slots.len().div_ceil(cols) as i32;
        let card_h = self.u(56.0);
        let gap = self.u(6.0);
        let r = self.sheet_rect(c, self.u(38.0) + rows * (card_h + gap) + gap);
        let title = CATEGORIES[(cat - 1) as usize].1;
        let inner = self.draw_sheet_frame(c, r, title);
        let cw = (inner.w - gap * (cols as i32 - 1)) / cols as i32;
        for (i, (slot, t)) in slots.into_iter().enumerate() {
            let (col, row) = ((i % cols) as i32, (i / cols) as i32);
            let cr = Rect::new(inner.x + col * (cw + gap), inner.y + row * (card_h + gap), cw, card_h);
            let a = Action::Build { cat, slot, btype: t };
            let pressed = self.pressed.as_ref().is_some_and(|(_, p)| *p == a);
            c.fill_rounded_rect(cr.x, cr.y, cr.w, cr.h, self.u(8.0) as f32, if pressed { CARD_HI } else { CARD_BG });
            let icon_w = self.u(64.0);
            let ir = Rect::new(cr.x + self.u(3.0), cr.y + self.u(3.0), icon_w, cr.h - self.u(6.0));
            if let Some(img) = self.icon(cat, slot).cloned() {
                c.fill_rounded_rect(ir.x, ir.y, ir.w, ir.h, self.u(6.0) as f32, Rgba::rgb(20, 24, 32));
                blit_fit(c, &img, ir.inset(1));
            } else {
                draw_house(c, ir, self.k());
            }
            let name = self.building_name(m, t);
            let tx = ir.x + ir.w + self.u(5.0);
            let tw = cr.x + cr.w - tx - self.u(3.0);
            draw_wrapped(c, Rect::new(tx, cr.y, tw, cr.h), self.k() * 12.5, &name);
            self.add(cr, a);
        }
    }

    fn draw_settlers_sheet(&mut self, m: &mut Machine, c: &mut Layer) {
        let Some(s) = self.snap.clone() else { return };
        let mut rows: Vec<(String, u32)> = s
            .settlers
            .iter()
            .enumerate()
            .filter(|(_, &n)| n > 0)
            .map(|(t, &n)| (settler_label(self, m, t as u8), n))
            .collect();
        rows.sort_by_key(|r| std::cmp::Reverse(r.1));
        let total: u32 = s.settlers.iter().sum();
        let line = self.u(19.0);
        let cols = 3;
        let nrows = rows.len().div_ceil(cols).max(1) as i32;
        let bh = self.u(34.0);
        let gap = self.u(6.0);
        let r = self.sheet_rect(c, self.u(64.0) + nrows * line + self.u(22.0) + 2 * (bh + gap));
        let inner = self.draw_sheet_frame(c, r, &format!("Settlers: {total}   Buildings: {}", s.buildings));
        // Recruitment (the original settlers page 3 tab 3, names = the game's own button tooltips).
        let amazon = s.race == 3;
        let grid_h = 2 * bh + gap;
        let gy = inner.y + inner.h - grid_h;
        let recruit = self.name(m, 0x8005c);
        c.text(inner.x, gy - self.u(20.0), self.k() * 13.0, Weight::Bold, ACCENT, &recruit);
        let bw = (inner.w - 3 * gap) / 4;
        for (i, &(kind, count, tid)) in RECRUIT_ACTIONS.iter().enumerate() {
            let label = self.name(m, if amazon { tid + 0xfe } else { tid });
            let (col, row) = ((i / 2) as i32, (i % 2) as i32);
            let br = Rect::new(inner.x + col * (bw + gap), gy + row * (bh + gap), bw, bh);
            let a = Action::Recruit { kind, count };
            let pressed = self.pressed.as_ref().is_some_and(|(_, p)| *p == a);
            c.fill_rounded_rect(br.x, br.y, br.w, br.h, self.u(8.0) as f32, if pressed { CARD_HI } else { BTN_BG });
            draw_wrapped(c, br.inset(self.u(4.0)), self.k() * 11.5, &label);
            self.add(br, a);
        }
        let cw = inner.w / cols as i32;
        for (i, (label, n)) in rows.iter().enumerate() {
            let (col, row) = ((i % cols) as i32, (i / cols) as i32);
            let (x, y) = (inner.x + col * cw, inner.y + row * line);
            c.text(x, y, self.k() * 13.0, Weight::Regular, TEXT_DIM, label);
            let ns = n.to_string();
            let (tw, _) = measure_text(self.k() * 13.0, Weight::Bold, &ns);
            c.text(x + cw - tw - self.u(10.0), y, self.k() * 13.0, Weight::Bold, TEXT, &ns);
        }
    }

    /// Goods in stock (the original goods page's "Inventory" tab, natively).
    fn draw_stats_sheet(&mut self, m: &mut Machine, c: &mut Layer) {
        let Some(s) = self.snap.clone() else { return };
        let rows: Vec<(usize, u32)> = s.goods.iter().enumerate().filter(|(_, &n)| n > 0).map(|(g, &n)| (g, n)).collect();
        let line = self.u(19.0);
        let cols = 4;
        let nrows = rows.len().div_ceil(cols).max(1) as i32;
        let r = self.sheet_rect(c, self.u(64.0) + nrows * line);
        let total: u32 = rows.iter().map(|r| r.1).sum();
        let inv = self.name(m, 0x80051);
        let inner = self.draw_sheet_frame(c, r, &format!("{inv}: {total}"));
        let cw = inner.w / cols as i32;
        for (i, (g, n)) in rows.iter().enumerate() {
            let (col, row) = ((i % cols) as i32, (i / cols) as i32);
            let (x, y) = (inner.x + col * cw, inner.y + row * line);
            match self.good_icon(*g as u8).cloned() {
                Some(img) => blit_fit(c, &img, Rect::new(x, y, self.u(16.0), self.u(16.0))),
                None => c.fill_circle(x as f32 + self.k() * 6.0, y as f32 + self.k() * 8.0, self.k() * 5.0, good_color(*g as u8)),
            }
            c.text(x + self.u(20.0), y, self.k() * 13.0, Weight::Regular, TEXT_DIM, &good_label(*g as u8));
            let ns = n.to_string();
            let (tw, _) = measure_text(self.k() * 13.0, Weight::Bold, &ns);
            c.text(x + cw - tw - self.u(10.0), y, self.k() * 13.0, Weight::Bold, TEXT, &ns);
        }
        if rows.is_empty() {
            c.text(inner.x, inner.y, self.k() * 13.0, Weight::Regular, TEXT_DIM, "No goods in storage yet");
        }
    }

    /// Centred dialog over a dimmed map; returns the inner rect.
    fn dialog(&mut self, c: &mut Layer, rows: i32, title: &str) -> Rect {
        self.dialog_w(c, rows, 320.0, title)
    }

    /// [`Self::dialog`] with a width in HUD units.
    fn dialog_w(&mut self, c: &mut Layer, rows: i32, width: f32, title: &str) -> Rect {
        let v = self.view;
        let above_bar = v.h - self.u(BAR_H);
        c.fill_rect(v.x, v.y, v.w, above_bar, Rgba::rgba(0, 0, 0, 110));
        self.add(Rect::new(v.x, v.y, v.w, above_bar), Action::CloseSheet);
        let mw = self.u(width).min(v.w - self.u(20.0));
        let bh = self.u(40.0);
        let gap = self.u(7.0);
        let mh = (self.u(40.0) + rows * (bh + gap)).min(above_bar - self.u(8.0));
        let r = Rect::new(v.x + (v.w - mw) / 2, v.y + (above_bar - mh) / 2, mw, mh);
        self.draw_sheet_frame(c, r, title)
    }

    fn draw_menu(&mut self, c: &mut Layer) {
        let bh = self.u(40.0);
        let gap = self.u(7.0);
        let inner = self.dialog(c, 4, "Game menu");
        let full = Rect::new(inner.x, inner.y, inner.w, bh);
        let half = (inner.w - gap) / 2;
        let row = |i: i32| inner.y + i * (bh + gap);
        self.button(c, Rect { y: row(0), ..full }, "Resume", BTN_ACTIVE, Action::MenuResume);
        self.button(c, Rect::new(inner.x, row(1), half, bh), "Save game", BTN_BG, Action::MenuSave);
        self.button(c, Rect::new(inner.x + half + gap, row(1), half, bh), "Pause", BTN_BG, Action::MenuPause);
        self.button(c, Rect { y: row(2), ..full }, "Settings...", BTN_BG, Action::MenuSettings);
        self.button(c, Rect { y: row(3), ..full }, "Quit game", BTN_DANGER, Action::MenuQuit);
    }

    /// "Quit this game?" with Yes / No (No goes back to the game menu).
    fn draw_quit_confirm(&mut self, c: &mut Layer) {
        let bh = self.u(40.0);
        let gap = self.u(7.0);
        let inner = self.dialog(c, 2, "Quit this game?");
        let half = (inner.w - gap) / 2;
        let px = self.k() * 13.0;
        c.text_centered(inner.x, inner.y, inner.w, bh, px, Weight::Regular, TEXT_DIM, "Progress since the last save is lost.");
        let y = inner.y + bh + gap;
        self.button(c, Rect::new(inner.x, y, half, bh), "No", BTN_BG, Action::Menu);
        self.button(c, Rect::new(inner.x + half + gap, y, half, bh), "Yes, quit", BTN_DANGER, Action::MenuQuitYes);
    }

    /// Settings: display (crate::display: mode / filter / FPS counter), zoom, HUD size, full-screen map, UI mode.
    fn draw_settings(&mut self, m: &mut Machine, c: &mut Layer) {
        let bh = self.u(40.0);
        let gap = self.u(7.0);
        let inner = self.dialog(c, 8, "Settings");
        let full = Rect::new(inner.x, inner.y, inner.w, bh);
        let half = (inner.w - gap) / 2;
        let third = (inner.w - 2 * gap) / 3;
        let row = |i: i32| inner.y + i * (bh + gap);
        let d = crate::display::display_settings(m);
        self.button(c, Rect::new(inner.x, row(0), half, bh), &format!("Screen: {}", d.mode.label()), BTN_BG, Action::CycleDisplayMode);
        self.button(c, Rect::new(inner.x + half + gap, row(0), half, bh), &format!("Filter: {}", d.filter.label()), BTN_BG, Action::CycleFilter);
        self.button(c, Rect::new(inner.x, row(1), half, bh), &format!("FPS counter: {}", d.fps_counter.label()), BTN_BG, Action::CycleFps);
        let fl = crate::display::fps_limit_label(m);
        self.button(c, Rect::new(inner.x + half + gap, row(1), half, bh), &fl, BTN_BG, Action::CycleFpsLimit);
        self.button(c, Rect::new(inner.x, row(2), half, bh), &format!("FPS position: {}", d.fps_pos.label()), BTN_BG, Action::CycleFpsPos);
        let fs = format!("Fast speed: {}", crate::speed::choice_label(m));
        self.button(c, Rect::new(inner.x + half + gap, row(2), half, bh), &fs, BTN_BG, Action::CycleFastSpeed);
        self.button(c, Rect::new(inner.x, row(3), third, bh), "Zoom -", BTN_BG, Action::Zoom(-1));
        let zl = zoom_label(m);
        c.text_centered(inner.x + third + gap, row(3), third, bh, self.k() * 13.0, Weight::Bold, TEXT, &zl);
        self.button(c, Rect::new(inner.x + 2 * (third + gap), row(3), third, bh), "Zoom +", BTN_BG, Action::Zoom(1));
        self.button(c, Rect::new(inner.x, row(4), third, bh), "HUD -", BTN_BG, Action::MenuScale(-1));
        c.text_centered(inner.x + third + gap, row(4), third, bh, self.k() * 14.0, Weight::Bold, TEXT, &format!("HUD {:.0}%", self.scale_pref * 100.0));
        self.button(c, Rect::new(inner.x + 2 * (third + gap), row(4), third, bh), "HUD +", BTN_BG, Action::MenuScale(1));
        let fm = if self.fullscreen_map { "Full-screen map: on" } else { "Full-screen map: off" };
        self.button(c, Rect::new(inner.x, row(5), half, bh), fm, BTN_BG, Action::MenuToggleFullMap);
        let gt = if crate::game_time::enabled(m) { "Game time: on" } else { "Game time: off" };
        self.button(c, Rect::new(inner.x + half + gap, row(5), half, bh), gt, BTN_BG, Action::ToggleGameTime);
        let mode = crate::settings(m).ui_mode;
        let mode_label = if mode == UiMode::MobileHud { "UI: Mobile HUD  >  Classic Touch" } else { "UI: Classic  >  Mobile HUD" };
        self.button(c, Rect { y: row(6), ..full }, mode_label, BTN_BG, Action::MenuToggleMode);
        self.button(c, Rect { y: row(7), ..full }, "< Back", BTN_ACTIVE, Action::SettingsBack);
    }

    /// Resource strip (top left; the FPS counter uses the top-right corner): building materials, food and population, with the game's own goods icons.
    fn draw_resources(&mut self, c: &mut Layer) {
        let Some(s) = self.snap.as_ref() else { return };
        let gv = |g: usize| s.goods.get(g).copied().unwrap_or(0);
        // (icon good id, amount, fallback label). Food = bread + fish + meat.
        let items: Vec<(Option<u8>, u32, &str)> = vec![
            (Some(1), gv(1), "Boards"),
            (Some(2), gv(2), "Stone"),
            (Some(3), gv(3), "Logs"),
            (Some(4), gv(4) + gv(8) + gv(24), "Food"),
            (None, s.settlers.iter().sum(), "Pop"),
        ];
        let px = self.k() * 13.0;
        let h = self.u(24.0);
        let isz = self.u(18.0);
        let pad = self.u(6.0);
        let label = |it: &(Option<u8>, u32, &str), has_icon: bool| {
            if has_icon { it.1.to_string() } else { format!("{} {}", it.2, it.1) }
        };
        let has: Vec<bool> = items.iter().map(|it| it.0.is_some_and(|g| self.good_icon(g).is_some())).collect();
        let widths: Vec<i32> = items
            .iter()
            .zip(&has)
            .map(|(it, &hi)| measure_text(px, Weight::Bold, &label(it, hi)).0 + if hi { isz + self.u(4.0) } else { self.u(10.0) } + pad)
            .collect();
        let total: i32 = widths.iter().sum::<i32>() + pad;
        let y0 = self.view.y + self.u(4.0);
        let x0 = self.view.x + self.u(4.0);
        c.fill_rounded_rect(x0, y0, total, h, self.u(10.0) as f32, Rgba::rgba(18, 22, 30, 200));
        let mut x = x0 + pad;
        for ((it, &hi), wd) in items.iter().zip(&has).zip(widths) {
            let t = label(it, hi);
            let ty = y0 + (h - measure_text(px, Weight::Bold, &t).1) / 2;
            if hi {
                let img = self.good_icon(it.0.unwrap()).cloned().unwrap_or_default();
                blit_fit(c, &img, Rect::new(x, y0 + (h - isz) / 2, isz, isz));
                c.text(x + isz + self.u(4.0), ty, px, Weight::Bold, TEXT, &t);
            } else {
                c.fill_circle(x as f32 + self.k() * 4.0, y0 as f32 + h as f32 / 2.0, self.k() * 4.0, ACCENT);
                c.text(x + self.u(10.0), ty, px, Weight::Bold, TEXT, &t);
            }
            x += wd;
        }
        // Game speed pill right of the strip (single player).
        if let Some((sp, fast)) = self.speed.clone() {
            let t = format!(">> {sp}");
            let sw = measure_text(px, Weight::Bold, &t).0 + self.u(18.0);
            let r = Rect::new(x0 + total + self.u(6.0), y0, sw, h);
            let bg = if fast { ACCENT.fade(0.85) } else { Rgba::rgba(18, 22, 30, 200) };
            c.fill_rounded_rect(r.x, r.y, r.w, r.h, self.u(10.0) as f32, bg);
            c.text_centered(r.x, r.y, r.w, r.h, px, Weight::Bold, TEXT, &t);
            self.add(r, Action::Speed);
        }
    }

    fn good_icon(&self, g: u8) -> Option<&ImageRef> {
        self.good_icons.iter().find(|(k, _)| *k == g).map(|(_, i)| i)
    }

    fn draw_selection(&mut self, m: &mut Machine, c: &mut Layer) {
        let Some(s) = self.snap.clone() else { return };
        let w = self.view.x + self.view.w;
        let h = self.view.y + self.view.h;
        let cw = self.u(250.0);
        let bh = self.u(36.0);
        if s.page == 5 && !s.selection.is_empty() {
            let lines: Vec<String> = s.sel_types.iter().take(3).map(|(t, n)| format!("{n} x {}", settler_label(self, m, *t))).collect();
            let ch = self.u(34.0) + lines.len() as i32 * self.u(16.0) + bh + self.u(8.0);
            let r = Rect::new(w - cw - self.u(6.0), h - self.u(BAR_H) - ch - self.u(6.0), cw, ch);
            c.fill_rounded_rect(r.x, r.y, r.w, r.h, self.u(10.0) as f32, SHEET_BG);
            self.add(r, Action::Nothing);
            c.text(r.x + self.u(10.0), r.y + self.u(6.0), self.k() * 15.0, Weight::Bold, ACCENT, &format!("{} selected", s.selection.len()));
            for (i, l) in lines.iter().enumerate() {
                c.text(r.x + self.u(10.0), r.y + self.u(28.0) + i as i32 * self.u(16.0), self.k() * 13.0, Weight::Regular, TEXT, l);
            }
            let by = r.y + r.h - bh - self.u(6.0);
            let half = (r.w - self.u(26.0)) / 2;
            self.button(c, Rect::new(r.x + self.u(10.0), by, half, bh), "Move to...", BTN_ACTIVE, Action::MoveUnits);
            self.button(c, Rect::new(r.x + self.u(16.0) + half, by, half, bh), "Deselect", BTN_BG, Action::Deselect);
        } else if s.page == 4 && s.building != 0 {
            let name = self.building_name(m, s.building_type);
            let ch = self.u(40.0) + bh + self.u(8.0);
            let r = Rect::new(w - cw - self.u(6.0), h - self.u(BAR_H) - ch - self.u(6.0), cw, ch);
            c.fill_rounded_rect(r.x, r.y, r.w, r.h, self.u(10.0) as f32, SHEET_BG);
            self.add(r, Action::Nothing);
            c.text(r.x + self.u(10.0), r.y + self.u(8.0), self.k() * 15.0, Weight::Bold, ACCENT, &name);
            let by = r.y + r.h - bh - self.u(6.0);
            let third = (r.w - self.u(32.0)) / 3;
            let armed = self.now < self.destroy_armed_ms;
            self.button(c, Rect::new(r.x + self.u(10.0), by, third, bh), "Details", BTN_BG, Action::BuildingDetails);
            self.button(c, Rect::new(r.x + self.u(16.0) + third, by, third, bh), if armed { "Sure?" } else { "Destroy" }, BTN_DANGER, Action::Destroy);
            self.button(c, Rect::new(r.x + self.u(22.0) + 2 * third, by, third, bh), "Close", BTN_BG, Action::Deselect);
        }
    }

    fn draw_placement(&mut self, m: &mut Machine, c: &mut Layer) {
        let Some(s) = self.snap.clone() else { return };
        if s.page != 1 || s.slot == 0 || s.btype == 0 {
            return;
        }
        let name = self.building_name(m, s.btype as u8);
        let v = self.view;
        let bw = self.u(330.0).min(v.w - self.u(150.0));
        let r = Rect::new(v.x + (v.w - bw) / 2, v.y + self.u(32.0), bw, self.u(44.0));
        c.fill_rounded_rect(r.x, r.y, r.w, r.h, self.u(10.0) as f32, SHEET_BG);
        self.add(r, Action::Nothing);
        let cat = s.cat;
        let ir = Rect::new(r.x + self.u(4.0), r.y + self.u(4.0), self.u(52.0), r.h - self.u(8.0));
        if let Some(img) = self.icon(cat, s.slot).cloned() {
            blit_fit(c, &img, ir);
        }
        c.text(ir.x + ir.w + self.u(6.0), r.y + self.u(5.0), self.k() * 14.0, Weight::Bold, ACCENT, &name);
        c.text(ir.x + ir.w + self.u(6.0), r.y + self.u(23.0), self.k() * 12.0, Weight::Regular, TEXT_DIM, "Tap the map to place");
        let cb = Rect::new(r.x + r.w - self.u(84.0), r.y + self.u(5.0), self.u(78.0), r.h - self.u(10.0));
        self.button(c, cb, "Cancel", BTN_DANGER, Action::CancelBuild);
    }

    /// Minimap widget (bottom left, above the bar): the game's minimap image + camera rectangle.
    fn draw_minimap_widget(&mut self, c: &mut Layer) {
        self.mini_geom = None;
        let (Some(d), Some(img)) = (self.mini.clone(), self.mini_img.clone()) else { return };
        let v = self.view;
        let h = self.u(96.0);
        let w = h * 3 / 2;
        let pad = self.u(5.0);
        let card = Rect::new(v.x + self.u(6.0), v.y + v.h - self.u(BAR_H) - h - 2 * pad - self.u(6.0), w + 2 * pad, h + 2 * pad);
        let pressed = self.pressed.as_ref().is_some_and(|(_, a)| *a == Action::Minimap);
        c.fill_rounded_rect(card.x, card.y, card.w, card.h, self.u(10.0) as f32, Rgba::rgba(12, 15, 22, 215));
        c.stroke_rounded_rect(card.x, card.y, card.w, card.h, self.u(10.0) as f32, 1.0, if pressed { CARD_HI } else { Rgba::rgba(255, 255, 255, 60) });
        let g = MapGeom::fit(d.n, card.x + pad, card.y + pad, w, h);
        minimap::draw(c, &img, &d, &g, Rgba::WHITE);
        // Small "expand" hint in the top-left corner (empty part of the sheared shape).
        let hx = card.x + self.u(7.0);
        let hy = card.y + self.u(7.0);
        let hs = self.u(10.0);
        c.fill_rect(hx, hy, hs, 2.max(self.u(1.5)), ACCENT);
        c.fill_rect(hx, hy, 2.max(self.u(1.5)), hs, ACCENT);
        self.mini_geom = Some(g);
        self.add(card, Action::Minimap);
    }

    /// Full-screen map overlay: the whole map scaled to the screen, camera rectangle, tap = jump + close.
    fn draw_full_map(&mut self, c: &mut Layer) {
        self.map_geom = None;
        let v = self.view;
        c.fill_rect(v.x, v.y, v.w, v.h, Rgba::rgba(8, 10, 16, 240));
        self.add(v, Action::Nothing);
        let top = self.u(40.0);
        c.text(v.x + self.u(12.0), v.y + self.u(10.0), self.k() * 17.0, Weight::Bold, ACCENT, "Map");
        c.text(v.x + self.u(60.0), v.y + self.u(13.0), self.k() * 12.0, Weight::Regular, TEXT_DIM, "Tap a place to go there");
        match (self.mini.clone(), self.mini_img.clone()) {
            (Some(d), Some(img)) => {
                let g = MapGeom::fit(d.n, v.x + self.u(8.0), v.y + top, v.w - self.u(16.0), v.h - top - self.u(8.0));
                minimap::draw(c, &img, &d, &g, Rgba::WHITE);
                minimap::draw_outline(c, &g, Rgba::rgba(255, 255, 255, 70));
                let (bw, bh) = g.box_size();
                self.add(Rect::new(g.x as i32, g.y as i32, bw as i32, bh as i32), Action::MapPoint);
                self.map_geom = Some(g);
            }
            _ => {
                c.text_centered(v.x, v.y + v.h / 2 - self.u(12.0), v.w, self.u(24.0), self.k() * 14.0, Weight::Regular, TEXT_DIM, "Map not available yet");
            }
        }
        let close = Rect::new(v.x + v.w - self.u(96.0), v.y + self.u(6.0), self.u(90.0), self.u(32.0));
        self.button(c, close, "Close", BTN_ACTIVE, Action::CloseSheet);
    }

    fn overwrite_armed(&self, slot: u32) -> bool {
        self.overwrite.is_some_and(|(s, until)| s == slot && self.now < until)
    }

    /// A grid of buttons (`cols` per row) from the top of `area`; returns the y below the last row.
    fn button_grid(&mut self, c: &mut Layer, area: Rect, cols: i32, items: Vec<(String, Rgba, Action)>) -> i32 {
        let bh = self.u(36.0);
        let gap = self.u(6.0);
        let bw = (area.w - gap * (cols - 1)) / cols;
        let mut y = area.y;
        for (i, (label, bg, a)) in items.into_iter().enumerate() {
            let (col, row) = (i as i32 % cols, i as i32 / cols);
            let r = Rect::new(area.x + col * (bw + gap), area.y + row * (bh + gap), bw, bh);
            let pressed = self.pressed.as_ref().is_some_and(|(_, p)| *p == a);
            c.fill_rounded_rect(r.x, r.y, r.w, r.h, self.u(8.0) as f32, if pressed { CARD_HI } else { bg });
            draw_wrapped(c, r.inset(self.u(4.0)), self.k() * 12.0, &label);
            self.add(r, a);
            y = r.y + r.h + gap;
        }
        y
    }

    /// The selected building (replaces the original panel's page 4): state line and its actions.
    fn draw_building_sheet(&mut self, m: &mut Machine, c: &mut Layer) {
        let Some(s) = self.snap.clone() else { return };
        if s.page != 4 || s.building == 0 {
            self.sheet = Sheet::None;
            return;
        }
        let t = s.building_type;
        let own = s.building_owner as u32 == s.player;
        let working = s.building_flags & 1 != 0;
        let has_priority = s.building_flags & 0x1e != 0;
        let mut actions: Vec<(String, Rgba, Action)> = Vec::new();
        let mut garrison: Vec<(String, Rgba, Action)> = Vec::new();
        let status;
        if s.building_site {
            status = format!(
                "Construction site{}{}",
                if working { "" } else { "  -  stopped" },
                if has_priority { "  -  highest priority" } else { "" }
            );
            if own {
                actions.push((self.name(m, if working { 0x80053 } else { 0x80054 }), BTN_BG, Action::ToggleWork));
                if has_priority {
                    actions.push(("Normal priority".into(), BTN_ACTIVE, Action::SitePriority));
                } else if working && s.priority_sites < 10 {
                    actions.push((self.name(m, 0x80063), BTN_BG, Action::SitePriority));
                }
            }
        } else {
            status = if STOP_WORK_TYPES.contains(&t) {
                if working { "Working".to_string() } else { "Stopped".to_string() }
            } else if GARRISON_TYPES.contains(&t) {
                "Military building".to_string()
            } else {
                String::new()
            };
            if own {
                if STOP_WORK_TYPES.contains(&t) {
                    let bg = if working { BTN_BG } else { BTN_ACTIVE };
                    actions.push((self.name(m, if working { 0x80055 } else { 0x80056 }), bg, Action::ToggleWork));
                }
                if WORK_AREA_TYPES.contains(&t) {
                    actions.push((self.name(m, 0x80059), BTN_BG, Action::WorkArea));
                }
                // The original page links: barracks -> recruitment, residences -> population (our settlers sheet).
                if t == 0x13 || (0x1a..=0x1c).contains(&t) {
                    actions.push((self.name(m, if t == 0x13 { 0x8005c } else { 0x8005a }), BTN_BG, Action::Settlers));
                }
                if GARRISON_TYPES.contains(&t) {
                    // Original layout: "add" row above the "remove" row, Maximum / Minimum Garrison at the ends.
                    for &(n, tid) in GARRISON_ACTIONS.iter().filter(|(n, _)| (2..=4).contains(n)).chain(&GARRISON_ACTIONS[6..7]) {
                        garrison.push((self.name(m, tid), BTN_BG, Action::Garrison(n)));
                    }
                    for &(n, tid) in GARRISON_ACTIONS.iter().filter(|(n, _)| (6..=8).contains(n)).chain(&GARRISON_ACTIONS[7..8]) {
                        garrison.push((self.name(m, tid), BTN_BG, Action::Garrison(n)));
                    }
                }
            }
        }
        let armed = self.now < self.destroy_armed_ms;
        let mut last: Vec<(String, Rgba, Action)> = Vec::new();
        if own {
            let crush = self.name(m, if s.building_site { 0x80057 } else { 0x80058 });
            last.push((if armed { "Sure? Tap again".into() } else { crush }, BTN_DANGER, Action::Destroy));
        }
        last.push(("Close".into(), BTN_BG, Action::Deselect));
        let cols = 4;
        let bh = self.u(36.0) + self.u(6.0);
        let rows = |n: usize| (n as i32 + cols - 1) / cols;
        let h = self.u(64.0) + (rows(actions.len()) + rows(garrison.len()) + 1) * bh + if garrison.is_empty() { 0 } else { self.u(20.0) };
        let r = self.sheet_rect(c, h);
        let name = self.building_name(m, t);
        let inner = self.draw_sheet_frame(c, r, &name);
        c.text(inner.x, inner.y, self.k() * 13.0, Weight::Regular, TEXT_DIM, if status.is_empty() { " " } else { &status });
        let mut y = inner.y + self.u(22.0);
        if !actions.is_empty() {
            y = self.button_grid(c, Rect::new(inner.x, y, inner.w, 0), cols, actions);
        }
        if !garrison.is_empty() {
            c.text(inner.x, y, self.k() * 12.0, Weight::Bold, ACCENT, "Garrison");
            y += self.u(18.0);
            y = self.button_grid(c, Rect::new(inner.x, y, inner.w, 0), cols, garrison);
        }
        let _ = y;
        let lw = (inner.w - self.u(6.0)) / 2;
        let ly = inner.y + inner.h - self.u(36.0);
        self.button_grid(c, Rect::new(inner.x + inner.w - lw * 2 - self.u(6.0), ly, lw * 2 + self.u(6.0), 0), 2, last);
    }

    /// Save game (replaces the original panel's page 7 slot list): the 8 slots as the game describes them.
    fn draw_save_sheet(&mut self, m: &mut Machine, c: &mut Layer) {
        let slots = m.state::<HudShared>().borrow_mut().save_slots.clone();
        self.save_slots_drawn = slots.is_some();
        let bh = self.u(40.0);
        let gap = self.u(7.0);
        let inner = self.dialog_w(c, 5, 560.0, "Save game");
        let half = (inner.w - gap) / 2;
        let row = |i: i32| inner.y + i * (bh + gap);
        let px = self.k() * 12.0;
        if let Some((slot, _)) = self.saving {
            let t = if slot == 0 { "Saving...".to_string() } else { format!("Saving into slot {slot}...") };
            c.text_centered(inner.x, row(1), inner.w, bh, self.k() * 15.0, Weight::Bold, TEXT, &t);
        } else {
            match slots {
                None => c.text_centered(inner.x, row(1), inner.w, bh, px, Weight::Regular, TEXT_DIM, "Reading save slots..."),
                Some(sl) if sl.multiplayer => {
                    self.button(c, Rect::new(inner.x, row(1), inner.w, bh), "Save multiplayer game", BTN_ACTIVE, Action::SaveSlot(0));
                }
                Some(sl) => {
                    for (i, info) in sl.slots.iter().enumerate().take(8) {
                        let (col, rr) = ((i / 4) as i32, (i % 4) as i32);
                        let r = Rect::new(inner.x + col * (half + gap), row(rr), half, bh);
                        let a = Action::SaveSlot(i as u32 + 1);
                        let pressed = self.pressed.as_ref().is_some_and(|(_, p)| *p == a);
                        c.fill_rounded_rect(r.x, r.y, r.w, r.h, self.u(8.0) as f32, if pressed { CARD_HI } else { BTN_BG });
                        let (l1, l2) = match info {
                            Some([a1, b, c2]) => {
                                // Line 1 is the map path (e.g. "\Random"): show its last part.
                                let map = a1.trim().rsplit(['\\', '/']).next().unwrap_or("").to_string();
                                let l1 = if map.is_empty() { format!("{}. Saved game", i + 1) } else { format!("{}. {map}", i + 1) };
                                let l1 = if self.overwrite_armed(i as u32 + 1) { format!("{}. Tap again to overwrite", i + 1) } else { l1 };
                                (l1, format!("{} {}", b.trim(), c2.trim()).trim().to_string())
                            }
                            None => (format!("{}. Empty slot", i + 1), String::new()),
                        };
                        c.set_clip(r.x, r.y, r.w, r.h);
                        let tx = r.x + self.u(8.0);
                        if l2.is_empty() {
                            let th = measure_text(px, Weight::Bold, &l1).1;
                            c.text(tx, r.y + (r.h - th) / 2, px, Weight::Bold, TEXT, &l1);
                        } else {
                            c.text(tx, r.y + self.u(4.0), px, Weight::Bold, TEXT, &l1);
                            c.text(tx, r.y + self.u(21.0), self.k() * 11.0, Weight::Regular, TEXT_DIM, &l2);
                        }
                        c.reset_clip();
                        self.add(r, a);
                    }
                }
            }
        }
        self.button(c, Rect::new(inner.x, row(4), inner.w, bh), "< Back", BTN_BG, Action::Menu);
    }

    fn draw_preparing(&mut self, c: &mut Layer) {
        let (w, h) = (c.width() as i32, c.height() as i32);
        c.fill_rect(0, 0, w, h, Rgba::rgba(10, 12, 18, 235));
        c.text_centered(0, h / 2 - self.u(30.0), w, self.u(30.0), self.k() * 20.0, Weight::Bold, ACCENT, "Preparing touch HUD");
        c.text_centered(0, h / 2, w, self.u(24.0), self.k() * 13.0, Weight::Regular, TEXT_DIM, "Reading building icons from your game...");
        self.add(Rect::new(0, 0, w, h), Action::Nothing);
    }

    fn draw_toast(&mut self, c: &mut Layer) {
        let Some((t, until)) = self.toast.clone() else { return };
        if self.now >= until {
            self.toast = None;
            return;
        }
        let px = self.k() * 14.0;
        let (tw, th) = measure_text(px, Weight::Bold, &t);
        let v = self.view;
        let r = Rect::new(v.x + (v.w - tw) / 2 - self.u(14.0), v.y + self.u(84.0), tw + self.u(28.0), th + self.u(14.0));
        c.fill_rounded_rect(r.x, r.y, r.w, r.h, (r.h / 2) as f32, Rgba::rgba(10, 12, 18, 210));
        c.text_centered(r.x, r.y, r.w, r.h, px, Weight::Bold, TEXT, &t);
    }
}

impl Mod for MobileHud {
    fn name(&self) -> &str {
        "ui-hud"
    }

    fn tick(&mut self, m: &mut Machine, now: u64) {
        let speed = crate::speed::button_state(m);
        if speed != self.speed {
            self.speed = speed;
            self.dirty = true;
        }
        self.now = now;
        let (mode, scale) = {
            let st = m.state::<ModsState>();
            let s = st.borrow_mut();
            (s.settings.ui_mode, s.settings.hud_scale)
        };
        let active = mode == UiMode::MobileHud;
        if active != self.active {
            self.active = active;
            self.sheet = Sheet::None;
            self.dirty = true;
        }
        self.scale_pref = scale;
        if !self.active {
            self.sync_layout(m);
            return;
        }
        let (last_panel, hooked) = {
            let st = m.state::<HudShared>();
            let s = st.borrow_mut();
            (s.last_panel_ms, s.hooked)
        };
        // In game = the panel renderer ran recently (it runs every in-game frame, even when skipped).
        let in_game = hooked && last_panel != 0 && now.saturating_sub(last_panel) < 1500;
        if in_game != self.in_game {
            self.in_game = in_game;
            self.dirty = true;
            if !in_game {
                self.sheet = Sheet::None;
                self.saving = None;
            }
        }
        if now.saturating_sub(self.last_snap_ms) >= 200 {
            self.last_snap_ms = now;
            let snap = if in_game { game::snapshot(m) } else { None };
            if snap != self.snap {
                self.snap = snap;
                self.dirty = true;
            }
            if !self.text_dumped && std::env::var("S3_HUD_TEXT_DUMP").is_ok() {
                self.dump_texts(m);
            }
        }
        // Save sheet: result of a queued save, or slot list read by the logic thread.
        let (result, slots_ready) = {
            let st = m.state::<HudShared>();
            let mut s = st.borrow_mut();
            (s.save_result.take(), s.save_slots.is_some())
        };
        if let Some((slot, ok)) = result {
            self.saving = None;
            let msg = match (slot, ok) {
                (0, _) => "Saving the multiplayer game".to_string(),
                (n, true) => format!("Game saved (slot {n})"),
                (n, false) => format!("Saving into slot {n} failed"),
            };
            self.set_toast(&msg);
            if self.sheet == Sheet::Save {
                self.sheet = Sheet::None;
            }
        } else if self.saving.is_some_and(|(_, t)| now.saturating_sub(t) > 20_000) {
            self.saving = None;
            self.dirty = true;
        }
        if self.overwrite.is_some_and(|(_, until)| now >= until) {
            self.overwrite = None;
            self.dirty = true;
        }
        if self.sheet == Sheet::Save && slots_ready != self.save_slots_drawn {
            self.dirty = true;
        }
        // Minimap image + camera rectangle (only while our widget / full-screen map can show it).
        if in_game && now.saturating_sub(self.last_mini_ms) >= MINI_REFRESH_MS {
            self.last_mini_ms = now;
            let d = if game::full_layout_active(m) { minimap::read(m) } else { None };
            if d != self.mini {
                if std::env::var("S3_HUD_DEBUG").is_ok() {
                    if let Some(d) = &d {
                        eprintln!("ui-hud: minimap n {} world {} cam {:?}", d.n, d.world, d.cam);
                    }
                }
                let same_pixels = matches!((&d, &self.mini), (Some(a), Some(b)) if a.n == b.n && a.pixels == b.pixels);
                if !same_pixels {
                    self.mini_img = d.as_ref().map(|d| Arc::new(minimap::to_image(d)));
                }
                self.mini = d;
                self.dirty = true;
            }
        }
        self.harvest_tick(m);
        self.sync_layout(m);
    }

    fn wants_draw(&self) -> bool {
        self.active && self.in_game
    }

    fn needs_redraw(&self) -> bool {
        self.active && self.dirty
    }

    fn draw_window(&mut self, m: &mut Machine, c: &mut Layer, frame: &FrameBuffer) {
        self.frame = (c.width(), c.height());
        let vp = crate::draw_viewport(m);
        self.view = visible_view(&vp, c.width() as i32, c.height() as i32);
        self.dirty = false;
        self.targets.clear();
        self.mini_geom = None;
        self.map_geom = None;
        if !self.active || !self.in_game {
            return;
        }
        self.harvest_capture(m, frame);
        if let Harvest::Cat { .. } = self.harvest {
            self.draw_preparing(c);
            return;
        }
        let pw = m.mem.read_u32(game::L_PANEL_W) as i32;
        if pw != 0 {
            // Cover mode (full-screen map off, or the landscape buffer is still too narrow until the next
            // display setup): hide the original panel below the minimap; the game's minimap stays usable.
            let (fw, fh) = (frame.width as f32, frame.height as f32);
            let mini_bottom = m.mem.read_u32(game::L_MINI_BOTTOM) as f32;
            let (x0, top) = vp.game_to_window(0.0, mini_bottom);
            let (x1, y1) = vp.game_to_window((pw as f32).min(fw), fh);
            let (x0, x1, y1) = (x0.round() as i32, x1.round() as i32, y1.round() as i32);
            let top = top.round() as i32 + self.u(4.0);
            let r = Rect::new(x0, top, x1 - x0, y1 - top);
            c.fill_rect(r.x, r.y, r.w, r.h, Rgba::rgb(16, 19, 26));
            c.fill_rect(x1 - 1, top, 1, r.h, Rgba::rgba(255, 255, 255, 40));
            self.add(r, Action::Nothing);
        }
        if self.sheet == Sheet::Map {
            if self.widget_active(m) {
                self.draw_full_map(c);
                self.draw_toast(c);
                return;
            }
            self.sheet = Sheet::None;
        }
        self.draw_resources(c);
        self.draw_placement(m, c);
        if matches!(self.sheet, Sheet::None) {
            if self.widget_active(m) {
                self.draw_minimap_widget(c);
            }
            self.draw_selection(m, c);
        }
        match self.sheet {
            Sheet::Build(cat) => self.draw_build_sheet(m, c, cat),
            Sheet::Settlers => self.draw_settlers_sheet(m, c),
            Sheet::Stats => self.draw_stats_sheet(m, c),
            Sheet::Menu => self.draw_menu(c),
            Sheet::Settings => self.draw_settings(m, c),
            Sheet::QuitConfirm => self.draw_quit_confirm(c),
            Sheet::Building => self.draw_building_sheet(m, c),
            Sheet::Save => self.draw_save_sheet(m, c),
            Sheet::Map | Sheet::None => {}
        }
        self.draw_bar(c);
        self.draw_toast(c);
    }

    fn on_touch(&mut self, m: &mut Machine, ev: &TouchEvent) -> ModEvent {
        if !self.active || !self.in_game {
            return ModEvent::Pass;
        }
        // HUD elements live in window pixels.
        let (x, y) = crate::touch_window_pos(m, ev);
        match *ev {
            TouchEvent::Down { id, .. } => {
                if let Some(a) = self.hit(x, y) {
                    if a == Action::Minimap {
                        self.mini_drag = Some((x, y, false));
                        self.mini_last_tile = None;
                    }
                    self.pressed = Some((id, a));
                    self.dirty = true;
                    return ModEvent::Consumed;
                }
                if self.sheet != Sheet::None {
                    // Tap outside an open sheet closes it (and does not reach the map).
                    self.pressed = Some((id, Action::CloseSheet));
                    return ModEvent::Consumed;
                }
                ModEvent::Pass
            }
            TouchEvent::Move { id, .. } => {
                if let Some((pid, Action::Minimap)) = &self.pressed {
                    if *pid == id {
                        if let Some((x0, y0, dragging)) = self.mini_drag {
                            let slop = self.k() * 7.0;
                            let drag = dragging || (x - x0).abs() > slop || (y - y0).abs() > slop;
                            self.mini_drag = Some((x0, y0, drag));
                            if drag {
                                if let Some(g) = self.mini_geom {
                                    self.center_at(m, &g, x, y);
                                }
                            }
                        }
                        return ModEvent::Consumed;
                    }
                }
                ModEvent::Pass
            }
            TouchEvent::Up { id, .. } => {
                let Some((pid, a)) = self.pressed.take() else { return ModEvent::Pass };
                if pid != id {
                    self.pressed = Some((pid, a));
                    return ModEvent::Pass;
                }
                self.dirty = true;
                if a == Action::Minimap {
                    // Tap (no drag) = full-screen map; a drag already moved the camera.
                    let dragged = self.mini_drag.take().is_some_and(|d| d.2);
                    if !dragged {
                        self.run(m, Action::Minimap);
                    }
                    return ModEvent::Consumed;
                }
                if a == Action::MapPoint {
                    if let Some(g) = self.map_geom {
                        if g.in_box(x, y) {
                            self.center_at(m, &g, x, y);
                            self.sheet = Sheet::None;
                        }
                    }
                    return ModEvent::Consumed;
                }
                let still_inside = match &a {
                    Action::CloseSheet => true,
                    a => self.targets.iter().any(|(r, t)| t == a && r.contains(x, y)),
                };
                if still_inside {
                    self.run(m, a);
                }
                ModEvent::Consumed
            }
            TouchEvent::Cancel { id } => {
                if self.pressed.as_ref().is_some_and(|(p, _)| *p == id) {
                    self.pressed = None;
                    self.mini_drag = None;
                    self.dirty = true;
                }
                ModEvent::Pass
            }
            _ => ModEvent::Pass,
        }
    }

    fn on_key(&mut self, _m: &mut Machine, vk: u32, down: bool) -> ModEvent {
        // Back (Android) / Esc closes the full-screen map, the settings page (back to the menu) or a sheet.
        if vk != 0x1b || !self.active || !self.in_game {
            return ModEvent::Pass;
        }
        let next = match self.sheet {
            Sheet::None => return ModEvent::Pass,
            Sheet::Settings | Sheet::QuitConfirm | Sheet::Save => Sheet::Menu,
            _ => Sheet::None,
        };
        if !down {
            self.sheet = next;
            self.dirty = true;
        }
        ModEvent::Consumed
    }
}

impl MobileHud {
    fn hit(&self, x: f32, y: f32) -> Option<Action> {
        self.targets.iter().rev().find(|(r, _)| r.contains(x, y)).map(|(_, a)| a.clone())
    }

    /// Debug (`S3_HUD_TEXT_DUMP=1`): log every string of the game's text tables once (game data: local logs only).
    fn dump_texts(&mut self, m: &mut Machine) {
        if m.state::<HudShared>().borrow_mut().strtab == 0 {
            return;
        }
        self.text_dumped = true;
        for t in 0..0x20u32 {
            let n = game::text_count(m, t).min(2000);
            for i in 0..n {
                if let Some(s) = game::text(m, (t << 16) | i) {
                    eprintln!("hud-text {:#x} {s:?}", (t << 16) | i);
                }
            }
        }
    }
}

// ---------------------------------------------------------------- helpers

/// The player-command request a building-sheet button sends, built like the original page 4 handlers
/// (docs/reverse-engineering/game-state-units.md "Player commands"); `None` if it does not apply.
pub(crate) fn building_request(s: &GameSnapshot, a: &Action) -> Option<LogicAction> {
    let b = s.building;
    if b == 0 || s.building_owner as u32 != s.player {
        return None;
    }
    let (hi, lo, player) = ((b >> 8) as u8, b as u8, s.player as u8);
    let working = s.building_flags & 1 != 0;
    let (bytes, sound) = match *a {
        // 0x531d4a: sound 0x35 (resume) / 0x36 (stop), new bit 0 = !bit 0.
        Action::ToggleWork => (vec![0x21, 0x04, 0x08, 0, hi, lo, (!s.building_flags) & 1, player], if working { 0x36 } else { 0x35 }),
        // 0x531e72: priority bits clear -> 0xf5 (only while building and below 10 priority sites), else 0xf6.
        Action::SitePriority if s.building_site => {
            if s.building_flags & 0x1e == 0 {
                if !working || s.priority_sites >= 10 {
                    return None;
                }
                (vec![0x21, 0xf5, 0x08, 0, hi, lo, s.building_owner, 0], 0x2f)
            } else {
                (vec![0x21, 0xf6, 0x08, 0, hi, lo, s.building_owner, 0], 0x30)
            }
        }
        // 0x531499..0x531ab6: command 0x0c {building, player, action}; sounds 0x31 max, 0x2f add, 0x32 min, 0x30 remove.
        Action::Garrison(n) if (1..=8).contains(&n) && GARRISON_TYPES.contains(&s.building_type) && !s.building_site => {
            let sound = match n {
                1 => 0x31,
                5 => 0x32,
                2..=4 => 0x2f,
                _ => 0x30,
            };
            (vec![0x21, 0x0c, 0x08, 0, hi, lo, player, n], sound)
        }
        _ => return None,
    };
    Some(LogicAction::Request { bytes, sound })
}

/// Pixel rectangle of build-list slot `slot` (1..12) in the original panel (game-state-ui.md §2 grid map).
pub(crate) fn slot_rect(slot: u32, cell: i32, gx: i32, gy: i32) -> Rect {
    let k = ((slot - 1) / 2) as i32;
    let col0 = if slot % 2 == 1 { 1 } else { 21 };
    let row0 = 22 + 12 * k;
    Rect::new(gx + col0 * cell, gy + row0 * cell, 19 * cell, 10 * cell)
}

/// Copy a rectangle of the frame into an RGBA image.
fn copy_rect(c: &FrameBuffer, r: Rect) -> Image {
    let (fw, fh) = (c.width as i32, c.height as i32);
    let mut img = Image::new(r.w.max(0) as u32, r.h.max(0) as u32);
    for y in 0..r.h {
        for x in 0..r.w {
            let (sx, sy) = (r.x + x, r.y + y);
            if sx < 0 || sy < 0 || sx >= fw || sy >= fh {
                continue;
            }
            let (cr, cg, cb) = s3_runtime::host::rgb565_to_rgb888(c.pixels[(sy * fw + sx) as usize]);
            let i = ((y * r.w + x) * 4) as usize;
            img.rgba[i..i + 4].copy_from_slice(&[cr, cg, cb, 255]);
        }
    }
    img
}

/// Blit keeping the aspect ratio, centred in `r`.
fn blit_fit(c: &mut Layer, img: &ImageRef, r: Rect) {
    if img.width == 0 || img.height == 0 || r.w <= 0 || r.h <= 0 {
        return;
    }
    let s = (r.w as f32 / img.width as f32).min(r.h as f32 / img.height as f32);
    let (w, h) = ((img.width as f32 * s) as i32, (img.height as f32 * s) as i32);
    c.blit_scaled(img, r.x + (r.w - w) / 2, r.y + (r.h - h) / 2, w, h, 1.0);
}

/// Text in up to two lines inside `r` (vertically centred).
fn draw_wrapped(c: &mut Layer, r: Rect, px: f32, text: &str) {
    let words: Vec<&str> = text.split_whitespace().collect();
    // Shrink the font if a single word does not fit (e.g. "Woodcutter's" in a narrow card).
    let widest = words.iter().map(|w| measure_text(px, Weight::Bold, w).0).max().unwrap_or(0);
    let px = if widest > r.w && widest > 0 { (px * r.w as f32 / widest as f32).max(px * 0.7) } else { px };
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for w in words {
        let cand = if cur.is_empty() { w.to_string() } else { format!("{cur} {w}") };
        if measure_text(px, Weight::Bold, &cand).0 > r.w && !cur.is_empty() {
            lines.push(std::mem::take(&mut cur));
            cur = w.to_string();
        } else {
            cur = cand;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines.truncate(2);
    let lh = measure_text(px, Weight::Bold, "Ag").1;
    let total = lh * lines.len() as i32;
    let mut y = r.y + (r.h - total) / 2;
    for l in lines {
        c.set_clip(r.x, r.y, r.w, r.h);
        c.text(r.x, y, px, Weight::Bold, TEXT, &l);
        c.reset_clip();
        y += lh;
    }
}

fn draw_house(c: &mut Layer, r: Rect, k: f32) {
    let s = (r.w.min(r.h) as f32 * 0.8) as i32;
    let (x, y) = (r.x + (r.w - s) / 2, r.y + (r.h - s) / 2);
    c.fill_rect(x + s / 6, y + s / 2, s * 2 / 3, s / 2, TEXT_DIM);
    for i in 0..s / 2 {
        c.fill_rect(x + s / 2 - i, y + i, 2 * i + 1, 1, ACCENT);
    }
    let _ = k;
}

fn draw_person(c: &mut Layer, r: Rect, k: f32) {
    let (cx, cy) = (r.x as f32 + r.w as f32 / 2.0, r.y as f32 + r.h as f32 / 2.0);
    c.fill_circle(cx, cy - 6.0 * k, 5.0 * k, TEXT);
    c.fill_rounded_rect((cx - 8.0 * k) as i32, (cy) as i32, (16.0 * k) as i32, (10.0 * k) as i32, 4.0 * k, TEXT);
}

fn draw_bars(c: &mut Layer, r: Rect, k: f32) {
    let base = r.y + r.h - (2.0 * k) as i32;
    let bw = (5.0 * k) as i32;
    let x0 = r.x + r.w / 2 - (10.0 * k) as i32;
    for (i, hh) in [8.0, 14.0, 10.0].iter().enumerate() {
        let h = (hh * k) as i32;
        c.fill_rect(x0 + i as i32 * (bw + (2.0 * k) as i32), base - h, bw, h, if i == 1 { ACCENT } else { TEXT });
    }
}

fn draw_burger(c: &mut Layer, r: Rect, k: f32) {
    let w = (20.0 * k) as i32;
    let x = r.x + (r.w - w) / 2;
    let y = r.y + r.h / 2 - (7.0 * k) as i32;
    for i in 0..3 {
        c.fill_rounded_rect(x, y + i * (6.0 * k) as i32, w, (3.0 * k).max(2.0) as i32, 1.5, TEXT);
    }
}

/// The part of the window that shows the game (`Fit` leaves bars), clamped to the layer.
fn visible_view(vp: &crate::display::Viewport, cw: i32, ch: i32) -> Rect {
    let (x, y, w, h) = vp.dst_rect_clipped();
    let (x0, y0) = (x.clamp(0, cw), y.clamp(0, ch));
    let (x1, y1) = ((x + w).clamp(0, cw), (y + h).clamp(0, ch));
    if x1 - x0 < 160 || y1 - y0 < 120 {
        return Rect::new(0, 0, cw, ch);
    }
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

/// Zoom one step (`dir` > 0 = closer). Uses the game's own resolutions (F1..F3, like pinch in touch.rs) until a
/// finer zoom API exists (res-zoom, docs/porting/resolution-zoom.md). Returns a toast text.
fn zoom_step(m: &mut Machine, dir: i32) -> Option<String> {
    let res = m.mem.read_u32(game::L_RES) as i32;
    let scale = if dir > 0 { 1.5 } else { 0.6 };
    match crate::touch_logic::pinch_zoom_key(scale, res) {
        Some(vk) => {
            m.input.push(s3_runtime::HostEvent::Key { vk, down: true });
            m.input.push(s3_runtime::HostEvent::Key { vk, down: false });
            Some(if dir > 0 { "Zoom in".into() } else { "Zoom out".into() })
        }
        None => Some(if dir > 0 { "Closest zoom".into() } else { "Farthest zoom".into() }),
    }
}

/// Current zoom level label (the game resolution).
fn zoom_label(m: &mut Machine) -> String {
    let (w, h) = (m.mem.read_u32(game::L_SCREEN_W), m.mem.read_u32(game::L_SCREEN_H));
    if w == 0 || h == 0 {
        return "-".into();
    }
    format!("{w}x{h}")
}

/// Label of a settler type. Only the soldier kinds seen at the start are known (game-state-units.md;
/// HYPOTHESIS names), the rest show the type number.
fn settler_label(hud: &mut MobileHud, m: &mut Machine, t: u8) -> String {
    let _ = (hud, m);
    match t {
        0 => "Carriers".into(),
        5 => "Swordsmen".into(),
        8 => "Bowmen".into(),
        15 => "Soldiers (3rd)".into(),
        // VERIFIED 2026-11-08 (hud-native): "1 New Geologist" / "5 New Pioneers" turned carriers into these types.
        22 => "Geologists".into(),
        40 => "Pioneers".into(),
        t => format!("Type {t}"),
    }
}

/// Label of a good id. Good ids -> names are not mapped yet (game-state-ui.md open questions).
fn good_label(g: u8) -> String {
    good_name(g).map(str::to_string).unwrap_or_else(|| format!("Good {g}"))
}

/// A stable colour per good id.
fn good_color(g: u8) -> Rgba {
    const P: [Rgba; 8] = [
        Rgba::rgb(214, 160, 92),
        Rgba::rgb(170, 170, 170),
        Rgba::rgb(110, 180, 90),
        Rgba::rgb(220, 200, 80),
        Rgba::rgb(90, 150, 220),
        Rgba::rgb(200, 100, 90),
        Rgba::rgb(160, 110, 200),
        Rgba::rgb(90, 200, 190),
    ];
    P[g as usize % P.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_rects_640() {
        // 640x480: cell 3, origin (8, 152). Slot 1 = cols 1..19, rows 22..31.
        assert_eq!(slot_rect(1, 3, 8, 152), Rect::new(11, 218, 57, 30));
        assert_eq!(slot_rect(2, 3, 8, 152), Rect::new(71, 218, 57, 30));
        assert_eq!(slot_rect(3, 3, 8, 152), Rect::new(11, 254, 57, 30));
    }

    #[test]
    fn rect_contains() {
        let r = Rect::new(10, 10, 20, 20);
        assert!(r.contains(10.0, 10.0) && r.contains(29.9, 29.9) && !r.contains(30.0, 15.0));
    }

    fn snap(btype: u8, site: bool, flags: u8) -> GameSnapshot {
        GameSnapshot { player: 0, building: 0x0102, building_type: btype, building_owner: 0, building_site: site, building_flags: flags, ..Default::default() }
    }

    fn req(s: &GameSnapshot, a: Action) -> Option<(Vec<u8>, u32)> {
        match building_request(s, &a)? {
            LogicAction::Request { bytes, sound } => Some((bytes, sound)),
            _ => None,
        }
    }

    #[test]
    fn building_requests_match_panel_handlers() {
        // Stop working (bit 0 set -> new flag 0, sound 0x36), resume (-> 1, sound 0x35).
        assert_eq!(req(&snap(2, false, 1), Action::ToggleWork), Some((vec![0x21, 4, 8, 0, 1, 2, 0, 0], 0x36)));
        assert_eq!(req(&snap(2, false, 0), Action::ToggleWork), Some((vec![0x21, 4, 8, 0, 1, 2, 1, 0], 0x35)));
        // Site priority on (0xf5) / off (0xf6); not on finished buildings, not while the site is stopped.
        assert_eq!(req(&snap(2, true, 1), Action::SitePriority), Some((vec![0x21, 0xf5, 8, 0, 1, 2, 0, 0], 0x2f)));
        assert_eq!(req(&snap(2, true, 3), Action::SitePriority), Some((vec![0x21, 0xf6, 8, 0, 1, 2, 0, 0], 0x30)));
        assert_eq!(req(&snap(2, true, 0), Action::SitePriority), None);
        assert_eq!(req(&snap(2, false, 1), Action::SitePriority), None);
        // Garrison only on finished towers/castle.
        assert_eq!(req(&snap(0x0f, false, 1), Action::Garrison(1)), Some((vec![0x21, 0x0c, 8, 0, 1, 2, 0, 1], 0x31)));
        assert_eq!(req(&snap(0x0f, false, 1), Action::Garrison(6)), Some((vec![0x21, 0x0c, 8, 0, 1, 2, 0, 6], 0x30)));
        assert_eq!(req(&snap(2, false, 1), Action::Garrison(1)), None);
        // Not for buildings of other players.
        let mut other = snap(2, false, 1);
        other.building_owner = 1;
        assert_eq!(req(&other, Action::ToggleWork), None);
    }

    #[test]
    fn centre_tile_formula_matches_game_arithmetic() {
        // camY 0 -> q = 1600/18 = 88, ebx = -24; rows = (480+255)/9 + 11 = 92; cols = ((640+23)/16 + 10)/2 = 25;
        // x = (1000-208)/16 - (384+24)/2 + 92/4 + 25 = 49 - 204 + 23 + 25 = -107 -> 0 (clamped); y = -24 + 46 = 22.
        assert_eq!(crate::ui_hud_game::centre_tile_formula(1000, 0, 384, 640, 480), (0, 22));
        // camX 5000, camY 1800: q = 188, ebx = 176; x = 299 - 104 + 23 + 25 = 243; y = 176 + 46 = 222.
        assert_eq!(crate::ui_hud_game::centre_tile_formula(5000, 1800, 384, 640, 480), (243, 222));
    }
}
