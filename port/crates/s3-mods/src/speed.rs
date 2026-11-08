//! Game speed (single player): normal or a chosen fast speed 2x..8x, like Settlers United's speed-up (user
//! requests 2026-11-08: "up to 8x; the button toggles the chosen speed; the speed is chosen in the menu").
//!
//! The guest clock runs `scale` times as fast as the wall clock (`Machine::set_time_scale`: GetTickCount,
//! timeGetTime, QueryPerformanceCounter, waits and timers), so the game's logic ticks faster; display pacing,
//! audio and the mods' UI stay on the wall clock (`Machine::real_ms`). The game itself is not patched. Only in a
//! single-player game (`*(0x7e3408)` is 0 in multiplayer, where lockstep peers must share one speed); back to 1x
//! outside a game. If the phone cannot keep up, the game simply runs as fast as it can.

use s3_runtime::{LogLevel, Machine};

use crate::Mod;

/// Fast speeds selectable in the menus.
pub const FAST_CHOICES: [u32; 7] = [2, 3, 4, 5, 6, 7, 8];
/// Default fast speed.
pub const DEFAULT_FAST: u32 = 2;

/// `CGameData*` global: non-zero in a single-player game, 0 in multiplayer (game-state-ui.md).
const GD_GLOBAL_SP: u32 = 0x007e_3408;

/// Can the speed be changed now (single-player game running)?
pub fn allowed(m: &mut Machine) -> bool {
    m.mem.read_u32(GD_GLOBAL_SP) != 0 && crate::res::in_game_frame(m)
}

/// Current speed factor.
pub fn current(m: &Machine) -> f64 {
    m.time_scale()
}

/// Is the game running fast now?
pub fn is_fast(m: &Machine) -> bool {
    current(m) > 1.001
}

/// The chosen fast speed (settings, clamped to [`FAST_CHOICES`]).
pub fn fast_choice(m: &mut Machine) -> u32 {
    crate::settings(m).fast_speed.clamp(FAST_CHOICES[0], FAST_CHOICES[FAST_CHOICES.len() - 1])
}

/// Label of the chosen fast speed, e.g. "4x".
pub fn choice_label(m: &mut Machine) -> String {
    format!("{}x", fast_choice(m))
}

/// Speed button: normal <-> chosen fast speed. Returns the new factor (1 when not allowed).
pub fn toggle(m: &mut Machine) -> f64 {
    let next = if allowed(m) && !is_fast(m) { fast_choice(m) as f64 } else { 1.0 };
    set(m, next);
    next
}

/// Menu: next fast speed choice (wraps); applied at once while running fast. Returns the new choice.
pub fn cycle_choice(m: &mut Machine) -> u32 {
    let cur = fast_choice(m);
    let i = FAST_CHOICES.iter().position(|&c| c == cur).unwrap_or(0);
    let next = FAST_CHOICES[(i + 1) % FAST_CHOICES.len()];
    let mut s = crate::settings(m);
    s.fast_speed = next;
    let _ = crate::set_settings(m, s);
    if is_fast(m) && allowed(m) {
        set(m, next as f64);
    }
    next
}

/// Set the speed factor.
pub fn set(m: &mut Machine, s: f64) {
    if (current(m) - s).abs() > 0.001 {
        m.set_time_scale(s);
        m.log(LogLevel::Info, &format!("speed: {s}x"));
    }
}

/// Speed button state for the UIs: (chosen label, running fast) while the speed can be changed.
pub fn button_state(m: &mut Machine) -> Option<(String, bool)> {
    allowed(m).then(|| (choice_label(m), is_fast(m)))
}

/// Resets the speed to 1x outside a single-player game.
#[derive(Default)]
struct SpeedMod {
    out_since: u64,
}

impl Mod for SpeedMod {
    fn name(&self) -> &str {
        "speed"
    }
    fn tick(&mut self, m: &mut Machine, now: u64) {
        if !is_fast(m) {
            self.out_since = 0;
            return;
        }
        if allowed(m) {
            self.out_since = 0;
        } else if self.out_since == 0 {
            self.out_since = now.max(1);
        } else if now.saturating_sub(self.out_since) > 1500 {
            set(m, 1.0);
        }
    }
}

pub fn install(m: &mut Machine) {
    crate::add_mod(m, Box::new(SpeedMod::default()));
}
