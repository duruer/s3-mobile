//! Mod / hook framework for the Settlers III port (workstream W1, docs/porting/mods.md).
//!
//! - [`hooks`]: run host code when guest EIP reaches an address; skip functions, post-call callbacks.
//! - [`view`]: typed views of guest memory ([`guest_struct!`]).
//! - [`call`]: call guest functions (stdcall/cdecl/thiscall/fastcall/virtual).
//! - [`patchset`]: hooks/byte patches keyed by the S3.EXE sha256 (GOG `00a925cf…`).
//! - [`overlay`]: draw on every presented frame (rect, rounded rect, circle, image, text).
//! - [`gesture`]: raw pointer events -> [`TouchEvent`]s (tap, double tap, long press, pan, pinch, multi-finger
//!   taps), shared by every frontend; mods see them before the game ([`Mod::on_touch`] -> [`ModEvent`]),
//!   unconsumed ones go through the [`fallback`] touch->mouse mapping.
//! - [`settings`]: `ui_mode` (ClassicTouch / MobileHud) and thresholds, JSON in the data dir.
//!
//! Entry point: [`install`] (called by `s3-engine` after loading the exe). Environment: `S3_MODS=0` disables
//! exe patch sets, `S3_MODS_DEBUG=1` turns on the touch debug overlay.

pub mod call;
pub mod menu_layout;
pub mod messages; // game message lines in the window layer (game-state-ui.md section 7) // full-screen layouts of the 800x600 menus (docs/porting/menu-layout.md)
pub mod panel_text; // panel / tooltip texts of the text DIB in the window layer (docs/porting/display.md)
pub mod cursor_layer; // the game's cursor in the window layer (docs/porting/display.md)
pub mod speed; // game speed 1x-4x (single player)
pub mod display; // display modes + shared screen<->game transform (docs/porting/display.md)
pub mod fps; // FPS counter overlay
pub mod game_time; // game time h:mm:ss overlay
pub mod end_stats; // statistics over game time on the end-of-game statistics screen (docs/porting/menu-layout.md)
pub mod stone; // generated stone texture behind menu layouts
pub mod fallback;
pub mod fast; // native fast paths for hot guest loops (docs/porting/performance.md)
pub mod fast_blit;
pub mod fast_land;
pub mod fast_land_rows; // dirty-triangle rasteriser rows (perf-landfull)
pub mod fast_obj;
pub mod fast_shadow; // shadow sprite row loops (perf-shadow)
pub mod fast_snap;
pub mod fast_sprite;
pub mod fast_tile;
pub mod gesture;
pub mod hooks;
pub mod layer; // window-resolution overlay layer (docs/porting/mods.md "Window layer")
pub mod overlay;
pub mod patchset;
pub mod res; // custom resolutions (docs/porting/resolution-zoom.md)
pub mod settings;
pub mod sha256;
pub mod touch;
pub mod touch_fx;
pub mod touch_game;
pub mod touch_logic;
pub mod view;
pub mod zoom; // zoom = render resolution levels, pinch (docs/porting/resolution-zoom.md)
pub mod ui_classic;
pub mod ui_hud; // UI mode B "Mobile HUD" (docs/porting/ui-hud.md)
mod ui_hud_data;
pub mod ui_hud_minimap; // HUD minimap widget + full-screen map (docs/porting/ui-hud.md)
pub mod ui_hud_game;
pub mod unit_orders; // waypoint / patrol / no-fight orders, unit paths, control groups (docs/porting/touch.md)

mod debug_overlay;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod fast_tests;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

pub use s3_mem;
use std::sync::Arc;

use s3_runtime::{FrameBuffer, HostEvent, LogLevel, Machine, PointerPhase};

pub use gesture::{GestureConfig, GestureRecognizer, Phase, TouchEvent};
pub use hooks::{add_hook, HookCtx, PostCtx};
pub use layer::{ImageRef, Layer, LayerRaster, Scene, SceneSlot};
pub use overlay::{Canvas, Image, Rgba, Weight};
pub use s3_runtime::HookFlow;
pub use settings::{Settings, UiMode};

/// Whether a mod handled an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModEvent {
    /// Handled: later mods and the game do not see it. Consuming a raw `Down` captures that pointer: its
    /// Move/Up go only to this mod and no gestures are recognized for it.
    Consumed,
    /// Not handled: offer it to the next mod, then to the touch->mouse fallback.
    Pass,
}

/// A host-side mod. Mods added later get events first (they are "on top").
pub trait Mod: Send {
    fn name(&self) -> &str;
    /// Every touch event, before any mod handles it (debug views, analytics). Cannot consume.
    fn observe(&mut self, _m: &mut Machine, _ev: &TouchEvent) {}
    /// Handle a touch event.
    fn on_touch(&mut self, _m: &mut Machine, _ev: &TouchEvent) -> ModEvent {
        ModEvent::Pass
    }
    /// Does this mod draw into the window layer now? (Checked per presented frame; keep it cheap.)
    fn wants_draw(&self) -> bool {
        false
    }
    /// Draw into the window-resolution layer (window pixels; only called when `wants_draw`). `frame` is the raw
    /// guest frame (read-only, e.g. to sample panel icons); map game pixels to the window with
    /// [`draw_viewport`].
    fn draw_window(&mut self, _m: &mut Machine, _l: &mut Layer, _frame: &FrameBuffer) {}
    /// Does this mod draw into the game frame itself now (game pixels, costs a frame copy)?
    fn wants_draw_frame(&self) -> bool {
        false
    }
    /// Draw on a copy of the presented frame (only called when `wants_draw_frame`).
    fn draw(&mut self, _m: &mut Machine, _c: &mut Canvas) {}
    /// Should the last frame be re-presented with a new overlay (the game is not presenting)?
    fn needs_redraw(&self) -> bool {
        false
    }
    /// Called once per scheduler slice.
    fn tick(&mut self, _m: &mut Machine, _now_ms: u64) {}
    /// A host key event (Windows VK; Android Back arrives as `VK_ESCAPE` 0x1b), top mod first. `Consumed` keeps
    /// it from the game (e.g. Back closes a HUD overlay).
    fn on_key(&mut self, _m: &mut Machine, _vk: u32, _down: bool) -> ModEvent {
        ModEvent::Pass
    }
}

/// Offer a key event to the mods (top first). True if one consumed it.
fn dispatch_key(m: &mut Machine, vk: u32, down: bool) -> bool {
    let n = mod_count(m);
    (0..n).rev().any(|i| with_mod(m, i, |md, m| md.on_key(m, vk, down)) == Some(ModEvent::Consumed))
}

type Deferred = Box<dyn FnOnce(&mut Machine) + Send>;

/// All mod state (one per machine, `m.state::<ModsState>()`).
#[derive(Default)]
pub struct ModsState {
    pub(crate) hooks: hooks::HookTable,
    pub(crate) safe_points: HashSet<u32>,
    deferred: Vec<Deferred>,
    mods: Vec<Option<Box<dyn Mod>>>,
    gestures: Option<GestureRecognizer>,
    /// Pointer id -> index of the mod that captured it.
    captured: HashMap<u32, usize>,
    pub(crate) fallback: fallback::TouchMouse,
    pub settings: Settings,
    pub data_dir: PathBuf,
    /// Lower-case hex sha256 of the loaded exe.
    pub exe_sha256: String,
    /// Name of the applied patch set.
    pub patch_set: Option<&'static str>,
    installed: bool,
    /// Thread id of the guest main thread (deferred calls run only there).
    main_tid: u32,
    /// Frame size of the last guest present.
    pub frame_size: (u32, u32),
    last_refresh_ms: u64,
    warned_no_safe_point: bool,
    /// Window layer: the frontend's mailbox, the last published list (+ panel split, frame size) and the
    /// viewport used while the mods draw.
    layer_sink: Option<Arc<SceneSlot>>,
    layer_last: Option<(Arc<layer::DrawList>, Option<display::PanelSplit>, (u32, u32))>,
    draw_vp: Option<display::Viewport>,
    /// Window positions of the fingers down (from `HostEvent::Pointer::wx/wy`) and of the last pointer event.
    win_ptrs: HashMap<u32, (f32, f32)>,
    win_cur: (f32, f32),
}

/// What [`install`] did.
#[derive(Debug, Clone)]
pub struct InstallReport {
    pub exe_sha256: String,
    pub patch_set: Option<&'static str>,
    pub hooks: usize,
}

/// Install the mod framework into a freshly loaded machine (main thread created, before running).
pub fn install(m: &mut Machine, exe_bytes: &[u8]) -> InstallReport {
    let sha = sha256::sha256_hex(exe_bytes);
    let data_dir = m.host.data_dir();
    let (mut settings, err) = Settings::load(&data_dir);
    if let Some(e) = err {
        m.log(LogLevel::Warn, &format!("mods: settings: {e} (using defaults)"));
    }
    if std::env::var("S3_MODS").map(|v| v == "0").unwrap_or(false) {
        settings.patches_enabled = false;
    }
    if std::env::var("S3_MODS_DEBUG").map(|v| v != "0").unwrap_or(false) {
        settings.debug_overlay = true;
    }
    let tramp = m.registry.register("s3mods.dll", "post_call_return", hooks::post_call_return);
    let main_tid = m.threads.first().map(|t| t.tid).unwrap_or(0);
    {
        let st = m.state::<ModsState>();
        let mut s = st.borrow_mut();
        s.hooks.trampoline = tramp;
        s.gestures = Some(GestureRecognizer::new(GestureConfig::from(&settings.touch)));
        s.fallback.long_press_right_click = settings.touch.long_press_right_click;
        s.settings = settings.clone();
        s.data_dir = data_dir;
        s.exe_sha256 = sha.clone();
        s.main_tid = main_tid;
        s.installed = true;
    }
    m.ext.hook_dispatch = Some(hooks::dispatch);
    m.ext.event_filter = Some(event_filter);
    m.ext.overlay_wanted = Some(overlay_wanted);
    m.ext.overlay_compose = Some(overlay_compose);
    m.ext.window_overlay = Some(window_compose);
    m.add_service("s3mods", service);
    add_mod(m, Box::new(debug_overlay::DebugTouch::default()));

    let mut applied = None;
    match patchset::find(&sha) {
        Some(ps) if settings.patches_enabled => match (ps.apply)(m) {
            Ok(()) => {
                applied = Some(ps.name);
                m.log(LogLevel::Info, &format!("mods: S3.EXE sha256 {}…: patch set '{}' applied", &sha[..8], ps.name));
            }
            Err(e) => {
                hooks::remove_hooks_all(m);
                m.log(LogLevel::Warn, &format!("mods: patch set '{}' failed: {e}; game patches disabled", ps.name));
            }
        },
        Some(ps) => m.log(LogLevel::Info, &format!("mods: patch set '{}' disabled by settings/S3_MODS=0", ps.name)),
        None => m.log(
            LogLevel::Warn,
            &format!("mods: unknown S3.EXE (sha256 {sha}); game patches disabled (expected GOG {}…)", &patchset::GOG_SHA256[..8]),
        ),
    }
    // Custom resolutions + zoom (res-zoom): always with the GOG patch set, so every peer has the same heap layout.
    if applied == Some("gog-base") {
        match res::install(m) {
            Ok(()) => zoom::install(m),
            Err(e) => m.log(LogLevel::Warn, &format!("mods: {e}; custom resolutions disabled")),
        }
    }
    // Context-aware touch (W3): GOG exe with its patch set applied, unless turned off in the settings.
    // Installed before the UI modes so their mods sit on top of it (they get touches first).
    if applied == Some("gog-base") && settings.touch.context {
        if let Err(e) = touch::install(m) {
            m.log(LogLevel::Warn, &format!("mods: {e}; context-aware touch disabled"));
        }
    }
    // Game message lines at a constant size in the window layer. Below the UI modes: their sheets and dialogs cover
    // the lines (user report 2026-11-08) and get taps first; the tutorial Next tap still reaches the lines before
    // the touch mod.
    if applied == Some("gog-base") {
        messages::install(m);
        panel_text::install(m);
    }
    // Unit orders, paths and group buttons: below the UI modes (their sheets cover the group row and get taps first).
    unit_orders::install(m, applied == Some("gog-base"));
    ui_classic::install(m, applied.is_some()); // UI mode A (docs/porting/ui-classic.md)
    ui_hud::install(m, applied.is_some()); // UI mode B (docs/porting/ui-hud.md)
    add_mod(m, Box::new(end_stats::EndStatsMod::default())); // charts on the end-of-game statistics screen
    add_mod(m, Box::new(game_time::GameTimeMod::default())); // game time, drawn on top of the UI modes
    add_mod(m, Box::new(fps::FpsMod::default())); // FPS counter, drawn on top of the UI modes
    speed::install(m); // back to 1x outside a single-player game
    cursor_layer::install(m); // the game's mouse cursor at a constant size, on top of everything
    hooks::commit(m);
    display::apply_fps_limit(m); // FPS limit setting -> DirectDraw frame limiter
    fast::install(m, &sha); // after commit: regions containing hooks are skipped
    let n = m.hook_addrs().map(|h| h.len()).unwrap_or(0);
    let st = m.state::<ModsState>();
    st.borrow_mut().patch_set = applied;
    InstallReport { exe_sha256: sha, patch_set: applied, hooks: n }
}

/// Add a mod (on top of the existing ones).
pub fn add_mod(m: &mut Machine, md: Box<dyn Mod>) {
    m.state::<ModsState>().borrow_mut().mods.push(Some(md));
}

/// Mark `addr` (a hooked address on the main thread's loop) as a safe point for [`defer`]red work.
pub fn add_safe_point(m: &mut Machine, addr: u32) {
    m.state::<ModsState>().borrow_mut().safe_points.insert(addr);
}

/// Run `f` at the next safe point of the guest main thread (where calling game functions is safe).
pub fn defer(m: &mut Machine, f: impl FnOnce(&mut Machine) + Send + 'static) {
    m.state::<ModsState>().borrow_mut().deferred.push(Box::new(f));
}

/// Run queued deferred work (called from safe-point hooks; only on the main thread).
pub(crate) fn run_deferred(m: &mut Machine) {
    let st = m.state::<ModsState>();
    let work = {
        let mut s = st.borrow_mut();
        if s.deferred.is_empty() || m.current_tid() != s.main_tid {
            return;
        }
        std::mem::take(&mut s.deferred)
    };
    for f in work {
        f(m);
    }
}

/// Current settings.
pub fn settings(m: &mut Machine) -> Settings {
    m.state::<ModsState>().borrow_mut().settings.clone()
}

/// Replace and persist the settings (gesture thresholds apply at once).
pub fn set_settings(m: &mut Machine, s: Settings) -> Result<(), String> {
    let st = m.state::<ModsState>();
    let mut g = st.borrow_mut();
    if let Some(r) = g.gestures.as_mut() {
        r.cfg = GestureConfig::from(&s.touch);
    }
    g.fallback.long_press_right_click = s.touch.long_press_right_click;
    g.settings = s.clone();
    let dir = g.data_dir.clone();
    drop(g);
    s.save(&dir)
}

/// Arm "the next tap is a right click" (letterbox RMB button in the legacy UI).
pub fn set_right_next(m: &mut Machine, on: bool) {
    let st = m.state::<ModsState>();
    let mut s = st.borrow_mut();
    s.fallback.right_next = on;
    s.fallback.right_next_vk = 0;
}

/// Arm "the next tap is a right click with key `vk` held" (unit orders: Shift / Alt / Ctrl, [`unit_orders`]).
pub fn set_order_next(m: &mut Machine, vk: u32) {
    let st = m.state::<ModsState>();
    let mut s = st.borrow_mut();
    s.fallback.right_next = true;
    s.fallback.right_next_vk = vk;
}

/// (right click armed, its modifier VK or 0).
pub fn order_next(m: &mut Machine) -> (bool, u32) {
    let st = m.state::<ModsState>();
    let s = st.borrow_mut();
    (s.fallback.right_next, s.fallback.right_next_vk)
}

/// Is "the next tap is a right click" armed? (Cleared by the next tap.)
pub fn right_next(m: &mut Machine) -> bool {
    m.state::<ModsState>().borrow_mut().fallback.right_next
}

/// True once after a three-finger tap (frontend shows the soft keyboard).
pub fn take_keyboard_request(m: &mut Machine) -> bool {
    std::mem::take(&mut m.state::<ModsState>().borrow_mut().fallback.keyboard_request)
}

/// Frontend: register the mailbox the window layer ([`Scene`]s) is published to.
pub fn set_layer_sink(m: &mut Machine, sink: Arc<SceneSlot>) {
    let st = m.state::<ModsState>();
    let mut s = st.borrow_mut();
    s.layer_sink = Some(sink);
    s.layer_last = None;
}

/// Viewport of the frame the mods are drawing for (inside [`Mod::draw_window`]; else the current
/// [`display::frame_viewport`]). Window pixels = `vp.game_to_window(game x, y)`.
pub fn draw_viewport(m: &mut Machine) -> display::Viewport {
    let vp = m.state::<ModsState>().borrow_mut().draw_vp;
    vp.unwrap_or_else(|| display::frame_viewport(m))
}

/// Window position of a touch event (the finger(s) that produced it), for hit tests of window-layer UI. Raw
/// events use their pointer; taps / long presses the last pointer event; two-finger gestures the centroid.
pub fn touch_window_pos(m: &mut Machine, ev: &TouchEvent) -> (f32, f32) {
    let st = m.state::<ModsState>();
    let s = st.borrow_mut();
    let centroid = || {
        let n = s.win_ptrs.len();
        if n == 0 {
            return s.win_cur;
        }
        let (sx, sy) = s.win_ptrs.values().fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
        (sx / n as f32, sy / n as f32)
    };
    match *ev {
        TouchEvent::Down { id, .. } | TouchEvent::Move { id, .. } | TouchEvent::Up { id, .. } | TouchEvent::Cancel { id } => {
            s.win_ptrs.get(&id).copied().unwrap_or(s.win_cur)
        }
        TouchEvent::TwoFingerTap { .. } | TouchEvent::ThreeFingerTap { .. } | TouchEvent::Pinch { .. } => centroid(),
        TouchEvent::Pan { fingers, .. } if fingers > 1 => centroid(),
        _ => s.win_cur,
    }
}

/// Is the framework installed in this machine?
pub fn is_installed(m: &mut Machine) -> bool {
    m.state::<ModsState>().borrow_mut().installed
}

// ---------------------------------------------------------------- dispatch helpers

/// Call `f` on mod `i` without holding the state lock (mods may call guest code / the mods API).
fn with_mod<R>(m: &mut Machine, i: usize, f: impl FnOnce(&mut dyn Mod, &mut Machine) -> R) -> Option<R> {
    let st = m.state::<ModsState>();
    let mut md = st.borrow_mut().mods.get_mut(i)?.take()?;
    let r = f(md.as_mut(), m);
    if let Some(slot) = st.borrow_mut().mods.get_mut(i) {
        *slot = Some(md);
    }
    Some(r)
}

fn mod_count(m: &mut Machine) -> usize {
    m.state::<ModsState>().borrow_mut().mods.len()
}

/// Offer touch events to the mods (top first), then to the fallback mapping.
fn dispatch_touch(m: &mut Machine, events: Vec<TouchEvent>, now: u64) {
    let n = mod_count(m);
    let st = m.state::<ModsState>();
    for ev in events {
        for i in 0..n {
            with_mod(m, i, |md, m| md.observe(m, &ev));
        }
        let mut consumed_by = None;
        for i in (0..n).rev() {
            if with_mod(m, i, |md, m| md.on_touch(m, &ev)) == Some(ModEvent::Consumed) {
                consumed_by = Some(i);
                break;
            }
        }
        let mut s = st.borrow_mut();
        match (consumed_by, ev) {
            (Some(i), TouchEvent::Down { id, .. }) => {
                // Captured: the recognizer forgets this pointer.
                s.captured.insert(id, i);
                if let Some(g) = s.gestures.as_mut() {
                    g.cancel(id, &mut Vec::new());
                }
            }
            (Some(_), _) => {}
            (None, ev) => {
                let frame = s.frame_size;
                s.fallback.handle(&ev, now, frame);
            }
        }
    }
}

fn handle_pointer(m: &mut Machine, id: u32, phase: PointerPhase, x: i32, y: i32, wx: i32, wy: i32) {
    // Window position (frontends deliver it; else map the game position through the shown viewport).
    let wp = if wx >= 0 && wy >= 0 {
        (wx as f32 + 0.5, wy as f32 + 0.5)
    } else {
        display::frame_viewport(m).game_to_window(x as f32 + 0.5, y as f32 + 0.5)
    };
    {
        let st = m.state::<ModsState>();
        let mut s = st.borrow_mut();
        s.win_cur = wp;
        s.win_ptrs.insert(id, wp);
    }
    handle_pointer_game(m, id, phase, x, y);
    if matches!(phase, PointerPhase::Up | PointerPhase::Cancel) {
        m.state::<ModsState>().borrow_mut().win_ptrs.remove(&id);
    }
}

fn handle_pointer_game(m: &mut Machine, id: u32, phase: PointerPhase, x: i32, y: i32) {
    let now = m.real_ms();
    let st = m.state::<ModsState>();
    let (fx, fy) = (x as f32, y as f32);
    let captured = st.borrow_mut().captured.get(&id).copied();
    if let Some(i) = captured {
        let ev = match phase {
            PointerPhase::Down => TouchEvent::Down { id, x: fx, y: fy },
            PointerPhase::Move => TouchEvent::Move { id, x: fx, y: fy },
            PointerPhase::Up => TouchEvent::Up { id, x: fx, y: fy },
            PointerPhase::Cancel => TouchEvent::Cancel { id },
        };
        let n = mod_count(m);
        for k in 0..n {
            with_mod(m, k, |md, m| md.observe(m, &ev));
        }
        with_mod(m, i, |md, m| md.on_touch(m, &ev));
        if matches!(phase, PointerPhase::Up | PointerPhase::Cancel) {
            st.borrow_mut().captured.remove(&id);
        }
        return;
    }
    let mut evs = Vec::new();
    {
        let mut s = st.borrow_mut();
        let Some(g) = s.gestures.as_mut() else { return };
        match phase {
            PointerPhase::Down => g.down(id, fx, fy, now, &mut evs),
            PointerPhase::Move => g.moved(id, fx, fy, now, &mut evs),
            PointerPhase::Up => g.up(id, fx, fy, now, &mut evs),
            PointerPhase::Cancel => g.cancel(id, &mut evs),
        }
    }
    dispatch_touch(m, evs, now);
}

/// `MachineExt::event_filter`: raw pointers go through gestures/mods/fallback; everything else passes.
fn event_filter(m: &mut Machine, e: HostEvent, out: &mut Vec<HostEvent>) {
    match e {
        HostEvent::Pointer { id, phase, x, y, wx, wy } => {
            handle_pointer(m, id, phase, x, y, wx, wy);
            let now = m.real_ms();
            out.extend(m.state::<ModsState>().borrow_mut().fallback.drain_due(now));
        }
        HostEvent::Key { vk, down } => {
            if !dispatch_key(m, vk, down) {
                out.push(e);
            }
        }
        e => out.push(e),
    }
}

/// Scheduler service: long press timing, delayed mouse events, mod ticks, overlay refresh.
fn service(m: &mut Machine) {
    let now = m.real_ms();
    let st = m.state::<ModsState>();
    let mut evs = Vec::new();
    {
        let mut s = st.borrow_mut();
        if let Some(g) = s.gestures.as_mut() {
            if g.fingers() > 0 {
                g.tick(now, &mut evs);
            }
        }
    }
    if !evs.is_empty() {
        dispatch_touch(m, evs, now);
    }
    let due = {
        let mut s = st.borrow_mut();
        if s.fallback.pending() {
            s.fallback.drain_due(now)
        } else {
            Vec::new()
        }
    };
    for e in due {
        m.input.push(e);
    }
    let n = mod_count(m);
    let mut redraw = false;
    for i in 0..n {
        redraw |= with_mod(m, i, |md, m| {
            md.tick(m, now);
            md.needs_redraw()
        })
        .unwrap_or(false);
    }
    {
        let mut s = st.borrow_mut();
        if !s.deferred.is_empty() && s.safe_points.is_empty() && !s.warned_no_safe_point {
            s.warned_no_safe_point = true;
            drop(s);
            m.log(LogLevel::Warn, "mods: deferred work queued but no safe point is installed (unknown exe?)");
        }
    }
    if redraw {
        let due = {
            let mut s = st.borrow_mut();
            if now.saturating_sub(s.last_refresh_ms) >= 33 {
                s.last_refresh_ms = now;
                true
            } else {
                false
            }
        };
        if due {
            m.refresh_overlay();
        }
    }
}

fn overlay_wanted(m: &mut Machine) -> bool {
    let size = m.last_frame_size();
    let st = m.state::<ModsState>();
    let mut s = st.borrow_mut();
    s.frame_size = size;
    s.mods.iter().any(|md| md.as_ref().is_some_and(|md| md.wants_draw_frame()))
}

/// `MachineExt::window_overlay`: record the window layer of every drawing mod and publish it when it changed.
fn window_compose(m: &mut Machine, frame: &FrameBuffer) {
    let t0 = m.stats.host_prof_start();
    let size = (frame.width, frame.height);
    m.state::<ModsState>().borrow_mut().frame_size = size;
    let vp = display::frame_viewport(m);
    m.state::<ModsState>().borrow_mut().draw_vp = Some(vp);
    let mut l = Layer::new(vp.win_w, vp.win_h);
    let n = mod_count(m);
    let mut drew = false;
    for i in 0..n {
        let wants = m.state::<ModsState>().borrow_mut().mods.get(i).and_then(|o| o.as_ref()).is_some_and(|md| md.wants_draw());
        if wants {
            drew = true;
            with_mod(m, i, |md, m| md.draw_window(m, &mut l, frame));
        }
    }
    let list = l.finish();
    let now = m.real_ms();
    let st = m.state::<ModsState>();
    let mut s = st.borrow_mut();
    s.draw_vp = None;
    if drew {
        s.last_refresh_ms = now;
    }
    let same = s.layer_last.as_ref().is_some_and(|(l0, p0, f0)| {
        l0.hash == list.hash && l0.items.len() == list.items.len() && *p0 == vp.panel && *f0 == size
    });
    if !same {
        let list = Arc::new(list);
        s.layer_last = Some((list.clone(), vp.panel, size));
        if let Some(sink) = s.layer_sink.clone() {
            drop(s);
            sink.publish(list, vp.panel, size);
        }
    }
    m.stats.host_prof_section("present: window layer record", t0);
}

/// The last published window layer (draw list, panel split, frame size), e.g. for screenshots.
pub fn last_layer(m: &mut Machine) -> Option<(Arc<layer::DrawList>, Option<display::PanelSplit>, (u32, u32))> {
    m.state::<ModsState>().borrow_mut().layer_last.clone()
}

fn overlay_compose(m: &mut Machine, fb: &mut FrameBuffer) -> bool {
    let n = mod_count(m);
    let mut drew = false;
    for i in 0..n {
        let st = m.state::<ModsState>();
        let wants = st.borrow_mut().mods.get(i).and_then(|o| o.as_ref()).is_some_and(|md| md.wants_draw_frame());
        if !wants {
            continue;
        }
        drew = true;
        with_mod(m, i, |md, m| {
            let mut c = Canvas::new(fb);
            md.draw(m, &mut c);
        });
    }
    if drew {
        m.state::<ModsState>().borrow_mut().last_refresh_ms = m.real_ms();
    }
    drew
}
