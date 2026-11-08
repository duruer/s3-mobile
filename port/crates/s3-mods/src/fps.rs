//! FPS counter overlay (docs/porting/display.md "FPS counter"). Setting `display.fps_counter`:
//! `Off`, `Fps` (game frames per second) or `Detailed` (+ frame time avg / worst, guest MIPS, display rate).
//!
//! Counts real guest presents (`Machine::stats.presents`, overlay-only refreshes do not count) and guest
//! instructions over a 500 ms window. Drawn small in the top-right corner (Mobile HUD: top centre) of the part of the
//! window that shows the game, into the window layer ([`crate::layer`]): the same size on screen at every zoom
//! level. Presentation only.

use s3_runtime::Machine;

use s3_runtime::FrameBuffer;

use crate::display::{DisplayState, FpsCounter};
use crate::layer::Layer;
use crate::overlay::{measure_text, Rgba, Weight};
use crate::{Mod, ModsState};

/// Measurement window.
const WINDOW_MS: u64 = 500;

/// Rolling statistics (platform independent; unit tested).
#[derive(Debug, Clone, Default)]
pub struct FpsStats {
    win_start_ms: u64,
    win_presents: u64,
    win_instr: u64,
    last_present_ms: Option<u64>,
    frame_sum_ms: u64,
    frame_n: u64,
    frame_max_ms: u64,
    /// Results of the last finished window.
    pub fps: f32,
    pub frame_avg_ms: f32,
    pub frame_max_ms_shown: u64,
    pub mips: f32,
    pub have: bool,
}

impl FpsStats {
    /// A real present happened at `now`.
    pub fn on_present(&mut self, now: u64) {
        if let Some(l) = self.last_present_ms {
            let d = now.saturating_sub(l);
            self.frame_sum_ms += d;
            self.frame_n += 1;
            self.frame_max_ms = self.frame_max_ms.max(d);
        }
        self.last_present_ms = Some(now);
    }

    /// Sample totals (`presents`, `instructions`) at `now`; closes the window every [`WINDOW_MS`].
    pub fn sample(&mut self, now: u64, presents: u64, instructions: u64) {
        if self.win_start_ms == 0 && !self.have && self.win_presents == 0 && self.win_instr == 0 {
            self.win_start_ms = now.max(1);
            self.win_presents = presents;
            self.win_instr = instructions;
            return;
        }
        let dt = now.saturating_sub(self.win_start_ms);
        if dt < WINDOW_MS {
            return;
        }
        let secs = dt as f32 / 1000.0;
        self.fps = presents.saturating_sub(self.win_presents) as f32 / secs;
        self.mips = instructions.saturating_sub(self.win_instr) as f32 / secs / 1e6;
        self.frame_avg_ms = if self.frame_n > 0 { self.frame_sum_ms as f32 / self.frame_n as f32 } else { 0.0 };
        self.frame_max_ms_shown = self.frame_max_ms;
        self.frame_sum_ms = 0;
        self.frame_n = 0;
        self.frame_max_ms = 0;
        self.win_start_ms = now;
        self.win_presents = presents;
        self.win_instr = instructions;
        self.have = true;
    }

    /// Text lines for the counter.
    pub fn lines(&self, mode: FpsCounter, host_fps: Option<f32>, refresh_hz: Option<f32>) -> Vec<String> {
        if !self.have {
            return vec!["-- fps".into()];
        }
        let mut v = vec![format!("{:.0} fps", self.fps)];
        if mode == FpsCounter::Detailed {
            v.push(format!("{:.1} ms (max {})", self.frame_avg_ms, self.frame_max_ms_shown));
            v.push(format!("{:.0} MIPS", self.mips));
            match (host_fps, refresh_hz) {
                (Some(h), Some(r)) => v.push(format!("disp {h:.0}/{r:.0} Hz")),
                (Some(h), None) => v.push(format!("disp {h:.0}")),
                (None, Some(r)) => v.push(format!("{r:.0} Hz")),
                _ => {}
            }
        }
        v
    }
}

/// The overlay mod (installed by [`crate::install`]).
#[derive(Default)]
pub struct FpsMod {
    mode: FpsCounter,
    stats: FpsStats,
    seen_presents: u64,
}

/// Guest instructions retired so far (all threads).
fn total_instructions(m: &Machine) -> u64 {
    m.threads.iter().map(|t| t.cpu.instructions).sum::<u64>().max(m.stats.instructions)
}

impl Mod for FpsMod {
    fn name(&self) -> &str {
        "fps-counter"
    }

    fn tick(&mut self, m: &mut Machine, now_ms: u64) {
        self.mode = m.state::<ModsState>().borrow_mut().settings.display.fps_counter;
        if self.mode == FpsCounter::Off {
            self.stats = FpsStats::default();
            return;
        }
        let p = m.stats.presents;
        let i = total_instructions(m);
        self.stats.sample(now_ms, p, i);
    }

    fn wants_draw(&self) -> bool {
        self.mode != FpsCounter::Off
    }

    fn draw_window(&mut self, m: &mut Machine, c: &mut Layer, _frame: &FrameBuffer) {
        let now = m.now_ms();
        let p = m.stats.presents;
        if p != self.seen_presents {
            self.seen_presents = p;
            self.stats.on_present(now);
        }
        let (host_fps, refresh) = {
            let st = m.state::<DisplayState>();
            let s = st.borrow_mut();
            (s.host_fps, s.refresh_hz)
        };
        let lines = self.stats.lines(self.mode, host_fps, refresh);
        let (vx, vy, vw, vh) = crate::draw_viewport(m).dst_rect_clipped();
        // Window pixels: 11 px at a 600 px high window (as it looked at a 600 px high frame shown full screen).
        let sk = c.ui().clamp(0.3, 4.0);
        let px = 11.0 * sk;
        let pad = (4.0 * sk).round().max(1.0) as i32;
        let lh = measure_text(px, Weight::Bold, "0").1.max(1);
        let w = lines.iter().map(|l| measure_text(px, Weight::Bold, l).0).max().unwrap_or(0) + 2 * pad;
        let h = lh * lines.len() as i32 + pad;
        // Position setting; Auto = top right, in the Mobile HUD top centre (its full-screen map "Close" and "Back to
        // HUD" sit top right). Never over the HUD's resource strip / bottom bar or the Classic panel.
        let hud = m.state::<crate::ModsState>().borrow_mut().settings.ui_mode == crate::settings::UiMode::MobileHud;
        let pos = crate::display::display_settings(m).fps_pos;
        let vp = crate::draw_viewport(m);
        let m2 = (2.0 * sk).round() as i32;
        let (bar, strip) = if hud { crate::ui_hud::bar_and_strip_h(m, vw, vh) } else { (0, 0) };
        // Left edge of the free area: right of the Classic panel (incl. the minimap frame's edge at the top).
        let left = match vp.panel {
            Some(p) => vp.game_to_window(p.art_right(0.0).max(p.w as f32) - 1.0, 0.0).0.round() as i32 + m2,
            None => vx + m2,
        };
        use crate::display::FpsPos as P;
        let pos = if pos == P::Auto { if hud { P::TopCenter } else { P::TopRight } } else { pos };
        let x = match pos {
            P::TopLeft | P::BottomLeft => left,
            P::TopCenter | P::BottomCenter => left + (vx + vw - left - w) / 2,
            _ => (vx + vw - w - m2).max(left),
        };
        let y = match pos {
            P::TopLeft if hud => vy + strip + m2,
            P::BottomLeft | P::BottomCenter | P::BottomRight => vy + vh - bar - h - m2,
            _ => vy + m2,
        };
        c.fill_rect(x, y, w, h, Rgba::rgba(0, 0, 0, 150));
        let color = if !self.stats.have || self.stats.fps >= 55.0 {
            Rgba::rgb(140, 255, 140)
        } else if self.stats.fps >= 30.0 {
            Rgba::rgb(255, 230, 110)
        } else {
            Rgba::rgb(255, 130, 110)
        };
        for (k, l) in lines.iter().enumerate() {
            c.text(x + pad, y + pad / 2 + k as i32 * lh, px, Weight::Bold, if k == 0 { color } else { Rgba::WHITE }, l);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_rates() {
        let mut s = FpsStats::default();
        s.sample(1000, 10, 1_000_000);
        for k in 0..31 {
            s.on_present(1000 + k * 16);
        }
        s.sample(1200, 20, 2_000_000);
        assert!(!s.have, "window not finished yet");
        s.sample(1500, 40, 51_000_000);
        assert!(s.have);
        assert!((s.fps - 60.0).abs() < 0.01, "{}", s.fps);
        assert!((s.mips - 100.0).abs() < 0.01, "{}", s.mips);
        assert!((s.frame_avg_ms - 16.0).abs() < 0.01);
        assert_eq!(s.frame_max_ms_shown, 16);
        assert_eq!(s.lines(FpsCounter::Fps, None, None), vec!["60 fps".to_string()]);
        let d = s.lines(FpsCounter::Detailed, Some(120.0), Some(120.0));
        assert_eq!(d.len(), 4);
        assert_eq!(d[2], "100 MIPS");
        assert_eq!(d[3], "disp 120/120 Hz");
    }
}
