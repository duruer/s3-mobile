//! Extra PCM streams mixed into the one audio output (added by the directx agent, 2026-11-05).
//!
//! The DirectSound software mixer (s3-dx `dsound.rs`) is the only caller of `Host::queue_audio` (44100 Hz
//! stereo i16). Other sound sources (winmm waveOut / CD-audio music from `MUSIC\TrackNN.ogg`) push interleaved
//! PCM into `m.state::<AudioStreams>()`; the mixer pulls, resamples (linear) and mixes it. The mixer service is
//! registered by `s3_dx::register_factories`, so it runs even if the game never creates DirectSound.
//! Producers should keep roughly 100-300 ms queued (`AudioStream::queued_frames`) and top up from a service.

use std::collections::{HashMap, VecDeque};

/// One PCM stream fed by a non-DirectSound producer.
#[derive(Debug, Default)]
pub struct AudioStream {
    /// Sample rate of `samples`.
    pub rate: u32,
    /// 1 or 2.
    pub channels: u16,
    /// Interleaved signed 16-bit samples, consumed from the front by the mixer.
    pub samples: VecDeque<i16>,
    /// Linear gain 0.0..=1.0 (1.0 if left at 0.0 and `gain_set` is false).
    pub gain: f32,
    pub gain_set: bool,
}

impl AudioStream {
    /// Frames (per-channel samples) still queued.
    pub fn queued_frames(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }
}

/// Extra audio streams mixed by the DirectSound mixer, keyed by a producer-chosen id.
#[derive(Debug, Default)]
pub struct AudioStreams {
    pub streams: HashMap<u32, AudioStream>,
}
