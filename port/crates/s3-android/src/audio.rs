//! Audio path: the engine thread pushes PCM through [`Resampler`] into a lock-free single-producer /
//! single-consumer [`AudioRing`] of interleaved stereo i16 at [`OUT_RATE`]; the AAudio data callback (real-time
//! thread, no locks/allocations) pops from it and plays silence on underrun.

use std::sync::atomic::{AtomicBool, AtomicI16, AtomicU64, AtomicUsize, Ordering};

pub const OUT_RATE: u32 = 44100;
pub const OUT_CHANNELS: usize = 2;

/// SPSC ring of stereo frames.
pub struct AudioRing {
    buf: Vec<AtomicI16>,
    /// Total samples written / read (monotonic; index = value % len).
    write: AtomicUsize,
    read: AtomicUsize,
    /// Samples dropped because the ring was full / underrun frames played as silence (diagnostics).
    pub dropped: AtomicU64,
    pub underruns: AtomicU64,
    /// Set while the stream is running (diagnostics; the producer keeps writing anyway).
    pub running: AtomicBool,
}

impl AudioRing {
    pub fn new(frames: usize) -> Self {
        let n = frames * OUT_CHANNELS;
        AudioRing {
            buf: (0..n).map(|_| AtomicI16::new(0)).collect(),
            write: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
            dropped: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
            running: AtomicBool::new(false),
        }
    }

    /// Queued stereo frames.
    pub fn queued_frames(&self) -> usize {
        (self.write.load(Ordering::Acquire) - self.read.load(Ordering::Acquire)) / OUT_CHANNELS
    }

    /// Producer: append interleaved stereo samples (drops what does not fit).
    pub fn push(&self, samples: &[i16]) {
        let w = self.write.load(Ordering::Relaxed);
        let r = self.read.load(Ordering::Acquire);
        let free = self.buf.len() - (w - r);
        let n = samples.len().min(free) & !1;
        let len = self.buf.len();
        for (i, &s) in samples[..n].iter().enumerate() {
            self.buf[(w + i) % len].store(s, Ordering::Relaxed);
        }
        self.write.store(w + n, Ordering::Release);
        if n < samples.len() {
            self.dropped.fetch_add((samples.len() - n) as u64, Ordering::Relaxed);
        }
    }

    /// Consumer: fill `out` (interleaved stereo), zero-filling on underrun. Returns samples copied from the ring.
    pub fn pop_into(&self, out: &mut [i16]) -> usize {
        let r = self.read.load(Ordering::Relaxed);
        let w = self.write.load(Ordering::Acquire);
        let n = out.len().min(w - r);
        let len = self.buf.len();
        for (i, o) in out[..n].iter_mut().enumerate() {
            *o = self.buf[(r + i) % len].load(Ordering::Relaxed);
        }
        out[n..].fill(0);
        self.read.store(r + n, Ordering::Release);
        if n < out.len() && n > 0 {
            self.underruns.fetch_add(1, Ordering::Relaxed);
        }
        n
    }

    /// Drop everything queued (consumer side; call only when the stream is stopped).
    pub fn clear(&self) {
        self.read.store(self.write.load(Ordering::Acquire), Ordering::Release);
    }
}

/// Linear-interpolating converter from any rate/channel count (1 or 2) to stereo [`OUT_RATE`].
#[derive(Debug, Default)]
pub struct Resampler {
    /// Position in input frames relative to `prev` (index 0 = prev).
    phase: f64,
    prev: (i16, i16),
    out: Vec<i16>,
    pub last_rate: u32,
}

impl Resampler {
    pub fn convert(&mut self, samples: &[i16], rate: u32, channels: u16) -> &[i16] {
        self.out.clear();
        let ch = channels.max(1) as usize;
        let n = samples.len() / ch;
        if n == 0 || rate == 0 {
            return &self.out;
        }
        self.last_rate = rate;
        let frame = |i: usize| -> (i16, i16) {
            let s = &samples[i * ch..];
            if ch == 1 {
                (s[0], s[0])
            } else {
                (s[0], s[1])
            }
        };
        if rate == OUT_RATE && self.phase == 0.0 {
            for i in 0..n {
                let (l, r) = frame(i);
                self.out.extend_from_slice(&[l, r]);
            }
            self.prev = frame(n - 1);
            return &self.out;
        }
        let step = rate as f64 / OUT_RATE as f64;
        let mut t = self.phase;
        // f(0) = prev, f(k) = frame(k-1); interpolate between f(i) and f(i+1) for i < n.
        while t < n as f64 {
            let i = t as usize;
            let fr = t - i as f64;
            let a = if i == 0 { self.prev } else { frame(i - 1) };
            let b = frame(i);
            let l = a.0 as f64 + (b.0 as f64 - a.0 as f64) * fr;
            let r = a.1 as f64 + (b.1 as f64 - a.1 as f64) * fr;
            self.out.push(l.round() as i16);
            self.out.push(r.round() as i16);
            t += step;
        }
        self.phase = t - n as f64;
        self.prev = frame(n - 1);
        &self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_roundtrip_and_underrun() {
        let r = AudioRing::new(4);
        r.push(&[1, 2, 3, 4]);
        assert_eq!(r.queued_frames(), 2);
        let mut out = [9i16; 6];
        assert_eq!(r.pop_into(&mut out), 4);
        assert_eq!(out, [1, 2, 3, 4, 0, 0]);
        // Overflow drops.
        r.push(&[5; 10]);
        assert_eq!(r.queued_frames(), 4);
        assert_eq!(r.dropped.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn mono_22050_doubles_frames() {
        let mut rs = Resampler::default();
        let input: Vec<i16> = (0..100).map(|i| i * 10).collect();
        let n1 = rs.convert(&input, 22050, 1).len();
        let n2 = rs.convert(&input, 22050, 1).len();
        // 100 input frames -> ~200 output frames (stereo -> x2 samples).
        assert!((n1 as i32 / 2 - 200).abs() <= 1, "{n1}");
        assert!((n2 as i32 / 2 - 200).abs() <= 1, "{n2}");
        // Interpolated values are monotonic within a ramp (after the first frames, which blend with the
        // previous call's last frame).
        let out = rs.convert(&input, 22050, 1).to_vec();
        assert!(out[4..].windows(4).step_by(2).all(|w| w[2] >= w[0]));
    }

    #[test]
    fn passthrough_at_out_rate() {
        let mut rs = Resampler::default();
        assert_eq!(rs.convert(&[1, 2, 3, 4], OUT_RATE, 2), &[1, 2, 3, 4]);
    }
}
