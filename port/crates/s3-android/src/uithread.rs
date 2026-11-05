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

    /// Update `state` with the real IME visibility (the user can close the keyboard with Back or its own button).
    pub fn refresh_keyboard(&self, state: Arc<Mutex<bool>>) {
        self.run("keyboard state", Box::new(move |env, act| {
            let win = env.call_method(act, jni_str!("getWindow"), jni_sig!("()Landroid/view/Window;"), &[])?.l()?;
            let decor = env.call_method(&win, jni_str!("getDecorView"), jni_sig!("()Landroid/view/View;"), &[])?.l()?;
            if let Some(v) = ime_visible(env, &decor) {
                *state.lock().unwrap() = v;
            }
            Ok(())
        }));
    }

    /// Show (`Some(true)`), hide (`Some(false)`) or toggle (`None`) the soft keyboard. Uses the focused view (the
    /// NativeActivity content view), like `ANativeActivity_showSoftInput`.
    pub fn soft_keyboard(&self, want: Option<bool>, state: Arc<Mutex<bool>>) {
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
            }
            *state.lock().unwrap() = show;
            Ok(())
        }));
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
