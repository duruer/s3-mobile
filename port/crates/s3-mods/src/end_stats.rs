//! Statistics over game time on the end-of-game "Game Statistics" screen (user request 2026-11-08: "make that
//! scoreboard like Settlers United made it"). Settlers United adds a "View Extended Game Stats" button there that
//! opens charts of Points / Settlers / Buildings / Food / Mines / Gold / Mana / Soldiers / Battles against game
//! time (docs/settlers-united/README.md). This is our own version of that idea, drawn in the window layer:
//!
//! - While a game runs, every [`SAMPLE_TICKS`] of game time, the game's own per-player statistics counters are
//!   read (`STATS_BASE`, one 0x44-byte record per player id 0..19; docs/reverse-engineering/game-state-ui.md
//!   "End-of-game statistics"). Read only: guest memory is never written (lockstep MP unaffected).
//! - On the statistics screen (detected menu layout `statistics*`), a button "Statistics over time" opens a
//!   full-window sheet: one tab per column of the game's table, a line per player, the game's own player rows
//!   (rank, team, name; copied from the frame) as the legend. Any other tap still leaves the screen as before.
//!
//! The history starts when the game (or the loaded save) starts in this session; it is not saved.

use s3_runtime::{FrameBuffer, Machine};

use crate::gesture::TouchEvent;
use crate::layer::Layer;
use crate::overlay::{measure_text, Rgba, Weight};
use crate::{Mod, ModEvent};

/// Per-player statistics records: `STATS_BASE + id * STATS_STRIDE`, 20 players.
pub const STATS_BASE: u32 = 0x007a_cfac;
pub const STATS_STRIDE: u32 = 0x44;
pub const PLAYERS: usize = 20;
/// Game clock (logic ticks of 64 ms), see `game_time.rs`.
const GAME_CLOCK_TICKS: u32 = 0x007d_fd48;
/// Sample every 10 s of game time (156 ticks of 64 ms).
pub const SAMPLE_TICKS: u32 = 156;
/// At most this many samples per game (older ones thinned out: every second one dropped).
const MAX_SAMPLES: usize = 720;
/// Out of the game for longer than this, a new game (or a loaded save) starts a new history.
const NEW_GAME_GAP_MS: u64 = 5_000;

/// Columns of the game's table (same order). Record offsets: rank `+0`, score `+0x1c`, counters `+0x20..+0x3c`.
pub const COLUMNS: [&str; 9] = ["Score", "Settlers", "Buildings", "Food", "Mines", "Gold", "Manna", "Soldiers", "Battles"];

/// One sample: game ticks and per player the 9 column values.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub tick: u32,
    pub v: [[u32; 9]; PLAYERS],
}

/// The 9 values of one record as the game's table shows them. `final_` = after the game computed its statistics
/// (`0x53e720`: score stored at `+0x1c`, gold halved in place); during the game the score is computed with the
/// same formula: 2 x settlers + buildings + food + mines + gold + manna + 2 x soldiers + 5 x battles.
pub fn values(rec: &[u32; 17], final_: bool) -> [u32; 9] {
    let c = |i: usize| rec[8 + i];
    let (settlers, buildings, food, mines, gold, manna, soldiers, battles) = (c(0), c(1), c(2), c(3), c(4), c(5), c(6), c(7));
    let w = u32::wrapping_add;
    if final_ {
        [rec[7], settlers, buildings, food, mines, gold, manna, soldiers, battles]
    } else {
        let score = [settlers, settlers, buildings, food, mines, gold, manna, soldiers, soldiers, battles.wrapping_mul(5)]
            .into_iter()
            .fold(0, w);
        [score, settlers, buildings, food, mines, gold >> 1, manna, soldiers, battles]
    }
}

fn read_record(m: &mut Machine, id: usize) -> [u32; 17] {
    let base = STATS_BASE + id as u32 * STATS_STRIDE;
    std::array::from_fn(|i| m.mem.read_u32(base + 4 * i as u32))
}

/// Sample history of one game.
#[derive(Debug, Default, Clone)]
pub struct History {
    pub samples: Vec<Sample>,
    /// Current sampling interval (doubles when the history is thinned out).
    pub every: u32,
}

impl History {
    /// Add a sample if `every` ticks passed since the last one (or it is the first / the final one).
    pub fn push(&mut self, s: Sample, force: bool) {
        if self.every == 0 {
            self.every = SAMPLE_TICKS;
        }
        if let Some(last) = self.samples.last_mut() {
            if s.tick < last.tick {
                return;
            }
            if s.tick == last.tick {
                *last = s;
                return;
            }
            if !force && s.tick < last.tick + self.every {
                return;
            }
        }
        self.samples.push(s);
        if self.samples.len() > MAX_SAMPLES {
            // Keep the first and every second one after it.
            let mut i = 0;
            self.samples.retain(|_| {
                i += 1;
                i % 2 == 1
            });
            self.every *= 2;
        }
    }
}

/// Players shown on the statistics screen: (player id, rank 1..), by rank (the game's row order).
pub fn ranked(rec_rank: &[u32; PLAYERS]) -> Vec<(usize, u32)> {
    let mut v: Vec<(usize, u32)> = rec_rank.iter().enumerate().filter(|(_, &r)| (1..=PLAYERS as u32).contains(&r)).map(|(i, &r)| (i, r)).collect();
    v.sort_by_key(|&(_, r)| r);
    v.dedup_by_key(|p| p.1);
    v
}

/// "Nice" axis maximum >= v (1, 2, 5 x 10^k).
pub fn nice_max(v: u32) -> u32 {
    let v = v.max(1) as u64;
    let mut p = 1u64;
    loop {
        for k in [1, 2, 5] {
            if k * p >= v {
                return (k * p).min(u32::MAX as u64) as u32;
            }
        }
        p *= 10;
    }
}

const SHEET: Rgba = Rgba::rgba(16, 20, 28, 244);
const PLOT_BG: Rgba = Rgba::rgba(30, 36, 48, 255);
const GRID: Rgba = Rgba::rgba(90, 100, 120, 120);
const TEXT: Rgba = Rgba::rgb(240, 236, 226);
const TEXT_DIM: Rgba = Rgba::rgb(168, 174, 188);
const BTN: Rgba = Rgba::rgba(52, 61, 80, 255);
const BTN_ON: Rgba = Rgba::rgba(196, 138, 36, 255);
const OPEN_BG: Rgba = Rgba::rgba(20, 16, 10, 215);
const GOLD: Rgba = Rgba::rgb(236, 196, 92);

/// Line colours by rank (distinct, readable on the dark plot).
const COLOURS: [Rgba; PLAYERS] = [
    Rgba::rgb(236, 72, 60),
    Rgba::rgb(76, 140, 255),
    Rgba::rgb(80, 200, 90),
    Rgba::rgb(245, 208, 50),
    Rgba::rgb(190, 110, 240),
    Rgba::rgb(250, 150, 50),
    Rgba::rgb(70, 214, 220),
    Rgba::rgb(240, 240, 240),
    Rgba::rgb(250, 120, 190),
    Rgba::rgb(170, 125, 80),
    Rgba::rgb(160, 230, 70),
    Rgba::rgb(120, 120, 255),
    Rgba::rgb(255, 90, 120),
    Rgba::rgb(40, 170, 140),
    Rgba::rgb(210, 180, 140),
    Rgba::rgb(150, 150, 150),
    Rgba::rgb(255, 180, 120),
    Rgba::rgb(130, 200, 255),
    Rgba::rgb(200, 90, 160),
    Rgba::rgb(110, 160, 60),
];

type Rect = (i32, i32, i32, i32);

fn hit(r: Rect, x: f32, y: f32) -> bool {
    x >= r.0 as f32 && y >= r.1 as f32 && x < (r.0 + r.2) as f32 && y < (r.1 + r.3) as f32
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Target {
    Open,
    Close,
    Tab(usize),
}

#[derive(Default)]
pub struct EndStatsMod {
    hist: History,
    /// Final sample taken for the current game (statistics screen seen).
    final_taken: bool,
    /// Ranks of the final sample (by player id).
    ranks: [u32; PLAYERS],
    in_game: bool,
    /// Real ms when the game was last seen running.
    last_game_ms: u64,
    /// The statistics screen is shown (menu layout `statistics*`).
    on_screen: bool,
    open: bool,
    tab: usize,
    /// Hit targets of the last drawn layer (window px).
    targets: Vec<(Rect, Target)>,
    /// Pointer captured by us, and the target it went down on.
    pressed: Option<(u32, Option<Target>)>,
    dirty: bool,
}

impl EndStatsMod {
    fn sample(m: &mut Machine, final_: bool) -> (Sample, [u32; PLAYERS]) {
        let tick = m.mem.read_u32(GAME_CLOCK_TICKS);
        let mut v = [[0u32; 9]; PLAYERS];
        let mut ranks = [0u32; PLAYERS];
        for (id, row) in v.iter_mut().enumerate() {
            let r = read_record(m, id);
            *row = values(&r, final_);
            ranks[id] = r[0];
        }
        (Sample { tick, v }, ranks)
    }

    fn shown_players(&self) -> Vec<(usize, u32)> {
        ranked(&self.ranks)
    }

    fn usable(&self) -> bool {
        self.final_taken && self.hist.samples.len() >= 2 && !self.shown_players().is_empty()
    }

    fn target_at(&self, x: f32, y: f32) -> Option<Target> {
        self.targets.iter().rev().find(|(r, _)| hit(*r, x, y)).map(|&(_, t)| t)
    }

    fn act(&mut self, t: Target) {
        match t {
            Target::Open => self.open = true,
            Target::Close => self.open = false,
            Target::Tab(i) => self.tab = i,
        }
        self.dirty = true;
    }

    fn draw_open_button(&mut self, c: &mut Layer, sk: f32) {
        let label = "Statistics over time";
        let px = 17.0 * sk;
        let (tw, th) = measure_text(px, Weight::Bold, label);
        let pad = (12.0 * sk) as i32;
        let (w, h) = (tw + 2 * pad, th + pad * 3 / 2);
        let mrg = (14.0 * sk) as i32;
        let (x, y) = (c.width() as i32 - w - mrg, c.height() as i32 - h - mrg);
        c.fill_rounded_rect(x, y, w, h, 8.0 * sk, OPEN_BG);
        c.stroke_rounded_rect(x, y, w, h, 8.0 * sk, 2.0 * sk, GOLD);
        c.text_centered(x, y, w, h, px, Weight::Bold, GOLD, label);
        self.targets.push(((x, y, w, h), Target::Open));
        let hint = "Tap anywhere else to continue";
        let hpx = 13.0 * sk;
        let (hw, hh) = measure_text(hpx, Weight::Regular, hint);
        c.fill_rounded_rect(mrg, y + h - hh - pad, hw + pad, hh + pad / 2, 6.0 * sk, Rgba::rgba(0, 0, 0, 120));
        c.text(mrg + pad / 2, y + h - hh - pad * 3 / 4, hpx, Weight::Regular, TEXT_DIM, hint);
    }

    fn draw_sheet(&mut self, c: &mut Layer, sk: f32) {
        let (ww, wh) = (c.width() as i32, c.height() as i32);
        c.fill_rect(0, 0, ww, wh, SHEET);
        let mrg = (12.0 * sk) as i32;
        // Header: title + Close.
        let hh = (40.0 * sk) as i32;
        c.text(mrg, mrg + (6.0 * sk) as i32, 20.0 * sk, Weight::Bold, GOLD, "Statistics over game time");
        let cw = (110.0 * sk) as i32;
        let close = (ww - cw - mrg, mrg, cw, hh);
        c.fill_rounded_rect(close.0, close.1, close.2, close.3, 8.0 * sk, BTN);
        c.text_centered(close.0, close.1, close.2, close.3, 17.0 * sk, Weight::Bold, TEXT, "Close");
        self.targets.push((close, Target::Close));
        // Tabs: the game's columns.
        let ty = mrg * 2 + hh;
        let th = (38.0 * sk) as i32;
        let gap = (6.0 * sk) as i32;
        let n = COLUMNS.len() as i32;
        let tw = (ww - 2 * mrg - gap * (n - 1)) / n;
        for (i, name) in COLUMNS.iter().enumerate() {
            let r = (mrg + i as i32 * (tw + gap), ty, tw, th);
            c.fill_rounded_rect(r.0, r.1, r.2, r.3, 7.0 * sk, if i == self.tab { BTN_ON } else { BTN });
            c.text_centered(r.0, r.1, r.2, r.3, 16.0 * sk, Weight::Bold, TEXT, name);
            self.targets.push((r, Target::Tab(i)));
        }
        // Body: plot left, legend right (below the plot on a narrow window).
        let by = ty + th + mrg;
        let (bw, bh) = (ww - 2 * mrg, wh - by - mrg);
        let players = self.shown_players();
        let row_h = (26.0 * sk) as i32;
        let legend_w = ((330.0 * sk) as i32).min(bw * 2 / 5);
        let side = bw > bh;
        let (plot, legend) = if side {
            ((mrg, by, bw - legend_w - mrg, bh), (mrg + bw - legend_w, by, legend_w, bh))
        } else {
            let lh = (row_h * players.len() as i32 + mrg).min(bh / 2);
            ((mrg, by, bw, bh - lh - mrg), (mrg, by + bh - lh, bw, lh))
        };
        self.draw_plot(c, sk, plot, &players);
        self.draw_legend(c, sk, legend, &players, row_h);
    }

    fn draw_plot(&self, c: &mut Layer, sk: f32, r: Rect, players: &[(usize, u32)]) {
        let s = &self.hist.samples;
        let (Some(first), Some(last)) = (s.first(), s.last()) else { return };
        let col = self.tab;
        let max = players.iter().flat_map(|&(id, _)| s.iter().map(move |x| x.v[id][col])).max().unwrap_or(0);
        let top = nice_max(max);
        let lpx = 14.0 * sk;
        let (lw, lh) = measure_text(lpx, Weight::Regular, &top.to_string());
        let left = r.0 + lw + (10.0 * sk) as i32;
        let bottom = r.1 + r.3 - lh - (10.0 * sk) as i32;
        let (px0, py0, pw, ph) = (left, r.1 + lh / 2, r.0 + r.2 - left, bottom - r.1 - lh / 2);
        if pw < 20 || ph < 20 {
            return;
        }
        c.fill_rect(px0, py0, pw, ph, PLOT_BG);
        // Grid + y labels (whole numbers: 0 .. top in `div` steps), "Count".
        let div = [4u32, 5, 2, 1].into_iter().find(|d| top % d == 0).unwrap_or(1);
        for k in 0..=div {
            let y = py0 + ph - (ph as i64 * k as i64 / div as i64) as i32;
            c.line(px0 as f32, y as f32, (px0 + pw) as f32, y as f32, 1.0, GRID);
            let label = (top as u64 * k as u64 / div as u64).to_string();
            let (w, h) = measure_text(lpx, Weight::Regular, &label);
            c.text(left - w - (6.0 * sk) as i32, y - h / 2, lpx, Weight::Regular, TEXT_DIM, &label);
        }
        c.text(px0 + (6.0 * sk) as i32, py0 + (4.0 * sk) as i32, lpx, Weight::Regular, TEXT_DIM, "Count");
        // x labels: game time.
        let (t0, t1) = (first.tick, last.tick.max(first.tick + 1));
        for k in 0..=4u32 {
            let t = t0 as u64 + (t1 - t0) as u64 * k as u64 / 4;
            let label = crate::game_time::format(t * 64);
            let (w, _) = measure_text(lpx, Weight::Regular, &label);
            let x = px0 + (pw as i64 * k as i64 / 4) as i32;
            let x = (x - w / 2).clamp(px0, px0 + pw - w);
            c.text(x, bottom + (6.0 * sk) as i32, lpx, Weight::Regular, TEXT_DIM, &label);
            c.line(x as f32 + w as f32 / 2.0, py0 as f32, x as f32 + w as f32 / 2.0, (py0 + ph) as f32, 1.0, GRID.fade(0.5));
        }
        let gt = "Game time";
        let (gw, _) = measure_text(lpx, Weight::Regular, gt);
        c.text(px0 + pw - gw - (6.0 * sk) as i32, py0 + ph - lh - (6.0 * sk) as i32, lpx, Weight::Regular, TEXT_DIM, gt);
        // Lines (at most ~1 point per 4 px), lowest rank drawn last = on top.
        let step = (s.len() / (pw as usize / 4).max(2)).max(1);
        let pts = |id: usize| {
            let mut v: Vec<(f32, f32)> = s
                .iter()
                .enumerate()
                .filter(|(i, _)| i % step == 0 || *i == s.len() - 1)
                .map(|(_, x)| {
                    let fx = px0 as f32 + pw as f32 * (x.tick - t0) as f32 / (t1 - t0) as f32;
                    let fy = (py0 + ph) as f32 - ph as f32 * x.v[id][col] as f32 / top as f32;
                    (fx, fy)
                })
                .collect();
            v.dedup();
            v
        };
        for &(id, rank) in players.iter().rev() {
            let colr = COLOURS[(rank as usize - 1) % PLAYERS];
            let p = pts(id);
            for w in p.windows(2) {
                c.line(w[0].0, w[0].1, w[1].0, w[1].1, 2.5 * sk, colr);
            }
            if let Some(&(x, y)) = p.last() {
                c.fill_circle(x, y, 4.0 * sk, colr);
            }
        }
    }

    fn draw_legend(&self, c: &mut Layer, sk: f32, r: Rect, players: &[(usize, u32)], row_h: i32) {
        let Some(last) = self.hist.samples.last() else { return };
        let col = self.tab;
        let pad = (8.0 * sk) as i32;
        c.fill_rounded_rect(r.0, r.1, r.2, r.3, 8.0 * sk, PLOT_BG);
        let cols = if r.2 > r.3 * 2 { 2 } else { 1 };
        let cw = (r.2 - pad) / cols;
        let per_col = ((r.3 - pad) / row_h.max(1)).max(1);
        for (n, &(id, rank)) in players.iter().enumerate() {
            let (ci, ri) = (n as i32 / per_col, n as i32 % per_col);
            if ci >= cols {
                break;
            }
            let (x, y) = (r.0 + pad + ci * cw, r.1 + pad / 2 + ri * row_h);
            let sw = (14.0 * sk) as i32;
            c.fill_rect(x, y + (row_h - sw) / 2, sw, sw, COLOURS[(rank as usize - 1) % PLAYERS]);
            // The game's own row: rank, team, name (frame px 14..186 of row `rank`).
            let value = last.v[id][col].to_string();
            let vpx = 16.0 * sk;
            let (vw, vh) = measure_text(vpx, Weight::Bold, &value);
            let name_h = row_h - (2.0 * sk) as i32;
            let name_w = (name_h * 172 / 20).min(cw - sw - vw - 3 * pad);
            let src_w = (name_w as f32 * 20.0 / name_h.max(1) as f32).min(172.0);
            let gy = 102.0 + 20.0 * (rank as f32 - 1.0);
            c.frame_inset([14.0, gy, src_w, 20.0], x + sw + pad / 2, y + 1, name_w, name_h);
            c.text(x + cw - vw - pad, y + (row_h - vh) / 2, vpx, Weight::Bold, TEXT, &value);
        }
    }
}

impl Mod for EndStatsMod {
    fn name(&self) -> &str {
        "end-stats"
    }

    fn tick(&mut self, m: &mut Machine, _now: u64) {
        let now = m.real_ms();
        if crate::res::in_game_frame(m) {
            if !self.in_game && now.saturating_sub(self.last_game_ms) > NEW_GAME_GAP_MS {
                // A new game or a loaded save.
                self.hist = History::default();
                self.final_taken = false;
                self.ranks = [0; PLAYERS];
            }
            self.in_game = true;
            self.last_game_ms = now;
            let (s, _) = Self::sample(m, false);
            if s.tick < self.hist.samples.last().map_or(0, |l| l.tick) {
                self.hist = History::default(); // clock went back: another game
            }
            self.hist.push(s, false);
        } else {
            self.in_game = false;
        }
        let on = crate::display::frame_viewport(m).layout.is_some_and(|l| l.name.starts_with("statistics"));
        if on && !self.final_taken && !self.hist.samples.is_empty() {
            let (s, ranks) = Self::sample(m, true);
            let tick = s.tick.max(self.hist.samples.last().map_or(0, |l| l.tick));
            self.hist.push(Sample { tick, ..s }, true);
            self.ranks = ranks;
            self.final_taken = true;
            self.dirty = true;
        }
        if on != self.on_screen {
            self.on_screen = on;
            if !on {
                self.open = false;
                self.pressed = None;
            }
            self.dirty = true;
        }
    }

    fn wants_draw(&self) -> bool {
        self.on_screen && self.usable()
    }

    fn needs_redraw(&self) -> bool {
        self.dirty
    }

    fn draw_window(&mut self, _m: &mut Machine, c: &mut Layer, _frame: &FrameBuffer) {
        self.dirty = false;
        self.targets.clear();
        // Window height / 600, but not wider than the window allows (tall inner screens).
        let sk = c.ui().min(c.width() as f32 / 900.0).clamp(0.5, 4.0);
        if self.open {
            self.draw_sheet(c, sk);
        } else {
            self.draw_open_button(c, sk);
        }
    }

    fn on_touch(&mut self, m: &mut Machine, ev: &TouchEvent) -> ModEvent {
        if !self.on_screen || !self.usable() {
            return ModEvent::Pass;
        }
        match *ev {
            TouchEvent::Down { id, .. } => {
                let (wx, wy) = crate::touch_window_pos(m, ev);
                let t = self.target_at(wx, wy);
                if t.is_some() || self.open {
                    self.pressed = Some((id, t));
                    return ModEvent::Consumed;
                }
                ModEvent::Pass
            }
            TouchEvent::Move { id, .. } | TouchEvent::Cancel { id } if self.pressed.is_some_and(|p| p.0 == id) => {
                if matches!(ev, TouchEvent::Cancel { .. }) {
                    self.pressed = None;
                }
                ModEvent::Consumed
            }
            TouchEvent::Up { id, .. } if self.pressed.is_some_and(|p| p.0 == id) => {
                let (wx, wy) = crate::touch_window_pos(m, ev);
                if let Some((_, Some(t))) = self.pressed.take() {
                    if self.target_at(wx, wy) == Some(t) {
                        self.act(t);
                    }
                }
                ModEvent::Consumed
            }
            // While the sheet is open, nothing reaches the game (its click would leave the screen).
            _ if self.open => ModEvent::Consumed,
            _ => ModEvent::Pass,
        }
    }

    fn on_key(&mut self, _m: &mut Machine, vk: u32, down: bool) -> ModEvent {
        if self.open && vk == 0x1b {
            if !down {
                self.open = false;
                self.dirty = true;
            }
            return ModEvent::Consumed;
        }
        ModEvent::Pass
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_formula_matches_the_game() {
        // rank, ..., score at [7], counters at [8..16].
        let mut r = [0u32; 17];
        r[8..16].copy_from_slice(&[10, 4, 3, 2, 8, 1, 6, 1]);
        let v = values(&r, false);
        assert_eq!(v[0], 2 * 10 + 4 + 3 + 2 + 8 + 1 + 2 * 6 + 5);
        assert_eq!(v[5], 4, "gold is shown halved");
        // Final: the game's own stored score and (already halved) gold.
        r[7] = 77;
        r[12] = 4;
        let f = values(&r, true);
        assert_eq!((f[0], f[5]), (77, 4));
    }

    #[test]
    fn history_samples_and_thins_out() {
        let mut h = History::default();
        let s = |tick| Sample { tick, v: [[tick; 9]; PLAYERS] };
        h.push(s(0), false);
        h.push(s(10), false); // too early
        h.push(s(SAMPLE_TICKS), false);
        assert_eq!(h.samples.len(), 2);
        h.push(s(SAMPLE_TICKS + 3), true); // final
        assert_eq!(h.samples.len(), 3);
        for i in 3..2000 {
            h.push(s(i * SAMPLE_TICKS * 2), false);
        }
        assert!(h.samples.len() <= MAX_SAMPLES);
        assert_eq!(h.samples[0].tick, 0);
        assert!(h.every > SAMPLE_TICKS);
    }

    #[test]
    fn ranks_and_axis() {
        let mut r = [0u32; PLAYERS];
        r[0] = 2;
        r[1] = 1;
        assert_eq!(ranked(&r), vec![(1, 1), (0, 2)]);
        assert_eq!(nice_max(0), 1);
        assert_eq!(nice_max(7), 10);
        assert_eq!(nice_max(130), 200);
        assert_eq!(nice_max(500), 500);
    }
}
