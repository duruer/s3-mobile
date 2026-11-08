//! Background play bookkeeping (docs/porting/android.md "Background play"), platform-independent and unit-tested:
//! coalescing of game message lines into rate-limited notifications, and chronometer drift (the notification
//! counts wall time; game time runs faster at 2x-8x speed and stops in the in-game pause menu).

/// Minimum time between two event notifications; lines arriving in between are grouped into the next one.
pub const EVENT_MIN_INTERVAL_MS: u64 = 4000;
/// At most this many lines per notification (the newest are kept).
pub const EVENT_MAX_LINES: usize = 6;
/// Re-sync the chronometer when the game time is off by more than this.
pub const DRIFT_MS: i64 = 3000;
/// Background while not in a game (game ended, back in the menus) for this long: pause like in the menus.
pub const NOT_IN_GAME_PAUSE_MS: u64 = 5000;

#[derive(Debug, Default)]
pub struct Background {
    pending: Vec<String>,
    last_post_ms: Option<u64>,
    /// (wall ms, game ms) of the last chronometer sync.
    base: Option<(u64, u64)>,
}

impl Background {
    /// Entered the background: the chronometer starts at `game_ms`.
    pub fn begin(&mut self, now_ms: u64, game_ms: u64) {
        *self = Background { base: Some((now_ms, game_ms)), ..Default::default() };
    }

    pub fn push(&mut self, lines: impl IntoIterator<Item = String>) {
        for l in lines {
            if self.pending.last() != Some(&l) {
                self.pending.push(l);
            }
        }
        if self.pending.len() > EVENT_MAX_LINES {
            let cut = self.pending.len() - EVENT_MAX_LINES;
            self.pending.drain(..cut);
        }
    }

    /// Lines to post now (rate limited), if any.
    pub fn due(&mut self, now_ms: u64) -> Option<Vec<String>> {
        if self.pending.is_empty() {
            return None;
        }
        if let Some(t) = self.last_post_ms {
            if now_ms.saturating_sub(t) < EVENT_MIN_INTERVAL_MS {
                return None;
            }
        }
        self.last_post_ms = Some(now_ms);
        Some(std::mem::take(&mut self.pending))
    }

    /// `Some(game_ms)` when the chronometer must be re-synced to the game time.
    pub fn drift(&mut self, now_ms: u64, game_ms: u64) -> Option<u64> {
        let (w0, g0) = self.base?;
        let shown = g0 as i64 + now_ms.saturating_sub(w0) as i64;
        if (shown - game_ms as i64).abs() > DRIFT_MS {
            self.base = Some((now_ms, game_ms));
            return Some(game_ms);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesces_and_rate_limits() {
        let mut b = Background::default();
        b.begin(0, 0);
        assert_eq!(b.due(0), None);
        b.push(["coal found".to_string()]);
        assert_eq!(b.due(100), Some(vec!["coal found".to_string()]));
        b.push(["a".to_string(), "b".to_string()]);
        assert_eq!(b.due(1000), None);
        assert_eq!(b.due(4100), Some(vec!["a".to_string(), "b".to_string()]));
        b.push((0..10).map(|i| i.to_string()));
        assert_eq!(b.due(9000).unwrap(), (4..10).map(|i| i.to_string()).collect::<Vec<_>>());
    }

    #[test]
    fn drift_resyncs() {
        let mut b = Background::default();
        b.begin(1000, 50_000);
        assert_eq!(b.drift(11_000, 60_000), None);
        // 2x speed: game ran 20 s in 10 s.
        assert_eq!(b.drift(21_000, 80_000), Some(80_000));
        assert_eq!(b.drift(22_000, 81_000), None);
    }
}
