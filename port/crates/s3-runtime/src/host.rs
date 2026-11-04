//! The interface between the emulated machine and the platform frontend (desktop window, Android activity,
//! headless test runner).

use std::path::PathBuf;

/// A presented frame. Always RGB565 (`0bRRRRRGGGGGGBBBBB`); 8-bit palettized surfaces are converted by s3-dx.
#[derive(Debug, Clone, Default)]
pub struct FrameBuffer {
    pub width: u32,
    pub height: u32,
    /// `width * height` pixels, row-major, no padding.
    pub pixels: Vec<u16>,
}

impl FrameBuffer {
    pub fn new(width: u32, height: u32) -> Self {
        FrameBuffer { width, height, pixels: vec![0; (width * height) as usize] }
    }

    /// Convert to packed 8-bit RGB triples.
    pub fn to_rgb888(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pixels.len() * 3);
        for &p in &self.pixels {
            let (r, g, b) = rgb565_to_rgb888(p);
            out.extend_from_slice(&[r, g, b]);
        }
        out
    }
}

/// Expand one RGB565 pixel.
#[inline]
pub fn rgb565_to_rgb888(p: u16) -> (u8, u8, u8) {
    let r = ((p >> 11) & 0x1F) as u8;
    let g = ((p >> 5) & 0x3F) as u8;
    let b = (p & 0x1F) as u8;
    ((r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2))
}

/// Mouse buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Input / lifecycle events from the frontend. Coordinates are in game-screen pixels (the frontend undoes
/// its scaling/letterboxing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostEvent {
    MouseMove { x: i32, y: i32 },
    MouseButton { button: MouseButton, down: bool },
    /// Windows virtual-key code (VK_*).
    Key { vk: u32, down: bool },
    /// Text input (WM_CHAR), already translated.
    Char(u32),
    Quit,
    Pause,
    Resume,
}

/// Log levels for [`Host::log`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

/// Platform services. Implemented by s3-desktop (`s3run`), s3-android and tests.
pub trait Host: Send {
    /// Show a frame (called on DirectDraw Flip / primary-surface Blt).
    fn present(&mut self, fb: &FrameBuffer);
    /// Drain pending input / lifecycle events.
    fn poll_events(&mut self) -> Vec<HostEvent>;
    /// Queue interleaved PCM for playback.
    fn queue_audio(&mut self, _samples: &[i16], _rate: u32, _channels: u16) {}
    /// Frames queued in the host audio sink and not yet played (for mixer pacing). 0 if unknown.
    fn audio_queued_frames(&self) -> usize {
        0
    }
    /// Monotonic milliseconds (time base for GetTickCount/timeGetTime).
    fn now_ms(&self) -> u64;
    /// Read-only directory holding the user's game files (S3.EXE, GFX/, SND/, ...).
    fn game_dir(&self) -> PathBuf;
    /// Writable directory: saves, registry JSON, config, overlay of written files.
    fn data_dir(&self) -> PathBuf;
    /// Log a message.
    fn log(&self, level: LogLevel, msg: &str) {
        eprintln!("[{level:?}] {msg}");
    }
    /// Called when every guest thread is blocked; sleep a little instead of spinning.
    fn idle(&mut self, ms: u32) {
        std::thread::sleep(std::time::Duration::from_millis(ms as u64));
    }
}

/// Headless host for tests and `s3run --headless`: discards frames (but keeps the last one), no input.
pub struct NullHost {
    pub game_dir: PathBuf,
    pub data_dir: PathBuf,
    pub start: std::time::Instant,
    pub last_frame: Option<FrameBuffer>,
    pub presents: u64,
    pub events: Vec<HostEvent>,
    pub quiet: bool,
}

impl NullHost {
    pub fn new(game_dir: PathBuf, data_dir: PathBuf) -> Self {
        NullHost {
            game_dir,
            data_dir,
            start: std::time::Instant::now(),
            last_frame: None,
            presents: 0,
            events: Vec::new(),
            quiet: false,
        }
    }
}

impl Host for NullHost {
    fn present(&mut self, fb: &FrameBuffer) {
        self.presents += 1;
        self.last_frame = Some(fb.clone());
    }
    fn poll_events(&mut self) -> Vec<HostEvent> {
        std::mem::take(&mut self.events)
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
        if !self.quiet || level <= LogLevel::Warn {
            eprintln!("[{level:?}] {msg}");
        }
    }
}
