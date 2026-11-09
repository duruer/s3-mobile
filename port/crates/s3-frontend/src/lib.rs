//! Mobile frontend parts shared by the Android (`s3-android`) and iOS (`s3-ios`) apps: state shared between the UI
//! and engine threads + the [`frontend::AndroidHost`] `Host`, the engine thread body, the launcher (home screen) and
//! its upload server, letterboxing, touch -> raw pointer events, status screen, audio ring/resampler, key map, and
//! the GLES renderer (`glrender`, any current GL context; EGL stays in s3-android).
//!
//! Everything here builds and is unit-tested on the desktop. It is a plain rlib so the iOS static library does not
//! pull in the Android `cdylib`. See docs/porting/android.md and docs/porting/ios.md.

/// App version shown in the launcher. Release builds (CI, semantic-release) set `S3_VERSION` at compile time;
/// local builds fall back to the workspace version.
pub const VERSION: &str = match option_env!("S3_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

pub mod audio;
pub mod background;
pub mod frontend;
pub mod ime;
pub mod keys;
pub mod launcher;
pub mod layout;
pub mod perf_hint;
pub mod screen;
pub mod touch;
pub mod upload;

pub mod gl;
#[cfg(any(target_os = "android", target_os = "ios"))]
pub mod glrender;
#[cfg(target_os = "android")]
mod anime4k;
#[cfg(target_os = "android")]
pub mod upscale_ai;
