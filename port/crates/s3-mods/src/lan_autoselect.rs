//! LAN screen: select the (only) network provider automatically (user report 2026-11-08: New Game / Join Game
//! said "Before connecting, a provider has to be selected!"). Our DirectPlay HLE lists exactly one connection
//! ("Local network / Wi-Fi" or "Internet", docs/porting/multiplayer.md), so when the game's "Multi-player Game:
//! LAN" screen is shown (menu layout `lan-game` / `lan-game-full` detected) we click its first "Type of Network"
//! row once, like the player would. A popup over the screen (`lan-game+msg-box`) is left alone.

use s3_runtime::{Machine, MouseButton};

use crate::{Mod, ModsState};

/// First row of the "Type of Network" list (800x600 frame, list box 128..398 x 120..220).
const FIRST_PROVIDER_ROW: (i32, i32) = (200, 132);
/// Wait after the screen appeared (the list is filled when the screen opens).
const DELAY_MS: u64 = 700;

#[derive(Default)]
pub struct LanAutoSelect {
    since: u64,
    done: bool,
}

impl Mod for LanAutoSelect {
    fn name(&self) -> &str {
        "lan-autoselect"
    }

    fn tick(&mut self, m: &mut Machine, now: u64) {
        let on_lan = crate::draw_viewport(m).layout.is_some_and(|l| l.name == "lan-game" || l.name == "lan-game-full");
        if !on_lan {
            // A popup ("+msg-box") keeps the visit going; leaving the screen resets it.
            let popup = crate::draw_viewport(m).layout.is_some_and(|l| l.name.starts_with("lan-game+"));
            if !popup {
                self.since = 0;
                self.done = false;
            }
            return;
        }
        if self.since == 0 {
            self.since = now.max(1);
        }
        if !self.done && now.saturating_sub(self.since) >= DELAY_MS {
            self.done = true;
            m.state::<ModsState>().borrow_mut().fallback.click(now, FIRST_PROVIDER_ROW, MouseButton::Left);
        }
    }
}
