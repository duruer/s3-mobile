//! Mobile HUD (UI mode B): game-state access for the GOG S3.EXE (sha256 `00a925cf…`).
//!
//! Everything here reads/writes game memory at addresses documented in
//! docs/reverse-engineering/game-state-units.md (part 1) and game-state-ui.md (part 2), plus the hooks the
//! HUD needs (docs/porting/ui-hud.md):
//!
//! - `0x4b4d00` panel renderer (render thread, once per in-game frame): "in game" heartbeat, applies or
//!   restores the **full-screen map layout** (panel width 0, map view = screen width) and skips the panel.
//! - `0x4ff170` `gd::ScreenToMap`: shifts the screen x back to the original layout while the full-screen map is
//!   active (its offsets are constants for the original panel width).
//! - `0x524fa0` `CLogicThread::HandleInput` entry (logic thread): runs queued HUD actions (UI pokes, commands)
//!   where the game itself changes UI state.
//! - `0x5faaa0` text lookup: remembers the string table so the HUD can show the game's own (localised) names.

use s3_runtime::{HookFlow, LogLevel, Machine};

use crate::hooks::{self, HookCtx};

// ---------------------------------------------------------------- addresses (GOG exe only)

/// `*(0x7e3408)` = `CGameData*` (game-state-ui.md).
pub const GD_GLOBAL: u32 = 0x007e_3408;
/// `CGameData` vtable.
pub const GD_VTABLE: u32 = 0x0065_8c18;

pub const FN_PANEL_RENDER: u32 = 0x004b_4d00;
pub const FN_SCREEN_TO_MAP: u32 = 0x004f_f170;
pub const FN_HANDLE_INPUT: u32 = 0x0052_4fa0;
pub const FN_TEXT: u32 = 0x005f_aaa0;
pub const FN_QUEUE_POST: u32 = 0x0043_09c0;
/// `gd::0x4ff3f0` (thiscall, no args): new placement-marker generation (called by the build list click).
pub const FN_PLACEMENT_REFRESH: u32 = 0x004f_f3f0;
/// `gd::CenterOn(tx, ty)` `0x51b560` (thiscall, ret 8): center the camera on a tile, clamps.
pub const FN_CENTER_ON: u32 = 0x0051_b560;
/// `CLogicThread` "end game" (thiscall logic), called by the panel's quit confirmation YES (`0x52e993`).
pub const FN_QUIT_GAME: u32 = 0x0053_e720;
/// UI sound (thiscall logic, sound id); the YES button plays 0x33 (`0x52e99c`).
pub const FN_UI_SOUND: u32 = 0x0053_f560;
/// Layout table setter `0x4c6d70(resIndex)` (stdcall).
pub const FN_LAYOUT: u32 = 0x004c_6d70;
/// Landscape ring-buffer init `0x54bec0` (thiscall on `[gfx+0x5f6cc]`, arg0 = width), called by the display setup.
pub const FN_LANDSCAPE_INIT: u32 = 0x0054_bec0;
/// Frame renderer `0x4b26c0`: minimap blocks skipped in HUD mode (the HUD draws its own minimap,
/// `ui_hud_minimap`). Block 1 = incremental minimap copy into the back buffer (`0x4b2b34` -> `0x4b2ce5`); block 2 =
/// the per-frame minimap part right of the panel, the minimap frame art (resources `0x40001` / `0x40003`) and the
/// white camera rectangle (`0x4b366a` -> `0x4b3cf3`, the selection-box drawing that follows stays).
pub const MINIMAP_BLOCK1: (u32, u32) = (0x004b_2b34, 0x004b_2ce5);
pub const MINIMAP_BLOCK2: (u32, u32) = (0x004b_366a, 0x004b_3cf3);
/// `gfx+0x5f6cc` = landscape buffer object; its `+0x50` = buffer width (ring, the blitter wraps at it).
pub const GFX_LANDSCAPE: u32 = 0x5f6cc;

// Layout globals (`0x4c6d70`).
pub const L_RES: u32 = 0x0073_0f08;
pub const L_SCREEN_W: u32 = 0x0073_0f0c;
pub const L_SCREEN_H: u32 = 0x0073_0f10;
pub const L_MAP_W: u32 = 0x0073_0f18;
pub const L_PANEL_W: u32 = 0x0073_0f24;
pub const L_MINI_W: u32 = 0x0073_0f28;
pub const L_MINI_X: u32 = 0x0073_0f2c;
pub const L_MINI_Y: u32 = 0x0073_0f30;
pub const L_MINI_BOTTOM: u32 = 0x0073_0f34;
pub const L_CELL: u32 = 0x0073_0f38;
pub const L_GRID_X: u32 = 0x0073_0f3c;
pub const L_GRID_Y: u32 = 0x0073_0f40;

// gd fields.
pub const GD_CAM_X: u32 = 0x2d25c;
pub const GD_CAM_Y: u32 = 0x2d260;
pub const GD_CAM_X2: u32 = 0x2d254;
pub const GD_CAM_Y2: u32 = 0x2d258;
pub const GD_PANEL_DIRTY: u32 = 0x2d27c;
pub const GD_PAGE: u32 = 0x2d284;
pub const GD_BUILD_CAT: u32 = 0x2d288;
pub const GD_BUILD_SLOT: u32 = 0x2d28c;
pub const GD_SUBPAGE: u32 = 0x2d298;
pub const GD_SETTLERS_TAB: u32 = 0x2d2a0;
pub const GD_GOODS_TAB: u32 = 0x2d290;
pub const GD_WORLD_W: u32 = 0x2d2d0;
pub const GD_LOCAL_PLAYER: u32 = 0x2d2d4;
pub const GD_STEP: u32 = 0x2d2d8;
pub const GD_BUILD_TYPE: u32 = 0x2d2f0;
pub const GD_PLAYERS: u32 = 0x11943e0;
pub const GD_SETTLER_MAX: u32 = 0x12a5e5c;
pub const GD_SETTLERS: u32 = 0x12a5e60;
pub const GD_BUILDING_MAX: u32 = 0x1499e60;
pub const GD_BUILDINGS: u32 = 0x1499e64;
pub const GD_PILE_COUNT: u32 = 0x1238858;
pub const GD_PILES: u32 = 0x123885c;
pub const GD_TILES: u32 = 0x67444;

// UI selection globals.
pub const SEL_SLOT: u32 = 0x007d_fd68;
pub const SEL_COUNT: u32 = 0x007a_d620;
pub const SEL_IDS: u32 = 0x007a_d634;
pub const SEL_DIRTY: u32 = 0x007a_d5f8;
pub const SEL_BUILDING: u32 = 0x007d_fd6c;

/// Original layout (panel width, map view width) per resolution index 1..3 (`0x4c6d70`, VERIFIED).
pub fn original_layout(res: u32) -> Option<(u32, u32)> {
    match res {
        1 => Some((136, 504)),
        2 => Some((176, 624)),
        3 => Some((216, 808)),
        _ => None,
    }
}

/// Original layout of the running game: the values the layout setter wrote last (recorded before the HUD
/// changes them, so custom resolutions patched into `0x4c6d70` work too), else the table above.
pub fn current_original_layout(m: &mut Machine) -> Option<(u32, u32)> {
    let res = m.mem.read_u32(L_RES);
    let rec = m.state::<HudShared>().borrow_mut().orig_layout;
    match rec {
        Some((r, pw, mw)) if r == res && pw != 0 => Some((pw, mw)),
        _ => original_layout(res),
    }
}

/// Is `[addr, addr+len)` committed guest memory?
pub fn readable(m: &Machine, addr: u32, len: u32) -> bool {
    if addr < 0x1_0000 || addr.checked_add(len).is_none() {
        return false;
    }
    let mut a = addr;
    let end = addr + len;
    while a < end {
        let info = m.mem.virtual_query(a);
        if info.state != s3_mem::MEM_COMMIT {
            return false;
        }
        let next = info.base_address.wrapping_add(info.region_size);
        if next <= a {
            return false;
        }
        a = next;
    }
    true
}

/// The game data object, if a game is loaded.
pub fn gd(m: &Machine) -> Option<u32> {
    let p = m.mem.read_u32(GD_GLOBAL);
    if p == 0 || !readable(m, p, 0x153a128) || m.mem.read_u32(p) != GD_VTABLE {
        // Multiplayer: the global stays 0; use the logic thread's game data (touch_game::gd fallback).
        return crate::touch_game::gd(m).filter(|&q| readable(m, q, 0x153a128));
    }
    Some(p)
}

// ---------------------------------------------------------------- shared state (hooks <-> HUD mod)

/// An action that must run on the logic thread (queued by the HUD, run at `HandleInput` entry).
#[derive(Debug, Clone, PartialEq)]
pub enum LogicAction {
    /// Enter build placement for a building type.
    StartBuild { cat: u32, slot: u32, btype: u32 },
    /// Leave build placement.
    CancelBuild,
    /// Show a panel page (with optional sub-page) and mark the panel dirty.
    Page { page: u32, sub: Option<u32> },
    /// Settlers page tab (page 3).
    SettlersTab(u32),
    /// Goods page tab (page 2): 1 Inventory, 2 Tools/Weapons, 3 building priority, 4 goods priority.
    GoodsTab(u32),
    /// Deselect units and building.
    ClearSelection,
    /// Destroy a building (command 0x06, STATIC in part 1; HYPOTHESIS until verified).
    Destroy { building: u16 },
    /// Center the camera on a tile (host-side formula of `0x51b560`).
    CenterOn { tx: u32, ty: u32 },
    /// End the game like "Quit Game -> YES" on the original panel (page 7 sub-page 1): `0x53e720` (thiscall logic,
    /// the handler at `0x52e993` calls it; then the statistics screen and the main menu follow).
    QuitGame,
    /// Post a player-command request (8 or 12 bytes, docs/reverse-engineering/game-state-units.md "Player
    /// commands"), play UI sound `sound` first (0 = none) and mark the panel dirty, like the panel buttons do.
    Request { bytes: Vec<u8>, sound: u32 },
    /// Building panel "Set new working area" (`0x5320dd`): sound 0x34, `CLogicThread::0x523840(building)`,
    /// building sub-page 2; the next left click on the map sends command 5 (`0x53330e`).
    WorkArea { building: u16 },
    /// Settlers page "Recruitment" (`0x52cbf7..0x52d4ae`): command 0x10 `{kind, player, x, y, count}` at the tile
    /// in the view centre; kind 1 geologist, 2 thief, 3 pioneer, 4 pioneer -> carrier; count 1 or 5.
    Recruit { kind: u8, count: u8 },
    /// Read the 8 save slot descriptions with the game's slot reader `0x50b670` (into `HudShared::save_slots`).
    ReadSaveSlots,
    /// Save into slot 1..8 like the panel's slot list (`0x52e69a`): `CLogicThread::0x50ac40(0, slot)`
    /// (single player); in multiplayer the network save command 0xfa (`0x52e510`).
    SaveGame { slot: u32 },
}

/// The save slot list (index 0 = slot 1).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SaveSlots {
    /// Multiplayer game (`0x4318d0` on `0x7a7a18`): one network save instead of slots.
    pub multiplayer: bool,
    /// Per slot: `None` = empty, else the reader's three strings (line 1, then the two parts of line 2).
    pub slots: Vec<Option<[String; 3]>>,
}

/// `0x50b670` (thiscall logic): `(slot, 0, &line1, &part_b, &part_a)` -> al = slot exists. The strings are MSVC 6
/// `std::string` (16 bytes: +4 data pointer, +8 length), constructed by `0x523540`, destroyed by `0x401b60`.
pub const FN_SAVE_SLOT_INFO: u32 = 0x0050_b670;
/// `0x50ac40` (thiscall logic): `(multiplayer = 0, slot)` -> al = 1 saved (panel sub-page 3) / else failed (4).
pub const FN_SAVE_GAME: u32 = 0x0050_ac40;
/// `0x4318d0` (thiscall, `ecx = 0x7a7a18`): multiplayer session active.
pub const FN_IS_MULTIPLAYER: u32 = 0x0043_18d0;
pub const MP_OBJECT: u32 = 0x007a_7a18;
pub const FN_STRING_CTOR: u32 = 0x0052_3540;
pub const FN_STRING_DTOR: u32 = 0x0040_1b60;
/// `CLogicThread::0x523840(building)`: start choosing a new work area (marks the area on the map).
pub const FN_WORK_AREA_START: u32 = 0x0052_3840;
/// `CLogicThread::0x542630()`: called after recruit / network-save requests (HYPOTHESIS: refresh/notify).
pub const FN_LOGIC_NOTIFY: u32 = 0x0054_2630;

/// Render height the HUD icon harvest needs (the 640x480 panel art must fit).
pub const HARVEST_MIN_H: u32 = 480;

/// State shared by the HUD hooks and the HUD mod (`m.state::<HudShared>()`).
#[derive(Default)]
pub struct HudShared {
    /// The hooks are installed (GOG exe and patches enabled).
    pub hooked: bool,
    /// The HUD is reading building icons from the game's panel: the zoom keeps the render size at least
    /// [`HARVEST_MIN_H`] high meanwhile (the panel art needs it; `crate::zoom::target_size`).
    pub harvesting: bool,
    /// Save slots as the game's own slot reader `0x50b670` describes them (filled by
    /// [`LogicAction::ReadSaveSlots`]; `None` = not read yet).
    pub save_slots: Option<SaveSlots>,
    /// Result of the last [`LogicAction::SaveGame`]: (slot, success). Taken by the HUD for its message.
    pub save_result: Option<(u32, bool)>,
    /// Wanted: full-screen map (panel hidden).
    pub fullscreen_map: bool,
    /// The full-screen layout is currently written to the layout globals (with this original panel width).
    pub applied_panel_w: Option<u32>,
    /// Number of panel-render calls (= in-game frames rendered).
    pub panel_frames: u64,
    pub last_panel_ms: u64,
    /// String table object (`ecx` of `0x5faaa0`).
    pub strtab: u32,
    /// `CLogicThread` (`ecx` of HandleInput).
    pub logic: u32,
    pub actions: Vec<LogicAction>,
    /// 16-byte guest buffer for command requests.
    pub cmd_buf: u32,
    /// Log of executed actions (tests / debugging).
    pub done: Vec<String>,
    /// The user turned the full-screen map off (HUD menu).
    pub no_full: bool,
    /// Render screen object (`ecx` of the panel renderer, vtable `0x656aec`).
    pub gfx: u32,
    /// Width of the landscape ring buffer is at least the screen width (full-screen map possible).
    pub buffer_ok: bool,
    /// Render snapshot (argument of the panel renderer): `+0x4c` = minimap image (ui_hud_minimap).
    pub snap: u32,
    /// (resolution index, panel width, map view width) as the layout setter wrote them last.
    pub orig_layout: Option<(u32, u32, u32)>,
    /// Frames on which the game's own minimap blocks were skipped (tests / debugging).
    pub minimap_skips: u64,
}

/// Is the full-screen layout active right now (panel width 0, applied by the HUD)?
pub fn full_layout_active(m: &mut Machine) -> bool {
    let applied = m.state::<HudShared>().borrow_mut().applied_panel_w.is_some();
    applied && m.mem.read_u32(L_PANEL_W) == 0
}

/// Is the HUD mode selected in the settings (and the full-screen map not turned off)?
fn hud_full_wanted(m: &mut Machine) -> bool {
    let mode = m.state::<crate::ModsState>().borrow_mut().settings.ui_mode;
    mode == crate::settings::UiMode::MobileHud && !m.state::<HudShared>().borrow_mut().no_full
}

/// Queue an action for the logic thread.
pub fn queue(m: &mut Machine, a: LogicAction) {
    m.state::<HudShared>().borrow_mut().actions.push(a);
}

/// Install the HUD hooks (GOG exe only). Called before `hooks::commit`.
pub fn install_hooks(m: &mut Machine) {
    m.state::<HudShared>().borrow_mut().hooked = true;
    hooks::add_hook(m, FN_PANEL_RENDER, "ui-hud:panel", panel_hook);
    hooks::add_hook(m, FN_SCREEN_TO_MAP, "ui-hud:screen-to-map", screen_to_map_hook);
    hooks::add_hook(m, FN_HANDLE_INPUT, "ui-hud:logic", logic_hook);
    hooks::add_hook(m, FN_LAYOUT, "ui-hud:layout", |c: &mut HookCtx| {
        // Apply the full-screen layout right after the game set its layout, i.e. before the display setup
        // (0x42ef..) sizes the landscape buffer from the map view width.
        c.on_return(|p| {
            let (res, pw, mw) = (p.m.mem.read_u32(L_RES), p.m.mem.read_u32(L_PANEL_W), p.m.mem.read_u32(L_MAP_W));
            if pw != 0 {
                p.m.state::<HudShared>().borrow_mut().orig_layout = Some((res, pw, mw));
            }
            if hud_full_wanted(p.m) {
                apply_layout(p.m, true);
            }
        });
        HookFlow::Continue
    });
    hooks::add_hook(m, FN_LANDSCAPE_INIT, "ui-hud:landscape-init", |c: &mut HookCtx| {
        // Landscape ring buffer init (thiscall, arg0 = width = map view width + [0x730f44]): make it wide enough
        // for the full-screen map even if the original layout is active right now (panel peek).
        if hud_full_wanted(c.m) {
            let margin = c.m.mem.read_u32(0x0073_0f44);
            let sw = c.m.mem.read_u32(L_SCREEN_W);
            let a = c.esp() + 4;
            let w = c.m.mem.read_u32(a);
            let mw = c.m.mem.read_u32(L_MAP_W);
            if w == mw.wrapping_add(margin) && w < sw + margin && sw <= 4096 {
                c.m.mem.write_u32(a, sw + margin);
            }
            if std::env::var("S3_HUD_DEBUG").is_ok() {
                let ra = c.return_address();
                eprintln!("ui-hud: landscape init from {ra:#x}: width {w} -> {} (map {mw}, screen {sw}, margin {margin})", c.m.mem.read_u32(a));
            }
        }
        HookFlow::Continue
    });
    // Hide the game's own minimap (and its frame / camera rectangle) while the full-screen layout is active.
    for (at, to) in [MINIMAP_BLOCK1, MINIMAP_BLOCK2] {
        hooks::add_hook(m, at, "ui-hud:minimap-skip", move |c: &mut HookCtx| {
            if full_layout_active(c.m) {
                if at == MINIMAP_BLOCK2.0 {
                    c.m.state::<HudShared>().borrow_mut().minimap_skips += 1;
                }
                return c.jump(to);
            }
            HookFlow::Continue
        });
    }
    hooks::add_hook(m, FN_TEXT, "ui-hud:text", |c: &mut HookCtx| {
        let ecx = c.reg(s3_cpu::ECX);
        let st = c.m.state::<HudShared>();
        let mut s = st.borrow_mut();
        if s.strtab != ecx {
            s.strtab = ecx;
        }
        HookFlow::Continue
    });
}

/// Panel renderer entry (render thread, every in-game frame).
fn panel_hook(c: &mut HookCtx) -> HookFlow {
    let now = c.m.real_ms();
    let st = c.m.state::<HudShared>();
    let mut s = st.borrow_mut();
    s.panel_frames += 1;
    s.last_panel_ms = now;
    let gfx = c.reg(s3_cpu::ECX);
    s.gfx = gfx;
    drop(s);
    let snap = c.arg(0);
    c.m.state::<HudShared>().borrow_mut().snap = snap;
    // The full-screen map needs a landscape buffer at least as wide as the screen (else it wraps).
    let sw = c.m.mem.read_u32(L_SCREEN_W);
    let lb = if readable(c.m, gfx + GFX_LANDSCAPE, 4) { c.m.mem.read_u32(gfx + GFX_LANDSCAPE) } else { 0 };
    let buffer_ok = lb != 0 && readable(c.m, lb + 0x50, 4) && c.m.mem.read_u32(lb + 0x50) >= sw;
    let mut s = st.borrow_mut();
    s.buffer_ok = buffer_ok;
    let want = s.fullscreen_map && buffer_ok;
    drop(s);
    apply_layout(c.m, want);
    if want {
        // Skip the panel (thiscall gfx, one stdcall argument).
        return c.return_from_function(0, 1);
    }
    HookFlow::Continue
}

/// Move the camera by `dx` world pixels (clamped at 0; the game clamps the far edge when it next scrolls).
fn shift_camera_x(m: &mut Machine, dx: i32) {
    if dx == 0 {
        return;
    }
    if let Some(g) = gd(m) {
        let x = m.mem.read_u32(g + GD_CAM_X) as i32;
        m.mem.write_u32(g + GD_CAM_X, (x + dx).max(0) as u32);
    }
}

/// Write (or undo) the full-screen map layout. Returns true if the layout is now full screen.
pub fn apply_layout(m: &mut Machine, full: bool) -> bool {
    let Some((pw, mw)) = current_original_layout(m) else { return false };
    let sw = m.mem.read_u32(L_SCREEN_W);
    let st = m.state::<HudShared>();
    let mut s = st.borrow_mut();
    let cur_pw = m.mem.read_u32(L_PANEL_W);
    if full {
        // The game rewrites the table on a resolution change (F1..F3); re-apply whenever it differs.
        if cur_pw != 0 || m.mem.read_u32(L_MAP_W) != sw {
            m.mem.write_u32(L_PANEL_W, 0);
            m.mem.write_u32(L_MAP_W, sw);
            // The map view now starts at x 0 instead of after the panel: keep the world point at the view centre
            // (else the game start, centred on the castle for the narrow view, showed it off-centre).
            shift_camera_x(m, -(cur_pw as i32) / 2);
        }
        s.applied_panel_w = Some(pw);
        true
    } else {
        if s.applied_panel_w.is_some() {
            if cur_pw == 0 {
                m.mem.write_u32(L_PANEL_W, pw);
                m.mem.write_u32(L_MAP_W, mw);
                shift_camera_x(m, pw as i32 / 2);
            }
            drop(s);
            // The panel is only redrawn when dirty; without this the panel area keeps map pixels (VERIFIED).
            if let Some(g) = gd(m) {
                m.mem.write_u32(g + GD_PANEL_DIRTY, 1);
            }
            let st = m.state::<HudShared>();
            let mut s = st.borrow_mut();
            s.applied_panel_w = None;
        }
        false
    }
}

/// `gd::ScreenToMap(sx, sy)` entry: compensate the full-screen layout.
fn screen_to_map_hook(c: &mut HookCtx) -> HookFlow {
    let st = c.m.state::<HudShared>();
    let pw = st.borrow_mut().applied_panel_w;
    if let Some(pw) = pw {
        if c.m.mem.read_u32(L_PANEL_W) == 0 {
            let a = c.esp() + 4;
            let sx = c.m.mem.read_u32(a);
            c.m.mem.write_u32(a, sx.wrapping_add(pw));
        }
    }
    HookFlow::Continue
}

/// HandleInput entry (logic thread): run queued HUD actions.
fn logic_hook(c: &mut HookCtx) -> HookFlow {
    let logic = c.reg(s3_cpu::ECX);
    crate::touch_game::note_logic(c.m, logic);
    let st = c.m.state::<HudShared>();
    let actions = {
        let mut s = st.borrow_mut();
        s.logic = logic;
        if s.actions.is_empty() {
            return HookFlow::Continue;
        }
        std::mem::take(&mut s.actions)
    };
    for a in actions {
        let r = run_action(c.m, logic, &a);
        let msg = format!("{a:?}: {}", r.as_ref().err().map(String::as_str).unwrap_or("ok"));
        if let Err(e) = &r {
            c.m.log(LogLevel::Warn, &format!("ui-hud: action {a:?} failed: {e}"));
        }
        let st = c.m.state::<HudShared>();
        let mut s = st.borrow_mut();
        if s.done.len() > 64 {
            s.done.remove(0);
        }
        s.done.push(msg);
    }
    HookFlow::Continue
}

fn run_action(m: &mut Machine, logic: u32, a: &LogicAction) -> Result<(), String> {
    let g = gd(m).ok_or("no game")?;
    let w = |m: &mut Machine, off: u32, v: u32| m.mem.write_u32(g + off, v);
    match *a {
        LogicAction::StartBuild { cat, slot, btype } => {
            w(m, GD_PAGE, 1);
            w(m, GD_BUILD_CAT, cat);
            w(m, GD_BUILD_SLOT, slot);
            w(m, GD_BUILD_TYPE, btype);
            w(m, GD_PANEL_DIRTY, 1);
            if slot != 0 {
                // What the panel click does after choosing a building (0x526feb): bump the placement
                // generation so the renderer recomputes the placement markers (without it no markers are drawn
                // and map clicks cannot place, VERIFIED 2026-11-05).
                crate::call::call_thiscall(m, FN_PLACEMENT_REFRESH, g, &[]);
            }
        }
        LogicAction::CancelBuild => {
            w(m, GD_BUILD_SLOT, 0);
            if m.mem.read_u32(g + GD_PAGE) == 1 {
                w(m, GD_PAGE, 0);
            }
            w(m, GD_PANEL_DIRTY, 1);
        }
        LogicAction::Page { page, sub } => {
            w(m, GD_PAGE, page);
            if let Some(s) = sub {
                w(m, GD_SUBPAGE, s);
            }
            if page != 1 {
                w(m, GD_BUILD_SLOT, 0);
            }
            w(m, GD_PANEL_DIRTY, 1);
        }
        LogicAction::SettlersTab(t) => {
            w(m, GD_PAGE, 3);
            w(m, GD_SETTLERS_TAB, t);
            w(m, GD_PANEL_DIRTY, 1);
        }
        LogicAction::GoodsTab(t) => {
            w(m, GD_PAGE, 2);
            w(m, GD_BUILD_SLOT, 0);
            w(m, GD_GOODS_TAB, t);
            w(m, GD_PANEL_DIRTY, 1);
        }
        LogicAction::ClearSelection => {
            let slot = m.mem.read_i32(SEL_SLOT);
            if slot == 0 {
                m.mem.write_u16(SEL_COUNT, 0);
                m.mem.write_u32(SEL_DIRTY, 1);
            } else if (1..10).contains(&slot) {
                // A control group: keep its members (the game's deselect click only sets slot -1, `0x534178`).
                m.mem.write_u32(SEL_SLOT, u32::MAX);
            }
            m.mem.write_u32(SEL_BUILDING, 0);
            let page = m.mem.read_u32(g + GD_PAGE);
            if page == 4 || page == 5 {
                w(m, GD_PAGE, 0);
            }
            w(m, GD_PANEL_DIRTY, 1);
        }
        LogicAction::Destroy { building } => {
            let player = m.mem.read_u32(g + GD_LOCAL_PLAYER) as u8;
            let req = [0x21, 0x06, 0x08, 0x00, (building >> 8) as u8, building as u8, player, 0];
            post_request(m, logic, &req)?;
            m.mem.write_u32(SEL_BUILDING, 0);
            if m.mem.read_u32(g + GD_PAGE) == 4 {
                w(m, GD_PAGE, 0);
            }
            w(m, GD_PANEL_DIRTY, 1);
        }
        LogicAction::CenterOn { tx, ty } => {
            // The game's own function (it also makes the landscape cache redraw; writing the camera from the
            // host left stale ring-buffer columns after a big jump, VERIFIED 2026-11-05).
            let w = m.mem.read_u32(g + GD_WORLD_W).max(1);
            crate::call::call_thiscall(m, FN_CENTER_ON, g, &[tx.min(w - 1), ty.min(w - 1)]);
        }
        LogicAction::QuitGame => {
            // Exactly what the YES handler does: end the game, click sound, sub-page 0, panel dirty.
            crate::call::call_thiscall(m, FN_QUIT_GAME, logic, &[]);
            crate::call::call_thiscall(m, FN_UI_SOUND, logic, &[0x33]);
            w(m, GD_SUBPAGE, 0);
            w(m, GD_PANEL_DIRTY, 1);
        }
        LogicAction::Request { ref bytes, sound } => {
            if sound != 0 {
                crate::call::call_thiscall(m, FN_UI_SOUND, logic, &[sound]);
            }
            post_request(m, logic, bytes)?;
            w(m, GD_PANEL_DIRTY, 1);
        }
        LogicAction::WorkArea { building } => {
            crate::call::call_thiscall(m, FN_UI_SOUND, logic, &[0x34]);
            crate::call::call_thiscall(m, FN_WORK_AREA_START, logic, &[building as u32]);
            w(m, GD_SUBPAGE, 2);
            w(m, GD_PANEL_DIRTY, 1);
        }
        LogicAction::Recruit { kind, count } => {
            let (x, y) = view_centre_tile(m, g);
            let player = m.mem.read_u32(g + GD_LOCAL_PLAYER) as u8;
            let req = [0x21, 0x10, 0x0c, 0, kind, player, (x >> 8) as u8, x as u8, (y >> 8) as u8, y as u8, count, 0];
            crate::call::call_thiscall(m, FN_UI_SOUND, logic, &[0x2f]);
            post_request(m, logic, &req)?;
            crate::call::call_thiscall(m, FN_LOGIC_NOTIFY, logic, &[]);
        }
        LogicAction::ReadSaveSlots => {
            let mp = crate::call::call_thiscall(m, FN_IS_MULTIPLAYER, MP_OBJECT, &[]) & 0xff != 0;
            let mut slots = Vec::new();
            if !mp {
                let buf = m.mem.alloc(0x30);
                for slot in 1..=8u32 {
                    for i in 0..3 {
                        crate::call::call_thiscall(m, FN_STRING_CTOR, buf + i * 0x10, &[]);
                    }
                    // Same argument order as the panel (0x52e2fc): line 1 = third string argument.
                    let ok = crate::call::call_thiscall(m, FN_SAVE_SLOT_INFO, logic, &[slot, 0, buf, buf + 0x10, buf + 0x20]) & 0xff;
                    let strs = [guest_string(m, buf), guest_string(m, buf + 0x20), guest_string(m, buf + 0x10)];
                    for i in 0..3 {
                        crate::call::call_thiscall(m, FN_STRING_DTOR, buf + i * 0x10, &[]);
                    }
                    slots.push((ok != 0).then_some(strs));
                }
            }
            m.state::<HudShared>().borrow_mut().save_slots = Some(SaveSlots { multiplayer: mp, slots });
        }
        LogicAction::SaveGame { slot } => {
            let mp = crate::call::call_thiscall(m, FN_IS_MULTIPLAYER, MP_OBJECT, &[]) & 0xff != 0;
            let ok = if mp {
                // 0x52e507: sound 0x30, command 0xfa {player}, 0x542630.
                let player = m.mem.read_u32(g + GD_LOCAL_PLAYER) as u8;
                crate::call::call_thiscall(m, FN_UI_SOUND, logic, &[0x30]);
                post_request(m, logic, &[0x21, 0xfa, 0x08, 0, player, 0, 0, 0])?;
                crate::call::call_thiscall(m, FN_LOGIC_NOTIFY, logic, &[]);
                true
            } else {
                let ok = crate::call::call_thiscall(m, FN_SAVE_GAME, logic, &[0, slot]) & 0xff == 1;
                if ok {
                    // 0x52e958: clear the "not saved for N min" reminder, remember the save time.
                    m.mem.write_u32(0x007d_eb50, 0);
                    let t = m.mem.read_u32(0x007d_fd48);
                    m.mem.write_u32(0x007d_fd50, t);
                }
                ok
            };
            let st = m.state::<HudShared>();
            let mut s = st.borrow_mut();
            s.save_result = Some((slot, ok));
            s.save_slots = None;
        }
    }
    Ok(())
}

/// MSVC 6 `std::string` at `p` (+4 data pointer, +8 length) as text.
fn guest_string(m: &Machine, p: u32) -> String {
    let (ptr, len) = (m.mem.read_u32(p + 4), m.mem.read_u32(p + 8).min(200));
    if ptr == 0 || !readable(m, ptr, len.max(1)) {
        return String::new();
    }
    (0..len).map(|i| m.mem.read_u8(ptr + i)).take_while(|&b| b != 0).map(cp1252).collect()
}

/// The tile at the centre of the map view, computed like the recruitment buttons do (`0x52ccbc..0x52cd4a`):
/// `q = (camY + 1600) / 18`, `x = (camX - 208) / 16 - (W - (2q - 200)) / 2 + ((H + 255) / 9 + 11) / 4 +
/// ((mapW + 23) / 16 + 10) / 2`, `y = 2q - 200 + ((H + 255) / 9 + 11) / 2` (W world size, H/mapW map view).
pub fn view_centre_tile(m: &Machine, g: u32) -> (u32, u32) {
    let cam_x = m.mem.read_u32(g + GD_CAM_X) as i32;
    let cam_y = m.mem.read_u32(g + GD_CAM_Y) as i32;
    let world = m.mem.read_u32(g + GD_WORLD_W) as i32;
    let view_h = m.mem.read_u32(0x0073_0f1c) as i32;
    let view_w = m.mem.read_u32(L_MAP_W) as i32;
    centre_tile_formula(cam_x, cam_y, world, view_w, view_h)
}

pub(crate) fn centre_tile_formula(cam_x: i32, cam_y: i32, world: i32, view_w: i32, view_h: i32) -> (u32, u32) {
    let q = (cam_y + 0x640) / 18;
    let ebx = 2 * q - 200;
    let edi = (cam_x - 0xd0) / 16 - (((world - ebx) as u32) >> 1) as i32;
    let rows = (view_h + 0xff) / 9 + 0xb;
    let cols = ((((view_w + 0x17) / 16) + 0xa) as u32 >> 1) as i32;
    let x = ((rows as u32) >> 2) as i32 + edi + cols;
    let y = ebx + ((rows as u32) >> 1) as i32;
    (x.max(0) as u32, y.max(0) as u32)
}

/// Post a player-command request (`Queue::Post` on `[logic+0x3c4]`, like HandleInput). Logic thread only.
fn post_request(m: &mut Machine, logic: u32, req: &[u8]) -> Result<(), String> {
    if m.mem.read_u32(0x0073_0f64) == 0 {
        return Err("commands disabled ([0x730f64] == 0)".into());
    }
    let q = m.mem.read_u32(logic + 0x3c4);
    if q == 0 || !readable(m, q, 4) {
        return Err("no request queue".into());
    }
    let st = m.state::<HudShared>();
    let mut buf = st.borrow_mut().cmd_buf;
    if buf == 0 {
        buf = m.mem.alloc(16);
        st.borrow_mut().cmd_buf = buf;
    }
    m.mem.write_bytes(buf, req);
    crate::call::call_thiscall(m, FN_QUEUE_POST, q, &[buf, (req.len() / 4) as u32, 1]);
    Ok(())
}

// ---------------------------------------------------------------- reading game state

/// Text `id` (`table << 16 | index`) from the game's string table, like `0x5faaa0` (game-state-ui.md §6).
pub fn text(m: &mut Machine, id: u32) -> Option<String> {
    let st = m.state::<HudShared>().borrow_mut().strtab;
    if st == 0 || !readable(m, st, 0xc44) {
        return None;
    }
    let lang = m.mem.read_u32(st + 0xc40);
    let (t, ix) = (id >> 16, id & 0xffff);
    if t >= 0x60 {
        return None;
    }
    let hdr = m.mem.read_u32(st + 0x37c + t * 4);
    if !readable(m, hdr, 0xc) {
        return None;
    }
    let n = m.mem.read_u16(hdr + 8) as u32;
    if ix >= n || lang > 16 {
        return None;
    }
    let off_at = hdr + 0xc + (n * lang + ix) * 4;
    if !readable(m, off_at, 4) {
        return None;
    }
    let a = m.mem.read_u32(off_at).wrapping_add(m.mem.read_u32(st + 0x264 + t * 4));
    if !readable(m, a, 1) {
        return None;
    }
    let mut out = String::new();
    for i in 0..200 {
        if !readable(m, a + i, 1) {
            break;
        }
        let b = m.mem.read_u8(a + i);
        if b == 0 {
            break;
        }
        out.push(cp1252(b));
    }
    Some(out)
}

fn cp1252(b: u8) -> char {
    match b {
        0x80 => '€',
        0x8a => 'Š',
        0x8c => 'Œ',
        0x8e => 'Ž',
        0x9a => 'š',
        0x9c => 'œ',
        0x9e => 'ž',
        0x9f => 'Ÿ',
        0x91 | 0x92 => '\'',
        0x93 | 0x94 => '"',
        0x96 | 0x97 => '-',
        b if (0x80..=0x9f).contains(&b) => '?',
        _ => b as char,
    }
}

/// Number of strings in text table `t` (0 if unknown).
pub fn text_count(m: &mut Machine, t: u32) -> u32 {
    let st = m.state::<HudShared>().borrow_mut().strtab;
    if st == 0 || !readable(m, st, 0xc44) || t >= 0x60 {
        return 0;
    }
    let hdr = m.mem.read_u32(st + 0x37c + t * 4);
    if !readable(m, hdr, 0xc) {
        return 0;
    }
    m.mem.read_u16(hdr + 8) as u32
}

/// What the HUD shows, read once per HUD tick.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GameSnapshot {
    pub gd: u32,
    pub res: u32,
    pub page: u32,
    pub sub: u32,
    pub cat: u32,
    pub slot: u32,
    pub btype: u32,
    pub player: u32,
    pub race: u32,
    pub step: u32,
    /// Selected settler ids (UI selection).
    pub selection: Vec<u16>,
    /// Selected settlers by type: (type, count), most first.
    pub sel_types: Vec<(u8, u32)>,
    /// Selected building id (0 = none) and its type.
    pub building: u16,
    pub building_type: u8,
    /// Selected building: owner (+0x00), construction site (+0x21 != 0), flags (+0x18: bit 0 = working / building
    /// active, bits 1..4 = construction priority; building panel `0x531d4a`, `0x531e72`).
    pub building_owner: u8,
    pub building_site: bool,
    pub building_flags: u8,
    /// Construction sites with highest priority of the local player (player record +0x20; the panel allows
    /// another one only below 10, `0x531e8d`).
    pub priority_sites: u16,
    /// Settlers of the local player by type (index = type).
    pub settlers: Vec<u32>,
    /// Buildings of the local player (finished + sites).
    pub buildings: u32,
    /// Goods: amount summed over all piles (good id index). HYPOTHESIS: piles of all players are counted; on a
    /// single-player start the player's own piles dominate (see game-state-ui.md §5).
    pub goods: Vec<u32>,
}

/// Read a snapshot of the game state (`None` = no game loaded).
pub fn snapshot(m: &mut Machine) -> Option<GameSnapshot> {
    let g = gd(m)?;
    let r = |off: u32| m.mem.read_u32(g + off);
    let player = r(GD_LOCAL_PLAYER);
    if player > 7 {
        return None;
    }
    let mut s = GameSnapshot {
        gd: g,
        res: m.mem.read_u32(L_RES),
        page: r(GD_PAGE),
        sub: r(GD_SUBPAGE),
        cat: r(GD_BUILD_CAT),
        slot: r(GD_BUILD_SLOT),
        btype: r(GD_BUILD_TYPE),
        player,
        race: m.mem.read_u8(g + GD_PLAYERS + player * 0x148) as u32,
        step: r(GD_STEP),
        ..Default::default()
    };
    // Selection.
    let slot = m.mem.read_i32(SEL_SLOT);
    if (0..10).contains(&slot) {
        let n = (m.mem.read_u16(SEL_COUNT + slot as u32 * 2) as u32).min(100);
        let max_settler = r(GD_SETTLER_MAX).min(0x10000);
        let mut types = [0u32; 256];
        for i in 0..n {
            let id = m.mem.read_u16(SEL_IDS + slot as u32 * 200 + i * 2);
            s.selection.push(id);
            if id != 0 && (id as u32) <= max_settler {
                let e = g + GD_SETTLERS + id as u32 * 0x40;
                if m.mem.read_u8(e) != 0xff {
                    types[m.mem.read_u8(e + 1) as usize] += 1;
                }
            }
        }
        s.sel_types = types.iter().enumerate().filter(|(_, &c)| c > 0).map(|(t, &c)| (t as u8, c)).collect();
        s.sel_types.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    }
    let b = m.mem.read_u32(SEL_BUILDING);
    let bmax = r(GD_BUILDING_MAX).min(0x10000);
    if b != 0 && b <= bmax {
        s.building = b as u16;
        let e = g + GD_BUILDINGS + b * 0x3a;
        s.building_type = m.mem.read_u8(e + 6);
        s.building_owner = m.mem.read_u8(e);
        s.building_site = m.mem.read_u8(e + 0x21) != 0;
        s.building_flags = m.mem.read_u8(e + 0x18);
    }
    s.priority_sites = m.mem.read_u16(g + GD_PLAYERS + player * 0x148 + 0x20);
    // Settlers of the local player.
    let smax = r(GD_SETTLER_MAX).min(0x10000);
    s.settlers = vec![0; 64];
    for id in 1..=smax {
        let e = g + GD_SETTLERS + id * 0x40;
        if m.mem.read_u8(e) as u32 == player {
            let t = m.mem.read_u8(e + 1) as usize;
            if t < s.settlers.len() {
                s.settlers[t] += 1;
            }
        }
    }
    for id in 1..=bmax {
        if m.mem.read_u8(g + GD_BUILDINGS + id * 0x3a) as u32 == player {
            s.buildings += 1;
        }
    }
    // Goods from piles (stored piles, +4 == 0xfe).
    s.goods = vec![0; 64];
    let np = r(GD_PILE_COUNT).min(0x8000);
    let ws = r(GD_WORLD_W).min(768);
    for i in 0..=np {
        let e = g + GD_PILES + i * 0xe;
        let good = m.mem.read_u8(e) as usize;
        let amount = m.mem.read_u8(e + 1) as u32;
        if amount == 0 || amount > 8 || good >= s.goods.len() || m.mem.read_u8(e + 4) != 0xfe {
            continue;
        }
        // Only piles on the local player's land (tile +8 = owner). VERIFIED: without this filter a 1-vs-AI
        // random map showed twice the Storage Area numbers (both players' start piles).
        let (x, y) = (m.mem.read_u16(e + 6) as u32, m.mem.read_u16(e + 8) as u32);
        if x >= ws || y >= ws || m.mem.read_u8(g + GD_TILES + (x * 768 + y) * 0x18 + 8) as u32 != player {
            continue;
        }
        s.goods[good] += amount;
    }
    Some(s)
}

