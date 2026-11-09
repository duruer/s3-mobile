//! Keep the game's text field above the soft keyboard (serial-keyboard, 2026-11-09).
//!
//! The keyboard is drawn over the bottom of the window; the window is not resized (immersive full screen). On a
//! landscape phone it covers about half the height, and the serial number field of the menu popup (shown centred
//! and enlarged by the menu layouts) ended up under it. Like Android's `adjustPan`, the presenter moves the whole
//! picture (game frame + window layer) up by [`keyboard_pan`] window pixels while the keyboard is open for a game
//! text field ([`crate::frontend::Shared::text_field`], set from `s3_mods::text_input`), and the touch mapping adds
//! the same amount to the finger's window y ([`crate::frontend::Shared::pan_y`]).

use s3_mods::display::Viewport;

/// Window pixels to move the picture up so that `field` (game rect left, top, right, bottom) sits above a soft
/// keyboard `ime_h` window pixels high at the bottom of the window. 0 = no move (no keyboard, the field is visible
/// anyway, or it is not on screen). The field's top never leaves the window, and the move is never more than the
/// keyboard height.
pub fn keyboard_pan(vp: &Viewport, field: [i32; 4], ime_h: u32) -> i32 {
    let wh = vp.win_h as f32;
    if ime_h == 0 || vp.win_h == 0 {
        return 0;
    }
    let ime = (ime_h as f32).min(wh);
    let (_, top) = vp.game_to_window(field[0] as f32, field[1] as f32);
    let (_, bottom) = vp.game_to_window(field[2] as f32, field[3] as f32);
    // Pieces of a menu layout that are not shown map far off screen.
    if !(top.is_finite() && bottom.is_finite()) || bottom <= top || top < 0.0 || bottom > wh {
        return 0;
    }
    let margin = (wh / 40.0).max(8.0);
    let need = bottom + margin - (wh - ime);
    if need <= 0.0 {
        return 0;
    }
    need.min((top - margin).max(0.0)).min(ime).ceil() as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use s3_mods::display::DisplayMode;

    /// Serial number field of the menu popup (game rect from the s3run trace, docs game-state-ui.md section 9).
    const SERIAL: [i32; 4] = [220, 276, 580, 300];

    #[test]
    fn no_keyboard_or_field_above_it() {
        let vp = Viewport::new(DisplayMode::Fit, 800, 600, 2400, 1080);
        assert_eq!(keyboard_pan(&vp, SERIAL, 0), 0);
        // A small keyboard (hardware-keyboard bar) leaves the field visible.
        assert_eq!(keyboard_pan(&vp, SERIAL, 200), 0);
    }

    #[test]
    fn landscape_keyboard_moves_the_field_above_it() {
        // Fit: 800x600 -> 1440x1080, field rows 496.8..540 of 1080; a 540 px keyboard starts at row 540.
        let vp = Viewport::new(DisplayMode::Fit, 800, 600, 2400, 1080);
        let pan = keyboard_pan(&vp, SERIAL, 540);
        let margin = 1080.0 / 40.0;
        let bottom = vp.game_to_window(580.0, 300.0).1 - pan as f32;
        assert!(bottom + margin <= 540.0 + 1.0 && bottom + margin >= 540.0 - 1.0, "pan {pan}, bottom {bottom}");
        assert!(vp.game_to_window(220.0, 276.0).1 - pan as f32 >= margin, "top stays in the window");
    }

    #[test]
    fn huge_keyboard_keeps_the_field_top_in_the_window() {
        let vp = Viewport::new(DisplayMode::Fit, 800, 600, 2400, 1080);
        let pan = keyboard_pan(&vp, SERIAL, 1000);
        let top = vp.game_to_window(220.0, 276.0).1;
        assert_eq!(pan, (top - 1080.0 / 40.0).ceil() as i32);
    }

    #[test]
    fn serial_popup_menu_layout_on_a_wide_phone() {
        // What a 2316x904 phone shows (s3run --layout-preview of the serial dialog: main-menu+msg-box): the popup
        // centred and enlarged; its field (rows ~500..563) is under a half-height keyboard.
        let l = s3_mods::menu_layout::by_name("main-menu+msg-box").unwrap();
        let l = s3_mods::menu_layout::choose(l, 2316, 904);
        let vp = Viewport::with_layout(DisplayMode::NativeWide, l, 2316, 904);
        let (top, bottom) = (vp.game_to_window(220.0, 276.0).1, vp.game_to_window(580.0, 300.0).1);
        assert!(bottom > 904.0 - 452.0, "field under the keyboard without a move: {top}..{bottom}");
        let pan = keyboard_pan(&vp, SERIAL, 452);
        assert!(pan > 0);
        assert!(bottom - pan as f32 <= 452.0 && top - pan as f32 >= 0.0, "pan {pan}: field {top}..{bottom}");
    }

    #[test]
    fn field_off_screen_is_ignored() {
        let vp = Viewport::new(DisplayMode::Fit, 800, 600, 2400, 1080);
        assert_eq!(keyboard_pan(&vp, [0, 700, 10, 720], 540), 0);
        assert_eq!(keyboard_pan(&vp, [0, 300, 10, 290], 540), 0);
    }
}
