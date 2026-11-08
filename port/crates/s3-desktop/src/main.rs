//! `s3run` — run S3.EXE on the desktop (minifb window) or headless.
//!
//! ```text
//! s3run --game-dir <dir> --data-dir <dir> [--exe <S3.EXE>] [--headless] [--max-seconds N]
//!       [--screenshot-every-present <dir>] [--screenshot <file.png>] [--inject "t_ms:cmd args;..."]
//!       [--audio-wav <file.wav> [--audio-wav-seconds N]] [--audio-play]   (audio sink: see audio.rs)
//! ```
//! Inject commands (times in ms since start): `click x y`, `rclick x y`, `move x y`, `down x y`, `up x y`,
//! `key VK` (press+release, VK decimal or 0x..), `text abc` (one key press per character, 120 ms apart; letters,
//! digits, space, `-`, `_` as VK codes so the guest's TranslateMessage makes WM_CHAR), `shot file.png` (write the
//! current frame), `wshot file.png` (the window as the player sees it: frame scaled by the display mode + the mods'
//! window layer; headless: at `--window` size), `quit`.
//! Touch commands (raw pointer events in screen = game pixels, recognized by s3-mods like on Android):
//! `tap`, `dtap`, `tap2`, `tap3`, `drag`, `drag2`, `fling`, `longpress`, `lpdrag`, `pinch`; with a `w` prefix
//! (`wtap`, `wpinch`, ...) in window pixels; see touch_inject.rs.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

mod audio;
mod debug;
mod debug_units;
mod touch_inject;
mod mp_hash;

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
    /// `shot` / `wshot` inject commands: (time ms, png path, window-composited).
    inject_shots: Vec<(u64, PathBuf, bool)>,
    verbose: bool,
    audio_wav: Option<PathBuf>,
    audio_wav_seconds: f64,
    audio_play: bool,
    /// Debug inject commands (dump/peek/poke/regs, see debug.rs).
    debug_cmds: Vec<(u64, debug::DebugCmd)>,
    /// re-units read-only commands (xdump/xpeek/scan, see debug_units.rs).
    units_cmds: Vec<(u64, debug_units::UnitsCmd)>,
    /// `--mp-hash FILE`: per-game-step simulation hash (mp_hash.rs), every `mp_hash_every` steps.
    mp_hash: Option<PathBuf>,
    mp_hash_every: u32,
    /// Display overrides (docs/porting/display.md): `--display`, `--filter`, `--fps`, `--window WxH`.
    display_mode: Option<s3_mods::display::DisplayMode>,
    display_filter: Option<s3_mods::display::ScaleFilter>,
    fps_counter: Option<s3_mods::display::FpsCounter>,
    window_size: Option<(u32, u32)>,
}

fn usage() -> ! {
    eprintln!(
        "usage: s3run --game-dir <dir> --data-dir <dir> [--exe <path>] [--headless] [--max-seconds N]\n\
         \x20            [--screenshot-every-present <dir>] [--screenshot <file.png>] [--inject \"t:cmd x y;...\"] [-v]\n\
         \x20            [--audio-wav <file.wav> [--audio-wav-seconds N (300)]] [--audio-play (feature audio-cpal)]\n\
         \x20            [--mp-hash <file> [--mp-hash-every N]]   (multiplayer: env S3_RELAY_URL, S3_NET_NAME)\n\
         \x20            [--display fit|fill|stretch|native] [--filter nearest|linear|sharp] [--fps off|fps|full]\n\
         \x20            [--window WxH]   (window size; headless: the simulated screen for display modes)"
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

#[cfg(test)]
fn parse_inject(spec: &str) -> Result<Vec<(u64, HostEvent)>, String> {
    parse_inject_all(spec).map(|(v, _)| v)
}

/// VK code for a `text` character (US layout, unshifted keys only).
fn text_vk(c: char) -> Option<u32> {
    match c {
        'a'..='z' => Some(c.to_ascii_uppercase() as u32),
        'A'..='Z' | '0'..='9' | ' ' => Some(c as u32),
        '-' | '_' => Some(0xBD),
        _ => None,
    }
}

/// Parse inject events plus `shot` / `wshot` commands.
fn parse_inject_all(spec: &str) -> Result<(Vec<(u64, HostEvent)>, Vec<(u64, PathBuf, bool)>), String> {
    let mut out = Vec::new();
    let mut shots = Vec::new();
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
            Some("text") => {
                let txt = cmd.trim_start()["text".len()..].trim();
                for (i, c) in txt.chars().enumerate() {
                    let vk = text_vk(c).ok_or(format!("unsupported text char '{c}' in '{item}'"))?;
                    let at = t + 120 * i as u64;
                    let shift = c.is_ascii_uppercase() || c == '_';
                    if shift {
                        out.push((at, HostEvent::Key { vk: 0x10, down: true }));
                    }
                    out.push((at + 10, HostEvent::Key { vk, down: true }));
                    out.push((at + 60, HostEvent::Key { vk, down: false }));
                    if shift {
                        out.push((at + 70, HostEvent::Key { vk: 0x10, down: false }));
                    }
                }
            }
            Some(c @ ("shot" | "wshot")) => {
                let f = w.get(1).ok_or(format!("missing file in '{item}'"))?;
                shots.push((t, PathBuf::from(f), c == "wshot"));
            }
            Some("quit") => out.push((t, HostEvent::Quit)),
            Some(c) if touch_inject::split_window_prefix(c).is_some() => {
                let num = |i: usize| -> Result<f64, String> {
                    w.get(i).and_then(|s| s.parse::<f64>().ok()).ok_or(format!("missing/bad argument {i} in '{item}'"))
                };
                let opt = |i: usize, d: f64| w.get(i).and_then(|s| s.parse::<f64>().ok()).unwrap_or(d);
                touch_inject::touch_events_any(&mut out, t, c, &num, &opt)?;
            }
            _ => return Err(format!("unknown inject command '{cmd}'")),
        }
    }
    out.sort_by_key(|(t, _)| *t);
    shots.sort_by_key(|(t, _, _)| *t);
    Ok((out, shots))
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
        inject_shots: Vec::new(),
        verbose: false,
        audio_wav: None,
        audio_wav_seconds: 300.0,
        audio_play: false,
        debug_cmds: Vec::new(),
        units_cmds: Vec::new(),
        mp_hash: None,
        mp_hash_every: 1,
        display_mode: None,
        display_filter: None,
        fps_counter: None,
        window_size: None,
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
                let (spec, ucmds) = debug_units::split(&val()).unwrap_or_else(|e| {
                    eprintln!("{e}");
                    usage()
                });
                a.units_cmds = ucmds;
                let (spec, dbg) = debug::split(&spec).unwrap_or_else(|e| {
                    eprintln!("{e}");
                    usage()
                });
                a.debug_cmds = dbg;
                (a.inject, a.inject_shots) = parse_inject_all(&spec).unwrap_or_else(|e| {
                    eprintln!("{e}");
                    usage()
                })
            }
            "-v" | "--verbose" => a.verbose = true,
            "--audio-wav" => a.audio_wav = Some(val().into()),
            "--audio-wav-seconds" => a.audio_wav_seconds = val().parse().unwrap_or_else(|_| usage()),
            "--audio-play" => a.audio_play = true,
            "--mp-hash" => a.mp_hash = Some(val().into()),
            "--mp-hash-every" => a.mp_hash_every = val().parse().unwrap_or_else(|_| usage()),
            "--display" => a.display_mode = Some(s3_mods::display::DisplayMode::parse(&val()).unwrap_or_else(|| usage())),
            "--filter" => a.display_filter = Some(s3_mods::display::ScaleFilter::parse(&val()).unwrap_or_else(|| usage())),
            "--fps" => a.fps_counter = Some(s3_mods::display::FpsCounter::parse(&val()).unwrap_or_else(|| usage())),
            "--window" => {
                let v = val();
                let (w, h) = v.split_once('x').unwrap_or_else(|| usage());
                a.window_size = Some((w.parse().unwrap_or_else(|_| usage()), h.parse().unwrap_or_else(|_| usage())));
            }
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
    /// Viewport of the shown (or simulated headless) window, for `w`-prefixed touch injects.
    viewport: Option<s3_mods::display::Viewport>,
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
    audio: Option<audio::SharedSink>,
    /// Viewport frozen while injected fingers are down (like s3-android's `pinch_vp`: a pinch switches the render
    /// size, and mapping each move through the newest viewport would make the fingers jump in game pixels).
    touch_vp: Option<s3_mods::display::Viewport>,
    fingers: Vec<u32>,
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
        let vp = self.shared.lock().unwrap().viewport;
        while let Some((t, _)) = self.inject.front() {
            if *t > now {
                break;
            }
            let mut e = self.inject.pop_front().unwrap().1;
            // Window-coordinate touch inject: the game position through the viewport of the gesture's start.
            if let HostEvent::Pointer { id, phase, x, y, wx, wy } = &mut e {
                if self.fingers.is_empty() {
                    self.touch_vp = vp;
                }
                if *x < 0 && *wx >= 0 {
                    (*x, *y) = self.touch_vp.map_or((*wx, *wy), |v| v.window_to_game(*wx as f32 + 0.5, *wy as f32 + 0.5));
                }
                match phase {
                    s3_runtime::PointerPhase::Down => self.fingers.push(*id),
                    s3_runtime::PointerPhase::Up | s3_runtime::PointerPhase::Cancel => self.fingers.retain(|f| f != id),
                    _ => {}
                }
            }
            out.push(e);
        }
        out.extend(self.shared.lock().unwrap().events.drain(..));
        out
    }

    fn queue_audio(&mut self, samples: &[i16], rate: u32, channels: u16) {
        if let Some(a) = &self.audio {
            a.lock().unwrap().queue(samples, rate, channels);
        }
    }

    fn audio_queued_frames(&self) -> usize {
        self.audio.as_ref().map_or(0, |a| a.lock().unwrap().queued_frames())
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
    write_png_rgb(path, fb.width, fb.height, &fb.to_rgb888())
}

fn write_png_rgb(path: &Path, w: u32, h: u32, rgb: &[u8]) -> Result<(), String> {
    let f = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header().map_err(|e| e.to_string())?;
    wr.write_image_data(rgb).map_err(|e| e.to_string())
}

/// The window as the player sees it (XRGB8888): the frame scaled with the display settings (+ the Classic panel
/// split of the scene) and the mods' window layer on top.
fn compose_window(fb: &FrameBuffer, scene: &s3_mods::Scene, raster: &mut s3_mods::LayerRaster, d: s3_mods::display::DisplaySettings, ww: u32, wh: u32) -> (Vec<u32>, s3_mods::display::Viewport) {
    use s3_mods::display::Viewport;
    let panel = if scene.frame == (fb.width, fb.height) { scene.panel } else { None };
    let vp = Viewport::for_frame_split(d.mode, &fb.pixels, fb.width, fb.height, ww, wh, panel);
    let mut buf = vec![0u32; (ww * wh) as usize];
    s3_mods::display::scale_to_xrgb(&vp, d.filter, &fb.pixels, &mut buf);
    if scene.list.width == ww && scene.list.height == wh {
        raster.update(&scene.list);
        raster.blend_onto_xrgb(&mut buf, ww as usize);
    }
    (buf, vp)
}

/// `wshot`: write [`compose_window`] as a PNG.
fn write_window_png(path: &Path, buf: &[u32], w: u32, h: u32) -> Result<(), String> {
    let rgb: Vec<u8> = buf.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]).collect();
    write_png_rgb(path, w, h, &rgb)
}

mod layout_tool;

mod window {
    //! minifb window frontend (X11).
    use minifb::{Key, MouseButton as MfButton, MouseMode, ScaleMode, Window, WindowOptions};
    use s3_mods::display::{DisplaySettings, Viewport};
    use s3_mods::{LayerRaster, SceneSlot};
    use s3_runtime::{FrameBuffer, HostEvent, MouseButton};
    use std::sync::Arc;

    pub struct Win {
        win: Option<Window>,
        /// Window (= buffer) size; the game frame is scaled into it by `s3_mods::display` (CPU).
        size: (usize, usize),
        buf: Vec<u32>,
        /// Requested window size (`--window`), else the first frame's size.
        want: Option<(u32, u32)>,
        pub display: DisplaySettings,
        pub viewport: Option<Viewport>,
        last_frame: Option<FrameBuffer>,
        last_mouse: (i32, i32),
        buttons: [bool; 3],
        keys_down: Vec<Key>,
        /// The mods' window layer (s3_mods::layer): mailbox, raster, scene shown.
        pub layer: Option<Arc<SceneSlot>>,
        raster: LayerRaster,
        layer_seq: u64,
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
            Win {
                win: None,
                size: (0, 0),
                buf: Vec::new(),
                want: None,
                display: DisplaySettings::default(),
                viewport: None,
                last_frame: None,
                last_mouse: (-1, -1),
                buttons: [false; 3],
                keys_down: Vec::new(),
                layer: None,
                raster: LayerRaster::new(),
                layer_seq: u64::MAX,
            }
        }

        pub fn with_size(want: Option<(u32, u32)>) -> Self {
            Win { want, ..Win::new() }
        }

        pub fn is_open(&self) -> bool {
            self.win.as_ref().map(|w| w.is_open()).unwrap_or(true)
        }

        pub fn show(&mut self, fb: &FrameBuffer) {
            if self.win.is_none() {
                let size = self.want.map(|(w, h)| (w as usize, h as usize)).unwrap_or((fb.width as usize, fb.height as usize));
                let opts = WindowOptions { resize: true, scale_mode: ScaleMode::UpperLeft, ..WindowOptions::default() };
                match Window::new("Settlers 3 Port (s3run)", size.0, size.1, opts) {
                    Ok(mut w) => {
                        w.set_target_fps(0);
                        // The game's cursor is composed into the frame (s3_runtime::cursor).
                        w.set_cursor_visibility(false);
                        self.win = Some(w);
                        self.size = size;
                    }
                    Err(e) => {
                        eprintln!("window: {e}");
                        return;
                    }
                }
            }
            match &mut self.last_frame {
                Some(l) if l.width == fb.width && l.height == fb.height => l.pixels.copy_from_slice(&fb.pixels),
                l => *l = Some(fb.clone()),
            }
            self.redraw();
        }

        /// Scale the last frame into the window buffer with the current display settings.
        fn redraw(&mut self) {
            let Some(fb) = &self.last_frame else { return };
            let Some(w) = &mut self.win else { return };
            let (ww, wh) = w.get_size();
            let (ww, wh) = (ww.max(1), wh.max(1));
            self.size = (ww, wh);
            let scene = self.layer.as_ref().map(|l| l.latest()).unwrap_or_default();
            self.layer_seq = scene.seq;
            let panel = if scene.frame == (fb.width, fb.height) { scene.panel } else { None };
            let vp = Viewport::for_frame_split(self.display.mode, &fb.pixels, fb.width, fb.height, ww as u32, wh as u32, panel);
            self.buf.resize(ww * wh, 0);
            s3_mods::display::scale_to_xrgb(&vp, self.display.filter, &fb.pixels, &mut self.buf);
            // Window layer on top (only a layer made for this window size).
            if scene.list.width as usize == ww && scene.list.height as usize == wh {
                self.raster.update(&scene.list);
                self.raster.blend_onto_xrgb(&mut self.buf, ww);
            }
            self.viewport = Some(vp);
            let _ = w.update_with_buffer(&self.buf, ww, wh);
        }

        /// New display settings: redraw at once.
        pub fn set_display(&mut self, d: DisplaySettings) {
            if d != self.display {
                self.display = d;
                self.redraw();
            }
        }

        /// Pump window events (call regularly even without new frames).
        pub fn events(&mut self) -> Vec<HostEvent> {
            let mut out = Vec::new();
            let Some(w) = &mut self.win else { return out };
            let (cw, ch) = w.get_size();
            let layer_new = self.layer.as_ref().is_some_and(|l| l.seq() != self.layer_seq);
            if ((cw, ch) != self.size && cw > 0 && ch > 0) || layer_new {
                self.redraw(); // window resized / new window layer
            }
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
                // Window -> game pixels (shared transform, follows the display mode).
                let p = match &self.viewport {
                    Some(vp) => vp.window_to_game(x, y),
                    None => (x as i32, y as i32),
                };
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
    let raw: Vec<String> = std::env::args().skip(1).collect();
    if let Some(r) = layout_tool::run(&raw) {
        if let Err(e) = r {
            eprintln!("s3run: {e}");
            std::process::exit(2);
        }
        return;
    }
    let args = parse_args();
    std::fs::create_dir_all(&args.data_dir).ok();
    if let Some(d) = &args.shots_dir {
        std::fs::create_dir_all(d).ok();
    }
    let shared = Arc::new(Mutex::new(Shared::default()));
    let audio_sink: Option<audio::SharedSink> = if args.audio_wav.is_some() || args.audio_play {
        let wav = args.audio_wav.as_ref().and_then(|p| {
            audio::WavWriter::create(p, audio::SINK_RATE, 2, args.audio_wav_seconds)
                .map_err(|e| eprintln!("s3run: --audio-wav {}: {e}", p.display()))
                .ok()
        });
        Some(Arc::new(Mutex::new(audio::AudioSink::new(wav, args.audio_play))))
    } else {
        None
    };
    let host = DesktopHost {
        game_dir: args.game_dir.clone(),
        data_dir: args.data_dir.clone(),
        start: Instant::now(),
        shared: shared.clone(),
        inject: args.inject.iter().copied().collect(),
        shots_dir: args.shots_dir.clone(),
        shots: 0,
        verbose: args.verbose,
        audio: audio_sink.clone(),
        touch_vp: None,
        fingers: Vec::new(),
    };
    let mut engine = match Engine::new(Box::new(host), args.exe.as_deref()) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("s3run: {e}");
            std::process::exit(1);
        }
    };
    // S3_MT=1: multi-core mode (draw thread on its own host thread, docs/porting/multicore.md). Not deterministic.
    if std::env::var("S3_MT").is_ok_and(|v| v == "1") {
        engine.machine.enable_mt();
    }
    if std::env::var_os("S3_GAME_LOG").is_some() {
        mp_hash::install_game_log(&mut engine.machine);
    }
    if std::env::var_os("S3_MP_NETDBG").is_some() {
        mp_hash::install_net_debug(&mut engine.machine);
    }
    if let Some(p) = &args.mp_hash {
        mp_hash::install(&mut engine.machine, p, args.mp_hash_every);
    }
    let start = Instant::now();
    let mut win = if args.headless { None } else { Some(window::Win::with_size(args.window_size)) };
    // The mods' window layer (docs/porting/mods.md "Window layer").
    let layer = s3_mods::SceneSlot::new(None);
    s3_mods::set_layer_sink(&mut engine.machine, layer.clone());
    if let Some(w) = &mut win {
        w.layer = Some(layer.clone());
    }
    let mut shot_raster = s3_mods::LayerRaster::new();
    // Display overrides from the command line (in memory only, not saved to mods-settings.json).
    {
        let mut d = s3_mods::display::display_settings(&mut engine.machine);
        if let Some(v) = args.display_mode {
            d.mode = v;
        }
        if let Some(v) = args.display_filter {
            d.filter = v;
        }
        if let Some(v) = args.fps_counter {
            d.fps_counter = v;
        }
        s3_mods::display::override_display(&mut engine.machine, d);
    }
    let mut published_vp: Option<s3_mods::display::Viewport> = None;
    let mut shown_serial = 0;
    let sample = std::env::var_os("S3_SAMPLE").is_some();
    let mut last_sample = Instant::now();
    let mut sample_prev: Vec<u64> = Vec::new();
    let mut sample_presents = 0u64;
    let mut sample_cpu_ns = 0u64;
    // S3_PROFILE_FROM_MS: zero the profile counters at this time (profile only the in-game part).
    let mut profile_reset: Option<u64> = std::env::var("S3_PROFILE_FROM_MS").ok().and_then(|v| v.parse().ok());
    let mut inject_shots: VecDeque<(u64, PathBuf, bool)> = args.inject_shots.iter().cloned().collect();
    let mut debug_cmds: VecDeque<(u64, debug::DebugCmd)> = args.debug_cmds.iter().cloned().collect();
    let mut units_cmds: VecDeque<(u64, debug_units::UnitsCmd)> = args.units_cmds.iter().cloned().collect();
    loop {
        debug::run_due(&mut engine, &mut debug_cmds, start.elapsed().as_millis() as u64);
        debug_units::run_due(&mut engine, &mut units_cmds, start.elapsed().as_millis() as u64);
        while inject_shots.front().map(|(t, _, _)| *t <= start.elapsed().as_millis() as u64).unwrap_or(false) {
            let (_, p, windowed) = inject_shots.pop_front().unwrap();
            let f = shared.lock().unwrap().frame.clone();
            let d = s3_mods::display::display_settings(&mut engine.machine);
            let r = f.map(|f| {
                if !windowed {
                    return write_png(&p, &f);
                }
                let (ww, wh) = args.window_size.or(s3_mods::display::window_size(&mut engine.machine)).unwrap_or((f.width, f.height));
                let (buf, _) = compose_window(&f, &layer.latest(), &mut shot_raster, d, ww, wh);
                write_window_png(&p, &buf, ww, wh)
            });
            match r {
                Some(Ok(())) => eprintln!("s3run: shot {} at {} ms", p.display(), start.elapsed().as_millis()),
                Some(Err(e)) => eprintln!("s3run: shot {} failed: {e}", p.display()),
                None => eprintln!("s3run: shot {}: no frame yet", p.display()),
            }
        }
        if let Some(max) = args.max_seconds {
            if start.elapsed().as_secs_f64() >= max {
                eprintln!("s3run: --max-seconds reached");
                break;
            }
        }
        let r = engine.run_for_ms(16);
        // Headless + --window: publish the simulated screen so display modes (Native wide, Fill) work headless.
        if let (None, Some((ww, wh))) = (&win, args.window_size) {
            let (fw, fh) = engine.machine.last_frame_size();
            if fw > 0 {
                let mode = s3_mods::display::display_settings(&mut engine.machine).mode;
                let scene = layer.latest();
                let panel = if scene.frame == (fw, fh) { scene.panel } else { None };
                let vp = Some(s3_mods::display::Viewport::new_split(mode, fw, fh, ww, wh, panel));
                if vp != published_vp {
                    published_vp = vp;
                    s3_mods::display::set_viewport(&mut engine.machine, vp);
                    shared.lock().unwrap().viewport = vp;
                }
            }
        }
        if profile_reset.is_some_and(|t| start.elapsed().as_millis() as u64 >= t) {
            profile_reset = None;
            for t in engine.machine.threads.iter_mut() {
                t.cpu.cache.reset_profile();
            }
            eprintln!("s3run: profile counters reset");
        }
        // Diagnostics: S3_SAMPLE=1 prints per-thread instruction deltas and EIP once per second.
        if sample && last_sample.elapsed().as_secs_f64() >= 1.0 {
            last_sample = Instant::now();
            let pr = engine.machine.stats.presents;
            let ns = engine.machine.stats.cpu_ns;
            let mut line = format!(
                "[sample {:>5.1}s] fps:{} cpu:{}ms",
                start.elapsed().as_secs_f64(),
                pr - sample_presents,
                (ns - sample_cpu_ns) / 1_000_000
            );
            sample_presents = pr;
            sample_cpu_ns = ns;
            for (i, t) in engine.machine.threads.iter().enumerate() {
                let prev = sample_prev.get(i).copied().unwrap_or(0);
                line.push_str(&format!(" t{i}:{}M@{:#x}/{:?}", (t.cpu.instructions - prev) / 1_000_000, t.cpu.eip, t.state));
            }
            sample_prev = engine.machine.threads.iter().map(|t| t.cpu.instructions).collect();
            eprintln!("{line}");
        }
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
            w.set_display(s3_mods::display::display_settings(&mut engine.machine));
            if let Some(f) = frame {
                w.show(&f);
            }
            if w.viewport != published_vp {
                published_vp = w.viewport;
                s3_mods::display::set_viewport(&mut engine.machine, published_vp);
                shared.lock().unwrap().viewport = published_vp;
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
    // S3_PROFILE=<file>: per-thread decoded-block entry counts (thread start eip len entries), for
    // tools/perf_profile.py (function-level guest profile, see docs/porting/performance.md).
    if let Some(p) = std::env::var_os("S3_PROFILE") {
        let mut out = String::new();
        for (i, t) in engine.machine.threads.iter().enumerate() {
            for (start, len, n) in t.cpu.cache.profile() {
                if n > 0 {
                    out.push_str(&format!("{i} {start:#x} {len} {n}\n"));
                }
            }
        }
        if let Err(e) = std::fs::write(&p, out) {
            eprintln!("s3run: S3_PROFILE write failed: {e}");
        }
        let mut dis = String::new();
        for (i, t) in engine.machine.threads.iter().enumerate() {
            dis.push_str(&format!("=== thread {i}\n{}", t.cpu.cache.profile_disasm(60)));
        }
        let mut dp = p.clone();
        dp.push(".disasm");
        let _ = std::fs::write(dp, dis);
        let mut forms: std::collections::HashMap<String, u64> = Default::default();
        for t in engine.machine.threads.iter() {
            for (k, n) in t.cpu.cache.profile_forms() {
                *forms.entry(k).or_default() += n;
            }
        }
        let mut fv: Vec<_> = forms.into_iter().collect();
        fv.sort_by_key(|e| std::cmp::Reverse(e.1));
        let total: u64 = fv.iter().map(|e| e.1).sum::<u64>().max(1);
        let txt: String = fv.iter().take(150).map(|(k, n)| format!("{:6.2}% {k}\n", 100.0 * *n as f64 / total as f64)).collect();
        let mut fp = p.clone();
        fp.push(".forms");
        let _ = std::fs::write(fp, txt);
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
    if let Some(a) = &audio_sink {
        for line in a.lock().unwrap().finish() {
            eprintln!("{line}");
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
    use s3_runtime::PointerPhase;

    #[test]
    fn inject_touch_parsing() {
        let v = parse_inject("100:tap 10 20; 200:drag 0 0 100 0 100; 1000:pinch 300 200 2; 2000:longpress 5 5 600").unwrap();
        let pt = |id, phase, x, y| HostEvent::Pointer { id, phase, x, y, wx: -1, wy: -1 };
        assert_eq!(v[0], (100, pt(1, PointerPhase::Down, 10, 20)));
        assert_eq!(v[1], (170, pt(1, PointerPhase::Up, 10, 20)));
        assert!(v.contains(&(301, pt(1, PointerPhase::Up, 100, 0))));
        assert!(v.contains(&(1302, pt(2, PointerPhase::Up, 380, 200))));
        assert_eq!(v.last().unwrap(), &(2600, pt(1, PointerPhase::Up, 5, 5)));
        let v = parse_inject("0:lpdrag 10 10 110 60 700 160; 5000:drag2 300 200 400 200 160; 9000:tap3 50 50; 9500:dtap 5 5").unwrap();
        assert_eq!(v[0], (0, pt(1, PointerPhase::Down, 10, 10)));
        assert_eq!(v[1].0, 716, "lpdrag moves start after the hold");
        assert!(v.contains(&(861, pt(1, PointerPhase::Up, 110, 60))));
        assert!(v.contains(&(5005, pt(2, PointerPhase::Down, 330, 200))));
        assert!(v.contains(&(5172, pt(2, PointerPhase::Up, 430, 200))));
        // Window coordinates (`w` prefix): game position left to the host.
        let wv = parse_inject("0:wtap 1000 400").unwrap();
        assert_eq!(wv[0], (0, HostEvent::Pointer { id: 1, phase: PointerPhase::Down, x: -1, y: -1, wx: 1000, wy: 400 }));
        assert_eq!(v.iter().filter(|e| e.0 >= 9000 && e.0 < 9100 && matches!(e.1, HostEvent::Pointer { phase: PointerPhase::Down, .. })).count(), 3);
        assert_eq!(v.iter().filter(|e| e.0 >= 9500 && matches!(e.1, HostEvent::Pointer { phase: PointerPhase::Up, .. })).count(), 2);
    }

    #[test]
    fn inject_parsing() {
        let v = parse_inject("100:click 10 20; 50:key 0x1B ;300:quit").unwrap();
        assert_eq!(v[0], (50, HostEvent::Key { vk: 0x1B, down: true }));
        assert!(v.contains(&(100, HostEvent::MouseButton { button: MouseButton::Left, down: true })));
        assert!(v.contains(&(150, HostEvent::MouseButton { button: MouseButton::Left, down: false })));
        assert_eq!(v.last(), Some(&(300, HostEvent::Quit)));
        assert!(parse_inject("x:click").is_err());
        let (v, shots) = parse_inject_all("1000:text Ab1;2000:shot /tmp/x.png;3000:wshot /tmp/y.png").unwrap();
        assert_eq!(shots, vec![(2000, PathBuf::from("/tmp/x.png"), false), (3000, PathBuf::from("/tmp/y.png"), true)]);
        assert!(v.contains(&(1010, HostEvent::Key { vk: 0x41, down: true })));
        assert!(v.contains(&(1130, HostEvent::Key { vk: 0x42, down: true })));
        assert!(v.contains(&(1250, HostEvent::Key { vk: 0x31, down: true })));
        assert!(parse_inject("1:text a!").is_err());
    }
}
