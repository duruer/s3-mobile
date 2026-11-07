//! NativeActivity glue (android-activity): lifecycle, window, input, AAudio. Presentation runs on the render
//! thread in `present` (EGL/GLES).
//! Platform-independent logic lives in `frontend`, `touch`, `layout`, `screen`, `audio`, `keys`.

use s3_mods::display::Viewport;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use android_activity::input::{InputEvent, KeyAction, KeyMapChar, MotionAction, ToolType};
use android_activity::{AndroidApp, InputStatus, MainEvent, PollEvent, WindowManagerFlags};
use ndk::audio::{
    AudioCallbackResult, AudioDirection, AudioFormat, AudioPerformanceMode, AudioSharingMode, AudioStream,
    AudioStreamBuilder,
};
use s3_runtime::{HostEvent, MouseButton};

use crate::audio::{AudioRing, OUT_CHANNELS, OUT_RATE};
use crate::frontend::{run_engine, Shared};
use crate::present::Presenter;
use crate::touch::{pointer_events, Action, PointerFilter};
use crate::uithread::UiThread;

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
    let ui_thread = UiThread::new(&app);
    if let Some(u) = &ui_thread {
        u.immersive();
    }
    let kb_visible = Arc::new(Mutex::new(false));

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
    // Render thread (EGL/GLES presenter, docs/porting/display.md): the engine never waits for the display.
    let presenter = Presenter::start(shared.clone());
    // Refresh rate asked from the window manager (preferredDisplayModeId / preferredRefreshRate), per setting.
    let mut prefs_hz: Option<u32> = None;
    let mut kb_checked = 0u64;
    let mut finishing = false;
    // Fingers -> raw pointer events; gestures are recognized by s3-mods on the engine thread.
    let mut pointers = PointerFilter::default();
    let mut quit = false;
    let mut paused = false;
    let mut touches_logged = 0u32;
    // Viewport frozen while two or more fingers are down: a pinch switches the render size every few frames, and
    // mapping each finger move through the newest viewport made the measured finger distance (game pixels) jump
    // with every switch, so the zoom went back and forth. With one mapping per gesture the distance ratio is the
    // real one on screen.
    let mut pinch_vp: Option<Viewport> = None;

    while !quit {
        let mut wait = 250;
        if *kb_visible.lock().unwrap() {
            wait = wait.min(1000);
        }
        app.poll_events(Some(Duration::from_millis(wait)), |event| match event {
            PollEvent::Main(main) => match main {
                MainEvent::InitWindow { .. } => {
                    if let Some(u) = &ui_thread {
                        u.immersive();
                    }
                    prefs_hz = None;
                    presenter.set_window(app.native_window());
                }
                MainEvent::TerminateWindow { .. } => presenter.set_window(None),
                MainEvent::WindowResized { .. } | MainEvent::ConfigChanged { .. } => presenter.set_window(app.native_window()),
                MainEvent::RedrawNeeded { .. } | MainEvent::ContentRectChanged { .. } => presenter.invalidate(),
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
                    presenter.invalidate();
                }
                MainEvent::LostFocus => log::info!("lifecycle: lost focus"),
                MainEvent::GainedFocus => {
                    log::info!("lifecycle: gained focus");
                    // The system shows the bars again after dialogs / app switches.
                    if let Some(u) = &ui_thread {
                        u.immersive();
                    }
                }
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
                let read = iter.next(|event| match event {
                    InputEvent::MotionEvent(me) => {
                        // Screen -> game: the presenter's viewport (display mode), or the CPU fallback letterbox.
                        let Some(cur_vp) = *shared.viewport.lock().unwrap() else { return InputStatus::Handled };
                        let vp = match me.action() {
                            MotionAction::PointerDown => *pinch_vp.get_or_insert(cur_vp),
                            MotionAction::Up | MotionAction::Cancel => pinch_vp.take().unwrap_or(cur_vp),
                            _ => pinch_vp.unwrap_or(cur_vp),
                        };
                        let lb = *shared.letterbox.lock().unwrap();
                        let to_game = |x: f32, y: f32| match &lb {
                            Some(l) => l.window_to_game(x, y),
                            None => vp.window_to_game(x, y),
                        };
                        let idx = me.pointer_index();
                        let p = me.pointer_at_index(idx);
                        let win = (p.x(), p.y());
                        let game = to_game(win.0, win.1);
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
                        let action = match me.action() {
                            MotionAction::Down => Action::Down,
                            MotionAction::PointerDown => Action::PointerDown,
                            MotionAction::Move => Action::Move,
                            MotionAction::PointerUp => Action::PointerUp,
                            MotionAction::Up => Action::Up,
                            MotionAction::Cancel => Action::Cancel,
                            _ => return InputStatus::Handled,
                        };
                        if action == Action::Down && touches_logged < 1000 {
                            touches_logged += 1;
                            log::info!("touch down at window ({:.0},{:.0}) -> game {:?}", win.0, win.1, game);
                        }
                        let all: Vec<(i32, (i32, i32))> =
                            me.pointers().map(|q| (q.pointer_id(), to_game(q.x(), q.y()))).collect();
                        for ev in pointer_events(action, (p.pointer_id(), game), &all) {
                            if pointers.keep(&ev) {
                                shared.push_event(ev);
                            }
                        }
                        InputStatus::Handled
                    }
                    InputEvent::KeyEvent(ke) => {
                        let code: u32 = ke.key_code().into();
                        let down = match ke.action() {
                            KeyAction::Down => true,
                            KeyAction::Up => false,
                            _ => return InputStatus::Unhandled,
                        };
                        let mapped = crate::keys::android_to_vk(code);
                        // Text: the character the key produces with its meta state (soft keyboard, adb input text).
                        let uni = if down {
                            match app.device_key_character_map(ke.device_id()).and_then(|m| m.get(ke.key_code(), ke.meta_state())) {
                                Ok(KeyMapChar::Unicode(c)) => Some(c),
                                _ => None,
                            }
                        } else {
                            None
                        };
                        let ch = if down { crate::keys::char_for_key(mapped, uni) } else { None };
                        let vk = mapped.or_else(|| uni.and_then(crate::keys::vk_for_char));
                        if vk.is_none() && ch.is_none() {
                            return InputStatus::Unhandled;
                        }
                        if ke.repeat_count() == 0 || !down {
                            log::info!("key {code} -> vk {vk:x?} char {ch:x?} {}", if down { "down" } else { "up" });
                        }
                        if let Some(vk) = vk {
                            shared.push_event(HostEvent::Key { vk, down });
                        }
                        if let Some(c) = ch {
                            shared.push_event(HostEvent::Char(c));
                        }
                        InputStatus::Handled
                    }
                    _ => InputStatus::Unhandled,
                });
                if !read {
                    break;
                }
            }
        }
        if shared.keyboard_req.swap(false, Ordering::AcqRel) {
            log::info!("three-finger tap: toggle soft keyboard");
            if let Some(u) = &ui_thread {
                u.soft_keyboard(None, kb_visible.clone());
            }
        }
        {
            let t = now_ms(t0);
            // While the keyboard is believed open, check once a second whether the user closed it.
            if *kb_visible.lock().unwrap() && t >= kb_checked + 1000 {
                kb_checked = t;
                if let Some(u) = &ui_thread {
                    u.refresh_keyboard(kb_visible.clone());
                }
            }
            // Display cutout + high refresh display mode (window manager side; the presenter also asks the surface).
            let hz = shared.display.lock().unwrap().refresh_hz;
            if prefs_hz != Some(hz) && ui_thread.is_some() {
                prefs_hz = Some(hz);
                if let Some(u) = &ui_thread {
                    u.display_prefs(hz, shared.clone());
                }
            }
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

        if shared.guest_exited.load(Ordering::Acquire) && !finishing {
            finishing = true;
            match &ui_thread {
                Some(u) => u.finish(),
                None => quit = true,
            }
        }
    }

    log::info!("shutting down");
    drop(presenter);
    shared.request_quit();
    if let Some(s) = audio.take() {
        let _ = s.request_stop();
    }
    let _ = engine_thread.join();
    log::info!("bye");
    if finishing {
        // The game ended: end the process too, so the next launch starts from a clean state.
        std::process::exit(0);
    }
}
