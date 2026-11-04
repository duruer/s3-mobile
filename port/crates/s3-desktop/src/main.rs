//! `s3run` — run S3.EXE on the desktop (minifb window) or headless.
//!
//! ```text
//! s3run --game-dir <dir> --data-dir <dir> [--exe <S3.EXE>] [--headless] [--max-seconds N]
//!       [--screenshot-every-present <dir>] [--screenshot <file.png>] [--inject "t_ms:cmd args;..."]
//! ```
//! Inject commands (times in ms since start): `click x y`, `rclick x y`, `move x y`, `down x y`, `up x y`,
//! `key VK` (press+release, VK decimal or 0x..), `quit`.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use s3_engine::Engine;
use s3_runtime::{FrameBuffer, Host, HostEvent, LogLevel, MouseButton, SliceResult};

/// Max PNG files written by --screenshot-every-present (disk is shared and small).
const MAX_PRESENT_SHOTS: u64 = 300;

struct Args {
    game_dir: PathBuf,
    data_dir: PathBuf,
    exe: Option<PathBuf>,
    headless: bool,
    max_seconds: Option<f64>,
    shots_dir: Option<PathBuf>,
    screenshot: Option<PathBuf>,
    inject: Vec<(u64, HostEvent)>,
    verbose: bool,
}

fn usage() -> ! {
    eprintln!(
        "usage: s3run --game-dir <dir> --data-dir <dir> [--exe <path>] [--headless] [--max-seconds N]\n\
         \x20            [--screenshot-every-present <dir>] [--screenshot <file.png>] [--inject \"t:cmd x y;...\"] [-v]"
    );
    std::process::exit(2)
}

fn parse_num(s: &str) -> Option<i64> {
    if let Some(h) = s.strip_prefix("0x") {
        i64::from_str_radix(h, 16).ok()
    } else {
        s.parse().ok()
    }
}

fn parse_inject(spec: &str) -> Result<Vec<(u64, HostEvent)>, String> {
    let mut out = Vec::new();
    for item in spec.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        let (t, cmd) = item.split_once(':').ok_or(format!("bad inject item '{item}'"))?;
        let t: u64 = t.trim().parse().map_err(|_| format!("bad time in '{item}'"))?;
        let w: Vec<&str> = cmd.split_whitespace().collect();
        let xy = |w: &[&str]| -> Result<(i32, i32), String> {
            let x = w.get(1).and_then(|s| parse_num(s)).ok_or(format!("missing x in '{item}'"))?;
            let y = w.get(2).and_then(|s| parse_num(s)).ok_or(format!("missing y in '{item}'"))?;
            Ok((x as i32, y as i32))
        };
        match w.first().copied() {
            Some("move") => {
                let (x, y) = xy(&w)?;
                out.push((t, HostEvent::MouseMove { x, y }));
            }
            Some(c @ ("click" | "rclick" | "down" | "up")) => {
                let (x, y) = xy(&w)?;
                let button = if c == "rclick" { MouseButton::Right } else { MouseButton::Left };
                out.push((t, HostEvent::MouseMove { x, y }));
                if c != "up" {
                    out.push((t, HostEvent::MouseButton { button, down: true }));
                }
                if c != "down" {
                    out.push((t + 50, HostEvent::MouseButton { button, down: false }));
                }
            }
            Some("key") => {
                let vk = w.get(1).and_then(|s| parse_num(s)).ok_or(format!("missing vk in '{item}'"))? as u32;
                out.push((t, HostEvent::Key { vk, down: true }));
                out.push((t + 50, HostEvent::Key { vk, down: false }));
            }
            Some("quit") => out.push((t, HostEvent::Quit)),
            _ => return Err(format!("unknown inject command '{cmd}'")),
        }
    }
    out.sort_by_key(|(t, _)| *t);
    Ok(out)
}

fn parse_args() -> Args {
    let mut a = Args {
        game_dir: PathBuf::new(),
        data_dir: PathBuf::new(),
        exe: None,
        headless: false,
        max_seconds: None,
        shots_dir: None,
        screenshot: None,
        inject: Vec::new(),
        verbose: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = || it.next().unwrap_or_else(|| usage());
        match arg.as_str() {
            "--game-dir" => a.game_dir = val().into(),
            "--data-dir" => a.data_dir = val().into(),
            "--exe" => a.exe = Some(val().into()),
            "--headless" => a.headless = true,
            "--max-seconds" => a.max_seconds = Some(val().parse().unwrap_or_else(|_| usage())),
            "--screenshot-every-present" => a.shots_dir = Some(val().into()),
            "--screenshot" => a.screenshot = Some(val().into()),
            "--inject" => {
                a.inject = parse_inject(&val()).unwrap_or_else(|e| {
                    eprintln!("{e}");
                    usage()
                })
            }
            "-v" | "--verbose" => a.verbose = true,
            "-h" | "--help" => usage(),
            _ => usage(),
        }
    }
    if a.game_dir.as_os_str().is_empty() || a.data_dir.as_os_str().is_empty() {
        usage();
    }
    a
}

/// State shared between the host (owned by the machine) and the frontend loop.
#[derive(Default)]
struct Shared {
    frame: Option<FrameBuffer>,
    frame_serial: u64,
    events: VecDeque<HostEvent>,
}

struct DesktopHost {
    game_dir: PathBuf,
    data_dir: PathBuf,
    start: Instant,
    shared: Arc<Mutex<Shared>>,
    inject: VecDeque<(u64, HostEvent)>,
    shots_dir: Option<PathBuf>,
    shots: u64,
    verbose: bool,
}

impl Host for DesktopHost {
    fn present(&mut self, fb: &FrameBuffer) {
        if let Some(dir) = &self.shots_dir {
            if self.shots < MAX_PRESENT_SHOTS {
                let p = dir.join(format!("present-{:05}.png", self.shots));
                if let Err(e) = write_png(&p, fb) {
                    eprintln!("screenshot {}: {e}", p.display());
                }
            }
            self.shots += 1;
        }
        let mut s = self.shared.lock().unwrap();
        s.frame = Some(fb.clone());
        s.frame_serial += 1;
    }

    fn poll_events(&mut self) -> Vec<HostEvent> {
        let now = self.now_ms();
        let mut out: Vec<HostEvent> = Vec::new();
        while let Some((t, _)) = self.inject.front() {
            if *t > now {
                break;
            }
            out.push(self.inject.pop_front().unwrap().1);
        }
        out.extend(self.shared.lock().unwrap().events.drain(..));
        out
    }

    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    fn game_dir(&self) -> PathBuf {
        self.game_dir.clone()
    }

    fn data_dir(&self) -> PathBuf {
        self.data_dir.clone()
    }

    fn log(&self, level: LogLevel, msg: &str) {
        if self.verbose || level <= LogLevel::Info || level == LogLevel::Trace {
            eprintln!("[{level:?}] {msg}");
        }
    }
}

fn write_png(path: &Path, fb: &FrameBuffer) -> Result<(), String> {
    let f = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), fb.width, fb.height);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    w.write_image_data(&fb.to_rgb888()).map_err(|e| e.to_string())
}

mod window {
    //! minifb window frontend (X11).
    use minifb::{Key, MouseButton as MfButton, MouseMode, Window, WindowOptions};
    use s3_runtime::{FrameBuffer, HostEvent, MouseButton};

    pub struct Win {
        win: Option<Window>,
        size: (usize, usize),
        buf: Vec<u32>,
        last_mouse: (i32, i32),
        buttons: [bool; 3],
        keys_down: Vec<Key>,
    }

    fn vk(k: Key) -> Option<u32> {
        use Key::*;
        let v = match k {
            A => 0x41, B => 0x42, C => 0x43, D => 0x44, E => 0x45, F => 0x46, G => 0x47, H => 0x48, I => 0x49,
            J => 0x4A, K => 0x4B, L => 0x4C, M => 0x4D, N => 0x4E, O => 0x4F, P => 0x50, Q => 0x51, R => 0x52,
            S => 0x53, T => 0x54, U => 0x55, V => 0x56, W => 0x57, X => 0x58, Y => 0x59, Z => 0x5A,
            Key0 => 0x30, Key1 => 0x31, Key2 => 0x32, Key3 => 0x33, Key4 => 0x34, Key5 => 0x35, Key6 => 0x36,
            Key7 => 0x37, Key8 => 0x38, Key9 => 0x39,
            F1 => 0x70, F2 => 0x71, F3 => 0x72, F4 => 0x73, F5 => 0x74, F6 => 0x75, F7 => 0x76, F8 => 0x77,
            F9 => 0x78, F10 => 0x79, F11 => 0x7A, F12 => 0x7B,
            Escape => 0x1B, Enter => 0x0D, Space => 0x20, Backspace => 0x08, Tab => 0x09,
            Left => 0x25, Up => 0x26, Right => 0x27, Down => 0x28,
            LeftShift | RightShift => 0x10, LeftCtrl | RightCtrl => 0x11, LeftAlt | RightAlt => 0x12,
            Delete => 0x2E, Insert => 0x2D, Home => 0x24, End => 0x23, PageUp => 0x21, PageDown => 0x22,
            NumPadPlus => 0x6B, NumPadMinus => 0x6D, Pause => 0x13,
            _ => return None,
        };
        Some(v)
    }

    impl Win {
        pub fn new() -> Self {
            Win { win: None, size: (0, 0), buf: Vec::new(), last_mouse: (-1, -1), buttons: [false; 3], keys_down: Vec::new() }
        }

        pub fn is_open(&self) -> bool {
            self.win.as_ref().map(|w| w.is_open()).unwrap_or(true)
        }

        pub fn show(&mut self, fb: &FrameBuffer) {
            let size = (fb.width as usize, fb.height as usize);
            if self.win.is_none() || self.size != size {
                let opts = WindowOptions { resize: false, ..WindowOptions::default() };
                match Window::new("Settlers 3 Port (s3run)", size.0, size.1, opts) {
                    Ok(mut w) => {
                        w.set_target_fps(0);
                        self.win = Some(w);
                        self.size = size;
                    }
                    Err(e) => {
                        eprintln!("window: {e}");
                        return;
                    }
                }
            }
            self.buf.clear();
            self.buf.extend(fb.pixels.iter().map(|&p| {
                let (r, g, b) = s3_runtime::host::rgb565_to_rgb888(p);
                ((r as u32) << 16) | ((g as u32) << 8) | b as u32
            }));
            if let Some(w) = &mut self.win {
                let _ = w.update_with_buffer(&self.buf, size.0, size.1);
            }
        }

        /// Pump window events (call regularly even without new frames).
        pub fn events(&mut self) -> Vec<HostEvent> {
            let mut out = Vec::new();
            let Some(w) = &mut self.win else { return out };
            if self.buf.is_empty() {
                w.update();
            } else {
                let _ = w.update_with_buffer(&self.buf, self.size.0, self.size.1);
            }
            if !w.is_open() {
                out.push(HostEvent::Quit);
                return out;
            }
            if let Some((x, y)) = w.get_mouse_pos(MouseMode::Clamp) {
                let p = (x as i32, y as i32);
                if p != self.last_mouse {
                    self.last_mouse = p;
                    out.push(HostEvent::MouseMove { x: p.0, y: p.1 });
                }
            }
            for (i, (mb, b)) in [(MfButton::Left, MouseButton::Left), (MfButton::Right, MouseButton::Right), (MfButton::Middle, MouseButton::Middle)]
                .into_iter()
                .enumerate()
            {
                let d = w.get_mouse_down(mb);
                if d != self.buttons[i] {
                    self.buttons[i] = d;
                    out.push(HostEvent::MouseButton { button: b, down: d });
                }
            }
            let now: Vec<Key> = w.get_keys();
            for k in &now {
                if !self.keys_down.contains(k) {
                    if let Some(v) = vk(*k) {
                        out.push(HostEvent::Key { vk: v, down: true });
                    }
                }
            }
            for k in &self.keys_down {
                if !now.contains(k) {
                    if let Some(v) = vk(*k) {
                        out.push(HostEvent::Key { vk: v, down: false });
                    }
                }
            }
            self.keys_down = now;
            out
        }
    }
}

fn main() {
    let args = parse_args();
    std::fs::create_dir_all(&args.data_dir).ok();
    if let Some(d) = &args.shots_dir {
        std::fs::create_dir_all(d).ok();
    }
    let shared = Arc::new(Mutex::new(Shared::default()));
    let host = DesktopHost {
        game_dir: args.game_dir.clone(),
        data_dir: args.data_dir.clone(),
        start: Instant::now(),
        shared: shared.clone(),
        inject: args.inject.iter().copied().collect(),
        shots_dir: args.shots_dir.clone(),
        shots: 0,
        verbose: args.verbose,
    };
    let mut engine = match Engine::new(Box::new(host), args.exe.as_deref()) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("s3run: {e}");
            std::process::exit(1);
        }
    };
    let start = Instant::now();
    let mut win = if args.headless { None } else { Some(window::Win::new()) };
    let mut shown_serial = 0;
    loop {
        if let Some(max) = args.max_seconds {
            if start.elapsed().as_secs_f64() >= max {
                eprintln!("s3run: --max-seconds reached");
                break;
            }
        }
        let r = engine.run_for_ms(16);
        if let Some(w) = &mut win {
            let frame = {
                let s = shared.lock().unwrap();
                if s.frame_serial != shown_serial {
                    shown_serial = s.frame_serial;
                    s.frame.clone()
                } else {
                    None
                }
            };
            if let Some(f) = frame {
                w.show(&f);
            }
            let evs = w.events();
            shared.lock().unwrap().events.extend(evs);
            if !w.is_open() {
                break;
            }
        }
        if engine.machine.input.quit_requested {
            eprintln!("s3run: quit requested");
            break;
        }
        if r == SliceResult::Stopped {
            break;
        }
    }
    if let Some(p) = &args.screenshot {
        let f = shared.lock().unwrap().frame.clone();
        match f {
            Some(f) => match write_png(p, &f) {
                Ok(()) => eprintln!("s3run: screenshot written to {}", p.display()),
                Err(e) => eprintln!("s3run: screenshot failed: {e}"),
            },
            None => eprintln!("s3run: no frame was presented, no screenshot"),
        }
    }
    println!("{}", engine.summary());
    let code = match engine.machine.stop_reason() {
        Some(s3_runtime::StopReason::Exited(c)) => *c as i32,
        None => 0,
        Some(_) => 3,
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inject_parsing() {
        let v = parse_inject("100:click 10 20; 50:key 0x1B ;300:quit").unwrap();
        assert_eq!(v[0], (50, HostEvent::Key { vk: 0x1B, down: true }));
        assert!(v.contains(&(100, HostEvent::MouseButton { button: MouseButton::Left, down: true })));
        assert!(v.contains(&(150, HostEvent::MouseButton { button: MouseButton::Left, down: false })));
        assert_eq!(v.last(), Some(&(300, HostEvent::Quit)));
        assert!(parse_inject("x:click").is_err());
    }
}
