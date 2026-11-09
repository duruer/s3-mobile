//! Run small JNI jobs on the Java main (UI) thread without any Java code of our own.
//!
//! android-activity runs `android_main` on its own thread, but View calls (system bar visibility, IME) must run
//! on the UI thread. We register one end of a socket pair on the Java main looper (`ALooper_addFd` with a
//! callback); writing a byte to the other end makes the looper call us on the UI thread, where the queued jobs
//! run with a JNI env and the activity object.

use std::io::{Read, Write};
use std::os::fd::{AsRawFd, BorrowedFd};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};

use android_activity::AndroidApp;
use jni::objects::{JObject, JValue};
use jni::{jni_sig, jni_str, Env, JavaVM};
use ndk::looper::FdEvent;

use crate::frontend::Shared;

type Job = Box<dyn for<'a> FnOnce(&mut Env<'a>, &JObject<'a>) -> jni::errors::Result<()> + Send>;

#[derive(Clone)]
pub struct UiThread {
    tx: Arc<Mutex<UnixStream>>,
    jobs: Arc<Mutex<Vec<(&'static str, Job)>>>,
}

impl UiThread {
    pub fn new(app: &AndroidApp) -> Option<UiThread> {
        let (tx, mut rx) = UnixStream::pair().ok()?;
        rx.set_nonblocking(true).ok()?;
        tx.set_nonblocking(true).ok()?;
        let jobs: Arc<Mutex<Vec<(&'static str, Job)>>> = Arc::new(Mutex::new(Vec::new()));
        let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr() as *mut _) };
        let activity = app.activity_as_ptr() as usize;
        let j2 = jobs.clone();
        let fd = rx.as_raw_fd();
        let res = app.java_main_looper().add_fd_with_callback(
            unsafe { BorrowedFd::borrow_raw(fd) },
            FdEvent::INPUT,
            move |_fd, _ev| {
                let mut buf = [0u8; 64];
                while matches!(rx.read(&mut buf), Ok(n) if n > 0) {}
                let list = std::mem::take(&mut *j2.lock().unwrap());
                for (name, job) in list {
                    let r = vm.attach_current_thread(|env: &mut Env| -> jni::errors::Result<()> {
                        let act = unsafe { JObject::from_raw(env, activity as jni::sys::jobject) };
                        let r = job(env, &act);
                        if env.exception_check() {
                            env.exception_clear();
                        }
                        r
                    });
                    match r {
                        Ok(()) => log::debug!("ui thread: {name} ok"),
                        Err(e) => log::warn!("ui thread: {name} failed: {e:?}"),
                    }
                }
                true
            },
        );
        if let Err(e) = res {
            log::warn!("ui thread: ALooper_addFd failed: {e:?}");
            return None;
        }
        Some(UiThread { tx: Arc::new(Mutex::new(tx)), jobs })
    }

    pub fn run(&self, name: &'static str, job: Job) {
        self.jobs.lock().unwrap().push((name, job));
        let _ = self.tx.lock().unwrap().write(&[1]);
    }

    /// `Activity.finish()` (the game exited).
    pub fn finish(&self) {
        self.run("finish", Box::new(|env, act| {
            env.call_method(act, jni_str!("finish"), jni_sig!("()V"), &[])?;
            Ok(())
        }));
    }

    /// Hide status and navigation bars (sticky immersive: a swipe shows them briefly).
    pub fn immersive(&self) {
        self.run("immersive", Box::new(|env, act| {
            let win = env.call_method(act, jni_str!("getWindow"), jni_sig!("()Landroid/view/Window;"), &[])?.l()?;
            // API 30+: WindowInsetsController.
            let r = (|| -> jni::errors::Result<()> {
                let ctrl = env
                    .call_method(&win, jni_str!("getInsetsController"), jni_sig!("()Landroid/view/WindowInsetsController;"), &[])?
                    .l()?;
                let bars = env
                    .call_static_method(jni_str!("android/view/WindowInsets$Type"), jni_str!("systemBars"), jni_sig!("()I"), &[])?
                    .i()?;
                // BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE = 2
                env.call_method(&ctrl, jni_str!("setSystemBarsBehavior"), jni_sig!("(I)V"), &[JValue::Int(2)])?;
                env.call_method(&ctrl, jni_str!("hide"), jni_sig!("(I)V"), &[JValue::Int(bars)])?;
                Ok(())
            })();
            if r.is_ok() {
                return Ok(());
            }
            env.exception_clear();
            // API 26-29: IMMERSIVE_STICKY | FULLSCREEN | HIDE_NAVIGATION | LAYOUT_* flags.
            let decor = env.call_method(&win, jni_str!("getDecorView"), jni_sig!("()Landroid/view/View;"), &[])?.l()?;
            env.call_method(&decor, jni_str!("setSystemUiVisibility"), jni_sig!("(I)V"), &[JValue::Int(0x1706)])?;
            Ok(())
        }));
    }

    /// Update `state` with the real IME visibility (the user can close the keyboard with Back or its own button)
    /// and `shared.ime_h` with its height (the presenter keeps the game's text field above it, `s3_frontend::ime`).
    pub fn refresh_keyboard(&self, state: Arc<Mutex<bool>>, shared: Arc<Shared>) {
        self.run("keyboard state", Box::new(move |env, act| {
            let win = env.call_method(act, jni_str!("getWindow"), jni_sig!("()Landroid/view/Window;"), &[])?.l()?;
            let decor = env.call_method(&win, jni_str!("getDecorView"), jni_sig!("()Landroid/view/View;"), &[])?.l()?;
            if let Some(v) = ime_visible(env, &decor) {
                *state.lock().unwrap() = v;
            }
            let visible = *state.lock().unwrap();
            let h = if visible { ime_height(env, &decor) } else { 0 };
            if shared.ime_h.load(std::sync::atomic::Ordering::Acquire) != h {
                log::info!("soft keyboard: {} px high", h);
            }
            shared.set_ime_height(h);
            Ok(())
        }));
    }

    /// Show (`Some(true)`), hide (`Some(false)`) or toggle (`None`) the soft keyboard. Uses the focused view (the
    /// NativeActivity content view), like `ANativeActivity_showSoftInput`.
    pub fn soft_keyboard(&self, want: Option<bool>, state: Arc<Mutex<bool>>, shared: Arc<Shared>) {
        self.run("soft keyboard", Box::new(move |env, act| {
            let win = env.call_method(act, jni_str!("getWindow"), jni_sig!("()Landroid/view/Window;"), &[])?.l()?;
            let decor = env.call_method(&win, jni_str!("getDecorView"), jni_sig!("()Landroid/view/View;"), &[])?.l()?;
            let focus = env.call_method(act, jni_str!("getCurrentFocus"), jni_sig!("()Landroid/view/View;"), &[])?.l()?;
            let view = if focus.is_null() { decor } else { focus };
            let name = env.new_string("input_method")?;
            let imm = env
                .call_method(act, jni_str!("getSystemService"), jni_sig!("(Ljava/lang/String;)Ljava/lang/Object;"), &[JValue::Object(&name)])?
                .l()?;
            // Current visibility (API 30+); fall back to our own flag.
            let mut visible = *state.lock().unwrap();
            if let Some(v) = ime_visible(env, &view) {
                visible = v;
            }
            let show = want.unwrap_or(!visible);
            if show {
                env.call_method(&view, jni_str!("requestFocus"), jni_sig!("()Z"), &[])?;
                let ok = env
                    .call_method(&imm, jni_str!("showSoftInput"), jni_sig!("(Landroid/view/View;I)Z"), &[JValue::Object(&view), JValue::Int(0)])?
                    .z()?;
                log::info!("soft keyboard: show -> {ok} (was visible: {visible})");
            } else {
                let token = env.call_method(&view, jni_str!("getWindowToken"), jni_sig!("()Landroid/os/IBinder;"), &[])?.l()?;
                let ok = env
                    .call_method(
                        &imm,
                        jni_str!("hideSoftInputFromWindow"),
                        jni_sig!("(Landroid/os/IBinder;I)Z"),
                        &[JValue::Object(&token), JValue::Int(0)],
                    )?
                    .z()?;
                log::info!("soft keyboard: hide -> {ok}");
                shared.set_ime_height(0);
            }
            *state.lock().unwrap() = show;
            Ok(())
        }));
    }
}

impl UiThread {
    /// Use the whole screen and the fastest display mode (docs/porting/display.md):
    /// `layoutInDisplayCutoutMode = ALWAYS` (API 30, SHORT_EDGES on 28-29) so the game also covers the camera
    /// cutout strip, and `preferredDisplayModeId` / `preferredRefreshRate` = the mode with the current resolution
    /// and the refresh rate closest to `hz` (0 = highest). Stores the chosen rate in `shared.refresh_hz`.
    pub fn display_prefs(&self, hz: u32, shared: std::sync::Arc<crate::frontend::Shared>) {
        self.run("display prefs", Box::new(move |env, act| {
            let sdk = env.get_static_field(jni_str!("android/os/Build$VERSION"), jni_str!("SDK_INT"), jni_sig!("I"))?.i()?;
            let win = env.call_method(act, jni_str!("getWindow"), jni_sig!("()Landroid/view/Window;"), &[])?.l()?;
            let lp = env.call_method(&win, jni_str!("getAttributes"), jni_sig!("()Landroid/view/WindowManager$LayoutParams;"), &[])?.l()?;
            if sdk >= 28 {
                let mode = if sdk >= 30 { 3 } else { 1 };
                env.set_field(&lp, jni_str!("layoutInDisplayCutoutMode"), jni_sig!("I"), JValue::Int(mode))?;
            }
            let wm = env.call_method(act, jni_str!("getWindowManager"), jni_sig!("()Landroid/view/WindowManager;"), &[])?.l()?;
            let disp = env.call_method(&wm, jni_str!("getDefaultDisplay"), jni_sig!("()Landroid/view/Display;"), &[])?.l()?;
            let cur = env.call_method(&disp, jni_str!("getMode"), jni_sig!("()Landroid/view/Display$Mode;"), &[])?.l()?;
            let cw = env.call_method(&cur, jni_str!("getPhysicalWidth"), jni_sig!("()I"), &[])?.i()?;
            let ch = env.call_method(&cur, jni_str!("getPhysicalHeight"), jni_sig!("()I"), &[])?.i()?;
            let modes = env.call_method(&disp, jni_str!("getSupportedModes"), jni_sig!("()[Landroid/view/Display$Mode;"), &[])?.l()?;
            let n = env
                .call_static_method(jni_str!("java/lang/reflect/Array"), jni_str!("getLength"), jni_sig!("(Ljava/lang/Object;)I"), &[JValue::Object(&modes)])?
                .i()?;
            let mut best: Option<(i32, f32)> = None;
            let mut all = Vec::new();
            for i in 0..n {
                let md = env
                    .call_static_method(
                        jni_str!("java/lang/reflect/Array"),
                        jni_str!("get"),
                        jni_sig!("(Ljava/lang/Object;I)Ljava/lang/Object;"),
                        &[JValue::Object(&modes), JValue::Int(i)],
                    )?
                    .l()?;
                let w = env.call_method(&md, jni_str!("getPhysicalWidth"), jni_sig!("()I"), &[])?.i()?;
                let h = env.call_method(&md, jni_str!("getPhysicalHeight"), jni_sig!("()I"), &[])?.i()?;
                let id = env.call_method(&md, jni_str!("getModeId"), jni_sig!("()I"), &[])?.i()?;
                let r = env.call_method(&md, jni_str!("getRefreshRate"), jni_sig!("()F"), &[])?.f()?;
                all.push(format!("{id}:{w}x{h}@{r:.0}"));
                if (w, h) != (cw, ch) {
                    continue;
                }
                let better = match best {
                    None => true,
                    Some((_, br)) if hz == 0 => r > br,
                    Some((_, br)) => (r - hz as f32).abs() < (br - hz as f32).abs(),
                };
                if better {
                    best = Some((id, r));
                }
            }
            log::info!("display: modes [{}], current {cw}x{ch}, want {hz} Hz -> {best:?}", all.join(" "));
            if let Some((id, r)) = best {
                env.set_field(&lp, jni_str!("preferredDisplayModeId"), jni_sig!("I"), JValue::Int(id))?;
                env.set_field(&lp, jni_str!("preferredRefreshRate"), jni_sig!("F"), JValue::Float(r))?;
                shared.refresh_hz.store(r.to_bits(), std::sync::atomic::Ordering::Relaxed);
            }
            env.call_method(&win, jni_str!("setAttributes"), jni_sig!("(Landroid/view/WindowManager$LayoutParams;)V"), &[JValue::Object(&lp)])?;
            Ok(())
        }));
    }
}

/// Soft keyboard height in window pixels: `getRootWindowInsets().getInsets(WindowInsets.Type.ime()).bottom`
/// (API 30+). Older Android: half the view height (an estimate; the keyboard is believed open).
fn ime_height<'a>(env: &mut Env<'a>, view: &JObject<'a>) -> u32 {
    let q = (|| -> jni::errors::Result<i32> {
        let ins = env.call_method(view, jni_str!("getRootWindowInsets"), jni_sig!("()Landroid/view/WindowInsets;"), &[])?.l()?;
        if ins.is_null() {
            return Err(jni::errors::Error::NullPtr("insets"));
        }
        let ime = env.call_static_method(jni_str!("android/view/WindowInsets$Type"), jni_str!("ime"), jni_sig!("()I"), &[])?.i()?;
        let i = env.call_method(&ins, jni_str!("getInsets"), jni_sig!("(I)Landroid/graphics/Insets;"), &[JValue::Int(ime)])?.l()?;
        env.get_field(&i, jni_str!("bottom"), jni_sig!("I"))?.i()
    })();
    match q {
        Ok(h) => h.max(0) as u32,
        Err(_) => {
            env.exception_clear();
            let h = env.call_method(view, jni_str!("getHeight"), jni_sig!("()I"), &[]).and_then(|v| v.i()).unwrap_or(0);
            env.exception_clear();
            (h.max(0) / 2) as u32
        }
    }
}

/// `view.getRootWindowInsets().isVisible(WindowInsets.Type.ime())` (API 30+); `None` if unavailable.
fn ime_visible<'a>(env: &mut Env<'a>, view: &JObject<'a>) -> Option<bool> {
    let q = (|| -> jni::errors::Result<bool> {
        let ins = env.call_method(view, jni_str!("getRootWindowInsets"), jni_sig!("()Landroid/view/WindowInsets;"), &[])?.l()?;
        if ins.is_null() {
            return Err(jni::errors::Error::NullPtr("insets"));
        }
        let ime = env.call_static_method(jni_str!("android/view/WindowInsets$Type"), jni_str!("ime"), jni_sig!("()I"), &[])?.i()?;
        env.call_method(&ins, jni_str!("isVisible"), jni_sig!("(I)Z"), &[JValue::Int(ime)])?.z()
    })();
    match q {
        Ok(v) => Some(v),
        Err(_) => {
            env.exception_clear();
            None
        }
    }
}
