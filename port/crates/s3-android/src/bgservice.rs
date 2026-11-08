//! Background play (docs/porting/android.md "Background play"): drives the Java `GameService` (the only Java
//! code of the app, `port/android/java`) with plain intents over JNI.
//!
//! - [`start`]: `startForegroundService(START, game_ms)`, called synchronously from the activity thread while the
//!   `Pause` event is handled (the activity is still visible, so Android 12+ allows the start);
//! - [`update`]: re-sync the chronometer (game speed / in-game pause make game time drift from wall time);
//! - [`event`]: post one "Game events" notification (lines joined with `\n`);
//! - [`stop`]: `stopService` (removes the ongoing and the event notifications);
//! - [`ask_notification_permission`]: `POST_NOTIFICATIONS` runtime request (Android 13+), once per install.

use std::path::Path;

use android_activity::AndroidApp;
use jni::objects::{JObject, JValue};
use jni::{jni_sig, jni_str, Env, JavaVM};

const SERVICE: &str = "com.settlers3port.app.GameService";

fn with_activity(app: &AndroidApp, name: &str, f: impl for<'a> FnOnce(&mut Env<'a>, &JObject<'a>) -> jni::errors::Result<()>) {
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr() as *mut _) };
    let activity = app.activity_as_ptr() as usize;
    let r = vm.attach_current_thread(|env: &mut Env| -> jni::errors::Result<()> {
        let act = unsafe { JObject::from_raw(env, activity as jni::sys::jobject) };
        let r = f(env, &act);
        if env.exception_check() {
            env.exception_describe();
            env.exception_clear();
        }
        r
    });
    match r {
        Ok(()) => log::info!("bgservice: {name} ok"),
        Err(e) => log::warn!("bgservice: {name} failed: {e:?}"),
    }
}

/// `new Intent().setClassName(activity, GameService).setAction(action)`.
fn intent<'a>(env: &mut Env<'a>, act: &JObject<'a>, action: Option<&str>) -> jni::errors::Result<JObject<'a>> {
    let it = env.new_object(jni_str!("android/content/Intent"), jni_sig!("()V"), &[])?;
    let cls = env.new_string(SERVICE)?;
    env.call_method(
        &it,
        jni_str!("setClassName"),
        jni_sig!("(Landroid/content/Context;Ljava/lang/String;)Landroid/content/Intent;"),
        &[JValue::Object(act), JValue::Object(&cls)],
    )?;
    if let Some(a) = action {
        let a = env.new_string(format!("com.settlers3port.app.{a}"))?;
        env.call_method(&it, jni_str!("setAction"), jni_sig!("(Ljava/lang/String;)Landroid/content/Intent;"), &[JValue::Object(&a)])?;
    }
    Ok(it)
}

fn put_long<'a>(env: &mut Env<'a>, it: &JObject<'a>, key: &str, v: i64) -> jni::errors::Result<()> {
    let k = env.new_string(key)?;
    env.call_method(it, jni_str!("putExtra"), jni_sig!("(Ljava/lang/String;J)Landroid/content/Intent;"), &[JValue::Object(&k), JValue::Long(v)])?;
    Ok(())
}

/// Start the foreground service with the ongoing notification (chronometer at `game_ms`).
pub fn start(app: &AndroidApp, game_ms: u64) {
    with_activity(app, "start", |env, act| {
        let it = intent(env, act, Some("START"))?;
        put_long(env, &it, "game_ms", game_ms as i64)?;
        env.call_method(act, jni_str!("startForegroundService"), jni_sig!("(Landroid/content/Intent;)Landroid/content/ComponentName;"), &[JValue::Object(&it)])?;
        Ok(())
    });
}

/// Re-sync the chronometer of the running service.
pub fn update(app: &AndroidApp, game_ms: u64) {
    with_activity(app, "update", |env, act| {
        let it = intent(env, act, Some("UPDATE"))?;
        put_long(env, &it, "game_ms", game_ms as i64)?;
        env.call_method(act, jni_str!("startService"), jni_sig!("(Landroid/content/Intent;)Landroid/content/ComponentName;"), &[JValue::Object(&it)])?;
        Ok(())
    });
}

/// One "Game events" notification with these lines.
pub fn event(app: &AndroidApp, lines: &[String], vibrate: bool) {
    let text = lines.join("\n");
    with_activity(app, "event", |env, act| {
        let it = intent(env, act, Some("EVENT"))?;
        put_long(env, &it, "vibrate", vibrate as i64)?;
        let k = env.new_string("text")?;
        let v = env.new_string(&text)?;
        env.call_method(&it, jni_str!("putExtra"), jni_sig!("(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;"), &[JValue::Object(&k), JValue::Object(&v)])?;
        env.call_method(act, jni_str!("startService"), jni_sig!("(Landroid/content/Intent;)Landroid/content/ComponentName;"), &[JValue::Object(&it)])?;
        Ok(())
    });
}

/// Short vibration (touch feedback, game message), `ms` milliseconds (API 26+ `VibrationEffect`).
pub fn vibrate(app: &AndroidApp, ms: u32) {
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr() as *mut _) };
    let activity = app.activity_as_ptr() as usize;
    let r = vm.attach_current_thread(|env: &mut Env| -> jni::errors::Result<()> {
        let act = unsafe { JObject::from_raw(env, activity as jni::sys::jobject) };
        let name = env.new_string("vibrator")?;
        let r = env
            .call_method(&act, jni_str!("getSystemService"), jni_sig!("(Ljava/lang/String;)Ljava/lang/Object;"), &[JValue::Object(&name)])
            .and_then(|v| v.l())
            .and_then(|v| {
                if v.is_null() {
                    return Ok(());
                }
                let eff = env
                    .call_static_method(
                        jni_str!("android/os/VibrationEffect"),
                        jni_str!("createOneShot"),
                        jni_sig!("(JI)Landroid/os/VibrationEffect;"),
                        &[JValue::Long(ms as i64), JValue::Int(-1)],
                    )?
                    .l()?;
                env.call_method(&v, jni_str!("vibrate"), jni_sig!("(Landroid/os/VibrationEffect;)V"), &[JValue::Object(&eff)])?;
                Ok(())
            });
        if env.exception_check() {
            env.exception_describe();
            env.exception_clear();
        }
        r
    });
    if let Err(e) = r {
        log::warn!("vibrate failed: {e:?}");
    }
}

/// Stop the service (its notifications go with it).
pub fn stop(app: &AndroidApp) {
    with_activity(app, "stop", |env, act| {
        let it = intent(env, act, None)?;
        env.call_method(act, jni_str!("stopService"), jni_sig!("(Landroid/content/Intent;)Z"), &[JValue::Object(&it)])?;
        Ok(())
    });
}

/// Ask for `POST_NOTIFICATIONS` (Android 13+) once per install (marker file in the data dir). Runs on the UI
/// thread job queue.
pub fn ask_notification_permission(ui: &crate::uithread::UiThread, data_dir: &Path) {
    let marker = data_dir.join("notification-permission-asked.txt");
    if marker.exists() {
        return;
    }
    let _ = std::fs::create_dir_all(data_dir);
    let _ = std::fs::write(&marker, "1\n");
    ui.run("notification permission", Box::new(|env, act| {
        let sdk = env.get_static_field(jni_str!("android/os/Build$VERSION"), jni_str!("SDK_INT"), jni_sig!("I"))?.i()?;
        if sdk < 33 {
            return Ok(());
        }
        let perm = env.new_string("android.permission.POST_NOTIFICATIONS")?;
        let granted = env
            .call_method(act, jni_str!("checkSelfPermission"), jni_sig!("(Ljava/lang/String;)I"), &[JValue::Object(&perm)])?
            .i()?;
        log::info!("notification permission: granted={}", granted == 0);
        if granted != 0 {
            let arr = env.new_object_array(1, jni_str!("java/lang/String"), &perm)?;
            env.call_method(act, jni_str!("requestPermissions"), jni_sig!("([Ljava/lang/String;I)V"), &[JValue::Object(&arr), JValue::Int(1)])?;
        }
        Ok(())
    }));
}
