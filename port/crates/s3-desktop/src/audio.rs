//! Desktop audio sink for `s3run`: everything the game sends to `Host::queue_audio` (DirectSound mixer output,
//! which also carries the MCI CD music) can be
//! - written to a 16-bit PCM WAV file (`--audio-wav <file>`, bounded by `--audio-wav-seconds`, default 300 s),
//! - played on the default output device with cpal (`--audio-play`, only with the cargo feature `audio-cpal`),
//! - checked for timing: every `queue_audio` call is compared with the wall clock (lead = audio produced minus
//!   time elapsed since the first block). A healthy mixer keeps the lead flat (~LATENCY_MS); a growing lead means
//!   two producers or a too-fast mixer (drift), a negative/shrinking lead means underruns (gaps).

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Output format the sink expects (the s3-dx mixer's format). Other rates are converted (linear) on the fly.
pub const SINK_RATE: u32 = 44_100;

/// WAV writer that keeps a valid header on disk (patched every second of audio and at the end), so a crashed or
/// killed run still leaves a readable file.
pub struct WavWriter {
    path: PathBuf,
    w: BufWriter<File>,
    rate: u32,
    channels: u16,
    frames: u64,
    max_frames: u64,
    since_patch: u64,
}

fn wav_header(rate: u32, channels: u16, data_bytes: u32) -> [u8; 44] {
    let mut h = [0u8; 44];
    let ba = channels as u32 * 2;
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(36u32.saturating_add(data_bytes)).to_le_bytes());
    h[8..12].copy_from_slice(b"WAVE");
    h[12..16].copy_from_slice(b"fmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes());
    h[22..24].copy_from_slice(&channels.to_le_bytes());
    h[24..28].copy_from_slice(&rate.to_le_bytes());
    h[28..32].copy_from_slice(&(rate * ba).to_le_bytes());
    h[32..34].copy_from_slice(&(ba as u16).to_le_bytes());
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data_bytes.to_le_bytes());
    h
}

impl WavWriter {
    pub fn create(path: &Path, rate: u32, channels: u16, max_seconds: f64) -> std::io::Result<WavWriter> {
        let mut w = BufWriter::new(File::create(path)?);
        w.write_all(&wav_header(rate, channels, 0))?;
        Ok(WavWriter {
            path: path.to_path_buf(),
            w,
            rate,
            channels,
            frames: 0,
            max_frames: (max_seconds.max(0.0) * rate as f64) as u64,
            since_patch: 0,
        })
    }

    pub fn full(&self) -> bool {
        self.frames >= self.max_frames
    }

    /// Append interleaved samples (must match the writer's channel count); stops at the length bound.
    pub fn write(&mut self, samples: &[i16]) -> std::io::Result<()> {
        let ch = self.channels as usize;
        let room = self.max_frames.saturating_sub(self.frames) as usize;
        let n = (samples.len() / ch).min(room);
        if n == 0 {
            return Ok(());
        }
        let mut bytes = Vec::with_capacity(n * ch * 2);
        for s in &samples[..n * ch] {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        self.w.write_all(&bytes)?;
        self.frames += n as u64;
        self.since_patch += n as u64;
        if self.since_patch >= self.rate as u64 || self.full() {
            self.since_patch = 0;
            self.patch_header()?;
        }
        Ok(())
    }

    fn patch_header(&mut self) -> std::io::Result<()> {
        let data = (self.frames * self.channels as u64 * 2).min(u32::MAX as u64 - 36) as u32;
        self.w.flush()?;
        let f = self.w.get_mut();
        f.seek(SeekFrom::Start(0))?;
        f.write_all(&wav_header(self.rate, self.channels, data))?;
        f.seek(SeekFrom::End(0))?;
        Ok(())
    }

    pub fn finish(&mut self) -> std::io::Result<()> {
        self.patch_header()?;
        self.w.flush()
    }

    pub fn seconds(&self) -> f64 {
        self.frames as f64 / self.rate as f64
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Timing statistics of the `queue_audio` stream against the wall clock.
#[derive(Debug, Default, Clone)]
pub struct Timing {
    pub calls: u64,
    pub frames: u64,
    pub first_ms: Option<u64>,
    pub last_ms: u64,
    /// Lead (ms of audio produced minus ms elapsed since the first call), sampled after each call.
    pub lead_min: f64,
    pub lead_max: f64,
    pub lead_last: f64,
    /// Lead seen ~2 s after the first call (reference for drift).
    pub lead_ref: Option<f64>,
    /// Largest wall-clock gap between two calls (ms).
    pub max_gap_ms: u64,
    /// Calls with a format other than SINK_RATE stereo.
    pub other_format_calls: u64,
}

impl Timing {
    pub fn record(&mut self, now_ms: u64, frames: usize, rate: u32) {
        let first = *self.first_ms.get_or_insert(now_ms);
        if self.calls > 0 {
            self.max_gap_ms = self.max_gap_ms.max(now_ms.saturating_sub(self.last_ms));
        }
        self.calls += 1;
        self.last_ms = now_ms;
        self.frames += frames as u64;
        let lead = self.frames as f64 * 1000.0 / rate.max(1) as f64 - (now_ms - first) as f64;
        if self.calls == 1 {
            self.lead_min = lead;
            self.lead_max = lead;
        }
        self.lead_min = self.lead_min.min(lead);
        self.lead_max = self.lead_max.max(lead);
        self.lead_last = lead;
        if self.lead_ref.is_none() && now_ms - first >= 2000 {
            self.lead_ref = Some(lead);
        }
    }

    pub fn summary(&self) -> String {
        let span = self.last_ms.saturating_sub(self.first_ms.unwrap_or(0));
        format!(
            "audio timing: {} calls, {:.2} s audio over {:.2} s wall, lead min/max/last {:.0}/{:.0}/{:.0} ms, \
             drift since 2 s {:+.0} ms, max gap between calls {} ms, other-format calls {}",
            self.calls,
            self.frames as f64 / SINK_RATE as f64,
            span as f64 / 1000.0,
            self.lead_min,
            self.lead_max,
            self.lead_last,
            self.lead_last - self.lead_ref.unwrap_or(self.lead_last),
            self.max_gap_ms,
            self.other_format_calls,
        )
    }
}

/// Convert any (rate, channels) block to SINK_RATE stereo (linear interpolation, per-call; only used if a
/// producer ever sends another format, which the s3-dx mixer does not).
fn to_sink_format(samples: &[i16], rate: u32, channels: u16) -> Vec<i16> {
    let ch = channels.max(1) as usize;
    let frames = samples.len() / ch;
    if frames == 0 {
        return Vec::new();
    }
    let get = |f: usize, c: usize| samples[f.min(frames - 1) * ch + c.min(ch - 1)] as f32;
    let out_frames = (frames as u64 * SINK_RATE as u64 / rate.max(1) as u64) as usize;
    let step = rate as f64 / SINK_RATE as f64;
    let mut out = Vec::with_capacity(out_frames * 2);
    for i in 0..out_frames {
        let p = i as f64 * step;
        let k = p as usize;
        let t = (p - k as f64) as f32;
        for c in 0..2 {
            out.push((get(k, c) * (1.0 - t) + get(k + 1, c) * t) as i16);
        }
    }
    out
}

/// Everything the desktop host does with audio. Shared (Arc<Mutex>) between the host and `main` (summary).
pub struct AudioSink {
    pub wav: Option<WavWriter>,
    pub timing: Timing,
    pub start: Instant,
    #[cfg(feature = "audio-cpal")]
    pub player: Option<player::Player>,
}

impl AudioSink {
    pub fn new(wav: Option<WavWriter>, play: bool) -> AudioSink {
        #[cfg(feature = "audio-cpal")]
        let player = if play {
            match player::Player::start() {
                Ok(p) => Some(p),
                Err(e) => {
                    eprintln!("s3run: audio playback unavailable: {e}");
                    None
                }
            }
        } else {
            None
        };
        #[cfg(not(feature = "audio-cpal"))]
        if play {
            eprintln!("s3run: --audio-play needs a build with `--features audio-cpal`; ignoring");
        }
        AudioSink {
            wav,
            timing: Timing::default(),
            start: Instant::now(),
            #[cfg(feature = "audio-cpal")]
            player,
        }
    }

    pub fn queue(&mut self, samples: &[i16], rate: u32, channels: u16) {
        let now = self.start.elapsed().as_millis() as u64;
        let ch = channels.max(1) as usize;
        self.timing.record(now, samples.len() / ch, rate);
        let conv;
        let samples = if rate == SINK_RATE && channels == 2 {
            samples
        } else {
            self.timing.other_format_calls += 1;
            conv = to_sink_format(samples, rate, channels);
            &conv[..]
        };
        if let Some(w) = &mut self.wav {
            if let Err(e) = w.write(samples) {
                eprintln!("s3run: audio wav write failed: {e}; stopping the recording");
                self.wav = None;
            }
        }
        #[cfg(feature = "audio-cpal")]
        if let Some(p) = &self.player {
            p.push(samples);
        }
    }

    /// Frames waiting in the playback device queue (0 without playback).
    pub fn queued_frames(&self) -> usize {
        #[cfg(feature = "audio-cpal")]
        if let Some(p) = &self.player {
            return p.queued_frames();
        }
        0
    }

    pub fn finish(&mut self) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(w) = &mut self.wav {
            match w.finish() {
                Ok(()) => lines.push(format!("s3run: audio wav {} ({:.1} s)", w.path().display(), w.seconds())),
                Err(e) => lines.push(format!("s3run: audio wav finish failed: {e}")),
            }
        }
        #[cfg(feature = "audio-cpal")]
        if let Some(p) = &self.player {
            lines.push(format!("s3run: audio playback underrun frames {}", p.underruns()));
        }
        if self.timing.calls > 0 {
            lines.push(format!("s3run: {}", self.timing.summary()));
        }
        lines
    }
}

pub type SharedSink = Arc<Mutex<AudioSink>>;

#[cfg(feature = "audio-cpal")]
mod player {
    //! Real playback through cpal (ALSA on Linux). A lock-protected FIFO between `queue_audio` (engine thread)
    //! and the device callback; the queue is bounded (excess is dropped) so a stalled device cannot grow memory.
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    use super::SINK_RATE;

    /// At most 1 s queued.
    const MAX_QUEUED: usize = SINK_RATE as usize * 2;

    pub struct Player {
        queue: Arc<Mutex<VecDeque<i16>>>,
        underruns: Arc<AtomicU64>,
        out_channels: usize,
        _stream: cpal::Stream,
    }

    // cpal::Stream is !Send on some platforms; the sink is only touched from the engine thread, but the host
    // trait object requires Send.
    unsafe impl Send for Player {}

    impl Player {
        pub fn start() -> Result<Player, String> {
            let host = cpal::default_host();
            let dev = host.default_output_device().ok_or("no default output device")?;
            let def = dev.default_output_config().map_err(|e| e.to_string())?;
            let channels = def.channels().max(1);
            let cfg = cpal::StreamConfig {
                channels,
                sample_rate: cpal::SampleRate(SINK_RATE),
                buffer_size: cpal::BufferSize::Default,
            };
            let queue: Arc<Mutex<VecDeque<i16>>> = Arc::new(Mutex::new(VecDeque::new()));
            let underruns = Arc::new(AtomicU64::new(0));
            let (q, u) = (queue.clone(), underruns.clone());
            let och = channels as usize;
            let stream = dev
                .build_output_stream(
                    &cfg,
                    move |data: &mut [f32], _| {
                        let mut q = q.lock().unwrap();
                        let mut missing = 0u64;
                        for frame in data.chunks_mut(och) {
                            let (l, r) = match (q.pop_front(), q.pop_front()) {
                                (Some(l), Some(r)) => (l, r),
                                _ => {
                                    missing += 1;
                                    (0, 0)
                                }
                            };
                            let (l, r) = (l as f32 / 32768.0, r as f32 / 32768.0);
                            match frame.len() {
                                1 => frame[0] = (l + r) * 0.5,
                                _ => {
                                    frame[0] = l;
                                    frame[1] = r;
                                    for x in frame.iter_mut().skip(2) {
                                        *x = 0.0;
                                    }
                                }
                            }
                        }
                        if missing > 0 {
                            u.fetch_add(missing, Ordering::Relaxed);
                        }
                    },
                    |e| eprintln!("s3run: audio stream error: {e}"),
                    None,
                )
                .map_err(|e| e.to_string())?;
            stream.play().map_err(|e| e.to_string())?;
            Ok(Player { queue, underruns, out_channels: och, _stream: stream })
        }

        pub fn push(&self, samples: &[i16]) {
            let _ = self.out_channels;
            let mut q = self.queue.lock().unwrap();
            q.extend(samples.iter().copied());
            if q.len() > MAX_QUEUED {
                let excess = q.len() - MAX_QUEUED;
                q.drain(..excess & !1);
            }
        }

        pub fn queued_frames(&self) -> usize {
            self.queue.lock().unwrap().len() / 2
        }

        pub fn underruns(&self) -> u64 {
            self.underruns.load(Ordering::Relaxed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_is_bounded_and_valid() {
        let dir = std::env::temp_dir().join(format!("s3run-wav-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.wav");
        let mut w = WavWriter::create(&p, 1000, 2, 0.5).unwrap();
        w.write(&vec![7i16; 600 * 2]).unwrap();
        assert!(w.full());
        w.write(&[1, 2]).unwrap();
        w.finish().unwrap();
        let b = std::fs::read(&p).unwrap();
        assert_eq!(b.len(), 44 + 500 * 4);
        assert_eq!(&b[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(b[40..44].try_into().unwrap()), 2000);
        assert_eq!(u32::from_le_bytes(b[4..8].try_into().unwrap()), 2036);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn timing_tracks_lead_and_drift() {
        let mut t = Timing::default();
        // 60 ms ahead at start, then exactly real time: no drift.
        t.record(0, 2646, SINK_RATE);
        for i in 1..=300 {
            t.record(i * 10, 441, SINK_RATE);
        }
        assert!((t.lead_last - 60.0).abs() < 1.0, "{}", t.lead_last);
        assert!((t.lead_last - t.lead_ref.unwrap()).abs() < 1.0);
        // A second producer doubles the audio: lead grows.
        let mut t2 = t.clone();
        for i in 301..=400 {
            t2.record(i * 10, 882, SINK_RATE);
        }
        assert!(t2.lead_last > t.lead_last + 900.0);
    }

    #[test]
    fn converts_other_formats() {
        let mono22 = vec![1000i16; 2205];
        let out = to_sink_format(&mono22, 22_050, 1);
        assert_eq!(out.len(), 4410 * 2);
        assert!(out.iter().all(|&s| s == 1000));
    }
}
