//! Game time display (user request 2026-11-08, like Settlers United): `h:mm:ss` of the running game, on by default,
//! switched in the menus (Classic floating menu column, Mobile HUD settings).
//!
//! Source: the game's own clock `[0x7dfd48]` (logic ticks of 64 ms; what its T-key clock prints, docs/reverse-engineering/
//! game-state-ui.md "What is drawn as text over the map"), so it follows game speed, pauses and saved games.
//! Drawn in the window layer: Classic top centre of the map part; Mobile HUD below its resource strip (top right
//! when the FPS counter sits top left).

use s3_runtime::Machine;

use s3_runtime::FrameBuffer;
use crate::layer::Layer;
use crate::overlay::{measure_text, Rgba, Weight};
use crate::Mod;

/// Game clock in logic ticks of 64 ms (15.6 per s at 1x). Verified 2026-11-08: 838 ticks -> the game's own
/// T-key clock showed 00:00:53.
const GAME_CLOCK_TICKS: u32 = 0x007d_fd48;
const TICK_MS: u64 = 64;

/// Game time in ms (0 outside a game).
pub fn game_ms(m: &mut Machine) -> u64 {
    if !crate::res::in_game_frame(m) {
        return 0;
    }
    m.mem.read_u32(GAME_CLOCK_TICKS) as u64 * TICK_MS
}

/// `h:mm:ss`.
pub fn format(ms: u64) -> String {
    let s = ms / 1000;
    format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

/// Is the display switched on?
pub fn enabled(m: &mut Machine) -> bool {
    crate::settings(m).show_game_time
}

/// Menu toggle; returns the new state.
pub fn toggle(m: &mut Machine) -> bool {
    let mut s = crate::settings(m);
    s.show_game_time = !s.show_game_time;
    let on = s.show_game_time;
    let _ = crate::set_settings(m, s);
    on
}

#[derive(Default)]
pub struct GameTimeMod {
    on: bool,
    shown: String,
}

impl Mod for GameTimeMod {
    fn name(&self) -> &str {
        "game-time"
    }

    fn tick(&mut self, m: &mut Machine, _now: u64) {
        self.on = m.state::<crate::ModsState>().borrow_mut().settings.show_game_time;
    }

    fn wants_draw(&self) -> bool {
        self.on
    }

    fn draw_window(&mut self, m: &mut Machine, c: &mut Layer, _frame: &FrameBuffer) {
        if crate::draw_viewport(m).layout.is_some() || !crate::res::in_game_frame(m) {
            return;
        }
        self.shown = format(game_ms(m));
        let vp = crate::draw_viewport(m);
        let (vx, vy, vw, vh) = vp.dst_rect_clipped();
        let sk = c.ui().clamp(0.3, 4.0);
        let px = 13.0 * sk;
        let pad = (4.0 * sk).round().max(1.0) as i32;
        let m2 = (3.0 * sk).round() as i32;
        let (tw, th) = measure_text(px, Weight::Bold, &self.shown);
        let (w, h) = (tw + 2 * pad, th + pad);
        let hud = crate::settings(m).ui_mode == crate::settings::UiMode::MobileHud;
        let (_, strip) = if hud { crate::ui_hud::bar_and_strip_h(m, vw, vh) } else { (0, 0) };
        let left = match vp.panel {
            Some(p) => vp.game_to_window(p.art_right(0.0).max(p.w as f32) - 1.0, 0.0).0.round() as i32 + m2,
            None => vx + m2,
        };
        let d = crate::display::display_settings(m);
        let fps_top_left = d.fps_counter != crate::display::FpsCounter::Off && d.fps_pos == crate::display::FpsPos::TopLeft;
        let fps_top_center = d.fps_counter != crate::display::FpsCounter::Off && d.fps_pos == crate::display::FpsPos::TopCenter;
        // Classic: top centre of the map part (its selection chip sits top left, Deselect top right); HUD: below
        // the resource strip. Away from the FPS counter when it is placed there.
        let (x, y) = if hud {
            if fps_top_left { (vx + vw - w - m2, vy + m2) } else { (left, vy + strip + m2) }
        } else if fps_top_center {
            (left, vy + (40.0 * sk) as i32)
        } else {
            (left + (vx + vw - left - w) / 2, vy + m2)
        };
        c.fill_rounded_rect(x, y, w, h, 5.0 * sk, Rgba::rgba(0, 0, 0, 140));
        c.text(x + pad, y + pad / 2, px, Weight::Bold, Rgba::rgb(240, 225, 170), &self.shown);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn formats() {
        assert_eq!(super::format(0), "0:00:00");
        assert_eq!(super::format(3_723_000), "1:02:03");
    }
}
