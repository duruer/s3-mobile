//! Software audio mixer core (pure functions, unit-tested): PCM voice resampling with volume/pan, DirectSound
//! dB conversions, i32 -> i16 clamping.

/// Output format of the mixer: 44100 Hz, stereo, i16.
pub const OUT_RATE: u32 = 44_100;

/// PCM format of a voice (WAVEFORMATEX subset).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PcmFormat {
    pub rate: u32,
    /// 1 or 2.
    pub channels: u16,
    /// 8 (unsigned) or 16 (signed LE).
    pub bits: u16,
}

impl Default for PcmFormat {
    fn default() -> Self {
        PcmFormat { rate: 22_050, channels: 2, bits: 16 }
    }
}

impl PcmFormat {
    /// Bytes per frame.
    pub fn block_align(&self) -> u32 {
        self.channels as u32 * self.bits as u32 / 8
    }
}

/// DirectSound volume (hundredths of dB, -10000..=0) to linear gain.
pub fn db100_to_gain(v: i32) -> f32 {
    if v <= -10000 {
        0.0
    } else {
        10f32.powf(v.min(0) as f32 / 2000.0)
    }
}

/// DirectSound pan (-10000 = left only .. 10000 = right only) to (left, right) gains.
pub fn pan_gains(pan: i32) -> (f32, f32) {
    let pan = pan.clamp(-10000, 10000);
    if pan > 0 {
        (db100_to_gain(-pan), 1.0)
    } else {
        (1.0, db100_to_gain(pan))
    }
}

#[inline]
fn sample(data: &[u8], fmt: &PcmFormat, frame: usize, ch: usize) -> f32 {
    let ch = ch.min(fmt.channels as usize - 1);
    if fmt.bits == 8 {
        let i = frame * fmt.channels as usize + ch;
        data.get(i).map_or(0.0, |&b| ((b as i32 - 128) << 8) as f32)
    } else {
        let i = (frame * fmt.channels as usize + ch) * 2;
        match data.get(i..i + 2) {
            Some(b) => i16::from_le_bytes([b[0], b[1]]) as f32,
            None => 0.0,
        }
    }
}

/// Mix one voice into `out` (interleaved stereo i32 accumulators, `out.len() / 2` frames).
/// `pos` is the read position in source frames (fractional), `step` = source frames per output frame.
/// Linear interpolation. Returns false when a non-looping voice reached its end (`pos` is then 0).
pub fn mix_voice(
    data: &[u8],
    fmt: &PcmFormat,
    pos: &mut f64,
    step: f64,
    gains: (f32, f32),
    looping: bool,
    out: &mut [i32],
) -> bool {
    let frames = data.len() / fmt.block_align().max(1) as usize;
    if frames == 0 {
        *pos = 0.0;
        return false;
    }
    let total = frames as f64;
    for o in out.chunks_exact_mut(2) {
        if *pos >= total {
            if looping {
                *pos %= total;
            } else {
                *pos = 0.0;
                return false;
            }
        }
        let i = *pos as usize;
        let frac = (*pos - i as f64) as f32;
        let j = if i + 1 < frames { i + 1 } else if looping { 0 } else { i };
        let l = sample(data, fmt, i, 0) * (1.0 - frac) + sample(data, fmt, j, 0) * frac;
        let r = if fmt.channels > 1 {
            sample(data, fmt, i, 1) * (1.0 - frac) + sample(data, fmt, j, 1) * frac
        } else {
            l
        };
        o[0] += (l * gains.0) as i32;
        o[1] += (r * gains.1) as i32;
        *pos += step;
    }
    if *pos >= total && !looping {
        *pos = 0.0;
        return false;
    }
    true
}

/// Mix interleaved i16 PCM from a queue (consumed from the front) into `out`, resampling linearly.
/// `pos` carries the fractional position between calls. Stops early when the queue runs dry.
pub fn mix_stream(
    q: &mut std::collections::VecDeque<i16>,
    rate: u32,
    channels: u16,
    pos: &mut f64,
    gain: f32,
    out: &mut [i32],
) {
    let ch = channels.max(1) as usize;
    let step = rate as f64 / OUT_RATE as f64;
    for o in out.chunks_exact_mut(2) {
        let avail = q.len() / ch;
        if avail < 2 {
            break;
        }
        let i = *pos as usize;
        if i + 1 >= avail {
            break;
        }
        let frac = (*pos - i as f64) as f32;
        let get = |f: usize, c: usize| q[f * ch + c.min(ch - 1)] as f32;
        let l = get(i, 0) * (1.0 - frac) + get(i + 1, 0) * frac;
        let r = get(i, 1) * (1.0 - frac) + get(i + 1, 1) * frac;
        o[0] += (l * gain) as i32;
        o[1] += (r * gain) as i32;
        *pos += step;
        let consumed = *pos as usize;
        if consumed > 0 {
            q.drain(..consumed * ch);
            *pos -= consumed as f64;
        }
    }
}

/// Clamp accumulators to i16 with a master gain.
pub fn finish(acc: &[i32], master: f32, out: &mut Vec<i16>) {
    out.clear();
    out.extend(acc.iter().map(|&v| ((v as f32 * master) as i32).clamp(-32768, 32767) as i16));
}
