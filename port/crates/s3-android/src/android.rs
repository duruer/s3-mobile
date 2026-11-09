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
use crate::background::{Background, NOT_IN_GAME_PAUSE_MS};
use crate::bgservice;
use crate::frontend::{run_engine, EngineEnd, Shared};
use crate::launcher::{Launcher, Request, Touch};
use jni::objects::JValue;
use jni::{jni_sig, jni_str};
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

/// Frame cap while a game runs in the background (docs/porting/android.md "Background play").
const BACKGROUND_FPS: u32 = 10;

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
    log::info!("S3 Mobile {} starting", env!("CARGO_PKG_VERSION"));
    app.set_window_flags(WindowManagerFlags::FULLSCREEN | WindowManagerFlags::KEEP_SCREEN_ON, WindowManagerFlags::empty());
    crate::wifilock::install(&app);
    let ui_thread = UiThread::new(&app);
    if let Some(u) = &ui_thread {
        u.immersive();
    }
    let kb_visible = Arc::new(Mutex::new(false));

    let base = app
        .external_data_path()
        .or_else(|| app.internal_data_path())
        .unwrap_or_else(|| "/sdcard/Android/data/com.settlers3port.app/files".into());
    let data_dir = base.join("data");
    // `<data dir>/game-dir.txt` (debug / tests) points the game dir elsewhere.
    let game_dir = crate::launcher::game_dir_override(&data_dir, base.join("game"));
    let _ = std::fs::create_dir_all(&game_dir);
    log::info!("game dir: {} (copy your own install here); data dir: {}", game_dir.display(), data_dir.display());

    let data_dir_ui = data_dir.clone();
    let waker = app.create_waker();
    let shared = Shared::new(Box::new(move || waker.wake()));
    // Launcher first (docs/porting/android.md "Launcher"); the engine thread starts on Play and ends with the game.
    let mut launcher = Launcher::new(game_dir.clone(), data_dir.clone());
    let mut in_launcher = true;
    let mut launcher_drawn: Option<(u32, u32, u64)> = None;
    let mut launcher_dirty = true;
    let mut engine_thread: Option<std::thread::JoinHandle<()>> = None;
    let mut reqs: Vec<Request> = Vec::new();
    // `am start ... --ez play true` (scripts): skip the launcher once when the files are there.
    let autoplay = Arc::new(AtomicBool::new(false));
    let mut autoplay_checked = false;
    if let Some(u) = &ui_thread {
        let a = autoplay.clone();
        u.run("intent extras", Box::new(move |env, act| {
            let intent = env.call_method(act, jni_str!("getIntent"), jni_sig!("()Landroid/content/Intent;"), &[])?.l()?;
            if !intent.is_null() {
                let key = env.new_string("play")?;
                let v = env
                    .call_method(&intent, jni_str!("getBooleanExtra"), jni_sig!("(Ljava/lang/String;Z)Z"), &[JValue::Object(&key), JValue::Bool(false)])?
                    .z()?;
                a.store(v, Ordering::Release);
            }
            Ok(())
        }));
    }
    let start_t = Instant::now();

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
    // Background play: Some while the app is in the background and the game keeps running (foreground service +
    // notifications); `bg_out_of_game` = since when the game is no longer in a game (then pause like in menus).
    let mut bg: Option<Background> = None;
    let mut bg_out_of_game: Option<u64> = None;
    let mut asked_permission = false;

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
                    shared.density_dpi.store(app.config().density().unwrap_or(0), std::sync::atomic::Ordering::Relaxed);
                    presenter.set_window(app.native_window());
                }
                MainEvent::TerminateWindow { .. } => presenter.set_window(None),
                MainEvent::WindowResized { .. } | MainEvent::ConfigChanged { .. } => {
                    // Folding / unfolding changes the density too.
                    shared.density_dpi.store(app.config().density().unwrap_or(0), std::sync::atomic::Ordering::Relaxed);
                    presenter.set_window(app.native_window());
                }
                MainEvent::RedrawNeeded { .. } | MainEvent::ContentRectChanged { .. } => presenter.invalidate(),
                MainEvent::Pause => {
                    paused = true;
                    if shared.in_game.load(Ordering::Acquire) {
                        // In a game: keep it running (foreground service, notifications). Sound is muted.
                        let gm = shared.game_ms.load(Ordering::Acquire);
                        log::info!("lifecycle: pause -> background play (game time {} s)", gm / 1000);
                        shared.bg_lines.lock().unwrap().clear();
                        shared.background.store(true, Ordering::Release);
                        crate::frontend::background_fps(Some(BACKGROUND_FPS));
                        bgservice::start(&app, gm);
                        let mut b = Background::default();
                        b.begin(now_ms(t0), gm);
                        bg = Some(b);
                        bg_out_of_game = None;
                    } else {
                        log::info!("lifecycle: pause");
                        shared.set_paused(true);
                    }
                    if let Some(s) = &audio {
                        let _ = s.request_pause();
                    }
                }
                MainEvent::Resume { .. } => {
                    log::info!("lifecycle: resume");
                    paused = false;
                    if bg.take().is_some() {
                        log::info!("background play: back in the foreground (game time {} s)", shared.game_ms.load(Ordering::Acquire) / 1000);
                        shared.background.store(false, Ordering::Release);
                        crate::frontend::background_fps(None);
                        bgservice::stop(&app);
                    }
                    shared.set_paused(false);
                    // Drop sound queued before the pause (it would play late).
                    shared.audio.clear();
                    if let Some(s) = &audio {
                        let _ = s.request_start();
                    }
                    presenter.invalidate();
                }
                MainEvent::LostFocus => log::info!("lifecycle: lost focus"),
                MainEvent::GainedFocus => {
                    log::info!("lifecycle: gained focus");
                    // Notifications for background play (Android 13+): asked once per install, in the menus.
                    if !asked_permission && !shared.in_game.load(Ordering::Acquire) {
                        asked_permission = true;
                        if let Some(u) = &ui_thread {
                            bgservice::ask_notification_permission(u, &data_dir_ui);
                        }
                    }
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
                    InputEvent::MotionEvent(me) if in_launcher => {
                        // Launcher: the first finger only, in window pixels.
                        let t = match me.action() {
                            MotionAction::Down => Touch::Down,
                            MotionAction::Move => Touch::Move,
                            MotionAction::Up => Touch::Up,
                            MotionAction::Cancel => Touch::Cancel,
                            _ => return InputStatus::Handled,
                        };
                        let p = me.pointer_at_index(0);
                        if t == Touch::Down {
                            log::info!("launcher: touch at ({:.0},{:.0})", p.x(), p.y());
                        }
                        if let Some(r) = launcher.touch(t, p.x() as i32, p.y() as i32) {
                            reqs.push(r);
                        }
                        launcher_dirty = true;
                        InputStatus::Handled
                    }
                    InputEvent::KeyEvent(ke) if in_launcher => {
                        let code: u32 = ke.key_code().into();
                        if ke.action() != KeyAction::Down {
                            return InputStatus::Handled;
                        }
                        launcher_dirty = true;
                        if code == 4 {
                            // Back: sub-page -> Home; on Home close the app.
                            match launcher.back() {
                                (true, r) => reqs.extend(r),
                                (false, _) => reqs.push(Request::Quit),
                            }
                            return InputStatus::Handled;
                        }
                        let mapped = crate::keys::android_to_vk(code);
                        let uni = match app.device_key_character_map(ke.device_id()).and_then(|m| m.get(ke.key_code(), ke.meta_state())) {
                            Ok(KeyMapChar::Unicode(c)) => Some(c),
                            _ => None,
                        };
                        match crate::keys::char_for_key(mapped, uni) {
                            Some(c) => {
                                reqs.extend(launcher.char(c));
                                InputStatus::Handled
                            }
                            None => InputStatus::Unhandled,
                        }
                    }
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
                        // Game + window position (window-layer UI hit-tests in window pixels).
                        let pos = |x: f32, y: f32| {
                            let g = to_game(x, y);
                            (g.0, g.1, x.max(0.0) as i32, y.max(0.0) as i32)
                        };
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
                        let all: Vec<(i32, crate::touch::Pos)> =
                            me.pointers().map(|q| (q.pointer_id(), pos(q.x(), q.y()))).collect();
                        for ev in pointer_events(action, (p.pointer_id(), pos(win.0, win.1)), &all) {
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
        let buzz = std::mem::take(&mut *shared.haptic.lock().unwrap());
        if let Some(&ms) = buzz.iter().max() {
            bgservice::vibrate(&app, ms);
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

        // Background play: post new game messages (rate limited), keep the chronometer in sync, and pause like in
        // the menus once the game is over.
        if let Some(b) = &mut bg {
            let t = now_ms(t0);
            let lines = std::mem::take(&mut *shared.bg_lines.lock().unwrap());
            b.push(lines);
            if let Some(l) = b.due(t) {
                log::info!("background play: notify {l:?}");
                bgservice::event(&app, &l, shared.vibrate_messages.load(Ordering::Acquire));
            }
            if shared.in_game.load(Ordering::Acquire) {
                bg_out_of_game = None;
                if let Some(g) = b.drift(t, shared.game_ms.load(Ordering::Acquire)) {
                    log::info!("background play: game time {} s (chronometer re-synced)", g / 1000);
                    bgservice::update(&app, g);
                }
            } else if t.saturating_sub(*bg_out_of_game.get_or_insert(t)) > NOT_IN_GAME_PAUSE_MS {
                log::info!("background play: no longer in a game; pausing");
                bg = None;
                shared.background.store(false, Ordering::Release);
                crate::frontend::background_fps(None);
                bgservice::stop(&app);
                shared.set_paused(true);
            }
        }

        // The engine run ended (Exit Game, error): back to the launcher.
        if engine_thread.is_some() {
            if let Some(end) = shared.engine_end.lock().unwrap().take() {
                if let Some(t) = engine_thread.take() {
                    let _ = t.join();
                }
                log::info!("launcher: engine ended ({}{})", end.title, if end.exited { ", game exited" } else { "" });
                if bg.take().is_some() {
                    shared.background.store(false, Ordering::Release);
                    crate::frontend::background_fps(None);
                    bgservice::stop(&app);
                }
                shared.in_game.store(false, Ordering::Release);
                shared.frame.lock().unwrap().fb = None;
                launcher.message = Some(if end.exited {
                    ("The game was closed. Tap Play to start it again.".to_string(), false)
                } else {
                    let first = end.body.lines().next().unwrap_or("").to_string();
                    (format!("{}: {first}", end.title), true)
                });
                launcher.reload_settings();
                launcher.recheck();
                in_launcher = true;
                launcher_drawn = None;
                launcher_dirty = true;
                if finishing {
                    quit = true;
                }
            }
        }

        if in_launcher && !finishing {
            if !autoplay_checked && start_t.elapsed() > Duration::from_millis(300) {
                autoplay_checked = true;
                if autoplay.load(Ordering::Acquire) && launcher.ready() {
                    log::info!("launcher: autoplay (intent extra play=true)");
                    reqs.push(Request::Play);
                }
            }
            for r in std::mem::take(&mut reqs) {
                log::info!("launcher: {r:?}");
                match r {
                    Request::Play if engine_thread.is_none() => {
                        shared.engine_end.lock().unwrap().take();
                        shared.guest_exited.store(false, Ordering::Release);
                        shared.events.lock().unwrap().clear();
                        shared.frame.lock().unwrap().fb = None;
                        shared.set_status(crate::frontend::Phase::Starting, "Starting", "Loading the game ...");
                        if *kb_visible.lock().unwrap() {
                            if let Some(u) = &ui_thread {
                                u.soft_keyboard(Some(false), kb_visible.clone());
                            }
                        }
                        let sh = shared.clone();
                        let (g, d) = (launcher.game_dir().to_path_buf(), data_dir.clone());
                        engine_thread = Some(
                            std::thread::Builder::new()
                                .name("s3-engine".into())
                                .stack_size(16 << 20)
                                .spawn(move || {
                                    let end = run_engine(sh.clone(), g, d).unwrap_or(EngineEnd {
                                        title: "Stopped".into(),
                                        body: String::new(),
                                        exited: false,
                                    });
                                    *sh.engine_end.lock().unwrap() = Some(end);
                                    (sh.wake)();
                                })
                                .expect("spawn engine thread"),
                        );
                        in_launcher = false;
                        shared.set_launcher(None);
                    }
                    Request::Play => {}
                    Request::Keyboard(show) => {
                        if let Some(u) = &ui_thread {
                            u.soft_keyboard(Some(show), kb_visible.clone());
                        }
                    }
                    // iOS only (folder picker).
                    Request::PickFolder | Request::UseAppFolder => {}
                    Request::Quit => {
                        finishing = true;
                        launcher.stop_upload();
                        match &ui_thread {
                            Some(u) => u.finish(),
                            None => quit = true,
                        }
                    }
                }
            }
        }
        if in_launcher {
            launcher_dirty |= launcher.tick();
            let size = app.native_window().map(|w| (w.width().max(1) as u32, w.height().max(1) as u32));
            if let Some((w, h)) = size {
                let hz = f32::from_bits(shared.refresh_hz.load(Ordering::Relaxed));
                launcher.refresh_hz = if hz > 0.0 { hz.round() as u32 } else { 0 };
                let key = (w, h, shared.density_dpi.load(Ordering::Relaxed) as u64);
                if launcher_dirty || launcher_drawn.map(|d| (d.0, d.1)) != Some((w, h)) {
                    launcher_dirty = false;
                    let list = launcher.draw(w, h, key.2 as u32);
                    if launcher_drawn != Some((w, h, list.hash)) {
                        launcher_drawn = Some((w, h, list.hash));
                        shared.set_launcher(Some(Arc::new(list)));
                    }
                }
            }
        }
    }

    log::info!("shutting down");
    if bg.is_some() {
        bgservice::stop(&app);
    }
    drop(presenter);
    shared.request_quit();
    if let Some(s) = audio.take() {
        let _ = s.request_stop();
    }
    if let Some(t) = engine_thread.take() {
        let _ = t.join();
    }
    log::info!("bye");
    if finishing {
        // Quit from the launcher: end the process too, so the next launch starts from a clean state.
        std::process::exit(0);
    }
}
