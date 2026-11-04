//! NativeActivity glue (android-activity): lifecycle, window, input, presentation, AAudio.
//! Platform-independent logic lives in `frontend`, `touch`, `layout`, `screen`, `audio`, `keys`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use android_activity::input::{InputEvent, KeyAction, MotionAction, ToolType};
use android_activity::{AndroidApp, InputStatus, MainEvent, PollEvent, WindowManagerFlags};
use ndk::audio::{
    AudioCallbackResult, AudioDirection, AudioFormat, AudioPerformanceMode, AudioSharingMode, AudioStream,
    AudioStreamBuilder,
};
use ndk::hardware_buffer_format::HardwareBufferFormat;
use ndk::native_window::NativeWindow;
use s3_runtime::{FrameBuffer, HostEvent, MouseButton};

use crate::audio::{AudioRing, OUT_CHANNELS, OUT_RATE};
use crate::frontend::{run_engine, Phase, Shared};
use crate::layout::{blit_letterboxed, Letterbox};
use crate::screen;
use crate::touch::TouchMapper;

struct Ui {
    window: Option<NativeWindow>,
    /// Real window size (before our buffer geometry), physical pixels.
    win_size: (u32, u32),
    /// Geometry currently set on the window (buf_w, buf_h).
    geometry: Option<(u32, u32)>,
    lb: Option<Letterbox>,
    shown: (u64, u64),
    status_cache: Option<(u64, FrameBuffer)>,
    dirty: bool,
}

impl Ui {
    fn attach(&mut self, w: Option<NativeWindow>) {
        self.window = w;
        self.geometry = None;
        self.dirty = true;
        if let Some(w) = &self.window {
            // Reset to the window's own size to learn the real dimensions.
            let _ = w.set_buffers_geometry(0, 0, None);
            self.win_size = (w.width().max(1) as u32, w.height().max(1) as u32);
            log::info!("window: {}x{} format {:?}", self.win_size.0, self.win_size.1, w.format());
        }
    }

    fn render(&mut self, shared: &Shared) {
        let Some(window) = &self.window else { return };
        let frame_seq = shared.frame.lock().unwrap().seq;
        let status = shared.status.lock().unwrap().clone();
        if !self.dirty && self.shown == (frame_seq, status.seq) {
            return;
        }
        // Pick the image: game frame (with an overlay when the engine stopped) or the status screen.
        let mut owned: Option<FrameBuffer> = None;
        let slot = shared.frame.lock().unwrap();
        let fb: &FrameBuffer = match &slot.fb {
            Some(fb) if status.phase == Phase::Stopped => {
                let mut c = fb.clone();
                screen::draw_panel(&mut c, &status.title, &status.body, true);
                owned = Some(c);
                owned.as_ref().unwrap()
            }
            Some(fb) => fb,
            None => {
                if self.status_cache.as_ref().map(|c| c.0) != Some(status.seq) {
                    self.status_cache = Some((status.seq, screen::status_screen(&status.title, &status.body)));
                }
                &self.status_cache.as_ref().unwrap().1
            }
        };
        let lb = Letterbox::new(fb.width, fb.height, self.win_size.0, self.win_size.1);
        if self.geometry != Some((lb.buf_w, lb.buf_h)) {
            match window.set_buffers_geometry(lb.buf_w as i32, lb.buf_h as i32, Some(HardwareBufferFormat::R5G6B5_UNORM)) {
                Ok(()) => log::info!(
                    "geometry: game {}x{} -> buffer {}x{} (offset {},{}) on window {}x{}",
                    lb.game_w, lb.game_h, lb.buf_w, lb.buf_h, lb.off_x, lb.off_y, lb.win_w, lb.win_h
                ),
                Err(e) => log::error!("set_buffers_geometry: {e}"),
            }
            self.geometry = Some((lb.buf_w, lb.buf_h));
        }
        self.lb = Some(lb);
        match window.lock(None) {
            Ok(mut guard) => {
                let (bw, bh, stride) = (guard.width(), guard.height(), guard.stride());
                if guard.format() != HardwareBufferFormat::R5G6B5_UNORM {
                    log::warn!("window buffer format {:?}, expected RGB565", guard.format());
                }
                // Centre using the actual buffer size (in case the geometry was not applied yet).
                let mut l = lb;
                l.off_x = (bw as u32).saturating_sub(lb.game_w) / 2;
                l.off_y = (bh as u32).saturating_sub(lb.game_h) / 2;
                let dst = unsafe { std::slice::from_raw_parts_mut(guard.bits() as *mut u16, stride * bh) };
                blit_letterboxed(&l, &fb.pixels, dst, stride, bh);
            }
            Err(e) => log::error!("ANativeWindow_lock: {e}"),
        }
        drop(owned);
        self.shown = (frame_seq, status.seq);
        self.dirty = false;
    }
}

fn open_audio(ring: Arc<AudioRing>, dead: Arc<AtomicBool>) -> Option<AudioStream> {
    let r2 = ring.clone();
    let res = AudioStreamBuilder::new().and_then(|b| {
        b.direction(AudioDirection::Output)
            .format(AudioFormat::PCM_I16)
            .channel_count(OUT_CHANNELS as i32)
            .sample_rate(OUT_RATE as i32)
            .sharing_mode(AudioSharingMode::Shared)
            .performance_mode(AudioPerformanceMode::None)
            .data_callback(Box::new(move |_s, data, frames| {
                let out = unsafe { std::slice::from_raw_parts_mut(data as *mut i16, frames as usize * OUT_CHANNELS) };
                r2.pop_into(out);
                AudioCallbackResult::Continue
            }))
            .error_callback(Box::new(move |_s, e| {
                log::warn!("AAudio error callback: {e:?}");
                dead.store(true, Ordering::Release);
            }))
            .open_stream()
    });
    match res {
        Ok(s) => {
            log::info!(
                "AAudio: {} Hz, {} ch, burst {} frames, buffer {} frames",
                s.sample_rate(),
                s.channel_count(),
                s.frames_per_burst(),
                s.buffer_size_in_frames()
            );
            if let Err(e) = s.request_start() {
                log::warn!("AAudio start: {e:?}");
            }
            ring.running.store(true, Ordering::Release);
            Some(s)
        }
        Err(e) => {
            log::warn!("AAudio unavailable ({e:?}); running without sound");
            None
        }
    }
}

fn now_ms(t0: Instant) -> u64 {
    t0.elapsed().as_millis() as u64
}

#[no_mangle]
fn android_main(app: AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info).with_tag("s3port"),
    );
    std::panic::set_hook(Box::new(|info| {
        log::error!("PANIC: {info}");
    }));
    log::info!("Settlers 3 Port {} starting", env!("CARGO_PKG_VERSION"));
    app.set_window_flags(WindowManagerFlags::FULLSCREEN | WindowManagerFlags::KEEP_SCREEN_ON, WindowManagerFlags::empty());

    let base = app
        .external_data_path()
        .or_else(|| app.internal_data_path())
        .unwrap_or_else(|| "/sdcard/Android/data/com.settlers3port.app/files".into());
    let game_dir = base.join("game");
    let data_dir = base.join("data");
    log::info!("game dir: {} (copy your own install here); data dir: {}", game_dir.display(), data_dir.display());

    let waker = app.create_waker();
    let shared = Shared::new(Box::new(move || waker.wake()));
    let engine_thread = {
        let sh = shared.clone();
        std::thread::Builder::new()
            .name("s3-engine".into())
            .stack_size(16 << 20)
            .spawn(move || run_engine(sh, game_dir, data_dir))
            .expect("spawn engine thread")
    };

    let audio_dead = Arc::new(AtomicBool::new(false));
    let mut audio = open_audio(shared.audio.clone(), audio_dead.clone());

    let t0 = Instant::now();
    let mut ui = Ui { window: None, win_size: (1, 1), geometry: None, lb: None, shown: (0, 0), status_cache: None, dirty: true };
    let mut touch = TouchMapper::new(24.0);
    let mut quit = false;
    let mut paused = false;
    let mut touches_logged = 0u32;

    while !quit {
        let wait = touch.next_deadline(now_ms(t0)).map(|d| d.clamp(1, 250)).unwrap_or(250);
        app.poll_events(Some(Duration::from_millis(wait)), |event| match event {
            PollEvent::Main(main) => match main {
                MainEvent::InitWindow { .. } => {
                    ui.attach(app.native_window());
                    let m = ui.win_size.0.min(ui.win_size.1) as f32;
                    touch.set_slop((m * 0.03).max(8.0));
                }
                MainEvent::TerminateWindow { .. } => ui.attach(None),
                MainEvent::WindowResized { .. } | MainEvent::ConfigChanged { .. } => ui.attach(app.native_window()),
                MainEvent::RedrawNeeded { .. } | MainEvent::ContentRectChanged { .. } => ui.dirty = true,
                MainEvent::Pause => {
                    log::info!("lifecycle: pause");
                    paused = true;
                    shared.set_paused(true);
                    if let Some(s) = &audio {
                        let _ = s.request_pause();
                    }
                }
                MainEvent::Resume { .. } => {
                    log::info!("lifecycle: resume");
                    paused = false;
                    shared.set_paused(false);
                    if let Some(s) = &audio {
                        let _ = s.request_start();
                    }
                    ui.dirty = true;
                }
                MainEvent::LostFocus => log::info!("lifecycle: lost focus"),
                MainEvent::GainedFocus => log::info!("lifecycle: gained focus"),
                MainEvent::LowMemory => log::warn!("lifecycle: low memory"),
                MainEvent::Destroy => {
                    log::info!("lifecycle: destroy");
                    quit = true;
                }
                _ => {}
            },
            _ => {}
        });

        // Input.
        if let Ok(mut iter) = app.input_events_iter() {
            loop {
                let t = now_ms(t0);
                let read = iter.next(|event| match event {
                    InputEvent::MotionEvent(me) => {
                        let Some(lb) = ui.lb else { return InputStatus::Handled };
                        let idx = me.pointer_index();
                        let p = me.pointer_at_index(idx);
                        let win = (p.x(), p.y());
                        let game = lb.window_to_game(win.0, win.1);
                        if p.tool_type() == ToolType::Mouse {
                            // Real mouse (ChromeOS, DeX, emulator with a mouse device): map directly.
                            shared.push_event(HostEvent::MouseMove { x: game.0, y: game.1 });
                            let b = me.button_state();
                            let btn = match me.action() {
                                MotionAction::ButtonPress | MotionAction::ButtonRelease => {
                                    let bs: u32 = me.action_button().into();
                                    if bs & 2 != 0 { Some(MouseButton::Right) } else if bs & 4 != 0 { Some(MouseButton::Middle) } else { Some(MouseButton::Left) }
                                }
                                _ => None,
                            };
                            if let Some(button) = btn {
                                let down = me.action() == MotionAction::ButtonPress;
                                log::info!("mouse {button:?} {} at game {game:?}", if down { "down" } else { "up" });
                                shared.push_event(HostEvent::MouseButton { button, down });
                            }
                            let _ = b;
                            return InputStatus::Handled;
                        }
                        match me.action() {
                            MotionAction::Down => {
                                if touches_logged < 1000 {
                                    touches_logged += 1;
                                    log::info!("touch down at window ({:.0},{:.0}) -> game {:?}", win.0, win.1, game);
                                }
                                touch.down(p.pointer_id(), win, game, t)
                            }
                            MotionAction::PointerDown => touch.pointer_down(t),
                            MotionAction::PointerUp => touch.pointer_up(p.pointer_id(), t),
                            MotionAction::Up => touch.up(t),
                            MotionAction::Cancel => touch.cancel(t),
                            MotionAction::Move => {
                                for p in me.pointers() {
                                    let g = lb.window_to_game(p.x(), p.y());
                                    touch.moved(p.pointer_id(), (p.x(), p.y()), g, t);
                                }
                            }
                            _ => {}
                        }
                        InputStatus::Handled
                    }
                    InputEvent::KeyEvent(ke) => {
                        let code: u32 = ke.key_code().into();
                        match crate::keys::android_to_vk(code) {
                            Some(vk) => {
                                let down = match ke.action() {
                                    KeyAction::Down => true,
                                    KeyAction::Up => false,
                                    _ => return InputStatus::Handled,
                                };
                                if ke.repeat_count() == 0 || !down {
                                    log::info!("key {code} -> vk {vk:#x} {}", if down { "down" } else { "up" });
                                }
                                shared.push_event(HostEvent::Key { vk, down });
                                InputStatus::Handled
                            }
                            None => InputStatus::Unhandled,
                        }
                    }
                    _ => InputStatus::Unhandled,
                });
                if !read {
                    break;
                }
            }
        }
        for ev in touch.drain_due(now_ms(t0)) {
            if let HostEvent::MouseButton { .. } = ev {
                log::info!("touch -> {ev:?}");
            }
            shared.push_event(ev);
        }

        if audio_dead.swap(false, Ordering::AcqRel) {
            log::info!("AAudio: reopening stream");
            drop(audio.take());
            audio = open_audio(shared.audio.clone(), audio_dead.clone());
            if paused {
                if let Some(s) = &audio {
                    let _ = s.request_pause();
                }
            }
        }

        if !paused {
            ui.render(&shared);
        }
    }

    log::info!("shutting down");
    shared.request_quit();
    if let Some(s) = audio.take() {
        let _ = s.request_stop();
    }
    let _ = engine_thread.join();
    log::info!("bye");
}
