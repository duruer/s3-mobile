//! `winmm.dll` HLE (also serves `_inmm.dll`, the GOG wrapper S3.EXE imports): multimedia timers, MCI.
//!
//! - `timeSetEvent` timers run on a dedicated guest thread ("winmm timer thread", like Windows): its entry point is
//!   our thunk `winmm.dll!__s3port_timer_thread`, which blocks until a timer is due, then calls the guest
//!   `TimeProc(id, 0, user, 0, 0)` with `call_guest` (or sets/pulses the event for TIME_CALLBACK_EVENT_*).
//! - MCI `cdaudio` (commands and strings): `MUSIC\TrackNN.ogg` via `audio_music.rs`; MM_MCINOTIFY is posted to the
//!   callback window when the range ends. Every other device type (the intro / briefing `AVIVideo`) is a *fake
//!   video*: open/window/status succeed and `play` finishes at once (MM_MCINOTIFY successful), so the game moves
//!   on. Failing the open instead makes S3 show "Error during video play ... (Error code 266)" over the menu
//!   (observed 2026-11-05).

use std::collections::HashMap;

use s3_runtime::{KernelObject, LogLevel, Machine, Registry, ThunkResult};

use crate::audio_music::{ogg_duration_ms, MusicEvent, MusicPlayer};
use s3_runtime::audio::AudioStreams;

/// DLL name as registered.
pub const DLL: &str = "winmm.dll";

/// Known exports (static imports of S3.EXE first).
pub const NAMES: &[&str] = &[
    "mciGetErrorStringA", "timeGetTime", "timeBeginPeriod", "timeEndPeriod", "timeSetEvent",
    "timeKillEvent", "mciSendCommandA", "mciSendStringA", "timeGetDevCaps", "waveOutGetNumDevs",
    "mciGetDeviceIDA", "auxGetNumDevs", "joyGetNumDevs", "joyGetPosEx", "PlaySoundA",
    "sndPlaySoundA", "timeGetSystemTime", "joyGetDevCapsA", "auxSetVolume", "auxGetVolume",
];

const TIMER_THREAD: &str = "__s3port_timer_thread";

/// Declare all names and register implemented handlers.
pub fn register(reg: &mut Registry) {
    reg.declare_all(DLL, NAMES);
    reg.alias("_inmm.dll", DLL);
    let h: &[(&str, s3_runtime::Handler)] = &[
        ("timeGetTime", time_get_time),
        ("timeGetSystemTime", time_get_system_time),
        ("timeBeginPeriod", time_begin_period),
        ("timeEndPeriod", time_begin_period),
        ("timeGetDevCaps", time_get_dev_caps),
        ("timeSetEvent", time_set_event),
        ("timeKillEvent", time_kill_event),
        (TIMER_THREAD, timer_thread),
        ("mciSendCommandA", mci_send_command_a),
        ("mciSendStringA", mci_send_string_a),
        ("mciGetErrorStringA", mci_get_error_string_a),
        ("mciGetDeviceIDA", mci_get_device_id_a),
        ("waveOutGetNumDevs", ret1_0),
        ("auxGetNumDevs", ret0_0),
        ("joyGetNumDevs", ret0_0),
        ("joyGetPosEx", joy_unplugged_2),
        ("joyGetDevCapsA", joy_unplugged_3),
        ("auxSetVolume", aux_set_volume),
        ("auxGetVolume", aux_get_volume),
        ("PlaySoundA", play_sound_a),
        ("sndPlaySoundA", snd_play_sound_a),
    ];
    for (n, f) in h {
        reg.register(DLL, n, *f);
    }
}

fn ret0_0(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(0, 0)
}
fn ret1_0(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(1, 0)
}
fn joy_unplugged_2(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(167, 2) // JOYERR_UNPLUGGED
}
fn joy_unplugged_3(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(167, 3)
}
fn play_sound_a(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(1, 3)
}
fn snd_play_sound_a(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(1, 2)
}

// ---------------------------------------------------------------------------------------------- timers

fn time_get_time(m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(m.now_ms() as u32, 0)
}

/// timeGetSystemTime(MMTIME*, cb): TIME_MS.
fn time_get_system_time(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    if p != 0 {
        m.mem.write_u32(p, 1); // TIME_MS
        m.mem.write_u32(p + 4, m.now_ms() as u32);
    }
    ThunkResult::stdcall(0, 2)
}

fn time_begin_period(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(0, 1) // TIMERR_NOERROR
}

fn time_get_dev_caps(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    if p != 0 {
        m.mem.write_u32(p, 1);
        m.mem.write_u32(p + 4, 1_000_000);
    }
    ThunkResult::stdcall(0, 2)
}

#[derive(Debug, Clone)]
struct MmTimer {
    id: u32,
    period: u32,
    next_due: u64,
    callback: u32,
    user: u32,
    flags: u32,
}

#[derive(Default)]
struct TimerState {
    timers: Vec<MmTimer>,
    next_id: u32,
    thread_tid: Option<u32>,
}

const TIME_PERIODIC: u32 = 1;
const TIME_CALLBACK_EVENT_SET: u32 = 0x10;
const TIME_CALLBACK_EVENT_PULSE: u32 = 0x20;

/// timeSetEvent(uDelay, uResolution, lpTimeProc, dwUser, fuEvent)
fn time_set_event(m: &mut Machine) -> ThunkResult {
    let (delay, cb, user, flags) = (m.arg_u32(0), m.arg_u32(2), m.arg_u32(3), m.arg_u32(4));
    let now = m.now_ms();
    let st = m.state::<TimerState>();
    let (id, need_thread) = {
        let mut s = st.borrow_mut();
        s.next_id += 1;
        let id = s.next_id;
        s.timers.push(MmTimer { id, period: delay.max(1), next_due: now + delay.max(1) as u64, callback: cb, user, flags });
        (id, s.thread_tid.is_none())
    };
    if need_thread {
        let Some(entry) = m.registry.lookup(DLL, TIMER_THREAD) else {
            return ThunkResult::Error("winmm timer thread thunk missing".into());
        };
        match m.create_thread(entry, 0, 0x1_0000, false) {
            Ok(slot) => {
                let tid = m.threads[slot].tid;
                st.borrow_mut().thread_tid = Some(tid);
                m.log(LogLevel::Info, &format!("winmm: timer thread created (tid {tid:#x})"));
            }
            Err(e) => return ThunkResult::Error(format!("winmm timer thread: {e}")),
        }
    }
    m.log(LogLevel::Debug, &format!("timeSetEvent({delay} ms, cb {cb:#x}, user {user:#x}, flags {flags:#x}) -> {id}"));
    ThunkResult::stdcall(id, 5)
}

fn time_kill_event(m: &mut Machine) -> ThunkResult {
    let id = m.arg_u32(0);
    let st = m.state::<TimerState>();
    let mut s = st.borrow_mut();
    let before = s.timers.len();
    s.timers.retain(|t| t.id != id);
    ThunkResult::stdcall(if s.timers.len() != before { 0 } else { 97 /* MMSYSERR_INVALPARAM */ }, 1)
}

/// Body of the timer guest thread: fire due timers, then block. Never returns.
fn timer_thread(m: &mut Machine) -> ThunkResult {
    let now = m.now_ms();
    let st = m.state::<TimerState>();
    let due: Vec<MmTimer> = {
        let mut s = st.borrow_mut();
        let mut due = Vec::new();
        s.timers.retain_mut(|t| {
            if t.next_due > now {
                return true;
            }
            due.push(t.clone());
            if t.flags & TIME_PERIODIC != 0 {
                // No catch-up bursts: schedule from now if we fell far behind.
                t.next_due = (t.next_due + t.period as u64).max(now.saturating_sub(t.period as u64 * 4));
                true
            } else {
                false
            }
        });
        due
    };
    for t in due {
        if t.flags & (TIME_CALLBACK_EVENT_SET | TIME_CALLBACK_EVENT_PULSE) != 0 {
            if let Some(KernelObject::Event { signaled, .. }) = m.handles.get_mut(t.callback) {
                *signaled = true;
            }
            // PULSE: waiters are released on their next retry; then it is reset by the auto-reset semantics.
        } else if t.callback != 0 {
            m.call_guest(t.callback, &[t.id, 0, t.user, 0, 0]);
        }
        if m.is_stopped() {
            break;
        }
    }
    ThunkResult::Block
}

// ---------------------------------------------------------------------------------------------- MCI

pub const MM_MCINOTIFY: u32 = 0x3B9;
const MCI_NOTIFY_SUCCESSFUL: u32 = 1;
const MCI_NOTIFY_SUPERSEDED: u32 = 2;
const MCI_NOTIFY_ABORTED: u32 = 4;

const MCI_OPEN: u32 = 0x803;
const MCI_CLOSE: u32 = 0x804;
const MCI_PLAY: u32 = 0x806;
const MCI_SEEK: u32 = 0x807;
const MCI_STOP: u32 = 0x808;
const MCI_PAUSE: u32 = 0x809;
const MCI_INFO: u32 = 0x80A;
const MCI_GETDEVCAPS: u32 = 0x80B;
const MCI_SET: u32 = 0x80D;
const MCI_STATUS: u32 = 0x814;
const MCI_RESUME: u32 = 0x855;

const MCI_FLAG_NOTIFY: u32 = 0x1;
const MCI_FLAG_FROM: u32 = 0x4;
const MCI_FLAG_TO: u32 = 0x8;
const MCI_FLAG_TRACK: u32 = 0x10;

pub const MCIERR_INVALID_DEVICE_ID: u32 = 257;
pub const MCIERR_UNRECOGNIZED_COMMAND: u32 = 261;
pub const MCIERR_INVALID_DEVICE_NAME: u32 = 263;
pub const MCIERR_CANNOT_LOAD_DRIVER: u32 = 266;
pub const MCIERR_OUTOFRANGE: u32 = 282;
pub const MCIERR_UNSUPPORTED_FUNCTION: u32 = 274;
pub const MCIERR_HARDWARE: u32 = 262;

pub const MCI_FORMAT_MILLISECONDS: u32 = 0;
pub const MCI_FORMAT_MSF: u32 = 2;
pub const MCI_FORMAT_TMSF: u32 = 10;

const MCI_MODE_STOP: u32 = 525;
const MCI_MODE_PLAY: u32 = 526;
const MCI_MODE_PAUSE: u32 = 529;

#[derive(Debug, Clone)]
struct MciDevice {
    alias: String,
    /// Fake video device (AVIVideo etc.): plays finish immediately.
    video: bool,
    time_format: u32,
    notify: Option<u32>,
}

#[derive(Default)]
struct MciState {
    devices: HashMap<u32, MciDevice>,
    next_id: u32,
    player: MusicPlayer,
    /// Device whose play is in progress (for notifications).
    playing_dev: Option<u32>,
    num_tracks: Option<u32>,
    external_mixer: bool,
    service_added: bool,
    resolver_set: bool,
}

fn mci(m: &mut Machine) -> s3_runtime::Shared<MciState> {
    m.state::<MciState>()
}

fn track_guest_path(n: u32) -> String {
    format!("{}\\MUSIC\\Track{:02}.ogg", s3_runtime::GUEST_GAME_DIR, n)
}

fn ensure_music(m: &mut Machine) {
    let st = mci(m);
    let (need_resolver, need_service) = {
        let s = st.borrow_mut();
        (!s.resolver_set, !s.service_added)
    };
    if need_resolver {
        let vfs_game = m.host.game_dir();
        let vfs_data = m.host.data_dir();
        let resolver = move |n: u32| {
            let v = s3_runtime::Vfs::new(vfs_game.clone(), vfs_data.clone());
            v.resolve_read(&track_guest_path(n))
        };
        let mut s = st.borrow_mut();
        s.player.set_resolver(Box::new(resolver));
        s.resolver_set = true;
        // Debug/test knob: S3_MUSIC_VOLUME=0..1 overrides the default CD music volume (0 isolates effects).
        if let Some(v) = std::env::var("S3_MUSIC_VOLUME").ok().and_then(|v| v.parse::<f32>().ok()) {
            s.player.volume = v.clamp(0.0, 1.0);
        }
        if s.num_tracks.is_none() {
            let mut max = 1;
            for n in 2..100 {
                if m.vfs.resolve_read(&track_guest_path(n)).is_some() {
                    max = n;
                }
            }
            s.num_tracks = Some(max);
            drop(s);
            m.log(LogLevel::Info, &format!("MCI cdaudio: {max} tracks (MUSIC\\TrackNN.ogg)"));
        }
    }
    if need_service {
        mci(m).borrow_mut().service_added = true;
        m.add_service("winmm-music", music_service);
    }
}

/// Let another mixer (DirectSound) own the host audio stream; it must then call [`mix_music`].
pub fn set_external_music_mixer(m: &mut Machine, external: bool) {
    mci(m).borrow_mut().external_mixer = external;
}

/// Add the current CD music into `out` (interleaved i16, `channels`, `rate`). Posts MCI notifications.
pub fn mix_music(m: &mut Machine, out: &mut [i16], rate: u32, channels: u16) {
    let now = m.now_ms();
    let st = mci(m);
    let ev = st.borrow_mut().player.mix(out, channels, rate, now);
    if ev == Some(MusicEvent::Finished) {
        finish_play(m, MCI_NOTIFY_SUCCESSFUL);
    }
}

/// Set the music volume (0..1).
pub fn set_music_volume(m: &mut Machine, v: f32) {
    mci(m).borrow_mut().player.volume = v.clamp(0.0, 1.0);
}

const OUT_RATE: u32 = 44100;
/// Music kept queued ahead in the shared mixer stream (more than the mixer's 250 ms max catch-up chunk).
const LEAD_MS: u64 = 300;
/// Id of the CD music stream in `s3_runtime::audio::AudioStreams` ("CDDA").
pub const MUSIC_STREAM_ID: u32 = 0x4344_4441;

/// Drop music that is queued but not yet mixed (stop / new play), so the old track does not linger.
fn flush_music_stream(m: &mut Machine) {
    m.state::<AudioStreams>().borrow_mut().streams.remove(&MUSIC_STREAM_ID);
}

/// Music producer: decodes ahead into the `AudioStreams` entry [`MUSIC_STREAM_ID`] (44100 Hz stereo, ~LEAD_MS
/// queued); the DirectSound mixer (s3-dx, the only `Host::queue_audio` producer) mixes it with the effects.
/// Earlier versions queued to `Host::queue_audio` directly, which interleaved music blocks *between* the
/// mixer's blocks (two producers -> twice as much audio as wall time, effects and music not overlapped).
fn music_service(m: &mut Machine) {
    let st = mci(m);
    {
        let s = st.borrow_mut();
        if s.external_mixer || !s.player.is_playing() {
            return;
        }
    }
    let streams = m.state::<AudioStreams>();
    let queued = streams.borrow_mut().streams.get(&MUSIC_STREAM_ID).map_or(0, |s| s.queued_frames());
    let want = (LEAD_MS * OUT_RATE as u64 / 1000) as usize;
    if queued + 512 > want {
        return;
    }
    let frames = want - queued;
    let mut buf = vec![0i16; frames * 2];
    mix_music(m, &mut buf, OUT_RATE, 2);
    let mut ex = streams.borrow_mut();
    let e = ex.streams.entry(MUSIC_STREAM_ID).or_default();
    e.rate = OUT_RATE;
    e.channels = 2;
    e.gain = 1.0;
    e.gain_set = true;
    e.samples.extend(buf);
}

/// End the current play: post MM_MCINOTIFY(code) if the play asked for notification.
fn finish_play(m: &mut Machine, code: u32) {
    let st = mci(m);
    let (dev, notify) = {
        let mut s = st.borrow_mut();
        let Some(dev) = s.playing_dev.take() else { return };
        let notify = s.devices.get_mut(&dev).and_then(|d| d.notify.take());
        (dev, notify)
    };
    if let Some(hwnd) = notify {
        m.log(LogLevel::Debug, &format!("MCI notify dev {dev} code {code} -> hwnd {hwnd:#x}"));
        crate::user32::post_message(m, hwnd, MM_MCINOTIFY, code, dev);
    }
}

fn open_device(m: &mut Machine, ty: &str, alias: Option<&str>) -> Result<u32, u32> {
    let t = ty.to_ascii_lowercase();
    if t.is_empty() {
        return Err(MCIERR_INVALID_DEVICE_NAME);
    }
    let video = t != "cdaudio";
    if video {
        m.log(LogLevel::Info, &format!("MCI open {ty:?}: fake video device (plays end immediately)"));
    } else {
        ensure_music(m);
    }
    let st = mci(m);
    let mut s = st.borrow_mut();
    s.next_id += 1;
    let id = s.next_id;
    s.devices.insert(id, MciDevice { alias: alias.unwrap_or(&t).to_string(), video, time_format: MCI_FORMAT_MSF, notify: None });
    Ok(id)
}

fn close_device(m: &mut Machine, id: u32) -> u32 {
    let playing = mci(m).borrow_mut().playing_dev == Some(id);
    if playing {
        mci(m).borrow_mut().player.stop();
        flush_music_stream(m);
        finish_play(m, MCI_NOTIFY_ABORTED);
    }
    if mci(m).borrow_mut().devices.remove(&id).is_some() {
        0
    } else {
        MCIERR_INVALID_DEVICE_ID
    }
}

/// Track number for a position in the device time format.
fn pos_to_track(fmt: u32, v: u32) -> u32 {
    match fmt {
        MCI_FORMAT_TMSF => v & 0xFF,
        _ => v, // HYPOTHESIS: only TMSF is used by S3 (0x615e60 sets it before playing)
    }
}

fn play(m: &mut Machine, dev: u32, from: Option<u32>, to: Option<u32>, notify: Option<u32>) -> u32 {
    let (fmt, ntracks, cur, video) = {
        let st = mci(m);
        let s = st.borrow_mut();
        let Some(d) = s.devices.get(&dev) else { return MCIERR_INVALID_DEVICE_ID };
        (d.time_format, s.num_tracks.unwrap_or(1), s.player.cur_track, d.video)
    };
    if video {
        if let Some(hwnd) = notify {
            m.log(LogLevel::Info, &format!("MCI video play: skipped, notifying hwnd {hwnd:#x}"));
            crate::user32::post_message(m, hwnd, MM_MCINOTIFY, MCI_NOTIFY_SUCCESSFUL, dev);
        }
        return 0;
    }
    // A new play supersedes the pending notification.
    finish_play(m, MCI_NOTIFY_SUPERSEDED);
    let from_t = from.map(|v| pos_to_track(fmt, v)).unwrap_or(if cur >= 2 { cur } else { 2 });
    let to_t = to.map(|v| {
        let t = pos_to_track(fmt, v);
        // TMSF "to" = start of track t -> play up to t-1; a non-zero MSF part means "into track t" (round up).
        if fmt == MCI_FORMAT_TMSF && (v >> 8) != 0 { t + 1 } else { t }
    });
    let to_t = to_t.unwrap_or(ntracks + 1).max(from_t + 1);
    if from_t < 1 || from_t > ntracks {
        return MCIERR_OUTOFRANGE;
    }
    let now = m.now_ms();
    flush_music_stream(m);
    let r = mci(m).borrow_mut().player.play(from_t, to_t, now);
    match r {
        Ok(()) => {
            m.log(LogLevel::Info, &format!("MCI cdaudio play tracks {from_t}..{to_t} (notify {notify:?})"));
            let st = mci(m);
            let mut s = st.borrow_mut();
            s.playing_dev = Some(dev);
            if let Some(d) = s.devices.get_mut(&dev) {
                d.notify = notify;
            }
            0
        }
        Err(e) => {
            m.log(LogLevel::Warn, &format!("MCI cdaudio play {from_t}: {e}"));
            MCIERR_HARDWARE
        }
    }
}

fn stop(m: &mut Machine, dev: u32) -> u32 {
    match mci(m).borrow_mut().devices.get(&dev) {
        None => return MCIERR_INVALID_DEVICE_ID,
        Some(d) if d.video => return 0,
        Some(_) => {}
    }
    let playing = mci(m).borrow_mut().playing_dev == Some(dev);
    if playing {
        mci(m).borrow_mut().player.stop();
        flush_music_stream(m);
        finish_play(m, MCI_NOTIFY_ABORTED);
    }
    0
}

fn track_length_ms(m: &mut Machine, track: u32) -> u32 {
    m.vfs.resolve_read(&track_guest_path(track)).and_then(|p| ogg_duration_ms(&p)).unwrap_or(0)
}

fn ms_to_msf(ms: u32) -> u32 {
    let frames = ms * 75 / 1000;
    let (mm, ss, ff) = (frames / (75 * 60), (frames / 75) % 60, frames % 75);
    mm | ss << 8 | ff << 16
}

/// Status item value.
fn status(m: &mut Machine, dev: u32, item: u32, track: Option<u32>) -> Result<u32, u32> {
    let (fmt, ntracks, playing, paused, cur, started) = {
        let st = mci(m);
        let s = st.borrow_mut();
        let Some(d) = s.devices.get(&dev) else { return Err(MCIERR_INVALID_DEVICE_ID) };
        if d.video {
            return Ok(match item {
                4 => MCI_MODE_STOP,
                5 | 7 => 1,
                6 => d.time_format,
                _ => 0,
            });
        }
        let playing = s.playing_dev == Some(dev) && s.player.track.is_some();
        (d.time_format, s.num_tracks.unwrap_or(1), playing, s.player.paused, s.player.cur_track, s.player.track_started_ms)
    };
    Ok(match item {
        1 => {
            // MCI_STATUS_LENGTH
            let ms = match track {
                Some(t) => track_length_ms(m, t),
                None => (2..=ntracks).map(|t| track_length_ms(m, t)).sum(),
            };
            if fmt == MCI_FORMAT_MILLISECONDS { ms } else { ms_to_msf(ms) }
        }
        2 => {
            // MCI_STATUS_POSITION
            let el = if playing { (m.now_ms() - started) as u32 } else { 0 };
            match fmt {
                MCI_FORMAT_TMSF => cur.max(1) | (ms_to_msf(el) << 8),
                MCI_FORMAT_MILLISECONDS => el,
                _ => ms_to_msf(el),
            }
        }
        3 => ntracks,
        4 => {
            if playing && paused {
                MCI_MODE_PAUSE
            } else if playing {
                MCI_MODE_PLAY
            } else {
                MCI_MODE_STOP
            }
        }
        5 | 7 => 1,      // media present / ready
        6 => fmt,        // time format
        8 => cur.max(1), // current track
        0x4001 => {
            // MCI_CDA_STATUS_TYPE_TRACK
            if track == Some(1) { 1089 } else { 1088 }
        }
        _ => return Err(MCIERR_UNSUPPORTED_FUNCTION),
    })
}

/// mciSendCommandA(IDDevice, uMsg, fdwCommand, dwParam)
fn mci_send_command_a(m: &mut Machine) -> ThunkResult {
    let (dev, msg, flags, p) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_u32(3));
    let notify_hwnd = if flags & MCI_FLAG_NOTIFY != 0 && p != 0 { Some(m.mem.read_u32(p) & 0xFFFF_FFFF) } else { None };
    let r = match msg {
        MCI_OPEN => {
            // MCI_OPEN_PARMSA {dwCallback, wDeviceID, lpstrDeviceType, lpstrElementName, lpstrAlias}
            let ty_p = m.mem.read_u32(p + 8);
            let ty = if flags & 0x1000 != 0 {
                // MCI_OPEN_TYPE_ID: low word = MCI_DEVTYPE_*
                if ty_p & 0xFFFF == 516 { "cdaudio".to_string() } else { format!("devtype{}", ty_p & 0xFFFF) }
            } else if flags & 0x2000 != 0 && ty_p != 0 {
                m.mem.read_cstr(ty_p)
            } else if flags & 0x200 != 0 {
                m.mem.read_cstr(m.mem.read_u32(p + 12))
            } else {
                String::new()
            };
            let alias = if flags & 0x400 != 0 { Some(m.mem.read_cstr(m.mem.read_u32(p + 16))) } else { None };
            match open_device(m, &ty, alias.as_deref()) {
                Ok(id) => {
                    m.mem.write_u32(p + 4, id);
                    m.log(LogLevel::Info, &format!("MCI_OPEN {ty:?} -> device {id}"));
                    0
                }
                Err(e) => e,
            }
        }
        MCI_CLOSE => close_device(m, dev),
        MCI_SET => {
            if flags & 0x400 != 0 {
                let f = m.mem.read_u32(p + 4);
                let st = mci(m);
                let mut s = st.borrow_mut();
                match s.devices.get_mut(&dev) {
                    Some(d) => {
                        d.time_format = f;
                        0
                    }
                    None => MCIERR_INVALID_DEVICE_ID,
                }
            } else if mci(m).borrow_mut().devices.contains_key(&dev) {
                0
            } else {
                MCIERR_INVALID_DEVICE_ID
            }
        }
        MCI_PLAY => {
            let from = if flags & MCI_FLAG_FROM != 0 { Some(m.mem.read_u32(p + 4)) } else { None };
            let to = if flags & MCI_FLAG_TO != 0 { Some(m.mem.read_u32(p + 8)) } else { None };
            play(m, dev, from, to, notify_hwnd)
        }
        MCI_STOP => stop(m, dev),
        MCI_PAUSE => {
            mci(m).borrow_mut().player.paused = true;
            0
        }
        MCI_RESUME => {
            mci(m).borrow_mut().player.paused = false;
            0
        }
        MCI_SEEK => stop(m, dev),
        MCI_STATUS => {
            // MCI_STATUS_PARMS {dwCallback, dwReturn, dwItem, dwTrack}
            let item = m.mem.read_u32(p + 8);
            let track = if flags & MCI_FLAG_TRACK != 0 { Some(m.mem.read_u32(p + 12)) } else { None };
            match status(m, dev, item, track) {
                Ok(v) => {
                    m.mem.write_u32(p + 4, v);
                    0
                }
                Err(e) => e,
            }
        }
        MCI_GETDEVCAPS => {
            if p != 0 {
                m.mem.write_u32(p + 4, 1);
            }
            0
        }
        MCI_INFO => MCIERR_UNSUPPORTED_FUNCTION,
        other => {
            m.log(LogLevel::Debug, &format!("mciSendCommandA: unsupported message {other:#x}"));
            MCIERR_UNRECOGNIZED_COMMAND
        }
    };
    if r != 0 {
        m.log(LogLevel::Debug, &format!("mciSendCommandA(dev {dev}, {msg:#x}, flags {flags:#x}) -> error {r}"));
    }
    ThunkResult::stdcall(r, 4)
}

/// A parsed MCI command string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MciCommand {
    pub verb: String,
    pub device: String,
    /// Remaining words (lower-case keywords; values kept as written).
    pub args: Vec<String>,
}

/// Parse "verb device args..." (quoted device names allowed).
pub fn parse_mci_string(s: &str) -> Option<MciCommand> {
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut inq = false;
    for c in s.trim().chars() {
        match c {
            '"' => inq = !inq,
            c if c.is_whitespace() && !inq => {
                if !cur.is_empty() {
                    words.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    if words.len() < 2 {
        return None;
    }
    let verb = words.remove(0).to_ascii_lowercase();
    let device = words.remove(0);
    Some(MciCommand { verb, device, args: words })
}

impl MciCommand {
    fn has(&self, w: &str) -> bool {
        self.args.iter().any(|a| a.eq_ignore_ascii_case(w))
    }
    fn value_after(&self, w: &str) -> Option<&str> {
        let i = self.args.iter().position(|a| a.eq_ignore_ascii_case(w))?;
        self.args.get(i + 1).map(|s| s.as_str())
    }
    /// "status x number of tracks" -> "number of tracks".
    fn rest(&self) -> String {
        self.args.iter().map(|a| a.to_ascii_lowercase()).collect::<Vec<_>>().join(" ")
    }
}

/// Parse a position in TMSF ("3", "3:00:00:00") / ms / MSF text.
pub fn parse_position(s: &str, fmt: u32) -> Option<u32> {
    let parts: Vec<u32> = s.split(':').map(|p| p.trim().parse().ok()).collect::<Option<Vec<_>>>()?;
    Some(match fmt {
        MCI_FORMAT_TMSF => parts.iter().take(4).enumerate().fold(0, |acc, (i, v)| acc | (v & 0xFF) << (8 * i)),
        MCI_FORMAT_MSF => parts.iter().take(3).enumerate().fold(0, |acc, (i, v)| acc | (v & 0xFF) << (8 * i)),
        _ => parts[0],
    })
}

fn device_by_name(m: &mut Machine, name: &str) -> Option<u32> {
    let st = mci(m);
    let s = st.borrow_mut();
    s.devices.iter().find(|(_, d)| d.alias.eq_ignore_ascii_case(name)).map(|(id, _)| *id)
}

fn exec_mci_string(m: &mut Machine, cmd: &MciCommand, hwnd_cb: u32) -> Result<String, u32> {
    let notify = if cmd.has("notify") { Some(hwnd_cb) } else { None };
    if cmd.verb == "open" {
        let (ty, _elem) = match cmd.device.split_once('!') {
            Some((t, e)) => (t.to_string(), Some(e.to_string())),
            None => (cmd.value_after("type").map(|s| s.to_string()).unwrap_or(cmd.device.clone()), None),
        };
        let alias = cmd.value_after("alias").map(|s| s.to_string());
        let id = open_device(m, &ty, Some(alias.as_deref().unwrap_or(&cmd.device)))?;
        return Ok(id.to_string());
    }
    if cmd.verb == "close" && cmd.device.eq_ignore_ascii_case("all") {
        let ids: Vec<u32> = mci(m).borrow_mut().devices.keys().copied().collect();
        for id in ids {
            close_device(m, id);
        }
        return Ok(String::new());
    }
    let dev = device_by_name(m, &cmd.device).ok_or(MCIERR_INVALID_DEVICE_NAME)?;
    let fmt = mci(m).borrow_mut().devices.get(&dev).map(|d| d.time_format).unwrap_or(MCI_FORMAT_MSF);
    let err = |r: u32| if r == 0 { Ok(String::new()) } else { Err(r) };
    match cmd.verb.as_str() {
        "close" => err(close_device(m, dev)),
        "stop" | "seek" => err(stop(m, dev)),
        "pause" => {
            mci(m).borrow_mut().player.paused = true;
            Ok(String::new())
        }
        "resume" => {
            mci(m).borrow_mut().player.paused = false;
            Ok(String::new())
        }
        "play" => {
            let from = cmd.value_after("from").and_then(|v| parse_position(v, fmt));
            let to = cmd.value_after("to").and_then(|v| parse_position(v, fmt));
            err(play(m, dev, from, to, notify))
        }
        "set" => {
            let r = cmd.rest();
            if let Some(f) = r.strip_prefix("time format ") {
                let f = match f.trim() {
                    "tmsf" => MCI_FORMAT_TMSF,
                    "msf" => MCI_FORMAT_MSF,
                    "milliseconds" | "ms" => MCI_FORMAT_MILLISECONDS,
                    _ => return Err(MCIERR_OUTOFRANGE),
                };
                if let Some(d) = mci(m).borrow_mut().devices.get_mut(&dev) {
                    d.time_format = f;
                }
            }
            Ok(String::new())
        }
        "status" => {
            let r = cmd.rest().replace(" wait", "").replace(" notify", "");
            let (item, track) = if let Some(t) = r.strip_prefix("length track ") {
                (1, t.trim().parse().ok())
            } else if let Some(t) = r.strip_prefix("type track ") {
                (0x4001, t.trim().parse().ok())
            } else {
                let item = match r.trim() {
                    "window handle" => return Ok("0".into()),
                    "length" => 1,
                    "position" => 2,
                    "number of tracks" => 3,
                    "mode" => 4,
                    "media present" => 5,
                    "time format" => 6,
                    "ready" => 7,
                    "current track" => 8,
                    _ => return Err(MCIERR_UNSUPPORTED_FUNCTION),
                };
                (item, None)
            };
            let v = status(m, dev, item, track)?;
            Ok(match item {
                4 => match v {
                    MCI_MODE_PLAY => "playing".into(),
                    MCI_MODE_PAUSE => "paused".into(),
                    _ => "stopped".into(),
                },
                5 | 7 => "true".into(),
                6 => match v {
                    MCI_FORMAT_TMSF => "tmsf".into(),
                    MCI_FORMAT_MILLISECONDS => "milliseconds".into(),
                    _ => "msf".into(),
                },
                0x4001 => if v == 1088 { "audio".into() } else { "other".into() },
                1 | 2 if fmt != MCI_FORMAT_MILLISECONDS => {
                    let b = v.to_le_bytes();
                    if item == 2 && fmt == MCI_FORMAT_TMSF {
                        format!("{:02}:{:02}:{:02}:{:02}", b[0], b[1], b[2], b[3])
                    } else {
                        format!("{:02}:{:02}:{:02}", b[0], b[1], b[2])
                    }
                }
                _ => v.to_string(),
            })
        }
        "window" | "put" | "where" | "realize" | "update" | "cue" | "setaudio" | "setvideo" => Ok(String::new()),
        _ => Err(MCIERR_UNRECOGNIZED_COMMAND),
    }
}

/// mciSendStringA(lpszCommand, lpszReturnString, cchReturn, hwndCallback)
fn mci_send_string_a(m: &mut Machine) -> ThunkResult {
    let s = m.arg_str(0).unwrap_or_default();
    let (ret, cch, hwnd) = (m.arg_u32(1), m.arg_u32(2), m.arg_u32(3));
    let r = match parse_mci_string(&s) {
        None => Err(MCIERR_UNRECOGNIZED_COMMAND),
        Some(c) => exec_mci_string(m, &c, hwnd),
    };
    m.log(LogLevel::Info, &format!("mciSendStringA({s:?}) -> {r:?}"));
    let (code, text) = match r {
        Ok(t) => (0, t),
        Err(e) => (e, String::new()),
    };
    if ret != 0 && cch > 0 {
        let b: Vec<u8> = text.bytes().take(cch as usize - 1).collect();
        m.mem.write_cstr_bytes(ret, &b);
    }
    ThunkResult::stdcall(code, 4)
}

/// Error text for an MCI error code.
pub fn mci_error_text(e: u32) -> &'static str {
    match e {
        0 => "The specified command was carried out.",
        MCIERR_INVALID_DEVICE_ID => "Invalid MCI device ID.",
        MCIERR_UNRECOGNIZED_COMMAND => "The driver cannot recognize the specified command.",
        MCIERR_HARDWARE => "The specified device is not ready.",
        MCIERR_INVALID_DEVICE_NAME => "The specified device is not open or is not recognized by MCI.",
        MCIERR_CANNOT_LOAD_DRIVER => "The specified device driver cannot be loaded (not supported by the port).",
        MCIERR_UNSUPPORTED_FUNCTION => "The MCI device driver does not support this command.",
        MCIERR_OUTOFRANGE => "The specified parameter value is out of range.",
        _ => "Unknown MCI error.",
    }
}

/// mciGetErrorStringA(fdwError, lpszErrorText, cchErrorText)
fn mci_get_error_string_a(m: &mut Machine) -> ThunkResult {
    let (e, buf, n) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    if buf == 0 || n == 0 {
        return ThunkResult::stdcall(0, 3);
    }
    let b: Vec<u8> = mci_error_text(e).bytes().take(n as usize - 1).collect();
    m.mem.write_cstr_bytes(buf, &b);
    ThunkResult::stdcall(1, 3)
}

fn mci_get_device_id_a(m: &mut Machine) -> ThunkResult {
    let name = m.arg_str(0).unwrap_or_default();
    let id = device_by_name(m, &name).unwrap_or(0);
    ThunkResult::stdcall(id, 1)
}

fn aux_set_volume(m: &mut Machine) -> ThunkResult {
    let v = m.arg_u32(1);
    let l = (v & 0xFFFF) as f32 / 65535.0;
    set_music_volume(m, l);
    ThunkResult::stdcall(0, 2)
}

fn aux_get_volume(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(1);
    let v = (mci(m).borrow_mut().player.volume * 65535.0) as u32;
    if p != 0 {
        m.mem.write_u32(p, v | v << 16);
    }
    ThunkResult::stdcall(0, 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mci_string_parsing() {
        let c = parse_mci_string("open AVIVideo!.\\S3\\Add\\001\\01_00_00.dat alias test").unwrap();
        assert_eq!(c.verb, "open");
        assert_eq!(c.device, "AVIVideo!.\\S3\\Add\\001\\01_00_00.dat");
        assert_eq!(c.value_after("ALIAS"), Some("test"));
        let c = parse_mci_string("play cd from 3 to 4 notify").unwrap();
        assert!(c.has("notify"));
        assert_eq!(c.value_after("from"), Some("3"));
        let c = parse_mci_string("status \"my cd\" number of tracks").unwrap();
        assert_eq!(c.device, "my cd");
        assert_eq!(c.rest(), "number of tracks");
        assert!(parse_mci_string("close").is_none());
        assert_eq!(parse_position("3", MCI_FORMAT_TMSF), Some(3));
        assert_eq!(parse_position("3:01:02:00", MCI_FORMAT_TMSF), Some(3 | 1 << 8 | 2 << 16));
        assert_eq!(parse_position("1500", MCI_FORMAT_MILLISECONDS), Some(1500));
        assert_eq!(ms_to_msf(61_000), 1 | 1 << 8);
        assert_eq!(pos_to_track(MCI_FORMAT_TMSF, 0x0000_0007), 7);
    }
}
