//! Vibration requests (user request 2026-11-08): a short buzz on long presses (box select start, group assign)
//! and on game messages / notifications, each switchable in the menus (`Settings::vibrate_touch`,
//! `Settings::vibrate_messages`). The mods only queue durations; the Android frontend drains the queue and
//! calls the system Vibrator (desktop ignores it).

use std::sync::Mutex;

use s3_runtime::Machine;

static QUEUE: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// Touch feedback buzz (long press recognised, box selection starts, group assigned).
pub const TOUCH_MS: u32 = 25;
/// Game message buzz.
pub const MESSAGE_MS: u32 = 60;

fn push(ms: u32) {
    let mut q = QUEUE.lock().unwrap_or_else(|p| p.into_inner());
    if q.len() < 8 {
        q.push(ms);
    }
}

/// Touch feedback, if switched on.
pub fn touch(m: &mut Machine) {
    if crate::settings(m).vibrate_touch {
        push(TOUCH_MS);
    }
}

/// Game message feedback, if switched on.
pub fn message(m: &mut Machine) {
    if crate::settings(m).vibrate_messages {
        push(MESSAGE_MS);
    }
}

/// Drain the queued vibration durations (ms), oldest first.
pub fn take() -> Vec<u32> {
    QUEUE.lock().unwrap_or_else(|p| p.into_inner()).drain(..).collect()
}
