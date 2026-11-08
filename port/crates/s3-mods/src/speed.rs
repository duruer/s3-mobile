//! Game speed (single player): 1x / 2x / 3x / 4x, like Settlers United's speed-up (user request 2026-11-08).
//!
//! The guest clock runs `scale` times as fast as the wall clock (`Machine::set_time_scale`: GetTickCount,
//! timeGetTime, QueryPerformanceCounter, waits and timers), so the game's logic ticks faster; display pacing,
//! audio and the mods' UI stay on the wall clock (`Machine::real_ms`). The game itself is not patched. Only in a
//! single-player game (`*(0x7e3408)` is 0 in multiplayer, where lockstep peers must share one speed); back to 1x
//! outside a game. If the phone cannot keep up, the game simply runs as fast as it can.

use s3_runtime::{LogLevel, Machine};

use crate::Mod;

/// Speed steps cycled by the speed buttons.
pub const SPEEDS: [f64; 4] = [1.0, 2.0, 3.0, 4.0];

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

/// Short label, e.g. "2x".
pub fn label(m: &Machine) -> String {
    format!("{}x", current(m).round() as u32)
}

/// Next speed (wraps to 1x); 1x when not allowed.
pub fn cycle(m: &mut Machine) -> f64 {
    let next = if allowed(m) {
        let cur = current(m);
        let i = SPEEDS.iter().position(|&s| (s - cur).abs() < 0.01).unwrap_or(0);
        SPEEDS[(i + 1) % SPEEDS.len()]
    } else {
        1.0
    };
    set(m, next);
    next
}

/// Set the speed factor.
pub fn set(m: &mut Machine, s: f64) {
    if (current(m) - s).abs() > 0.001 {
        m.set_time_scale(s);
        m.log(LogLevel::Info, &format!("speed: {s}x"));
    }
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
        if current(m) == 1.0 {
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
