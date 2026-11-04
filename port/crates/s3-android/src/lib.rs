//! Android frontend (NativeActivity) for the Settlers III port: `libs3android.so`.
//!
//! The platform-independent parts (shared state + [`frontend::AndroidHost`], letterboxing, touch -> mouse,
//! status screen, audio ring/resampler, key map) build and are unit-tested on Linux. `android.rs` holds the
//! NativeActivity glue and only builds for `target_os = "android"`. See docs/porting/android.md.

pub mod audio;
pub mod frontend;
pub mod keys;
pub mod layout;
pub mod screen;
pub mod touch;

#[cfg(target_os = "android")]
mod android;

/// Package id of the APK.
pub const PACKAGE: &str = "com.settlers3port.app";
