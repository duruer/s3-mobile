//! DirectSound HLE (`dsound.dll`, `DirectSoundCreate` via LoadLibrary/GetProcAddress).
//!
//! Secondary buffers keep their PCM in guest memory (the game Locks, copies and Unlocks). A host-side
//! service ([`mixer_service`], run once per scheduler slice) mixes all playing buffers plus the extra streams
//! from `s3_runtime::audio::AudioStreams` into 44.1 kHz stereo i16 and hands it to `Host::queue_audio`.
//! Pacing is wall-clock based: the mixer keeps ~[`LATENCY_MS`] ahead of real time, so play cursors advance in
//! real time even without an audio device (headless).

use std::collections::HashMap;

use s3_runtime::audio::AudioStreams;
use s3_runtime::com::Guid;
use s3_runtime::{LogLevel, Machine, Registry, ThunkResult};

use crate::hr::*;
use crate::mixer::{self, PcmFormat, OUT_RATE};
use crate::{declare_interface, put_u32, reg_method, ret};

pub const DLL: &str = "dsound.dll";

pub const IID_IDIRECTSOUND: Guid = Guid::parse("279AFA83-4981-11CE-A521-0020AF0BE560");
pub const IID_IDIRECTSOUNDBUFFER: Guid = Guid::parse("279AFA85-4981-11CE-A521-0020AF0BE560");
const IID_IUNKNOWN: Guid = Guid::parse("00000000-0000-0000-C000-000000000046");

pub const EXPORTS: &[&str] = &["DirectSoundCreate", "DirectSoundEnumerateA"];

pub const IDIRECTSOUND: &[&str] = &[
    "QueryInterface", "AddRef", "Release", "CreateSoundBuffer", "GetCaps", "DuplicateSoundBuffer",
    "SetCooperativeLevel", "Compact", "GetSpeakerConfig", "SetSpeakerConfig", "Initialize",
];

pub const IDIRECTSOUNDBUFFER: &[&str] = &[
    "QueryInterface", "AddRef", "Release", "GetCaps", "GetCurrentPosition", "GetFormat", "GetVolume", "GetPan",
    "GetFrequency", "GetStatus", "Initialize", "Lock", "Play", "SetCurrentPosition", "SetFormat", "SetVolume",
    "SetPan", "SetFrequency", "Stop", "Unlock", "Restore",
];

pub const DS_OK: u32 = 0;
pub const DSERR_INVALIDPARAM: u32 = E_INVALIDARG;
pub const DSERR_INVALIDCALL: u32 = 0x8878_0032;
pub const DSERR_PRIOLEVELNEEDED: u32 = 0x8878_0046;
pub const DSERR_BADFORMAT: u32 = 0x8878_0064;
pub const DSERR_ALREADYINITIALIZED: u32 = 0x8878_0082;

pub const DSBCAPS_PRIMARYBUFFER: u32 = 0x1;
pub const DSBLOCK_FROMWRITECURSOR: u32 = 0x1;
pub const DSBLOCK_ENTIREBUFFER: u32 = 0x2;
pub const DSBPLAY_LOOPING: u32 = 0x1;
pub const DSBSTATUS_PLAYING: u32 = 0x1;
pub const DSBSTATUS_LOOPING: u32 = 0x4;

/// How far ahead of real time the mixer produces audio.
pub const LATENCY_MS: u64 = 60;
/// Maximum audio produced in one service call (catch-up after a long slice is dropped).
const MAX_CHUNK_MS: u64 = 250;
/// Upper bound for audio waiting in the host device queue (see the drift guard in [`mixer_service`]).
pub const MAX_HOST_QUEUE_MS: u64 = 250;
/// Distance of the reported write cursor ahead of the play cursor.
const WRITE_AHEAD_MS: u32 = 15;

/// One sound buffer (keyed by its interface pointer).
#[derive(Debug, Clone, Default)]
pub struct Buffer {
    pub primary: bool,
    pub flags: u32,
    /// Guest address and size of the PCM data (shared by duplicates).
    pub data: u32,
    pub size: u32,
    pub fmt: PcmFormat,
    /// Read position in source frames.
    pub pos: f64,
    pub playing: bool,
    pub looping: bool,
    /// Hundredths of dB (-10000..=0).
    pub volume: i32,
    pub pan: i32,
    /// 0 = original rate.
    pub freq: u32,
}

impl Buffer {
    fn rate(&self) -> u32 {
        if self.freq == 0 {
            self.fmt.rate
        } else {
            self.freq
        }
    }
}

/// DirectSound module state.
#[derive(Default)]
pub struct DsState {
    pub buffers: HashMap<u32, Buffer>,
    /// Number of buffers sharing each data block.
    data_refs: HashMap<u32, u32>,
    /// Master volume (primary buffer SetVolume).
    pub master_volume: i32,
    pub primary_fmt: PcmFormat,
    start_ms: Option<u64>,
    mixed_frames: u64,
    acc: Vec<i32>,
    out: Vec<i16>,
    /// Fractional positions of the extra streams.
    stream_pos: HashMap<u32, f64>,
    enum_strings: Option<(u32, u32)>,
    /// Total frames handed to the host (stats/tests).
    pub frames_out: u64,
    /// Frames not produced because the host queue was above MAX_HOST_QUEUE_MS (stats).
    pub drift_cut_frames: u64,
    /// Number of secondary-buffer Play calls (stats).
    pub voices_started: u64,
}

fn state(m: &mut Machine) -> s3_runtime::Shared<DsState> {
    m.state::<DsState>()
}

pub fn register(reg: &mut Registry) {
    reg.declare_all(DLL, EXPORTS);
    declare_interface(reg, DLL, "IDirectSound", IDIRECTSOUND);
    declare_interface(reg, DLL, "IDirectSoundBuffer", IDIRECTSOUNDBUFFER);
    reg.register(DLL, "DirectSoundCreate", direct_sound_create);
    reg.register(DLL, "DirectSoundEnumerateA", direct_sound_enumerate_a);

    let ds: &[(&str, s3_runtime::Handler)] = &[
        ("QueryInterface", ds_query_interface),
        ("AddRef", Machine::com_addref),
        ("Release", Machine::com_release),
        ("CreateSoundBuffer", ds_create_sound_buffer),
        ("GetCaps", ds_get_caps),
        ("DuplicateSoundBuffer", ds_duplicate_sound_buffer),
        ("SetCooperativeLevel", ok3),
        ("Compact", ok1),
        ("GetSpeakerConfig", ds_get_speaker_config),
        ("SetSpeakerConfig", ok2),
        ("Initialize", already2),
    ];
    for (n, h) in ds {
        reg_method(reg, DLL, "IDirectSound", n, *h);
    }
    let b: &[(&str, s3_runtime::Handler)] = &[
        ("QueryInterface", buf_query_interface),
        ("AddRef", Machine::com_addref),
        ("Release", buf_release),
        ("GetCaps", buf_get_caps),
        ("GetCurrentPosition", buf_get_current_position),
        ("GetFormat", buf_get_format),
        ("GetVolume", buf_get_volume),
        ("GetPan", buf_get_pan),
        ("GetFrequency", buf_get_frequency),
        ("GetStatus", buf_get_status),
        ("Initialize", already3),
        ("Lock", buf_lock),
        ("Play", buf_play),
        ("SetCurrentPosition", buf_set_current_position),
        ("SetFormat", buf_set_format),
        ("SetVolume", buf_set_volume),
        ("SetPan", buf_set_pan),
        ("SetFrequency", buf_set_frequency),
        ("Stop", buf_stop),
        ("Unlock", ok5),
        ("Restore", ok1),
    ];
    for (n, h) in b {
        reg_method(reg, DLL, "IDirectSoundBuffer", n, *h);
    }
}

crate::stubs! {
    ok1 = (1, DS_OK);
    ok2 = (2, DS_OK);
    ok3 = (3, DS_OK);
    ok5 = (5, DS_OK);
    already2 = (2, DSERR_ALREADYINITIALIZED);
    already3 = (3, DSERR_ALREADYINITIALIZED);
}

/// `DirectSoundCreate(lpGuid, lplpDS, pUnkOuter)`.
fn direct_sound_create(m: &mut Machine) -> ThunkResult {
    let out = m.arg_u32(1);
    if out == 0 {
        return ret(DSERR_INVALIDPARAM, 3);
    }
    let vt = m.com_vtable(DLL, "IDirectSound", IDIRECTSOUND);
    let p = m.com_new_object(vt, 0x10);
    m.mem.write_u32(out, p);
    ret(DS_OK, 3)
}

/// `DirectSoundEnumerateA(cb, ctx)`: one device, "Primary Sound Driver" (GUID NULL).
fn direct_sound_enumerate_a(m: &mut Machine) -> ThunkResult {
    let (cb, ctx) = (m.arg_u32(0), m.arg_u32(1));
    let st = state(m);
    let s = st.borrow_mut().enum_strings;
    let (d, n) = match s {
        Some(x) => x,
        None => {
            let d = m.mem.alloc(32);
            m.mem.write_cstr(d, "Primary Sound Driver");
            let n = m.mem.alloc(4);
            st.borrow_mut().enum_strings = Some((d, n));
            (d, n)
        }
    };
    if cb != 0 {
        m.call_guest(cb, &[0, d, n, ctx]);
    }
    ret(DS_OK, 2)
}

fn ds_query_interface(m: &mut Machine) -> ThunkResult {
    let (this, riid, out) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    if out == 0 {
        return ret(E_POINTER, 3);
    }
    let iid = Guid::read(m, riid);
    if iid == IID_IDIRECTSOUND || iid == IID_IUNKNOWN {
        let n = m.mem.read_u32(this + 4) + 1;
        m.mem.write_u32(this + 4, n);
        m.mem.write_u32(out, this);
        ret(S_OK, 3)
    } else {
        m.mem.write_u32(out, 0);
        ret(E_NOINTERFACE, 3)
    }
}

/// DSCAPS (0x60 bytes).
fn ds_get_caps(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(1);
    if p == 0 {
        return ret(DSERR_INVALIDPARAM, 2);
    }
    m.mem.fill(p + 4, 0x5C, 0);
    // PRIMARYMONO|PRIMARYSTEREO|PRIMARY8BIT|PRIMARY16BIT|CONTINUOUSRATE|SECONDARYMONO|STEREO|8BIT|16BIT
    m.mem.write_u32(p + 4, 0x1 | 0x2 | 0x4 | 0x8 | 0x10 | 0x100 | 0x200 | 0x400 | 0x800);
    m.mem.write_u32(p + 8, 100);
    m.mem.write_u32(p + 0xC, 100_000);
    ret(DS_OK, 2)
}

fn ds_get_speaker_config(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(1);
    put_u32(m, p, 4); // DSSPEAKER_STEREO
    ret(DS_OK, 2)
}

fn new_buffer_object(m: &mut Machine, b: Buffer) -> u32 {
    let vt = m.com_vtable(DLL, "IDirectSoundBuffer", IDIRECTSOUNDBUFFER);
    let p = m.com_new_object(vt, 0x10);
    let st = state(m);
    let mut s = st.borrow_mut();
    if b.data != 0 {
        *s.data_refs.entry(b.data).or_insert(0) += 1;
    }
    s.buffers.insert(p, b);
    p
}

fn read_wfx(m: &Machine, p: u32) -> Option<PcmFormat> {
    let tag = m.mem.read_u16(p);
    let channels = m.mem.read_u16(p + 2);
    let rate = m.mem.read_u32(p + 4);
    let bits = m.mem.read_u16(p + 14);
    if tag != 1 || !(1..=2).contains(&channels) || !(bits == 8 || bits == 16) || !(100..=200_000).contains(&rate) {
        return None;
    }
    Some(PcmFormat { rate, channels, bits })
}

/// CreateSoundBuffer(this, lpDSBufferDesc, lplpDSBuffer, pUnkOuter).
fn ds_create_sound_buffer(m: &mut Machine) -> ThunkResult {
    let (d, out) = (m.arg_u32(1), m.arg_u32(2));
    if d == 0 || out == 0 {
        return ret(DSERR_INVALIDPARAM, 4);
    }
    let flags = m.mem.read_u32(d + 4);
    let bytes = m.mem.read_u32(d + 8);
    let pwfx = m.mem.read_u32(d + 0x10);
    let p = if flags & DSBCAPS_PRIMARYBUFFER != 0 {
        let fmt = state(m).borrow_mut().primary_fmt;
        new_buffer_object(m, Buffer { primary: true, flags, fmt, ..Default::default() })
    } else {
        if pwfx == 0 || !(4..=0x0FFF_FFFF).contains(&bytes) {
            return ret(DSERR_INVALIDPARAM, 4);
        }
        let Some(fmt) = read_wfx(m, pwfx) else {
            m.log(LogLevel::Warn, "dsound: CreateSoundBuffer with unsupported format");
            return ret(DSERR_BADFORMAT, 4);
        };
        let data = m.mem.alloc(bytes);
        new_buffer_object(m, Buffer { flags, data, size: bytes, fmt, ..Default::default() })
    };
    m.mem.write_u32(out, p);
    ret(DS_OK, 4)
}

/// DuplicateSoundBuffer(this, lpDsbOriginal, lplpDsbDuplicate): shares the PCM memory.
fn ds_duplicate_sound_buffer(m: &mut Machine) -> ThunkResult {
    let (orig, out) = (m.arg_u32(1), m.arg_u32(2));
    let b = state(m).borrow_mut().buffers.get(&orig).cloned();
    let Some(b) = b else { return ret(DSERR_INVALIDPARAM, 3) };
    if b.primary || out == 0 {
        return ret(DSERR_INVALIDCALL, 3);
    }
    let dup = Buffer { pos: 0.0, playing: false, looping: false, ..b };
    let p = new_buffer_object(m, dup);
    m.mem.write_u32(out, p);
    ret(DS_OK, 3)
}

fn buf_query_interface(m: &mut Machine) -> ThunkResult {
    let (this, riid, out) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    if out == 0 {
        return ret(E_POINTER, 3);
    }
    let iid = Guid::read(m, riid);
    if iid == IID_IDIRECTSOUNDBUFFER || iid == IID_IUNKNOWN {
        let n = m.mem.read_u32(this + 4) + 1;
        m.mem.write_u32(this + 4, n);
        m.mem.write_u32(out, this);
        ret(S_OK, 3)
    } else {
        m.mem.write_u32(out, 0);
        ret(E_NOINTERFACE, 3)
    }
}

fn buf_release(m: &mut Machine) -> ThunkResult {
    let this = m.arg_u32(0);
    let n = m.mem.read_u32(this + 4).saturating_sub(1);
    m.mem.write_u32(this + 4, n);
    if n == 0 {
        let st = state(m);
        let free_data = {
            let mut s = st.borrow_mut();
            match s.buffers.remove(&this) {
                Some(b) if b.data != 0 => {
                    let r = s.data_refs.entry(b.data).or_insert(1);
                    *r -= 1;
                    if *r == 0 {
                        s.data_refs.remove(&b.data);
                        Some(b.data)
                    } else {
                        None
                    }
                }
                _ => None,
            }
        };
        if let Some(d) = free_data {
            m.mem.free(d);
        }
        m.mem.free(this);
    }
    ret(n, 1)
}

fn with_buf<R>(m: &mut Machine, p: u32, f: impl FnOnce(&mut Buffer) -> R) -> Option<R> {
    let st = state(m);
    let mut s = st.borrow_mut();
    s.buffers.get_mut(&p).map(f)
}

/// DSBCAPS {dwSize, dwFlags, dwBufferBytes, dwUnlockTransferRate, dwPlayCpuOverhead}.
fn buf_get_caps(m: &mut Machine) -> ThunkResult {
    let (this, p) = (m.arg_u32(0), m.arg_u32(1));
    let Some((flags, size)) = with_buf(m, this, |b| (b.flags, b.size)) else { return ret(DSERR_INVALIDPARAM, 2) };
    if p == 0 {
        return ret(DSERR_INVALIDPARAM, 2);
    }
    m.mem.write_u32(p + 4, flags | 0x8); // DSBCAPS_LOCSOFTWARE
    m.mem.write_u32(p + 8, size);
    m.mem.write_u32(p + 12, 0);
    m.mem.write_u32(p + 16, 0);
    ret(DS_OK, 2)
}

/// Play/write cursors in bytes.
fn cursors(b: &Buffer) -> (u32, u32) {
    if b.size == 0 {
        return (0, 0);
    }
    let ba = b.fmt.block_align().max(1);
    let play = (b.pos as u32 * ba) % b.size;
    let ahead = if b.playing { (b.rate() * WRITE_AHEAD_MS / 1000) * ba } else { 0 };
    (play, (play + ahead) % b.size)
}

fn buf_get_current_position(m: &mut Machine) -> ThunkResult {
    let (this, pp, pw) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    let Some((play, write)) = with_buf(m, this, |b| cursors(b)) else { return ret(DSERR_INVALIDPARAM, 3) };
    put_u32(m, pp, play);
    put_u32(m, pw, write);
    ret(DS_OK, 3)
}

/// GetFormat(this, lpwfx, dwSizeAllocated, lpdwSizeWritten).
fn buf_get_format(m: &mut Machine) -> ThunkResult {
    let (this, p, size, pw) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_u32(3));
    let Some(f) = with_buf(m, this, |b| b.fmt) else { return ret(DSERR_INVALIDPARAM, 4) };
    if p == 0 {
        put_u32(m, pw, 18);
        return ret(DS_OK, 4);
    }
    let ba = f.block_align();
    let mut wfx = Vec::with_capacity(18);
    wfx.extend_from_slice(&1u16.to_le_bytes());
    wfx.extend_from_slice(&f.channels.to_le_bytes());
    wfx.extend_from_slice(&f.rate.to_le_bytes());
    wfx.extend_from_slice(&(f.rate * ba).to_le_bytes());
    wfx.extend_from_slice(&(ba as u16).to_le_bytes());
    wfx.extend_from_slice(&f.bits.to_le_bytes());
    wfx.extend_from_slice(&0u16.to_le_bytes());
    let n = (size as usize).min(18);
    m.mem.write_bytes(p, &wfx[..n]);
    put_u32(m, pw, n as u32);
    ret(DS_OK, 4)
}

fn buf_get_volume(m: &mut Machine) -> ThunkResult {
    let (this, p) = (m.arg_u32(0), m.arg_u32(1));
    let st = state(m);
    let v = {
        let s = st.borrow_mut();
        s.buffers.get(&this).map(|b| if b.primary { s.master_volume } else { b.volume })
    };
    let Some(v) = v else { return ret(DSERR_INVALIDPARAM, 2) };
    put_u32(m, p, v as u32);
    ret(DS_OK, 2)
}

fn buf_get_pan(m: &mut Machine) -> ThunkResult {
    let (this, p) = (m.arg_u32(0), m.arg_u32(1));
    let Some(v) = with_buf(m, this, |b| b.pan) else { return ret(DSERR_INVALIDPARAM, 2) };
    put_u32(m, p, v as u32);
    ret(DS_OK, 2)
}

fn buf_get_frequency(m: &mut Machine) -> ThunkResult {
    let (this, p) = (m.arg_u32(0), m.arg_u32(1));
    let Some(v) = with_buf(m, this, |b| b.rate()) else { return ret(DSERR_INVALIDPARAM, 2) };
    put_u32(m, p, v);
    ret(DS_OK, 2)
}

fn buf_get_status(m: &mut Machine) -> ThunkResult {
    let (this, p) = (m.arg_u32(0), m.arg_u32(1));
    let Some(v) = with_buf(m, this, |b| {
        (if b.playing { DSBSTATUS_PLAYING } else { 0 }) | (if b.playing && b.looping { DSBSTATUS_LOOPING } else { 0 })
    }) else {
        return ret(DSERR_INVALIDPARAM, 2);
    };
    put_u32(m, p, v);
    ret(DS_OK, 2)
}

/// Lock(this, dwOffset, dwBytes, ppvAudioPtr1, pdwAudioBytes1, ppvAudioPtr2, pdwAudioBytes2, dwFlags).
fn buf_lock(m: &mut Machine) -> ThunkResult {
    let this = m.arg_u32(0);
    let (mut off, mut bytes) = (m.arg_u32(1), m.arg_u32(2));
    let (pp1, pl1, pp2, pl2, flags) = (m.arg_u32(3), m.arg_u32(4), m.arg_u32(5), m.arg_u32(6), m.arg_u32(7));
    let Some(b) = with_buf(m, this, |b| b.clone()) else { return ret(DSERR_INVALIDPARAM, 8) };
    if b.primary || b.size == 0 {
        return ret(DSERR_PRIOLEVELNEEDED, 8);
    }
    if flags & DSBLOCK_FROMWRITECURSOR != 0 {
        off = cursors(&b).1;
    }
    if flags & DSBLOCK_ENTIREBUFFER != 0 {
        bytes = b.size;
    }
    if off >= b.size || bytes == 0 || bytes > b.size {
        return ret(DSERR_INVALIDPARAM, 8);
    }
    let l1 = bytes.min(b.size - off);
    let l2 = bytes - l1;
    put_u32(m, pp1, b.data + off);
    put_u32(m, pl1, l1);
    put_u32(m, pp2, if l2 > 0 { b.data } else { 0 });
    put_u32(m, pl2, l2);
    ret(DS_OK, 8)
}

/// Play(this, dwReserved1, dwPriority, dwFlags).
fn buf_play(m: &mut Machine) -> ThunkResult {
    let (this, flags) = (m.arg_u32(0), m.arg_u32(3));
    let r = with_buf(m, this, |b| {
        if !b.primary {
            b.playing = true;
            b.looping = flags & DSBPLAY_LOOPING != 0;
        }
        (b.primary, b.volume, b.pan, b.rate(), b.size, b.fmt)
    });
    if let Some((false, vol, pan, rate, size, fmt)) = r {
        state(m).borrow_mut().voices_started += 1;
        m.log(LogLevel::Debug, &format!(
            "dsound: Play {this:#x} vol {vol} pan {pan} rate {rate} {}ch/{}bit {size} bytes flags {flags:#x}",
            fmt.channels, fmt.bits
        ));
    }
    ret(if r.is_some() { DS_OK } else { DSERR_INVALIDPARAM }, 4)
}

fn buf_stop(m: &mut Machine) -> ThunkResult {
    let this = m.arg_u32(0);
    with_buf(m, this, |b| b.playing = false);
    ret(DS_OK, 1)
}

fn buf_set_current_position(m: &mut Machine) -> ThunkResult {
    let (this, pos) = (m.arg_u32(0), m.arg_u32(1));
    with_buf(m, this, |b| {
        if b.size > 0 {
            b.pos = ((pos % b.size) / b.fmt.block_align().max(1)) as f64;
        }
    });
    ret(DS_OK, 2)
}

fn buf_set_format(m: &mut Machine) -> ThunkResult {
    let (this, p) = (m.arg_u32(0), m.arg_u32(1));
    let Some(primary) = with_buf(m, this, |b| b.primary) else { return ret(DSERR_INVALIDPARAM, 2) };
    if !primary {
        return ret(DSERR_INVALIDCALL, 2);
    }
    let Some(f) = (p != 0).then(|| read_wfx(m, p)).flatten() else { return ret(DSERR_BADFORMAT, 2) };
    with_buf(m, this, |b| b.fmt = f);
    state(m).borrow_mut().primary_fmt = f;
    ret(DS_OK, 2)
}

fn buf_set_volume(m: &mut Machine) -> ThunkResult {
    let (this, v) = (m.arg_u32(0), m.arg_i32(1));
    if !(-10000..=0).contains(&v) {
        return ret(DSERR_INVALIDPARAM, 2);
    }
    let st = state(m);
    let mut s = st.borrow_mut();
    let Some(b) = s.buffers.get_mut(&this) else { return ret(DSERR_INVALIDPARAM, 2) };
    if b.primary {
        s.master_volume = v;
    } else {
        b.volume = v;
    }
    ret(DS_OK, 2)
}

fn buf_set_pan(m: &mut Machine) -> ThunkResult {
    let (this, v) = (m.arg_u32(0), m.arg_i32(1));
    if !(-10000..=10000).contains(&v) {
        return ret(DSERR_INVALIDPARAM, 2);
    }
    with_buf(m, this, |b| b.pan = v);
    ret(DS_OK, 2)
}

fn buf_set_frequency(m: &mut Machine) -> ThunkResult {
    let (this, v) = (m.arg_u32(0), m.arg_u32(1));
    if v != 0 && !(100..=200_000).contains(&v) {
        return ret(DSERR_INVALIDPARAM, 2);
    }
    with_buf(m, this, |b| b.freq = v);
    ret(DS_OK, 2)
}

// ---------------------------------------------------------------------------------------------- mixer

/// Scheduler service: mix everything that should have been played by now (+ latency) and queue it.
pub fn mixer_service(m: &mut Machine) {
    let now = m.now_ms();
    if m.input.paused {
        // Do not run ahead while the app is in the background; restart pacing on resume.
        state(m).borrow_mut().start_ms = None;
        return;
    }
    let st = state(m);
    let streams = m.state::<AudioStreams>();
    let mut s = st.borrow_mut();
    let start = *s.start_ms.get_or_insert(now);
    let target = (now.saturating_sub(start) + LATENCY_MS) * OUT_RATE as u64 / 1000;
    let max = MAX_CHUNK_MS * OUT_RATE as u64 / 1000;
    if target <= s.mixed_frames {
        return;
    }
    let mut n = target - s.mixed_frames;
    if n > max {
        n = max;
    }
    s.mixed_frames = target;
    // Device-clock drift guard: a host with a real device reports its queue. If the device consumes slower than
    // the wall clock (or stalled), the queue grows; produce less so it never exceeds MAX_HOST_QUEUE_MS (voices
    // are delayed, not skipped). Hosts without a device report 0 and are paced by the wall clock alone.
    let queued = m.host.audio_queued_frames() as u64;
    let max_q = MAX_HOST_QUEUE_MS * OUT_RATE as u64 / 1000;
    if queued + n > max_q {
        let cut = (queued + n - max_q).min(n);
        n -= cut;
        s.drift_cut_frames += cut;
        if n == 0 {
            return;
        }
    }
    let frames = n as usize;
    let mut acc = std::mem::take(&mut s.acc);
    acc.clear();
    acc.resize(frames * 2, 0);

    for b in s.buffers.values_mut() {
        if !b.playing || b.primary || b.size == 0 {
            continue;
        }
        let data = m.mem.slice(b.data, b.size as usize);
        let step = b.rate() as f64 / OUT_RATE as f64;
        let vol = mixer::db100_to_gain(b.volume);
        let (l, r) = mixer::pan_gains(b.pan);
        let fmt = b.fmt;
        if !mixer::mix_voice(data, &fmt, &mut b.pos, step, (l * vol, r * vol), b.looping, &mut acc) {
            b.playing = false;
        }
    }
    {
        let mut ex = streams.borrow_mut();
        let ids: Vec<u32> = ex.streams.keys().copied().collect();
        for id in ids {
            let stm = ex.streams.get_mut(&id).unwrap();
            if stm.samples.is_empty() {
                continue;
            }
            let gain = if stm.gain_set { stm.gain } else { 1.0 };
            let pos = s.stream_pos.entry(id).or_insert(0.0);
            mixer::mix_stream(&mut stm.samples, stm.rate.max(1), stm.channels, pos, gain, &mut acc);
        }
    }
    let master = mixer::db100_to_gain(s.master_volume);
    let mut out = std::mem::take(&mut s.out);
    mixer::finish(&acc, master, &mut out);
    s.acc = acc;
    s.frames_out += frames as u64;
    m.host.queue_audio(&out, OUT_RATE, 2);
    s.out = out;
}
