//! Zoom = the game's internal render resolution (res-zoom, docs/porting/resolution-zoom.md).
//!
//! A zoom level is a render height from [`LEVEL_HEIGHTS`]; the width follows the aspect ratio of the window
//! (display mode "Native wide") or 4:3. The frontend scales the frame to the screen, so a smaller render size
//! shows less map with bigger pixels (zoomed in) and a bigger one more map (zoomed out). Pinch (both UI modes,
//! [`pinch`]) moves through the levels continuously and keeps the map point under the fingers fixed
//! ([`anchor_camera`]); menus call [`zoom_in`] / [`zoom_out`] / [`set_level`]. The level is saved in
//! `mods-settings.json` (`res.zoom_level`). [`ZoomMod`] publishes the wanted size to [`crate::res`] every slice.

use s3_runtime::Machine;

use crate::display::DisplayMode;
use crate::gesture::Phase;
use crate::res;
use crate::settings::UiMode;

/// Render heights of the zoom levels (index 0 = most zoomed in). Max = [`res::MAX_H`], min = [`res::MIN_H`].
/// 280 and 320 added 2026-11-08 (more zoom in); 200 and 240 by overlay-window (the HUD is drawn in the window layer
/// now, so it no longer depends on the render size); 840..1200 added by res-tall (taller snapshot grid).
pub const LEVEL_HEIGHTS: [u32; 22] =
    [200, 240, 280, 320, 360, 400, 440, 480, 520, 560, 600, 640, 680, 720, 760, 780, 840, 900, 960, 1040, 1120, 1200];
/// Default level: 600 px high (like the game's 800x600).
pub const DEFAULT_LEVEL: u32 = 10;
/// Lowest level in UI mode A (the original panel art needs at least 480 px).
pub const CLASSIC_MIN_LEVEL: u32 = 7;
/// Pinch scale factor per level (fingers apart by this factor = one level zoomed in).
pub const PINCH_STEP: f32 = 1.12;
/// Minimum time between two switches during a pinch (ms). The next switch also waits until the game finished
/// the previous one.
pub const PINCH_SWITCH_MS: u64 = 33;
/// During a pinch the render height follows the fingers continuously in steps of this many pixels (the levels
/// of [`LEVEL_HEIGHTS`] are 40 px apart, which looked like jumps); the end of the pinch snaps to the nearest level.
pub const PINCH_STEP_H: u32 = 8;

/// Size for a level: height from the table, width = height * aspect (multiple of 8), clamped to the limits.
pub fn size_for(level: u32, aspect: f32, min_level: u32) -> (u32, u32) {
    let l = level.clamp(min_level, LEVEL_HEIGHTS.len() as u32 - 1) as usize;
    size_for_height(LEVEL_HEIGHTS[l], aspect)
}

/// Size for a render height: width = height * aspect (multiple of 8), clamped to the limits.
pub fn size_for_height(h: u32, aspect: f32) -> (u32, u32) {
    let aspect = if aspect.is_finite() && aspect > 0.3 { aspect } else { 4.0 / 3.0 };
    let w = ((h as f32 * aspect / 8.0).round() as u32) * 8;
    // Wider than MAX_W: keep the width, lower the height (same aspect).
    let (w, h) = if w > res::MAX_W { (res::MAX_W, ((res::MAX_W as f32 / aspect).round() as u32).max(res::MIN_H)) } else { (w, h) };
    // Narrower than MIN_W (near-square windows zoomed far in, e.g. a foldable's inner screen at 1.2:1): keep the
    // window's aspect with the minimum width and a taller frame. Clamping only the width made the frame wider
    // than the window: black bars above and below (user report 2026-11-08).
    let (w, h) = if w < res::MIN_W { (res::MIN_W, ((res::MIN_W as f32 / aspect).round() as u32).min(res::MAX_H)) } else { (w, h) };
    res::clamp_size(w, h)
}

/// Level after a pinch of `scale` (current finger distance / start distance) that began at `start`.
pub fn pinch_level(start: u32, scale: f32, min_level: u32) -> u32 {
    let max = LEVEL_HEIGHTS.len() as i32 - 1;
    if !(scale.is_finite() && scale > 0.0) {
        return start;
    }
    // Fingers apart (scale > 1) = zoom in = smaller render size = lower level.
    let steps = -(scale.ln() / PINCH_STEP.ln());
    let steps = if steps.abs() < 0.6 { 0 } else { steps.round() as i32 };
    (start as i32 + steps).clamp(min_level as i32, max) as u32
}

/// Render width for height `h` in UI mode A with the panel shown at its own scale ([`crate::display::PanelSplit`],
/// window `ww` x `wh`): panel width + the map part that fills the rest of the window at the map's scale
/// (`wh / h`), a multiple of 8.
pub fn classic_width(h: u32, ww: u32, wh: u32) -> u32 {
    let v = if h >= res::CLASSIC_VARIANT.h { res::CLASSIC_VARIANT } else { res::variant_for(h) };
    let pw_win = (v.panel_w as f64 * wh as f64 / v.h as f64).round();
    let map = (ww as f64 - pw_win).max(64.0) * h as f64 / wh.max(1) as f64;
    let w = (v.panel_w + crate::display::map_skip(v.panel_w, v.h)) as f64 + map;
    ((((w / 8.0).round() as u32) * 8).clamp(res::MIN_W, res::MAX_W)).max(v.panel_w + 64)
}

/// Window size when UI mode A will show the panel at its own scale (Classic, `NativeWide`, window known).
fn classic_split_window(m: &mut Machine) -> Option<(u32, u32)> {
    let (ui, mode) = {
        let st = m.state::<crate::ModsState>();
        let s = st.borrow_mut();
        (s.settings.ui_mode, s.settings.display.mode)
    };
    if ui != UiMode::ClassicTouch || mode != DisplayMode::NativeWide || std::env::var("S3_PANEL_SPLIT").is_ok_and(|v| v == "0") {
        return None;
    }
    if env_size("S3_RES_ASPECT").is_some() {
        return None;
    }
    crate::display::window_size(m).filter(|&(w, h)| w > 0 && h > 0)
}

/// Size for height `h` with the current aspect (and the Classic panel split's width rule). With the split, heights
/// below the panel art (480) render at 480 and the map part is cropped instead ([`classic_crop`]).
fn size_now(m: &mut Machine, h: u32, a: f32) -> (u32, u32) {
    let split = classic_split_window(m);
    let h = if split.is_some() { h.max(res::CLASSIC_VARIANT.h) } else { h };
    let (w, h) = size_for_height(h, a);
    match classic_split_window(m) {
        Some((ww, wh)) => (classic_width(h, ww, wh).min(w.max(res::MIN_W)), h),
        None => (w, h),
    }
}

/// [`anchor_camera`] with `fx` a fraction of the map view (`size.0 - pw`) instead of the frame.
pub fn anchor_camera_map(cam: (i32, i32), (fx, fy): (f32, f32), old: (u32, u32), new: (u32, u32), old_pw: f32, new_pw: f32) -> (i32, i32) {
    let (fx, fy) = (fx.clamp(0.0, 1.0), fy.clamp(0.0, 1.0));
    let (ox, oy) = (fx * (old.0 as f32 - old_pw), fy * old.1 as f32);
    let (nx, ny) = (fx * (new.0 as f32 - new_pw), fy * new.1 as f32);
    (cam.0 + (ox - nx).round() as i32, cam.1 + (oy - ny).round() as i32)
}

/// New camera so that the world point under screen fraction `(fx, fy)` stays there when the frame size changes
/// from `old` to `new`. `old_pw` / `new_pw` = map view x origin (panel width) before / after.
pub fn anchor_camera(cam: (i32, i32), (fx, fy): (f32, f32), old: (u32, u32), new: (u32, u32), old_pw: f32, new_pw: f32) -> (i32, i32) {
    let (fx, fy) = (fx.clamp(0.0, 1.0), fy.clamp(0.0, 1.0));
    let (ox, oy) = (fx * old.0 as f32 - old_pw, fy * old.1 as f32);
    let (nx, ny) = (fx * new.0 as f32 - new_pw, fy * new.1 as f32);
    (cam.0 + (ox - nx).round() as i32, cam.1 + (oy - ny).round() as i32)
}

/// Aspect ratio (w/h) the render size should have now.
pub fn aspect(m: &mut Machine) -> f32 {
    if let Some((w, h)) = env_size("S3_RES_ASPECT") {
        return w as f32 / h as f32;
    }
    let d = crate::display::display_settings(m);
    if d.mode != DisplayMode::Fit {
        if let Some((w, h)) = crate::display::window_size(m) {
            if w > 0 && h > 0 {
                return w as f32 / h as f32;
            }
        }
    }
    4.0 / 3.0
}

fn env_size(name: &str) -> Option<(u32, u32)> {
    let v = std::env::var(name).ok()?;
    let (a, b) = v.split_once(['x', 'X', ':'])?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

fn min_level(m: &mut Machine) -> u32 {
    let mode = m.state::<crate::ModsState>().borrow_mut().settings.ui_mode;
    // UI mode A with the panel split zooms below the panel art by cropping ([`classic_crop`]).
    if mode == UiMode::MobileHud || classic_split_window(m).is_some() { 0 } else { CLASSIC_MIN_LEVEL }
}

/// UI mode A with the panel split, wanted height below the 480 px panel art: the frame stays 480 high and only
/// this many rows of the map part are shown (centred, the width to match), scaled to the window. A smaller
/// render size would show exactly the same map pixels, but the panel art needs 480 rows of the frame. Pure
/// presentation (no render-size switch), so it is also instant. `None` = no crop.
pub fn classic_crop(m: &mut Machine) -> Option<u32> {
    if !enabled(m) || classic_split_window(m).is_none() || env_size("S3_RES").is_some() {
        return None;
    }
    let h = wanted_height(m);
    (h < res::CLASSIC_VARIANT.h).then_some(h.max(res::MIN_H))
}

/// Number of zoom levels.
pub fn level_count() -> u32 {
    LEVEL_HEIGHTS.len() as u32
}

/// Current zoom level (from the settings).
pub fn level(m: &mut Machine) -> u32 {
    m.state::<crate::ModsState>().borrow_mut().settings.res.zoom_level.min(level_count() - 1)
}

/// Is the custom resolution / zoom active (setting on and the patch installed)?
pub fn enabled(m: &mut Machine) -> bool {
    let on = m.state::<crate::ModsState>().borrow_mut().settings.res.enabled;
    on && m.state::<res::ResState>().borrow_mut().installed
}

/// Set the zoom level (clamped; saved to the settings). `anchor` = screen fractions to keep fixed.
pub fn set_level_anchored(m: &mut Machine, l: u32, anchor: Option<(f32, f32)>, persist: bool) {
    let l = l.clamp(min_level(m), level_count() - 1);
    let mut s = crate::settings(m);
    if s.res.zoom_level == l && s.res.zoom_height.is_none() {
        return;
    }
    s.res.zoom_level = l;
    s.res.zoom_height = None;
    if persist {
        let _ = crate::set_settings(m, s);
    } else {
        m.state::<crate::ModsState>().borrow_mut().settings = s;
    }
    let split = crate::display::panel_split(m).is_some();
    {
        let st = m.state::<res::ResState>();
        let mut r = st.borrow_mut();
        r.anchor = Some(anchor.unwrap_or((0.5, 0.5)));
        r.anchor_map = split && anchor.is_none();
    }
    update_target(m);
}

pub fn set_level(m: &mut Machine, l: u32) {
    set_level_anchored(m, l, None, true);
}

/// One level zoomed in (smaller render size).
pub fn zoom_in(m: &mut Machine) {
    let l = level(m);
    set_level(m, l.saturating_sub(1));
}

/// One level zoomed out (bigger render size).
pub fn zoom_out(m: &mut Machine) {
    let l = level(m);
    set_level(m, l + 1);
}

/// Turn the custom resolution on/off (saved).
pub fn set_enabled(m: &mut Machine, on: bool) {
    let mut s = crate::settings(m);
    s.res.enabled = on;
    let _ = crate::set_settings(m, s);
    update_target(m);
}

/// Render size of the current level (what [`res`] switches to), `None` when off.
pub fn target_size(m: &mut Machine) -> Option<(u32, u32)> {
    if !enabled(m) {
        return None;
    }
    if let Some((w, h)) = env_size("S3_RES") {
        return Some(res::clamp_size(w, h));
    }
    let a = aspect(m);
    let h = wanted_height(m);
    Some(size_now(m, h, a))
}

/// The zoom height wanted now (pinch in progress, saved pinch height or level), before [`size_now`].
fn wanted_height(m: &mut Machine) -> u32 {
    let a = aspect(m);
    // The Mobile HUD reads its building icons from the game's panel once: that needs >= 480 px for a moment.
    if m.state::<crate::ui_hud_game::HudShared>().borrow_mut().harvesting {
        return m.state::<PinchState>().borrow_mut().height.unwrap_or(0).max(crate::ui_hud_game::HARVEST_MIN_H);
    }
    if let Some(h) = m.state::<PinchState>().borrow_mut().height {
        return h;
    }
    let ml = min_level(m);
    let saved = m.state::<crate::ModsState>().borrow_mut().settings.res.zoom_height;
    if let Some(h) = saved {
        let (lo, hi) = (LEVEL_HEIGHTS[ml as usize], LEVEL_HEIGHTS[LEVEL_HEIGHTS.len() - 1]);
        return h.clamp(lo, hi);
    }
    size_for(level(m), a, ml).1
}

/// Render height a pinch in progress asks for (overrides the level until the pinch ends).
#[derive(Default)]
struct PinchState {
    height: Option<u32>,
}

/// Level whose height is nearest to `h`.
fn nearest_level(h: u32, min_level: u32) -> u32 {
    (min_level..level_count()).min_by_key(|&l| (LEVEL_HEIGHTS[l as usize] as i32 - h as i32).abs()).unwrap_or(min_level)
}

/// Display mode for the current aspect: the largest zoom size (zoom steps then only change the presented
/// size, the fast path of [`res`]).
pub fn mode_size(m: &mut Machine) -> Option<(u32, u32)> {
    if !enabled(m) {
        return None;
    }
    if let Some((w, h)) = env_size("S3_RES") {
        return Some(res::clamp_size(w, h));
    }
    let a = aspect(m);
    Some(size_for(level_count() - 1, a, 0))
}

/// Publish the wanted size to the resolution patch.
pub fn update_target(m: &mut Machine) {
    let t = target_size(m);
    let mode = mode_size(m);
    let installed = m.state::<res::ResState>().borrow_mut().installed;
    if installed {
        res::set_target_mode(m, t, mode);
    }
}

/// Short label for menus, e.g. "Zoom 7/12 (1480x600)".
pub fn label(m: &mut Machine) -> String {
    match target_size(m) {
        Some((w, h)) => format!("Zoom {}/{} ({w}x{h})", level(m) + 1, level_count()),
        None => "Zoom off".to_string(),
    }
}

/// Pinch state (owned by the caller, e.g. the touch mod).
#[derive(Debug, Default, Clone)]
pub struct Pinch {
    start_level: Option<u32>,
    /// Frame width when the pinch began: pointer positions are in game pixels of the current frame, so the
    /// recognizer's scale must be corrected after each resolution switch (else zooming feeds back on itself).
    start_frame_w: u32,
    start_frame_h: u32,
    /// Panel width of the start frame when UI mode A shows the panel at its own scale (anchor = map fraction).
    start_split_pw: Option<u32>,
    last_switch_ms: u64,
    /// Direction of the last switch in this pinch (+1 bigger, -1 smaller, 0 none).
    dir: i8,
}

/// Handle a pinch event (game-pixel centre `cx, cy`). Returns true if zoom handled it (custom resolution on).
///
/// While the fingers move the render height follows them continuously ([`PINCH_STEP_H`] px steps, one switch at a
/// time); the end snaps to the nearest level (saved). Each switch keeps the map point under the fingers fixed.
pub fn pinch(m: &mut Machine, p: &mut Pinch, phase: Phase, cx: f32, cy: f32, scale: f32) -> bool {
    if !enabled(m) {
        return false;
    }
    let now = m.now_ms();
    let (fw, fh) = m.last_frame_size();
    // Size of the frame on screen (the viewport the touches were mapped through at the gesture start).
    let (mapped_w, mapped_h) =
        crate::display::viewport(m).map(|v| (v.game_w, v.game_h)).filter(|&(w, h)| w > 0 && h > 0).unwrap_or((fw, fh));
    // UI mode A crop below 480 rows: the effective zoom height is the shown map height.
    let crop = crate::display::panel_split(m).and_then(|p| p.crop());
    let start_h = crate::display::viewport(m).and_then(|v| v.panel).and_then(|p| p.crop()).unwrap_or(mapped_h);
    let fh = crop.unwrap_or(fh);
    if phase == Phase::Begin || p.start_level.is_none() {
        p.start_level = Some(level(m));
        p.start_frame_w = mapped_w;
        p.start_frame_h = start_h;
        p.start_split_pw = crate::display::panel_split(m).map(|s| s.w);
        p.dir = 0;
    }
    // Pointer positions of one multi-finger gesture are all mapped through the viewport the gesture started on
    // (frontends freeze it: s3-android pinch_vp; s3run injects fixed game coordinates), so `scale` is the real
    // on-screen ratio and the centre is in pixels of the start frame.
    let ml = min_level(m);
    let (hmin, hmax) = (LEVEL_HEIGHTS[ml as usize], LEVEL_HEIGHTS[LEVEL_HEIGHTS.len() - 1]);
    let h0 = if p.start_frame_h > 0 { p.start_frame_h } else { LEVEL_HEIGHTS[level(m) as usize] };
    // Fingers apart (scale > 1) = zoom in = smaller render height.
    let want_h = if scale.is_finite() && scale > 0.0 { (h0 as f32 / scale).round() as u32 } else { h0 };
    let want_h = (want_h.clamp(hmin, hmax) / PINCH_STEP_H) * PINCH_STEP_H;
    let end = phase == Phase::End;
    let (sw, sh) = (p.start_frame_w, p.start_frame_h);
    let anchor = (sw > 0 && sh > 0).then(|| match p.start_split_pw.filter(|&pw| pw + 1 < sw) {
        // Panel at its own scale: the map part is what zooms; anchor in its fractions.
        Some(pw) => (((cx - pw as f32) / (sw - pw) as f32).clamp(0.0, 1.0), (cy / sh as f32).clamp(0.0, 1.0)),
        None => ((cx / sw as f32).clamp(0.0, 1.0), (cy / sh as f32).clamp(0.0, 1.0)),
    });
    let anchor_map = p.start_split_pw.is_some();
    let busy = res::switch_pending(m) || now.saturating_sub(p.last_switch_ms) < PINCH_SWITCH_MS;
    if end {
        // Stay exactly where the fingers stopped (snapping to the nearest 40 px level made a last jump, sometimes
        // backwards); the level is kept as the nearest one for the menus' zoom steps.
        let l = nearest_level(want_h, ml);
        let end_h = m.state::<PinchState>().borrow_mut().height.take().unwrap_or(fh);
        let mut s = crate::settings(m);
        s.res.zoom_level = l;
        s.res.zoom_height = Some(end_h);
        let _ = crate::set_settings(m, s);
        if target_size(m) != Some((fw, fh)) {
            let st = m.state::<res::ResState>();
            let mut r = st.borrow_mut();
            r.anchor = Some(anchor.unwrap_or((0.5, 0.5)));
            r.anchor_map = anchor_map;
        }
        update_target(m);
        p.start_level = None;
        p.last_switch_ms = now;
    } else if !busy && want_h.abs_diff(fh) >= PINCH_STEP_H * if (want_h as i64 - fh as i64).signum() as i8 == -p.dir { 3 } else { 1 } {
        // Reversing the direction needs a bigger change (no back-and-forth on finger jitter).
        p.dir = (want_h as i64 - fh as i64).signum() as i8;
        m.state::<PinchState>().borrow_mut().height = Some(want_h);
        {
            let st = m.state::<res::ResState>();
            let mut r = st.borrow_mut();
            r.anchor = Some(anchor.unwrap_or((0.5, 0.5)));
            r.anchor_map = anchor_map;
        }
        update_target(m);
        p.last_switch_ms = now;
    }
    if std::env::var_os("S3_TOUCH_TRACE").is_some() {
        eprintln!("zoom: pinch {phase:?} scale {scale:.2} want_h {want_h} cur_h {fh} busy {busy}");
    }
    true
}

/// Host-side mod: keeps [`res::ResState::target`] in sync with the settings / window aspect every slice.
#[derive(Default)]
pub struct ZoomMod {
    last: (Option<(u32, u32)>, Option<(u32, u32)>),
    /// Since when no game frame was rendered (ms; 0 = in game).
    out_since: u64,
    /// The start zoom was set for the next game.
    start_zoom_set: bool,
}

/// Time outside a game (no game frame) before the start zoom is set: longer than any zoom step's pause.
const START_ZOOM_AFTER_MS: u64 = 3000;

/// Size of a starting territory (castle area incl. its border) in game pixels, with a small margin; measured on
/// the random 384 maps (border about 1130 x 800 px).
const START_AREA: (f32, f32) = (1250.0, 850.0);

/// Render height that shows the whole start area: the Mobile HUD's bars take ~15 % of the height, the Classic
/// panel ~12 % of the width.
pub fn start_height(m: &mut Machine) -> u32 {
    let a = aspect(m);
    let map_w = if classic_split_window(m).is_some() { 0.88 } else { 1.0 };
    let h = (START_AREA.1 / 0.85).max(START_AREA.0 / (a * map_w));
    (((h / 8.0).round() as u32) * 8).clamp(res::MIN_H, res::MAX_H)
}

impl crate::Mod for ZoomMod {
    fn name(&self) -> &str {
        "zoom"
    }
    fn tick(&mut self, m: &mut Machine, now: u64) {
        // Outside a game (menus) for a while: the next game starts at the zoom that shows the whole start area,
        // not at the last zoom of the previous game (user request 2026-11-08). The game centres the camera on
        // the castle itself (res switch hook, anchor centre).
        if res::in_game(m) {
            self.out_since = 0;
            self.start_zoom_set = false;
        } else if self.out_since == 0 {
            self.out_since = now.max(1);
        } else if !self.start_zoom_set && now.saturating_sub(self.out_since) >= START_ZOOM_AFTER_MS && enabled(m) {
            self.start_zoom_set = true;
            let h = start_height(m);
            let ml = min_level(m);
            let st = m.state::<crate::ModsState>();
            let mut s = st.borrow_mut();
            s.settings.res.zoom_height = Some(h);
            s.settings.res.zoom_level = nearest_level(h, ml);
        }
        let t = target_size(m);
        let mode = mode_size(m);
        if (t, mode) != self.last {
            self.last = (t, mode);
            res::set_target_mode(m, t, mode);
        }
    }
}

/// Register the zoom mod (after [`res::install`]).
pub fn install(m: &mut Machine) {
    crate::add_mod(m, Box::new(ZoomMod::default()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_for_levels() {
        // 4:3 reproduces the game's sizes at 480 / 600.
        assert_eq!(size_for(7, 4.0 / 3.0, 0), (640, 480));
        assert_eq!(size_for(DEFAULT_LEVEL, 4.0 / 3.0, 0), (800, 600));
        // Phone cover screen 2229x904 (aspect 2.466): 600 high -> 1480x600 (the user's example).
        let a = 2229.0 / 904.0;
        assert_eq!(size_for(10, a, 0), (1480, 600));
        assert_eq!(size_for(13, a, 0), (1776, 720));
        let (w, h) = size_for(15, a, 0);
        assert_eq!(h, 780);
        assert_eq!(w, 1920);
        // Classic mode never goes below 480.
        assert_eq!(size_for(0, 4.0 / 3.0, CLASSIC_MIN_LEVEL), (640, 480));
        // The new bottom (HUD): 200 high, at least MIN_W wide; narrower windows keep their aspect (taller frame).
        assert_eq!(size_for(0, 2316.0 / 904.0, 0), (512, 200));
        assert_eq!(size_for(0, 4.0 / 3.0, 0), (res::MIN_W, 240));
        assert_eq!(size_for(0, 2176.0 / 1812.0, 0), (res::MIN_W, 266));
        // Classic panel split widths: map part keeps the window aspect.
        for h in [480u32, 520, 600, 700, 768, 900, 1200] {
            let w = classic_width(h, 2316, 904);
            let v = res::CLASSIC_VARIANT;
            let pw_win = (v.panel_w as f32 * 904.0 / v.h as f32).round();
            let map_scale_x = (2316.0 - pw_win) / (w - v.panel_w - crate::display::map_skip(v.panel_w, v.h)) as f32;
            let map_scale_y = 904.0 / h as f32;
            assert!((map_scale_x / map_scale_y - 1.0).abs() < 0.012, "{h}: {w} {map_scale_x} {map_scale_y}");
            assert!(w <= size_for_height(h, 2316.0 / 904.0).0 + 16);
        }
        // Very wide: width capped, height follows.
        let (w, h) = size_for(level_count() - 1, 4.0, 0);
        assert_eq!(w, res::MAX_W);
        assert_eq!(h, res::MAX_W / 4);
        // The top level is the largest size the patched game supports.
        assert_eq!(size_for(level_count() - 1, 2316.0 / 904.0, 0), (3072, 1200));
        // Every level of a few aspects fits the snapshot grid.
        for a in [4.0 / 3.0, 16.0 / 9.0, 2229.0 / 904.0, 2176.0 / 1812.0, 3.0] {
            for l in 0..level_count() {
                let (w, h) = size_for(l, a, 0);
                assert!(res::grid_columns_needed(w, h) <= res::SNAP_COLS && res::grid_rows_needed(h) < res::SNAP_ROWS);
                assert!(res::sprite_buckets_needed(h) <= res::SPRITE_BUCKETS);
            }
        }
    }

    #[test]
    fn pinch_levels() {
        assert_eq!(pinch_level(6, 1.0, 0), 6);
        assert_eq!(pinch_level(6, 1.05, 0), 6);
        assert_eq!(pinch_level(6, 1.12, 0), 5);
        assert_eq!(pinch_level(6, 1.12f32.powi(3), 0), 3);
        assert_eq!(pinch_level(6, 1.0 / 1.12, 0), 7);
        assert_eq!(pinch_level(6, 0.01, 0), level_count() - 1);
        assert_eq!(pinch_level(6, 10.0, 0), 0);
        assert_eq!(pinch_level(6, 10.0, 3), 3);
    }

    #[test]
    fn camera_anchor_keeps_point() {
        // Centre anchor, 800x600 -> 1480x600, full-screen map: x moves by half the width difference.
        let c = anchor_camera((1000, 500), (0.5, 0.5), (800, 600), (1480, 600), 0.0, 0.0);
        assert_eq!(c, (1000 - 340, 500));
        // World point under the anchor is the same before and after.
        let (fx, fy) = (0.25, 0.75);
        let (old, new) = ((1480, 600), (1184, 480));
        let c0 = (2000, 900);
        let c1 = anchor_camera(c0, (fx, fy), old, new, 176.0, 136.0);
        let w0 = (c0.0 as f32 + fx * 1480.0 - 176.0, c0.1 as f32 + fy * 600.0);
        let w1 = (c1.0 as f32 + fx * 1184.0 - 136.0, c1.1 as f32 + fy * 480.0);
        assert!((w0.0 - w1.0).abs() <= 1.0 && (w0.1 - w1.1).abs() <= 1.0, "{w0:?} {w1:?}");
    }
}
