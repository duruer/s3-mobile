//! Mod/UI settings, persisted as JSON in `<data_dir>/mods-settings.json`. Unknown or missing fields fall back
//! to defaults, so old files keep loading after new fields are added.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// File name inside the data dir.
pub const SETTINGS_FILE: &str = "mods-settings.json";

/// The two selectable mobile UI modes (docs/porting/mobile-vision.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum UiMode {
    /// A: the original panel and game UI, patched for touch.
    #[default]
    ClassicTouch,
    /// B: a native large-button HUD on top of / instead of the original panel.
    MobileHud,
}

/// Gesture thresholds (game pixels / milliseconds), see [`crate::gesture::GestureConfig`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TouchSettings {
    pub long_press_ms: u64,
    pub tap_max_ms: u64,
    pub tap_slop: f32,
    pub double_tap_ms: u64,
    pub double_tap_slop: f32,
    /// Fallback touch->mouse mapping: a long press right-clicks (legacy behaviour; with `context` on, the
    /// context-aware touch mod consumes long presses, so this only applies when `context` is off).
    pub long_press_right_click: bool,
    /// Context-aware touch (W3, `touch.rs`; GOG exe only). Off = legacy touch->mouse mapping only.
    pub context: bool,
}

impl Default for TouchSettings {
    fn default() -> Self {
        TouchSettings {
            long_press_ms: LONG_PRESS_MS,
            tap_max_ms: 500,
            tap_slop: 8.0,
            double_tap_ms: 300,
            double_tap_slop: 24.0,
            long_press_right_click: true,
            context: true,
        }
    }
}

/// All mod settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    pub ui_mode: UiMode,
    /// Master switch for exe patch sets (hooks/byte patches). Input/overlay framework stays active.
    pub patches_enabled: bool,
    /// Draw touch points and recognized gestures (also `S3_MODS_DEBUG=1`).
    pub debug_overlay: bool,
    /// Scale factor for native HUD elements (mode B).
    pub hud_scale: f32,
    pub touch: TouchSettings,
    /// Display mode / filter / FPS counter / refresh rate (docs/porting/display.md, `crate::display`).
    pub display: crate::display::DisplaySettings,
    /// Custom game resolution + zoom level (docs/porting/resolution-zoom.md, `crate::res` / `crate::zoom`).
    pub res: crate::res::ResSettings,
    /// Game speed the speed button switches to (2..=8, `crate::speed`); chosen in the menus.
    pub fast_speed: u32,
    /// Show the game time (`crate::game_time`); on by default.
    pub show_game_time: bool,
    /// Draw the paths of all own units under a move order (selected units always show theirs; `crate::unit_orders`).
    pub show_unit_paths: bool,
    /// Control-group buttons 1..6 (tap = select / centre, long press = assign; `crate::unit_orders`).
    pub group_shortcuts: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            version: 1,
            ui_mode: UiMode::default(),
            patches_enabled: true,
            debug_overlay: false,
            hud_scale: 1.0,
            touch: TouchSettings::default(),
            display: crate::display::DisplaySettings::default(),
            res: crate::res::ResSettings::default(),
            fast_speed: crate::speed::DEFAULT_FAST,
            show_game_time: true,
            show_unit_paths: false,
            group_shortcuts: false,
        }
    }
}

/// Hold time before a touch becomes a long press (box select / drag start). Was 500 ms until 2026-11-08
/// ("press and drag takes too long").
pub const LONG_PRESS_MS: u64 = 350;
const OLD_LONG_PRESS_MS: u64 = 500;

impl Settings {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join(SETTINGS_FILE)
    }

    /// Load from `data_dir` (defaults if the file is missing or broken; the error is returned for logging).
    pub fn load(data_dir: &Path) -> (Settings, Option<String>) {
        let p = Self::path(data_dir);
        match std::fs::read(&p) {
            Ok(b) => match serde_json::from_slice::<Settings>(&b) {
                Ok(mut s) => {
                    // The old default was saved into every settings file: move it to the new one.
                    if s.touch.long_press_ms == OLD_LONG_PRESS_MS {
                        s.touch.long_press_ms = LONG_PRESS_MS;
                    }
                    (s, None)
                }
                Err(e) => (Settings::default(), Some(format!("{}: {e}", p.display()))),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Settings::default(), None),
            Err(e) => (Settings::default(), Some(format!("{}: {e}", p.display()))),
        }
    }

    /// Save atomically (write temp file, rename).
    pub fn save(&self, data_dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(data_dir).map_err(|e| e.to_string())?;
        let p = Self::path(data_dir);
        let tmp = p.with_extension("json.tmp");
        let s = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, s).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &p).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_partial_files() {
        let dir = std::env::temp_dir().join(format!("s3mods-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (s, err) = Settings::load(&dir);
        assert_eq!(s, Settings::default());
        assert!(err.is_none());
        let mut s2 = Settings::default();
        s2.ui_mode = UiMode::MobileHud;
        s2.touch.long_press_ms = 700;
        s2.save(&dir).unwrap();
        assert_eq!(Settings::load(&dir).0, s2);
        // Partial file: missing fields use defaults.
        std::fs::write(Settings::path(&dir), r#"{"ui_mode":"MobileHud","touch":{"tap_slop":12.5}}"#).unwrap();
        let s3 = Settings::load(&dir).0;
        assert_eq!(s3.ui_mode, UiMode::MobileHud);
        assert_eq!(s3.touch.tap_slop, 12.5);
        assert_eq!(s3.touch.long_press_ms, LONG_PRESS_MS);
        // Broken file: defaults + error.
        std::fs::write(Settings::path(&dir), "{").unwrap();
        let (s4, err) = Settings::load(&dir);
        assert_eq!(s4, Settings::default());
        assert!(err.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
