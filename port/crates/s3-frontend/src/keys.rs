//! Android key codes (AKEYCODE_*) -> Windows virtual-key codes (VK_*).

/// Map an Android key code to a Windows VK code. `None` = not forwarded.
pub fn android_to_vk(code: u32) -> Option<u32> {
    Some(match code {
        4 => 0x1B,            // BACK -> VK_ESCAPE (skips the intro, closes menus)
        111 => 0x1B,          // ESCAPE
        7..=16 => 0x30 + (code - 7), // 0..9
        29..=54 => 0x41 + (code - 29), // A..Z
        19 => 0x26,           // DPAD_UP -> VK_UP
        20 => 0x28,           // DPAD_DOWN
        21 => 0x25,           // DPAD_LEFT
        22 => 0x27,           // DPAD_RIGHT
        23 | 66 | 160 => 0x0D, // DPAD_CENTER / ENTER / NUMPAD_ENTER -> VK_RETURN
        62 => 0x20,           // SPACE
        61 => 0x09,           // TAB
        67 => 0x08,           // DEL (backspace)
        112 => 0x2E,          // FORWARD_DEL -> VK_DELETE
        59 | 60 => 0x10,      // SHIFT
        113 | 114 => 0x11,    // CTRL
        57 | 58 => 0x12,      // ALT
        92 => 0x21,           // PAGE_UP -> VK_PRIOR
        93 => 0x22,           // PAGE_DOWN -> VK_NEXT
        122 => 0x24,          // MOVE_HOME
        123 => 0x23,          // MOVE_END
        124 => 0x2D,          // INSERT
        131..=142 => 0x70 + (code - 131), // F1..F12
        55 => 0xBC,           // COMMA -> VK_OEM_COMMA
        56 => 0xBE,           // PERIOD -> VK_OEM_PERIOD
        69 => 0xBD,           // MINUS
        70 => 0xBB,           // EQUALS -> VK_OEM_PLUS
        81 => 0x6B,           // PLUS -> VK_ADD
        _ => return None,
    })
}

/// WM_CHAR code to send after a key-down (what Windows' TranslateMessage would produce).
///
/// `vk` is the mapped VK code (if any), `unicode` the character Android's KeyCharacterMap gives for the key with
/// its meta state (soft keyboards and `adb shell input text` deliver plain key events with Shift meta for capitals).
/// Control keys produce their ASCII control code: Enter 0x0D, Backspace 0x08, Esc 0x1B, Tab 0x09.
/// Once the host sends any Char, USER32 stops synthesizing chars itself (`host_chars`), so all of them come from here.
pub fn char_for_key(vk: Option<u32>, unicode: Option<char>) -> Option<u32> {
    match vk {
        Some(0x0D) => return Some(0x0D),
        Some(0x08) => return Some(0x08),
        Some(0x1B) => return Some(0x1B),
        Some(0x09) => return Some(0x09),
        _ => {}
    }
    let c = unicode? as u32;
    if c < 0x20 || c == 0x7F {
        return None;
    }
    Some(c)
}

/// VK code for a character typed on a soft keyboard whose Android key code has no VK mapping (e.g. punctuation
/// keys): the game still gets WM_CHAR from [`char_for_key`]; this gives a plausible WM_KEYDOWN for it.
pub fn vk_for_char(c: char) -> Option<u32> {
    Some(match c {
        'a'..='z' => c as u32 - 'a' as u32 + 0x41,
        'A'..='Z' | '0'..='9' => c as u32,
        ' ' => 0x20,
        '.' | '>' => 0xBE,
        ',' | '<' => 0xBC,
        '-' | '_' => 0xBD,
        '=' | '+' => 0xBB,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chars_for_keys() {
        assert_eq!(char_for_key(Some(0x0D), Some('\n')), Some(0x0D));
        assert_eq!(char_for_key(Some(0x08), None), Some(0x08));
        assert_eq!(char_for_key(Some(0x1B), None), Some(0x1B));
        assert_eq!(char_for_key(Some(0x41), Some('a')), Some('a' as u32));
        assert_eq!(char_for_key(Some(0x41), Some('A')), Some('A' as u32));
        assert_eq!(char_for_key(None, Some('!')), Some('!' as u32));
        assert_eq!(char_for_key(Some(0x10), None), None); // shift alone
        assert_eq!(char_for_key(Some(0x26), None), None); // arrow
        assert_eq!(vk_for_char('q'), Some(0x51));
        assert_eq!(vk_for_char('#'), None);
    }

    #[test]
    fn common_keys() {
        assert_eq!(android_to_vk(4), Some(0x1B));
        assert_eq!(android_to_vk(29), Some(b'A' as u32));
        assert_eq!(android_to_vk(54), Some(b'Z' as u32));
        assert_eq!(android_to_vk(7), Some(b'0' as u32));
        assert_eq!(android_to_vk(142), Some(0x7B));
        assert_eq!(android_to_vk(24), None); // volume up stays with the system
    }
}
