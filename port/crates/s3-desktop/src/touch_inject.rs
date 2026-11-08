//! Touch inject commands for `s3run --inject` (raw pointer events in screen = game pixels; s3-mods recognizes
//! gestures exactly like on Android). See docs/porting/touch.md. With a `w` prefix (`wtap`, `wpinch`, ...) the
//! coordinates are window pixels (`--window`), mapped to game pixels through the shown viewport when delivered.
//!
//! - `tap x y`, `dtap x y` (double tap), `tap2 x y` (two-finger tap), `tap3 x y` (three-finger tap)
//! - `drag x1 y1 x2 y2 [ms=300]` one finger; `drag2 x1 y1 x2 y2 [ms=300]` two fingers 60 px apart (pan)
//! - `fling x1 y1 x2 y2 [ms=120]` fast one-finger drag released while moving (camera inertia)
//! - `longpress x y [ms=800]`; `lpdrag x1 y1 x2 y2 [hold=700] [ms=400]` long press, then drag (box select)
//! - `pinch cx cy scale [ms=300]` (two fingers 80 px apart -> 80*scale)

use s3_runtime::{HostEvent, PointerPhase};

pub const COMMANDS: [&str; 10] = ["tap", "dtap", "tap2", "tap3", "drag", "drag2", "fling", "longpress", "lpdrag", "pinch"];

thread_local! {
    /// Coordinates of the command being parsed are window pixels (`w` prefix).
    static WINDOW: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn p(id: u32, phase: PointerPhase, x: f64, y: f64) -> HostEvent {
    let (x, y) = (x.round().max(0.0) as i32, y.round().max(0.0) as i32);
    if WINDOW.with(|w| w.get()) {
        // Game position filled in by the host when delivered (window -> game through the viewport).
        HostEvent::Pointer { id, phase, x: -1, y: -1, wx: x, wy: y }
    } else {
        HostEvent::Pointer { id, phase, x, y, wx: -1, wy: -1 }
    }
}

/// `cmd` with an optional `w` prefix (window coordinates): the plain command name and the flag.
pub fn split_window_prefix(cmd: &str) -> Option<(&str, bool)> {
    if COMMANDS.contains(&cmd) {
        return Some((cmd, false));
    }
    let c = cmd.strip_prefix('w')?;
    COMMANDS.iter().find(|&&k| k == c).map(|&k| (k, true))
}

/// [`touch_events`] for a command that may carry the `w` prefix.
pub fn touch_events_any(
    out: &mut Vec<(u64, HostEvent)>,
    t: u64,
    cmd: &str,
    num: &dyn Fn(usize) -> Result<f64, String>,
    opt: &dyn Fn(usize, f64) -> f64,
) -> Result<(), String> {
    let (c, win) = split_window_prefix(cmd).ok_or(format!("unknown touch command '{cmd}'"))?;
    WINDOW.with(|w| w.set(win));
    let r = touch_events(out, t, c, num, opt);
    WINDOW.with(|w| w.set(false));
    r
}

/// Moves of finger `id` from a to b over `ms` (every 16 ms), offset by `off`.
fn moves(out: &mut Vec<(u64, HostEvent)>, t: u64, ms: u64, a: (f64, f64), b: (f64, f64), ids: &[(u32, f64)]) {
    let steps = (ms / 16).max(1);
    for i in 1..=steps {
        let k = i as f64 / steps as f64;
        for &(id, off) in ids {
            out.push((t + i * ms / steps, p(id, PointerPhase::Move, a.0 + (b.0 - a.0) * k + off, a.1 + (b.1 - a.1) * k)));
        }
    }
}

/// Raw pointer events for one touch inject command.
pub fn touch_events(
    out: &mut Vec<(u64, HostEvent)>,
    t: u64,
    cmd: &str,
    num: &dyn Fn(usize) -> Result<f64, String>,
    opt: &dyn Fn(usize, f64) -> f64,
) -> Result<(), String> {
    use PointerPhase::{Down, Up};
    match cmd {
        "tap" | "tap2" | "tap3" => {
            let (x, y) = (num(1)?, num(2)?);
            out.push((t, p(1, Down, x, y)));
            let extra = match cmd {
                "tap2" => 1,
                "tap3" => 2,
                _ => 0,
            };
            for k in 0..extra {
                let dx = 40.0 * (k + 1) as f64;
                out.push((t + 10, p(2 + k, Down, x + dx, y)));
                out.push((t + 60, p(2 + k, Up, x + dx, y)));
            }
            out.push((t + 70, p(1, Up, x, y)));
        }
        "dtap" => {
            let (x, y) = (num(1)?, num(2)?);
            out.push((t, p(1, Down, x, y)));
            out.push((t + 60, p(1, Up, x, y)));
            out.push((t + 180, p(1, Down, x, y)));
            out.push((t + 240, p(1, Up, x, y)));
        }
        "longpress" => {
            let (x, y, ms) = (num(1)?, num(2)?, opt(3, 800.0) as u64);
            out.push((t, p(1, Down, x, y)));
            out.push((t + ms, p(1, Up, x, y)));
        }
        "drag" | "fling" => {
            let d = if cmd == "fling" { 120.0 } else { 300.0 };
            let (x1, y1, x2, y2, ms) = (num(1)?, num(2)?, num(3)?, num(4)?, opt(5, d).max(16.0) as u64);
            out.push((t, p(1, Down, x1, y1)));
            moves(out, t, ms, (x1, y1), (x2, y2), &[(1, 0.0)]);
            out.push((t + ms + 1, p(1, Up, x2, y2)));
        }
        "drag2" => {
            let (x1, y1, x2, y2, ms) = (num(1)?, num(2)?, num(3)?, num(4)?, opt(5, 300.0).max(16.0) as u64);
            out.push((t, p(1, Down, x1 - 30.0, y1)));
            out.push((t + 5, p(2, Down, x1 + 30.0, y1)));
            moves(out, t + 5, ms, (x1, y1), (x2, y2), &[(1, -30.0), (2, 30.0)]);
            out.push((t + ms + 10, p(1, Up, x2 - 30.0, y2)));
            out.push((t + ms + 12, p(2, Up, x2 + 30.0, y2)));
        }
        "lpdrag" => {
            let (x1, y1, x2, y2) = (num(1)?, num(2)?, num(3)?, num(4)?);
            let (hold, ms) = (opt(5, 700.0) as u64, opt(6, 400.0).max(16.0) as u64);
            out.push((t, p(1, Down, x1, y1)));
            moves(out, t + hold, ms, (x1, y1), (x2, y2), &[(1, 0.0)]);
            out.push((t + hold + ms + 1, p(1, Up, x2, y2)));
        }
        "pinch" => {
            let (cx, cy, scale, ms) = (num(1)?, num(2)?, num(3)?, opt(4, 300.0).max(16.0) as u64);
            let d0 = 40.0;
            out.push((t, p(1, Down, cx - d0, cy)));
            out.push((t, p(2, Down, cx + d0, cy)));
            let steps = (ms / 16).max(1);
            for i in 1..=steps {
                let d = d0 * (1.0 + (scale - 1.0) * i as f64 / steps as f64);
                let at = t + i * ms / steps;
                out.push((at, p(1, PointerPhase::Move, cx - d, cy)));
                out.push((at, p(2, PointerPhase::Move, cx + d, cy)));
            }
            out.push((t + ms + 1, p(1, Up, cx - d0 * scale, cy)));
            out.push((t + ms + 2, p(2, Up, cx + d0 * scale, cy)));
        }
        _ => return Err(format!("unknown touch command '{cmd}'")),
    }
    Ok(())
}
