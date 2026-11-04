//! Input state fed from [`Host::poll_events`](crate::Host::poll_events). Several consumers (USER32 message
//! queue, DirectInput buffers) read the same event stream, each with its own cursor.

use std::collections::VecDeque;

use crate::host::{HostEvent, MouseButton};

const LOG_CAP: usize = 4096;

/// Current input state plus a short event log.
#[derive(Debug, Default)]
pub struct InputState {
    pub mouse_x: i32,
    pub mouse_y: i32,
    /// Left, right, middle.
    pub buttons: [bool; 3],
    /// Indexed by VK code.
    pub keys: Vec<bool>,
    log: VecDeque<(u64, HostEvent)>,
    next_seq: u64,
    /// Set when the host asked to quit.
    pub quit_requested: bool,
    /// Set while the host app is paused (Android onPause).
    pub paused: bool,
}

impl InputState {
    pub fn new() -> Self {
        InputState { keys: vec![false; 256], ..Default::default() }
    }

    /// Record an event and update the state.
    pub fn push(&mut self, e: HostEvent) {
        match e {
            HostEvent::MouseMove { x, y } => {
                self.mouse_x = x;
                self.mouse_y = y;
            }
            HostEvent::MouseButton { button, down } => {
                let i = match button {
                    MouseButton::Left => 0,
                    MouseButton::Right => 1,
                    MouseButton::Middle => 2,
                };
                self.buttons[i] = down;
            }
            HostEvent::Key { vk, down } => {
                if let Some(k) = self.keys.get_mut(vk as usize & 0xFF) {
                    *k = down;
                }
            }
            HostEvent::Quit => self.quit_requested = true,
            HostEvent::Pause => self.paused = true,
            HostEvent::Resume => self.paused = false,
            HostEvent::Char(_) => {}
        }
        if self.log.len() == LOG_CAP {
            self.log.pop_front();
        }
        self.log.push_back((self.next_seq, e));
        self.next_seq += 1;
    }

    /// Events newer than `*cursor`; advances the cursor. A new consumer should start with
    /// [`InputState::cursor_now`] to skip history.
    pub fn events_since(&self, cursor: &mut u64) -> Vec<HostEvent> {
        let out: Vec<HostEvent> = self.log.iter().filter(|(s, _)| *s >= *cursor).map(|(_, e)| *e).collect();
        *cursor = self.next_seq;
        out
    }

    /// Cursor value that skips all events so far.
    pub fn cursor_now(&self) -> u64 {
        self.next_seq
    }
}
