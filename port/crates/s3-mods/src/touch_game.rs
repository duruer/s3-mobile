//! GOG S3.EXE memory access for context-aware touch (addresses: docs/reverse-engineering/game-state-units.md and
//! game-state-ui.md; GOG sha256 `00a925cf…` only). Reads build a [`View`]; writes are limited to UI state
//! (camera, build slot, selection) that the game itself keeps per client.

use std::sync::atomic::{AtomicU32, Ordering};

use s3_runtime::Machine;

use crate::touch_logic::{Hit, Layout, Mode, View};

/// `CGameData*` global (= `[logic+0x3a0]`); 0 before the first game. VERIFIED (s3run: `0x18790000`).
pub const GD_PTR: u32 = 0x007e_3408;
/// `CGameData` vtable.
pub const GD_VTABLE: u32 = 0x0065_8c18;
/// `CLogicThread::HandleInput` (thiscall logic), runs every logic tick while a game is running.
pub const HANDLE_INPUT: u32 = 0x0052_4fa0;

// gd offsets
const CAM_X: u32 = 0x2d25c;
const CAM_Y: u32 = 0x2d260;
const PANEL_DIRTY: u32 = 0x2d27c;
const PAGE: u32 = 0x2d284;
const BUILD_SLOT: u32 = 0x2d28c;
const WORLD: u32 = 0x2d2d0;
const LOCAL_PLAYER: u32 = 0x2d2d4;
const SETTLERS: u32 = 0x12a5e60;
const SETTLERS_MAX: u32 = 0x12a5e5c;
const BUILDINGS: u32 = 0x1499e64;
const BUILDINGS_MAX: u32 = 0x1499e60;

// globals
const LAYOUT: u32 = 0x0073_0f08;
const HITS: u32 = 0x007a_de08;
const HITS_COUNT: u32 = 0x007d_fd5c;
const HITS_BUF: u32 = 0x007d_fd64;
const SEL_SLOT: u32 = 0x007d_fd68;
const SEL_BUILDING: u32 = 0x007d_fd6c;
const SEL_COUNT: u32 = 0x007a_d620;
const SEL_IDS: u32 = 0x007a_d634;
const SEL_DIRTY: u32 = 0x007a_d5f8;
/// Hit list capacity (space up to the next global).
const HITS_MAX: u32 = 4096;

/// `CLogicThread` -> `CGameData*` offset.
pub const LOGIC_GD: u32 = 0x3a0;
/// `[logic+0x3a0]` seen by the last `HandleInput` call (see [`note_logic`]).
static LOGIC_GD_SEEN: AtomicU32 = AtomicU32::new(0);

/// Record the logic object's game data (call from a `HandleInput` hook, ECX = logic).
/// In a multiplayer game the global `GD_PTR` stays 0 (VERIFIED 2026-11-05, ux-verify1, joiner and host), but
/// `[logic+0x3a0]` holds the CGameData (ui-classic reads it there), so this is the fallback.
pub fn note_logic(m: &Machine, logic: u32) {
    if logic >= 0x1_0000 {
        LOGIC_GD_SEEN.store(m.mem.read_u32(logic.wrapping_add(LOGIC_GD)), Ordering::Relaxed);
    }
}

fn valid_gd(m: &Machine, gd: u32) -> bool {
    gd >= 0x1_0000 && m.mem.virtual_query(gd).state == s3_mem::MEM_COMMIT && m.mem.read_u32(gd) == GD_VTABLE
}

/// Game data pointer, if a game has been created: the global, else the one of the logic thread (multiplayer).
pub fn gd(m: &Machine) -> Option<u32> {
    let gd = m.mem.read_u32(GD_PTR);
    if valid_gd(m, gd) {
        return Some(gd);
    }
    let gd = LOGIC_GD_SEEN.load(Ordering::Relaxed);
    valid_gd(m, gd).then_some(gd)
}

/// Screen layout globals (`0x4c6d70`).
pub fn layout(m: &Machine) -> Option<Layout> {
    let r = |o: u32| m.mem.read_u32(LAYOUT + o) as i32;
    let l = Layout {
        res_index: r(0),
        screen_w: r(4),
        screen_h: r(8),
        map_w: r(0x10),
        map_h: r(0x14),
        panel_w: r(0x1c),
        mm_x: r(0x24),
        mm_bottom: r(0x2c),
    };
    // panel_w == 0 is valid: UI mode B (Mobile HUD) patches the layout to a full-screen map (ui-hud.md).
    ((1..=3).contains(&l.res_index) && l.screen_w > 0 && l.panel_w >= 0 && l.panel_w < l.screen_w).then_some(l)
}

pub fn local_player(m: &Machine, gd: u32) -> u8 {
    m.mem.read_u32(gd + LOCAL_PLAYER) as u8
}

/// UI selection (settler ids).
pub fn selection(m: &Machine) -> Vec<u16> {
    let slot = m.mem.read_u32(SEL_SLOT) as i32;
    if !(0..10).contains(&slot) {
        return Vec::new();
    }
    let n = (m.mem.read_u16(SEL_COUNT + slot as u32 * 2) as u32).min(100);
    (0..n).map(|i| m.mem.read_u16(SEL_IDS + slot as u32 * 200 + i * 2)).collect()
}

fn owner(m: &Machine, gd: u32, kind: u16, id: u16) -> Option<u8> {
    if kind == 100 {
        (id as u32 <= m.mem.read_u32(gd + BUILDINGS_MAX)).then(|| m.mem.read_u8(gd + BUILDINGS + id as u32 * 0x3a))
    } else {
        (id as u32 <= m.mem.read_u32(gd + SETTLERS_MAX)).then(|| m.mem.read_u8(gd + SETTLERS + id as u32 * 0x40))
    }
}

/// Selectable things drawn on screen (hit list, current buffer as read by the click handler).
pub fn hits(m: &Machine, gd: u32) -> Vec<Hit> {
    let n = m.mem.read_u32(HITS_COUNT).min(HITS_MAX);
    let buf = m.mem.read_u32(HITS_BUF) & 1;
    let me = local_player(m, gd);
    let mut out = Vec::with_capacity(n as usize);
    for i in 0..n {
        let a = HITS + (buf + 2 * i) * 10;
        let kind = m.mem.read_u16(a);
        let id = m.mem.read_u16(a + 6);
        if kind == 0 || id == 0 {
            continue;
        }
        let own = owner(m, gd, kind, id) == Some(me);
        out.push(Hit { kind, x: m.mem.read_u16(a + 2) as i16 as i32, y: m.mem.read_u16(a + 4) as i16 as i32, id, own });
    }
    out
}

/// Interaction mode (game-state-ui.md §3).
pub fn mode(m: &Machine, gd: u32) -> Mode {
    let page = m.mem.read_u32(gd + PAGE);
    if page == 9 {
        Mode::Paused
    } else if page == 1 && m.mem.read_u32(gd + BUILD_SLOT) != 0 {
        Mode::BuildPlacement
    } else if page == 4 && m.mem.read_u32(SEL_BUILDING) != 0 {
        Mode::BuildingSelected
    } else if selection_count(m) > 0 {
        Mode::UnitsSelected
    } else {
        Mode::Normal
    }
}

pub fn selection_count(m: &Machine) -> u32 {
    let slot = m.mem.read_u32(SEL_SLOT) as i32;
    if !(0..10).contains(&slot) {
        return 0;
    }
    m.mem.read_u16(SEL_COUNT + slot as u32 * 2) as u32
}

/// Full snapshot (`frame` = size of the presented frame; the layout must match it, else we are not in game).
pub fn view(m: &Machine, frame: (u32, u32), with_hits: bool) -> View {
    let Some(gd) = gd(m) else { return View::default() };
    let Some(l) = layout(m) else { return View::default() };
    if (l.screen_w as u32, l.screen_h as u32) != frame {
        return View::default();
    }
    View {
        layout: Some(l),
        mode: Some(mode(m, gd)),
        hits: if with_hits { hits(m, gd) } else { Vec::new() },
        selected: selection(m),
        vis: None,
        cancel: None,
        deselect_button: false,
    }
}

pub fn camera(m: &Machine, gd: u32) -> (i32, i32) {
    (m.mem.read_u32(gd + CAM_X) as i32, m.mem.read_u32(gd + CAM_Y) as i32)
}

pub fn set_camera(m: &mut Machine, gd: u32, x: i32, y: i32) {
    m.mem.write_u32(gd + CAM_X, x as u32);
    m.mem.write_u32(gd + CAM_Y, y as u32);
}

pub fn world_size(m: &Machine, gd: u32) -> i32 {
    m.mem.read_u32(gd + WORLD) as i32
}

/// Leave build placement: slot 0 + panel dirty (what the right-click handler does). VERIFIED by re-ui pokes.
pub fn cancel_build(m: &mut Machine, gd: u32) {
    m.mem.write_u32(gd + BUILD_SLOT, 0);
    m.mem.write_u32(gd + PANEL_DIRTY, 1);
}

/// Clear the UI unit selection (count 0, dirty so the logic copy is re-synced by the next command, panel page 0).
pub fn deselect(m: &mut Machine, gd: u32) {
    let slot = m.mem.read_u32(SEL_SLOT) as i32;
    if (0..10).contains(&slot) {
        m.mem.write_u16(SEL_COUNT + slot as u32 * 2, 0);
        m.mem.write_u32(SEL_DIRTY + slot as u32 * 4, 1);
    }
    if m.mem.read_u32(gd + PAGE) == 5 {
        m.mem.write_u32(gd + PAGE, 0);
    }
    m.mem.write_u32(gd + PANEL_DIRTY, 1);
}
