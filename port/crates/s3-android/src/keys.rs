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
        69 => 0xBD,           // MINUS
        70 => 0xBB,           // EQUALS -> VK_OEM_PLUS
        81 => 0x6B,           // PLUS -> VK_ADD
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
