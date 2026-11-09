//! Launcher (home screen) shown at app start, before the engine runs (docs/porting/android.md "Launcher").
//!
//! Pages: **Home** (game files status + S3.EXE version, Play, Settings, Game files, Quit), **Settings** (the
//! mods settings in `<data dir>/mods-settings.json` with the in-game menu labels, plus the Internet relay URL in
//! `relay-url.txt`), **Game files** (built-in web upload server [`crate::upload`], QR code, progress; adb hint).
//!
//! Platform independent (builds and is tested on the desktop): it records a window-resolution
//! [`DrawList`] with the mods' layer recorder; the Android presenter composites it like the in-game overlay layer.
//! The activity loop feeds it the primary finger, Back and typed characters, and carries out the returned
//! [`Request`]s (start the engine, soft keyboard, quit).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use s3_mods::display::FPS_LIMITS;
use s3_mods::layer::{DrawList, ImageRef, Layer};
use s3_mods::overlay::{measure_text, Image, Rgba, Weight};
use s3_mods::settings::{cycle_vibration, vibration_label, Settings, UiMode};

use crate::frontend::{missing_parts, user_visible_path, REQUIRED_PARTS};
use crate::upload::{UploadServer, UploadStatus};

/// File in the data dir with the multiplayer relay URL (read by `frontend::run_engine`).
pub const RELAY_FILE: &str = "relay-url.txt";
/// Debug / test override of the game dir (one line, absolute path): test the "missing files" state or uploads
/// without touching the real install.
pub const GAME_DIR_FILE: &str = "game-dir.txt";

/// The game dir: `<data dir>/game-dir.txt` if set, else `default`.
pub fn game_dir_override(data_dir: &Path, default: PathBuf) -> PathBuf {
    match std::fs::read_to_string(data_dir.join(GAME_DIR_FILE)) {
        Ok(s) if !s.trim().is_empty() => PathBuf::from(s.trim()),
        _ => default,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Home,
    Settings,
    Files,
}

/// What the activity loop has to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Play,
    Keyboard(bool),
    Quit,
    /// iOS: show the system folder picker; the shell then calls [`Launcher::set_game_dir`].
    PickFolder,
    /// iOS: forget the picked folder and use the app's own one again.
    UseAppFolder,
}

/// Finger phase (primary pointer only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Touch {
    Down,
    Move,
    Up,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Act {
    Play,
    Page(Page),
    Back,
    Quit,
    UploadOn,
    UploadOff,
    Recheck,
    UiMode,
    Screen,
    Filter,
    FpsCounter,
    FpsLimit,
    FpsPos,
    FastSpeed,
    GameTime,
    UnitPaths,
    Groups,
    Vibration,
    Hud(i32),
    EditRelay,
    RelayDefault,
    PickFolder,
    AppFolder,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

impl Rect {
    fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }
    fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
}

const BG: Rgba = Rgba::rgb(14, 20, 34);
const BAR: Rgba = Rgba::rgb(24, 36, 64);
const CARD: Rgba = Rgba::rgb(26, 36, 58);
const BTN: Rgba = Rgba::rgb(46, 64, 102);
const BTN_DOWN: Rgba = Rgba::rgb(78, 104, 156);
const PLAY: Rgba = Rgba::rgb(40, 140, 72);
const OFF: Rgba = Rgba::rgb(58, 62, 72);
const DANGER: Rgba = Rgba::rgb(140, 52, 52);
const TEXT: Rgba = Rgba::rgb(236, 240, 248);
const DIM: Rgba = Rgba::rgb(168, 180, 202);
const GOLD: Rgba = Rgba::rgb(236, 196, 96);
const GOOD: Rgba = Rgba::rgb(110, 220, 140);
const BAD: Rgba = Rgba::rgb(255, 128, 128);
const FIELD: Rgba = Rgba::rgb(10, 14, 24);

/// Game files state shown on Home and Game files.
#[derive(Debug, Clone, Default)]
pub struct FilesInfo {
    pub missing: Vec<&'static str>,
    pub version: String,
    /// (size, mtime) of the S3.EXE the version was computed for.
    key: Option<(u64, std::time::SystemTime)>,
}

/// What S3.EXE is, from its size and sha256 (docs/game-files/exe-variants.md).
pub fn exe_version(game_dir: &Path) -> (String, Option<(u64, std::time::SystemTime)>) {
    let Some(p) = crate::frontend::find_exe(game_dir) else { return ("S3.EXE not found".into(), None) };
    let Ok(md) = std::fs::metadata(&p) else { return ("S3.EXE not readable".into(), None) };
    let key = Some((md.len(), md.modified().unwrap_or(std::time::UNIX_EPOCH)));
    let Ok(b) = std::fs::read(&p) else { return ("S3.EXE not readable".into(), key) };
    let sha = s3_mods::sha256::sha256_hex(&b);
    let v = if sha.eq_ignore_ascii_case(s3_mods::patchset::GOG_SHA256) {
        "GOG / Ultimate Collection S3.EXE 1.60 (supported)".to_string()
    } else if b.len() == 3_842_048 {
        "S3.EXE 1.60 build, not the GOG one (mobile patches off)".to_string()
    } else {
        format!("unknown S3.EXE ({:.1} MB; mobile patches off)", b.len() as f64 / 1048576.0)
    };
    (v, key)
}

pub struct Launcher {
    pub page: Page,
    game_dir: PathBuf,
    data_dir: PathBuf,
    settings: Settings,
    relay: String,
    /// Relay field being edited (soft keyboard open).
    pub editing: bool,
    pub files: FilesInfo,
    /// One message on Home (game ended, engine error), and whether it is an error.
    pub message: Option<(String, bool)>,
    /// Display refresh rate (adds it to the FPS limit choices like the in-game menu).
    pub refresh_hz: u32,
    /// Platform folder picker (iOS): `None` = not available (Android); `Some(false)` = the app's own game folder
    /// is used, `Some(true)` = a folder the user picked outside the app (it survives deleting the app).
    pub folder_picker: Option<bool>,
    scroll: i32,
    max_scroll: i32,
    hits: Vec<(Rect, Act)>,
    /// Primary finger: down position, scroll at down, dragging.
    press: Option<(i32, i32, i32, bool)>,
    pressed: Act,
    server: Option<UploadServer>,
    server_err: Option<String>,
    ip: Option<String>,
    qr: Option<(String, ImageRef)>,
    seen_upload: u64,
    last_check: std::time::Instant,
    /// Window size + density of the last draw.
    size: (u32, u32, f32),
}

impl Launcher {
    pub fn new(game_dir: PathBuf, data_dir: PathBuf) -> Launcher {
        let (settings, err) = Settings::load(&data_dir);
        if let Some(e) = err {
            log::warn!("launcher: settings: {e}");
        }
        let relay = std::fs::read_to_string(data_dir.join(RELAY_FILE)).map(|s| s.trim().to_string()).unwrap_or_default();
        let mut l = Launcher {
            page: Page::Home,
            game_dir,
            data_dir,
            settings,
            relay,
            editing: false,
            files: FilesInfo::default(),
            message: None,
            refresh_hz: 0,
            folder_picker: None,
            scroll: 0,
            max_scroll: 0,
            hits: Vec::new(),
            press: None,
            pressed: Act::None,
            server: None,
            server_err: None,
            ip: None,
            qr: None,
            seen_upload: 0,
            last_check: std::time::Instant::now(),
            size: (1, 1, 1.0),
        };
        l.recheck();
        l
    }

    pub fn game_dir(&self) -> &Path {
        &self.game_dir
    }

    /// Use another game folder (iOS folder picker): stops the upload server (it serves the old folder).
    pub fn set_game_dir(&mut self, dir: PathBuf, external: bool) {
        log::info!("launcher: game dir {} ({})", dir.display(), if external { "picked folder" } else { "app folder" });
        self.stop_upload();
        self.game_dir = dir;
        if self.folder_picker.is_some() {
            self.folder_picker = Some(external);
        }
        self.files.key = None;
        self.recheck();
    }

    pub fn ready(&self) -> bool {
        self.files.missing.is_empty()
    }

    /// Look at the game dir again (missing parts; version only when S3.EXE changed).
    pub fn recheck(&mut self) {
        self.files.missing = missing_parts(&self.game_dir);
        let key = crate::frontend::find_exe(&self.game_dir)
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| (m.len(), m.modified().unwrap_or(std::time::UNIX_EPOCH)));
        if key.is_none() {
            self.files.version = "S3.EXE not found".into();
            self.files.key = None;
        } else if key != self.files.key {
            let (v, k) = exe_version(&self.game_dir);
            self.files.version = v;
            self.files.key = k;
        }
        self.last_check = std::time::Instant::now();
    }

    /// Settings may have been changed in the game: reload them (after the engine stopped).
    pub fn reload_settings(&mut self) {
        self.settings = Settings::load(&self.data_dir).0;
    }

    /// Periodic work (call every loop): upload progress, file re-checks. True if something changed.
    pub fn tick(&mut self) -> bool {
        let mut changed = false;
        if let Some(s) = &self.server {
            let seq = s.status.lock().unwrap().seq;
            if seq != self.seen_upload {
                self.seen_upload = seq;
                changed = true;
            }
        }
        // Files may arrive by adb / MTP / the upload page: look again every few seconds.
        let every = if self.server.is_some() { 2 } else { 5 };
        if self.last_check.elapsed().as_secs() >= every {
            let before = (self.files.missing.clone(), self.files.version.clone());
            self.recheck();
            changed |= before != (self.files.missing.clone(), self.files.version.clone());
        }
        changed
    }

    pub fn upload_running(&self) -> bool {
        self.server.is_some()
    }

    pub fn upload_status(&self) -> Option<UploadStatus> {
        self.server.as_ref().map(|s| s.snapshot())
    }

    /// The address shown for the running server.
    pub fn upload_url(&self) -> Option<String> {
        let s = self.server.as_ref()?;
        Some(s.url(self.ip.as_deref().unwrap_or("<phone IP>")))
    }

    fn start_upload(&mut self) {
        let _ = std::fs::create_dir_all(&self.game_dir);
        match UploadServer::start(self.game_dir.clone(), missing_parts) {
            Ok(s) => {
                self.ip = crate::upload::local_ip().map(|i| i.to_string());
                self.server = Some(s);
                self.server_err = None;
                if let Some(u) = self.upload_url() {
                    log::info!("launcher: upload server at {u}");
                    self.qr = qr_image(&u).map(|i| (u, Arc::new(i)));
                }
            }
            Err(e) => self.server_err = Some(format!("Could not start the server: {e}")),
        }
    }

    pub fn stop_upload(&mut self) {
        if self.server.take().is_some() {
            log::info!("launcher: upload server stopped");
        }
        self.recheck();
    }

    fn save_settings(&mut self) {
        if let Err(e) = self.settings.save(&self.data_dir) {
            log::warn!("launcher: saving settings: {e}");
        }
    }

    fn save_relay(&mut self) {
        let p = self.data_dir.join(RELAY_FILE);
        let url = self.relay.trim().to_string();
        let r = if url.is_empty() {
            match std::fs::remove_file(&p) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            }
        } else {
            std::fs::create_dir_all(&self.data_dir).and_then(|_| std::fs::write(&p, format!("{url}\n")))
        };
        if let Err(e) = r {
            log::warn!("launcher: saving {}: {e}", p.display());
        }
    }

    fn fps_limits(&self) -> Vec<u32> {
        let mut v = FPS_LIMITS.to_vec();
        if self.refresh_hz > *FPS_LIMITS.last().unwrap() {
            v.push(self.refresh_hz);
        }
        v
    }

    fn run(&mut self, a: Act) -> Option<Request> {
        match a {
            Act::Play => {
                self.recheck();
                if self.ready() {
                    self.stop_upload();
                    return Some(Request::Play);
                }
                return None;
            }
            Act::Page(p) => {
                self.page = p;
                self.scroll = 0;
                if p == Page::Files {
                    self.recheck();
                }
                return None;
            }
            Act::Back => return self.back_inner(),
            Act::Quit => return Some(Request::Quit),
            Act::UploadOn => {
                self.start_upload();
                return None;
            }
            Act::UploadOff => {
                self.stop_upload();
                return None;
            }
            Act::Recheck | Act::None => {
                self.recheck();
                return None;
            }
            Act::PickFolder => return Some(Request::PickFolder),
            Act::AppFolder => return Some(Request::UseAppFolder),
            Act::EditRelay => {
                self.editing = true;
                return Some(Request::Keyboard(true));
            }
            Act::RelayDefault => {
                self.relay.clear();
                self.editing = false;
                self.save_relay();
                return Some(Request::Keyboard(false));
            }
            _ => {}
        }
        let limits = self.fps_limits();
        let s = &mut self.settings;
        match a {
            Act::UiMode => {
                s.ui_mode = if s.ui_mode == UiMode::MobileHud { UiMode::ClassicTouch } else { UiMode::MobileHud };
            }
            Act::Screen => s.display.mode = s.display.mode.next(),
            Act::Filter => s.display.filter = s.display.filter.next(),
            Act::FpsCounter => s.display.fps_counter = s.display.fps_counter.next(),
            Act::FpsLimit => {
                let i = limits.iter().position(|&o| o == s.display.fps_limit);
                s.display.fps_limit = limits[i.map_or(0, |i| (i + 1) % limits.len())];
            }
            Act::FpsPos => s.display.fps_pos = s.display.fps_pos.next(),
            Act::FastSpeed => {
                let c = &s3_mods::speed::FAST_CHOICES;
                let cur = s.fast_speed.clamp(c[0], c[c.len() - 1]);
                let i = c.iter().position(|&x| x == cur).unwrap_or(0);
                s.fast_speed = c[(i + 1) % c.len()];
            }
            Act::GameTime => s.show_game_time = !s.show_game_time,
            Act::UnitPaths => s.unit_paths = s.unit_paths.next(),
            Act::Groups => s.group_shortcuts = !s.group_shortcuts,
            Act::Vibration => cycle_vibration(s),
            Act::Hud(d) => s.hud_scale = ((s.hud_scale * 10.0).round() / 10.0 + d as f32 * 0.1).clamp(0.7, 1.5),
            _ => {}
        }
        self.save_settings();
        None
    }

    fn back_inner(&mut self) -> Option<Request> {
        if self.editing {
            self.finish_edit();
            return Some(Request::Keyboard(false));
        }
        if self.page != Page::Home {
            self.page = Page::Home;
            self.scroll = 0;
        }
        None
    }

    /// Back key. False on Home (the app may close).
    pub fn back(&mut self) -> (bool, Option<Request>) {
        if self.page == Page::Home && !self.editing {
            return (false, None);
        }
        (true, self.back_inner())
    }

    fn finish_edit(&mut self) {
        if self.editing {
            self.editing = false;
            self.save_relay();
        }
    }

    /// A typed character (soft keyboard): only while the relay field is edited. 0x08 = backspace, 0x0D = done.
    pub fn char(&mut self, c: u32) -> Option<Request> {
        if !self.editing {
            return None;
        }
        match c {
            0x08 => {
                self.relay.pop();
            }
            0x0D => {
                self.finish_edit();
                return Some(Request::Keyboard(false));
            }
            c if c >= 0x20 && c != 0x7f => {
                if let Some(ch) = char::from_u32(c) {
                    if self.relay.len() < 200 && !ch.is_whitespace() {
                        self.relay.push(ch);
                    }
                }
            }
            _ => {}
        }
        None
    }

    fn act_at(&self, x: i32, y: i32) -> Act {
        self.hits.iter().rev().find(|(r, _)| r.contains(x, y)).map_or(Act::None, |h| h.1)
    }

    /// Primary finger in window pixels.
    pub fn touch(&mut self, t: Touch, x: i32, y: i32) -> Option<Request> {
        let slop = (10.0 * self.size.2) as i32;
        match t {
            Touch::Down => {
                self.press = Some((x, y, self.scroll, false));
                self.pressed = self.act_at(x, y);
                None
            }
            Touch::Move => {
                if let Some((x0, y0, s0, drag)) = self.press.as_mut() {
                    if !*drag && ((y - *y0).abs() > slop || (x - *x0).abs() > slop) {
                        *drag = true;
                        self.pressed = Act::None;
                    }
                    if *drag {
                        self.scroll = (*s0 - (y - *y0)).clamp(0, self.max_scroll);
                    }
                }
                None
            }
            Touch::Up => {
                let p = self.press.take();
                let a = std::mem::replace(&mut self.pressed, Act::None);
                match p {
                    Some((_, _, _, false)) if a != Act::None && self.act_at(x, y) == a => {
                        if self.editing && a != Act::EditRelay && a != Act::RelayDefault {
                            self.finish_edit();
                            let r = self.run(a);
                            return r.or(Some(Request::Keyboard(false)));
                        }
                        self.run(a)
                    }
                    Some((_, _, _, false)) if self.editing && a == Act::None => {
                        // Tap outside the field ends editing.
                        self.finish_edit();
                        Some(Request::Keyboard(false))
                    }
                    _ => None,
                }
            }
            Touch::Cancel => {
                self.press = None;
                self.pressed = Act::None;
                None
            }
        }
    }

    // ------------------------------------------------------------------ drawing

    /// Record the launcher for a `w` x `h` window; `dpi` = screen density (0 = unknown).
    pub fn draw(&mut self, w: u32, h: u32, dpi: u32) -> DrawList {
        let k = if dpi > 0 { dpi as f32 / 160.0 } else { (w.min(h) as f32 / 360.0).max(1.0) };
        // Keep everything on screen on small windows: at least ~330 dp of height.
        let k = k.min(h as f32 / 330.0).min(w as f32 / 340.0).max(0.5);
        self.size = (w, h, k);
        self.hits.clear();
        let mut c = Layer::new(w, h);
        let (w, h) = (w as i32, h as i32);
        c.fill_rect(0, 0, w, h, BG);
        let d = |v: f32| (v * k).round() as i32;
        // Top bar.
        let bar = d(52.0);
        c.fill_rect(0, 0, w, bar, BAR);
        let mut tx = d(16.0);
        let back = self.page != Page::Home;
        if back {
            let r = Rect::new(d(6.0), d(6.0), d(96.0), bar - d(12.0));
            self.button(&mut c, r, "< Back", BTN, Act::Back, k);
            tx = r.x + r.w + d(14.0);
        }
        let title = match self.page {
            Page::Home => "S3 Mobile",
            Page::Settings => "Settings",
            Page::Files => "Game files",
        };
        let tpx = 22.0 * k;
        let th = measure_text(tpx, Weight::Bold, title).1;
        c.text(tx, (bar - th) / 2, tpx, Weight::Bold, if back { TEXT } else { GOLD }, title);
        let ver = concat!("v", env!("CARGO_PKG_VERSION"));
        let vpx = 13.0 * k;
        let (vw, vh) = measure_text(vpx, Weight::Regular, ver);
        c.text(w - vw - d(16.0), (bar - vh) / 2, vpx, Weight::Regular, DIM, ver);
        // Scrolled content below the bar.
        let top = bar + d(10.0);
        c.set_clip(0, bar, w, h - bar);
        let y0 = top - self.scroll;
        let end = match self.page {
            Page::Home => self.draw_home(&mut c, y0, w, k),
            Page::Settings => self.draw_settings(&mut c, y0, w, k),
            Page::Files => self.draw_files(&mut c, y0, w, k),
        };
        let content = end - y0 + d(16.0);
        self.max_scroll = (content - (h - top)).max(0);
        if self.scroll > self.max_scroll {
            self.scroll = self.max_scroll;
        }
        if self.max_scroll > 0 {
            // Scroll bar.
            let view = h - top;
            let bh = (view * view / content.max(1)).max(d(24.0));
            let by = top + (view - bh) * self.scroll / self.max_scroll.max(1);
            c.fill_rounded_rect(w - d(5.0), by, d(3.0), bh, 1.5 * k, DIM.fade(0.6));
        }
        c.reset_clip();
        // Hits outside the content area (under the bar) must not trigger.
        let clip = Rect::new(0, bar, w, h - bar);
        self.hits.retain(|(r, a)| *a == Act::Back || r.y + r.h > clip.y);
        c.finish()
    }

    fn button(&mut self, c: &mut Layer, r: Rect, label: &str, bg: Rgba, a: Act, k: f32) {
        let bg = if self.pressed == a && a != Act::None { BTN_DOWN } else { bg };
        c.fill_rounded_rect(r.x, r.y, r.w, r.h, 8.0 * k, bg);
        c.text_centered(r.x, r.y, r.w, r.h, 16.0 * k, Weight::Bold, if bg == OFF { DIM } else { TEXT }, label);
        if a != Act::None {
            self.hits.push((r, a));
        }
    }

    /// Wrapped text; returns the y below it.
    fn para(c: &mut Layer, x: i32, y: i32, w: i32, px: f32, weight: Weight, col: Rgba, text: &str) -> i32 {
        let lh = (px * 1.3).round() as i32;
        let mut y = y;
        for l in wrap_px(text, px, weight, w) {
            c.text(x, y, px, weight, col, &l);
            y += lh;
        }
        y
    }

    /// Game files card; returns its bottom.
    fn files_card(&self, c: &mut Layer, x: i32, y: i32, w: i32, k: f32) -> i32 {
        let d = |v: f32| (v * k).round() as i32;
        let pad = d(14.0);
        let iw = w - 2 * pad;
        // Measure first (draw into a scratch layer is not possible): two passes with the same text.
        let ok = self.ready();
        let state = if ok {
            format!("Ready: {} found", REQUIRED_PARTS.join(", "))
        } else if self.files.missing.len() == REQUIRED_PARTS.len() {
            "No game files yet. Copy your own Settlers III installation (see Game files).".to_string()
        } else {
            format!("Still missing: {}", self.files.missing.join(", "))
        };
        let lines: Vec<(f32, Weight, Rgba, String)> = vec![
            (17.0, Weight::Bold, TEXT, "Game files".into()),
            (15.0, Weight::Bold, if ok { GOOD } else { BAD }, state),
            (14.0, Weight::Regular, DIM, format!("Version: {}", self.files.version)),
            (14.0, Weight::Regular, DIM, format!("Folder: {}", user_visible_path(&self.game_dir))),
        ];
        let mut hgt = 2 * pad - d(4.0);
        for (px, wt, _, t) in &lines {
            hgt += wrap_px(t, px * k, *wt, iw).len() as i32 * (px * k * 1.3).round() as i32 + d(4.0);
        }
        c.fill_rounded_rect(x, y, w, hgt, 10.0 * k, CARD);
        let mut yy = y + pad;
        for (px, wt, col, t) in &lines {
            yy = Self::para(c, x + pad, yy, iw, px * k, *wt, *col, t) + d(4.0);
        }
        y + hgt
    }

    fn draw_home(&mut self, c: &mut Layer, y: i32, w: i32, k: f32) -> i32 {
        let d = |v: f32| (v * k).round() as i32;
        let m = d(16.0);
        let gap = d(10.0);
        let wide = w as f32 / k >= 640.0;
        let (lx, lw, rx, rw) = if wide {
            let lw = (w - 3 * m) * 55 / 100;
            (m, lw, 2 * m + lw, w - 3 * m - lw)
        } else {
            (m, w - 2 * m, m, w - 2 * m)
        };
        let mut ly = self.files_card(c, lx, y, lw, k) + gap;
        if let Some((msg, err)) = &self.message {
            ly = Self::para(c, lx + d(4.0), ly + d(4.0), lw - d(8.0), 14.0 * k, Weight::Regular, if *err { BAD } else { DIM }, msg) + gap;
        }
        let mut ry = if wide { y } else { ly };
        let bh = d(52.0);
        let ok = self.ready();
        let play = Rect::new(rx, ry, rw, d(72.0));
        self.button(c, play, if ok { "Play" } else { "Play (game files missing)" }, if ok { PLAY } else { OFF }, if ok { Act::Play } else { Act::Page(Page::Files) }, k);
        ry += play.h + gap;
        let half = (rw - gap) / 2;
        self.button(c, Rect::new(rx, ry, half, bh), "Settings", BTN, Act::Page(Page::Settings), k);
        self.button(c, Rect::new(rx + half + gap, ry, rw - half - gap, bh), "Game files", BTN, Act::Page(Page::Files), k);
        ry += bh + gap;
        self.button(c, Rect::new(rx, ry, rw, bh), "Quit", DANGER, Act::Quit, k);
        ry += bh + gap;
        if !ok {
            ry = Self::para(c, rx, ry, rw, 14.0 * k, Weight::Regular, DIM, "Play starts once S3.EXE, GFX, SND and Map are in the game folder. \"Game files\" shows how to copy them (web upload from a PC, or adb).") + gap;
        }
        let note = "Not affiliated with Ubisoft / Blue Byte. No game files are included in this app.";
        let by = ly.max(ry);
        Self::para(c, m, by + d(4.0), w - 2 * m, 12.0 * k, Weight::Regular, DIM.fade(0.8), note)
    }

    fn draw_settings(&mut self, c: &mut Layer, y: i32, w: i32, k: f32) -> i32 {
        let d = |v: f32| (v * k).round() as i32;
        let m = d(16.0);
        let gap = d(8.0);
        let bh = d(48.0);
        let cols = if w as f32 / k >= 840.0 { 3 } else if w as f32 / k >= 560.0 { 2 } else { 1 };
        let cw = (w - 2 * m - (cols - 1) * gap) / cols;
        let s = self.settings.clone();
        let ui = if s.ui_mode == UiMode::MobileHud { "UI: Mobile HUD" } else { "UI: Classic Touch" };
        let fpsc = format!("FPS counter: {}", s.display.fps_counter.label());
        let items: Vec<(String, Act)> = vec![
            (ui.into(), Act::UiMode),
            (format!("Screen: {}", s.display.mode.label()), Act::Screen),
            (format!("Filter: {}", s.display.filter.label()), Act::Filter),
            (fpsc, Act::FpsCounter),
            (format!("FPS limit: {}", s.display.fps_limit), Act::FpsLimit),
            (format!("FPS position: {}", s.display.fps_pos.label()), Act::FpsPos),
            (format!("Fast speed: {}x", s.fast_speed), Act::FastSpeed),
            ((if s.show_game_time { "Game time: on" } else { "Game time: off" }).into(), Act::GameTime),
            (format!("Unit paths: {}", s.unit_paths.label()), Act::UnitPaths),
            ((if s.group_shortcuts { "Group buttons: on" } else { "Group buttons: off" }).into(), Act::Groups),
            (vibration_label(&s).into(), Act::Vibration),
        ];
        let mut yy = y;
        for (i, (label, a)) in items.iter().enumerate() {
            let col = i as i32 % cols;
            if i > 0 && col == 0 {
                yy += bh + gap;
            }
            self.button(c, Rect::new(m + col * (cw + gap), yy, cw, bh), label, BTN, *a, k);
        }
        // HUD size: in the rest of the last row if there is room, else its own row.
        let used = items.len() as i32 % cols;
        let (hx, hw) = if used != 0 { (m + used * (cw + gap), w - 2 * m - used * (cw + gap)) } else {
            yy += bh + gap;
            (m, w - 2 * m)
        };
        let third = (hw - 2 * gap) / 3;
        self.button(c, Rect::new(hx, yy, third, bh), "HUD -", BTN, Act::Hud(-1), k);
        c.text_centered(hx + third + gap, yy, third, bh, 15.0 * k, Weight::Bold, TEXT, &format!("HUD {:.0}%", s.hud_scale * 100.0));
        self.button(c, Rect::new(hx + 2 * (third + gap), yy, hw - 2 * (third + gap), bh), "HUD +", BTN, Act::Hud(1), k);
        yy += bh + d(18.0);
        // Relay URL.
        yy = Self::para(c, m, yy, w - 2 * m, 15.0 * k, Weight::Bold, TEXT, "Internet multiplayer relay") + d(2.0);
        let field = Rect::new(m, yy, w - 2 * m - d(120.0) - gap, bh);
        c.fill_rounded_rect(field.x, field.y, field.w, field.h, 6.0 * k, FIELD);
        c.stroke_rounded_rect(field.x, field.y, field.w, field.h, 6.0 * k, 2.0 * k.max(1.0), if self.editing { GOLD } else { BTN });
        let px = 15.0 * k;
        let shown = if self.relay.is_empty() && !self.editing { "default (built-in LAN relay)".to_string() } else { self.relay.clone() };
        // Keep the end (cursor) visible.
        let room = field.w - d(24.0);
        let mut t: &str = &shown;
        while measure_text(px, Weight::Regular, t).0 > room && !t.is_empty() {
            let mut it = t.char_indices();
            it.next();
            t = it.next().map_or("", |(i, _)| &t[i..]);
        }
        let th = measure_text(px, Weight::Regular, "Ag").1;
        let tw = c.text(field.x + d(10.0), field.y + (bh - th) / 2, px, Weight::Regular, if self.relay.is_empty() { DIM } else { TEXT }, t);
        if self.editing {
            let cx = field.x + d(10.0) + if self.relay.is_empty() { 0 } else { tw } + d(2.0);
            c.fill_rect(cx, field.y + d(10.0), d(2.0).max(2), bh - d(20.0), GOLD);
        }
        self.hits.push((field, Act::EditRelay));
        self.button(c, Rect::new(field.x + field.w + gap, yy, w - 2 * m - field.w - gap, bh), "Default", BTN, Act::RelayDefault, k);
        yy += bh + d(6.0);
        yy = Self::para(c, m, yy, w - 2 * m, 13.0 * k, Weight::Regular, DIM, "Tap the field to type (e.g. ws://my-server:7777/). LAN games need no relay.") + d(10.0);
        Self::para(c, m, yy, w - 2 * m, 13.0 * k, Weight::Regular, DIM, "Changes are saved at once and used when the game starts. The game's own menus change the same settings.")
    }

    fn draw_files(&mut self, c: &mut Layer, y: i32, w: i32, k: f32) -> i32 {
        let d = |v: f32| (v * k).round() as i32;
        let m = d(16.0);
        let gap = d(10.0);
        let bh = d(52.0);
        let running = self.server.is_some();
        // While the server runs, one status line instead of the card (the address must fit on the cover screen).
        let mut yy = if running {
            let (t, col) = if self.ready() {
                (format!("Game files: ready ({})", self.files.version), GOOD)
            } else {
                (format!("Game files still missing: {}", self.files.missing.join(", ")), BAD)
            };
            Self::para(c, m, y, w - 2 * m, 15.0 * k, Weight::Bold, col, &t) + gap
        } else {
            self.files_card(c, m, y, w - 2 * m, k) + gap
        };
        let wide = w as f32 / k >= 560.0;
        let bw = if wide { (w - 2 * m - gap) / 2 } else { w - 2 * m };
        self.button(c, Rect::new(m, yy, bw, bh), if running { "Stop upload server" } else { "Start upload server" }, if running { DANGER } else { PLAY }, if running { Act::UploadOff } else { Act::UploadOn }, k);
        let (rx, ry) = if wide { (m + bw + gap, yy) } else { (m, yy + bh + gap) };
        self.button(c, Rect::new(rx, ry, bw, bh), "Check again", BTN, Act::Recheck, k);
        yy = ry + bh + gap;
        if let Some(external) = self.folder_picker {
            // iOS: the game can run from a folder outside the app (kept when the app is deleted).
            self.button(c, Rect::new(m, yy, bw, bh), "Choose game folder...", BTN, Act::PickFolder, k);
            let (ax, ay) = if wide { (m + bw + gap, yy) } else { (m, yy + bh + gap) };
            if external {
                self.button(c, Rect::new(ax, ay, bw, bh), "Use app folder", BTN, Act::AppFolder, k);
                yy = ay + bh + gap;
            } else {
                yy += bh + gap;
            }
            let t = if external {
                format!("Game folder (picked): {}", self.game_dir.display())
            } else {
                "Game folder: the app's own folder (Files app: On My iPhone > S3 Mobile > Settlers3). It is \
                 deleted with the app; choose a folder of your own to keep the files."
                    .to_string()
            };
            yy = Self::para(c, m, yy, w - 2 * m, 13.0 * k, Weight::Regular, DIM, &t) + gap;
        }
        if let Some(e) = &self.server_err {
            yy = Self::para(c, m, yy, w - 2 * m, 14.0 * k, Weight::Regular, BAD, e) + gap;
        }
        if let (Some(url), Some(st)) = (self.upload_url(), self.upload_status()) {
            let qs = if self.qr.is_some() { d(150.0).min(w - 2 * m) } else { 0 };
            let tw = if wide && qs > 0 { w - 2 * m - qs - gap } else { w - 2 * m };
            let top = yy;
            yy = Self::para(c, m, yy, tw, 15.0 * k, Weight::Regular, TEXT, "On a PC (or tablet) in the same Wi-Fi network, open this address in a web browser:") + d(4.0);
            yy = Self::para(c, m, yy, tw, 20.0 * k, Weight::Bold, GOLD, &url) + d(6.0);
            yy = Self::para(c, m, yy, tw, 14.0 * k, Weight::Regular, DIM, "Then choose your Settlers III folder (the one with S3.EXE) and press Upload. Keep this page open until it is done.") + d(8.0);
            let prog = match &st.current {
                Some((p, a, b)) => format!("Receiving {p}: {} / {}", mb(*a), mb(*b)),
                None if st.files > 0 => "Waiting for the next file ...".into(),
                None => "Waiting for the browser ...".into(),
            };
            yy = Self::para(c, m, yy, tw, 15.0 * k, Weight::Bold, TEXT, &format!("Received: {} files, {}", st.files, mb(st.bytes))) + d(2.0);
            yy = Self::para(c, m, yy, tw, 14.0 * k, Weight::Regular, DIM, &prog) + d(2.0);
            if let Some(e) = &st.error {
                yy = Self::para(c, m, yy, tw, 14.0 * k, Weight::Regular, BAD, &format!("Last error: {e}")) + d(2.0);
            }
            if self.ip.is_none() {
                yy = Self::para(c, m, yy, tw, 14.0 * k, Weight::Regular, BAD, "No Wi-Fi address found: connect the phone to Wi-Fi (or turn on its hotspot) and start the server again.") + d(2.0);
            }
            if let Some((_, img)) = &self.qr {
                let (qx, qy) = if wide && qs > 0 { (w - m - qs, top) } else { (m, yy + gap) };
                c.fill_rounded_rect(qx, qy, qs, qs, 6.0 * k, Rgba::WHITE);
                let inset = d(8.0);
                c.blit_scaled(img, qx + inset, qy + inset, qs - 2 * inset, qs - 2 * inset, 1.0);
                if !wide {
                    yy = qy + qs;
                } else {
                    yy = yy.max(qy + qs);
                }
            }
            yy += gap;
        }
        yy = Self::para(c, m, yy + d(6.0), w - 2 * m, 15.0 * k, Weight::Bold, TEXT, "Other ways") + d(2.0);
        let adb = if self.folder_picker.is_some() {
            "Files app: copy the contents of your Settlers III folder (S3.EXE, GFX, SND, Map, ...) into the game \
             folder, e.g. from a USB drive, a zip or iCloud Drive. Files already there are kept; the upload overwrites \
             files with the same name."
                .to_string()
        } else {
            format!(
                "From a PC with adb: adb push \"<Settlers III folder>/.\" /sdcard/{}\nOr copy the folder over USB (MTP) into {}. Files already there are kept; the upload overwrites files with the same name.",
                user_visible_path(&self.game_dir),
                user_visible_path(&self.game_dir)
            )
        };
        let mut yy2 = yy;
        for l in adb.split('\n') {
            yy2 = Self::para(c, m, yy2, w - 2 * m, 13.0 * k, Weight::Regular, DIM, l) + d(4.0);
        }
        yy2
    }
}

fn mb(n: u64) -> String {
    format!("{:.1} MB", n as f64 / 1048576.0)
}

/// Greedy word wrap by measured width (long words are split by characters).
pub fn wrap_px(text: &str, px: f32, weight: Weight, max_w: i32) -> Vec<String> {
    let fits = |s: &str| measure_text(px, weight, s).0 <= max_w;
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            let cand = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if fits(&cand) {
                line = cand;
                continue;
            }
            if !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            if fits(word) {
                line = word.to_string();
            } else {
                for ch in word.chars() {
                    let mut t = line.clone();
                    t.push(ch);
                    if !fits(&t) && !line.is_empty() {
                        out.push(std::mem::take(&mut line));
                    }
                    line.push(ch);
                }
            }
        }
        out.push(line);
    }
    out
}

/// QR code of `text` as a black-on-white image with a 2-module quiet zone (the card around it adds more).
pub fn qr_image(text: &str) -> Option<Image> {
    let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::M).ok()?;
    let n = code.width() as u32;
    let q = 2u32;
    let size = n + 2 * q;
    let mut img = Image::new(size, size);
    img.rgba.fill(255);
    let colors = code.to_colors();
    for y in 0..n {
        for x in 0..n {
            if colors[(y * n + x) as usize] == qrcode::Color::Dark {
                let i = (((y + q) * size + x + q) * 4) as usize;
                img.rgba[i..i + 3].fill(0);
            }
        }
    }
    Some(img)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(tag: &str) -> (PathBuf, PathBuf) {
        let d = std::env::temp_dir().join(format!("s3launcher-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        (d.join("game"), d.join("data"))
    }

    fn tap(l: &mut Launcher, a: Act) -> Option<Request> {
        let r = l.hits.iter().find(|h| h.1 == a).map(|h| h.0).expect("button shown");
        let (x, y) = (r.x + r.w / 2, r.y + r.h / 2);
        l.touch(Touch::Down, x, y);
        l.touch(Touch::Up, x, y)
    }

    #[test]
    fn home_settings_and_back() {
        let (g, d) = dirs("nav");
        let mut l = Launcher::new(g.clone(), d.clone());
        assert!(!l.ready());
        for (w, h) in [(2316, 904), (2176, 1812), (904, 2316)] {
            let list = l.draw(w, h, 420);
            assert_eq!((list.width, list.height), (w, h));
        }
        l.draw(2316, 904, 420);
        // Play is disabled: it opens the Game files page.
        assert_eq!(tap(&mut l, Act::Page(Page::Files)), None);
        assert_eq!(l.page, Page::Files);
        assert_eq!(l.back(), (true, None));
        assert_eq!(l.page, Page::Home);
        assert_eq!(l.back(), (false, None));
        l.draw(2316, 904, 420);
        tap(&mut l, Act::Page(Page::Settings));
        l.draw(2316, 904, 420);
        tap(&mut l, Act::UiMode);
        assert_eq!(Settings::load(&d).0.ui_mode, UiMode::MobileHud);
        l.draw(2316, 904, 420);
        // Relay field: type, Enter saves.
        assert_eq!(tap(&mut l, Act::EditRelay), Some(Request::Keyboard(true)));
        for ch in "ws://x:1/".chars() {
            l.char(ch as u32);
        }
        l.char(0x08);
        assert_eq!(l.char(0x0D), Some(Request::Keyboard(false)));
        assert_eq!(std::fs::read_to_string(d.join(RELAY_FILE)).unwrap(), "ws://x:1\n");
        let _ = std::fs::remove_dir_all(g.parent().unwrap());
    }

    #[test]
    fn ready_when_parts_present() {
        let (g, d) = dirs("ready");
        for p in ["GFX", "Snd", "MAP"] {
            std::fs::create_dir_all(g.join(p)).unwrap();
        }
        std::fs::write(g.join("s3.exe"), b"MZ").unwrap();
        let mut l = Launcher::new(g.clone(), d);
        assert!(l.ready());
        assert!(l.files.version.starts_with("unknown S3.EXE"), "{}", l.files.version);
        l.draw(2316, 904, 420);
        assert_eq!(tap(&mut l, Act::Play), Some(Request::Play));
        let _ = std::fs::remove_dir_all(g.parent().unwrap());
    }

    /// Writes PNGs of every page at the Fold4 sizes into `$S3_LAUNCHER_PREVIEW` (no game files needed).
    #[test]
    #[ignore]
    fn launcher_preview() {
        let Some(out) = std::env::var_os("S3_LAUNCHER_PREVIEW").map(PathBuf::from) else { return };
        std::fs::create_dir_all(&out).unwrap();
        let (g, d) = dirs("preview");
        let mut l = Launcher::new(g.clone(), d);
        l.message = Some(("The game was closed. Tap Play to start it again.".into(), false));
        for (name, w, h, dpi) in [("cover", 2316u32, 904u32, 420u32), ("inner", 2176, 1812, 373)] {
            for page in [Page::Home, Page::Settings, Page::Files] {
                l.page = page;
                if page == Page::Files && l.server.is_none() {
                    l.start_upload();
                }
                let list = Arc::new(l.draw(w, h, dpi));
                let mut r = s3_mods::LayerRaster::new();
                r.update(&list);
                let rgb: Vec<u8> = r.rgba.chunks(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
                let f = std::fs::File::create(out.join(format!("launcher-{name}-{page:?}.png").to_lowercase())).unwrap();
                let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w, h);
                enc.set_color(png::ColorType::Rgb);
                enc.set_depth(png::BitDepth::Eight);
                enc.write_header().unwrap().write_image_data(&rgb).unwrap();
            }
        }
        l.stop_upload();
        let _ = std::fs::remove_dir_all(g.parent().unwrap());
    }

    #[test]
    fn wraps_and_qr() {
        let l = wrap_px("aaa bbb ccc", 20.0, Weight::Regular, measure_text(20.0, Weight::Regular, "aaa bbb").0);
        assert_eq!(l, vec!["aaa bbb", "ccc"]);
        let img = qr_image("http://192.168.1.20:8080/?k=abcdef").unwrap();
        assert!(img.width >= 25 && img.width == img.height);
    }
}
