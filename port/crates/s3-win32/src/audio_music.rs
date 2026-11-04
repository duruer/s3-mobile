//! CD-audio emulation for MCI `cdaudio`: track N plays `MUSIC\TrackNN.ogg` (the GOG `_inmm.dll` convention,
//! track 1 is the data track and has no file). Decoded with lewton (pure Rust Ogg Vorbis).
//!
//! Output: [`MusicPlayer::mix`] *adds* music into an interleaved i16 buffer at any rate/channel count (linear
//! resampling). By default winmm runs a scheduler service that queues music itself to `Host::queue_audio` at
//! 44100 Hz stereo, paced by the wall clock. If another mixer (DirectSound in s3-dx) owns the host audio stream,
//! it should call `winmm::set_external_music_mixer(m, true)` once and `winmm::mix_music(m, buf, rate, channels)`
//! for every block it produces, so there is only one producer for `queue_audio`.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use lewton::inside_ogg::OggStreamReader;

/// Default music volume (0..1). HYPOTHESIS: the real CD volume is set by the user in the Windows mixer.
pub const DEFAULT_MUSIC_VOLUME: f32 = 0.6;

/// A playing Ogg track.
pub struct Track {
    reader: OggStreamReader<BufReader<File>>,
    pub rate: u32,
    pub channels: u16,
    /// Decoded, not yet consumed interleaved samples.
    pending: Vec<i16>,
    pos: usize,
    /// Fractional source position for resampling.
    frac: f64,
    eof: bool,
}

impl Track {
    pub fn open(path: &Path) -> Result<Track, String> {
        let f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let reader = OggStreamReader::new(BufReader::new(f)).map_err(|e| format!("{}: {e}", path.display()))?;
        let rate = reader.ident_hdr.audio_sample_rate;
        let channels = reader.ident_hdr.audio_channels as u16;
        Ok(Track { reader, rate, channels, pending: Vec::new(), pos: 0, frac: 0.0, eof: false })
    }

    /// Next source frame as (left, right); None at the end.
    fn frame(&mut self, idx: usize) -> Option<(i16, i16)> {
        let ch = self.channels.max(1) as usize;
        while self.pos + (idx + 1) * ch > self.pending.len() {
            if self.eof {
                return None;
            }
            // Drop consumed samples, then decode another packet.
            if self.pos > 0 {
                self.pending.drain(..self.pos);
                self.pos = 0;
            }
            match self.reader.read_dec_packet_itl() {
                Ok(Some(p)) => self.pending.extend_from_slice(&p),
                Ok(None) | Err(_) => self.eof = true,
            }
        }
        let i = self.pos + idx * ch;
        let l = self.pending[i];
        let r = if ch > 1 { self.pending[i + 1] } else { l };
        Some((l, r))
    }

    /// Add up to `frames` frames into `out` (interleaved, `channels`) at `rate` and `volume`. Returns frames
    /// written (fewer at the end of the track).
    pub fn mix_into(&mut self, out: &mut [i16], channels: u16, rate: u32, volume: f32) -> usize {
        let och = channels.max(1) as usize;
        let frames = out.len() / och;
        let step = self.rate as f64 / rate.max(1) as f64;
        let ch = self.channels.max(1) as usize;
        for f in 0..frames {
            let i0 = self.frac.floor() as usize;
            let t = (self.frac - i0 as f64) as f32;
            let Some(a) = self.frame(i0) else { return f };
            let b = self.frame(i0 + 1).unwrap_or(a);
            let l = (a.0 as f32 * (1.0 - t) + b.0 as f32 * t) * volume;
            let r = (a.1 as f32 * (1.0 - t) + b.1 as f32 * t) * volume;
            let base = f * och;
            if och == 1 {
                out[base] = out[base].saturating_add(((l + r) * 0.5) as i16);
            } else {
                out[base] = out[base].saturating_add(l as i16);
                out[base + 1] = out[base + 1].saturating_add(r as i16);
            }
            self.frac += step;
            let adv = self.frac.floor() as usize;
            if adv > 0 {
                self.pos += adv * ch;
                self.frac -= adv as f64;
            }
        }
        frames
    }
}

/// Duration of an Ogg Vorbis file in ms from the last page's granule position (no decoding).
pub fn ogg_duration_ms(path: &Path) -> Option<u32> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    // Sample rate from the identification header (first page).
    let mut head = [0u8; 64];
    f.read_exact(&mut head).ok()?;
    let vi = head.windows(7).position(|w| w == b"\x01vorbis")?;
    let rate = u32::from_le_bytes(head.get(vi + 12..vi + 16)?.try_into().ok()?);
    let tail = len.min(65536);
    f.seek(SeekFrom::Start(len - tail)).ok()?;
    let mut buf = vec![0u8; tail as usize];
    f.read_exact(&mut buf).ok()?;
    let p = buf.windows(4).rposition(|w| w == b"OggS")?;
    let gran = u64::from_le_bytes(buf.get(p + 6..p + 14)?.try_into().ok()?);
    if rate == 0 {
        return None;
    }
    Some((gran * 1000 / rate as u64) as u32)
}

/// What the player reports after mixing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MusicEvent {
    /// The requested range finished (MCI notify successful).
    Finished,
}

/// CD player state: plays tracks `cur..end` (end exclusive).
pub struct MusicPlayer {
    pub track: Option<Track>,
    pub cur_track: u32,
    pub end_track: u32,
    pub paused: bool,
    pub volume: f32,
    pub track_started_ms: u64,
    resolve: Option<Box<dyn Fn(u32) -> Option<PathBuf> + Send>>,
}

impl Default for MusicPlayer {
    fn default() -> Self {
        MusicPlayer { track: None, cur_track: 0, end_track: 0, paused: false, volume: DEFAULT_MUSIC_VOLUME, track_started_ms: 0, resolve: None }
    }
}

impl MusicPlayer {
    /// Set the track -> file resolver.
    pub fn set_resolver(&mut self, f: Box<dyn Fn(u32) -> Option<PathBuf> + Send>) {
        self.resolve = Some(f);
    }

    pub fn is_playing(&self) -> bool {
        self.track.is_some() && !self.paused
    }

    /// Start playing tracks `from..to` (to exclusive). Returns an error text if no track could be opened.
    pub fn play(&mut self, from: u32, to: u32, now_ms: u64) -> Result<(), String> {
        self.stop();
        self.cur_track = from;
        self.end_track = to.max(from + 1);
        self.open_current(now_ms)
    }

    fn open_current(&mut self, now_ms: u64) -> Result<(), String> {
        while self.cur_track < self.end_track {
            let path = self.resolve.as_ref().and_then(|r| r(self.cur_track));
            if let Some(p) = path {
                match Track::open(&p) {
                    Ok(t) => {
                        self.track = Some(t);
                        self.paused = false;
                        self.track_started_ms = now_ms;
                        return Ok(());
                    }
                    Err(e) => return Err(e),
                }
            }
            self.cur_track += 1;
        }
        Err("no playable track in range".into())
    }

    pub fn stop(&mut self) {
        self.track = None;
        self.paused = false;
    }

    /// Mix music into `out`; returns Finished when the range ended during this block.
    pub fn mix(&mut self, out: &mut [i16], channels: u16, rate: u32, now_ms: u64) -> Option<MusicEvent> {
        if self.paused {
            return None;
        }
        let och = channels.max(1) as usize;
        let mut done = 0usize;
        let total = out.len() / och;
        while done < total {
            let Some(t) = self.track.as_mut() else { return None };
            let n = t.mix_into(&mut out[done * och..], channels, rate, self.volume);
            done += n;
            if done < total {
                // Track ended: next one in range, or finished.
                self.track = None;
                self.cur_track += 1;
                if self.cur_track >= self.end_track || self.open_current(now_ms).is_err() {
                    return Some(MusicEvent::Finished);
                }
            }
        }
        None
    }
}
