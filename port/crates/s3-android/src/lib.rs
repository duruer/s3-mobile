//! Android frontend (NativeActivity) for the Settlers III port: `libs3android.so`.
//!
//! The platform-independent parts (shared state + `frontend::AndroidHost`, launcher, letterboxing, touch -> raw
//! pointer events, status screen, audio ring/resampler, key map, GLES renderer) live in the `s3-frontend` crate,
//! shared with iOS, and are re-exported here under their old paths. `android.rs` holds the NativeActivity glue and
//! only builds for `target_os = "android"`. See docs/porting/android.md.

pub use s3_frontend::{audio, background, frontend, gl, keys, launcher, layout, perf_hint, screen, touch, upload};
#[cfg(target_os = "android")]
pub use s3_frontend::glrender;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
mod present;
#[cfg(target_os = "android")]
mod uithread;
#[cfg(target_os = "android")]
mod bgservice;
#[cfg(target_os = "android")]
mod wifilock;

/// Package id of the APK.
pub const PACKAGE: &str = "com.settlers3port.app";
