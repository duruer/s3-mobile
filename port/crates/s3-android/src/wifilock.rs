//! Wi-Fi multicast lock for LAN game discovery (docs/porting/multiplayer.md section 8).
//!
//! Many Wi-Fi drivers drop broadcast/multicast frames to save power unless an app holds a
//! `WifiManager.MulticastLock` (permission `CHANGE_WIFI_MULTICAST_STATE`). [`install`] registers a hook with
//! `s3_net::lan` that acquires the lock the first time the LAN discovery starts (the game's LAN menu) and keeps it
//! for the rest of the process.

use std::sync::OnceLock;

use android_activity::AndroidApp;
use jni::objects::{JObject, JValue};
use jni::{jni_sig, jni_str, Env, JavaVM};

static APP: OnceLock<AndroidApp> = OnceLock::new();

pub fn install(app: &AndroidApp) {
    let _ = APP.set(app.clone());
    s3_net::lan::set_start_hook(acquire);
}

fn acquire() {
    let Some(app) = APP.get() else { return };
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr() as *mut _) };
    let activity = app.activity_as_ptr() as usize;
    let r = vm.attach_current_thread(|env: &mut Env| -> jni::errors::Result<()> {
        let act = unsafe { JObject::from_raw(env, activity as jni::sys::jobject) };
        let r = (|| -> jni::errors::Result<()> {
            let name = env.new_string("wifi")?;
            let wm = env
                .call_method(&act, jni_str!("getSystemService"), jni_sig!("(Ljava/lang/String;)Ljava/lang/Object;"), &[JValue::Object(&name)])?
                .l()?;
            if wm.is_null() {
                return Ok(());
            }
            let tag = env.new_string("s3port-lan")?;
            let lock = env
                .call_method(
                    &wm,
                    jni_str!("createMulticastLock"),
                    jni_sig!("(Ljava/lang/String;)Landroid/net/wifi/WifiManager$MulticastLock;"),
                    &[JValue::Object(&tag)],
                )?
                .l()?;
            env.call_method(&lock, jni_str!("setReferenceCounted"), jni_sig!("(Z)V"), &[JValue::Bool(false)])?;
            env.call_method(&lock, jni_str!("acquire"), jni_sig!("()V"), &[])?;
            // MulticastLock releases itself when garbage-collected: keep a global reference forever.
            let g = env.new_global_ref(&lock)?;
            std::mem::forget(g);
            Ok(())
        })();
        if env.exception_check() {
            env.exception_describe();
            env.exception_clear();
        }
        r
    });
    match r {
        Ok(()) => log::info!("lan: Wi-Fi multicast lock acquired"),
        Err(e) => log::warn!("lan: Wi-Fi multicast lock failed: {e:?}"),
    }
}
