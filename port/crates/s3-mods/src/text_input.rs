//! Soft keyboard for the game's own text fields (`CEditControl`, docs/reverse-engineering/game-state-ui.md
//! section 9).
//!
//! User report 2026-11-09 (Motorola phone): a copy without `regs.cmd` asks for the serial number, but the soft
//! keyboard never appeared (the only way to open it was a three-finger tap, which nobody knows and which some
//! phones use for screenshots). Now:
//! - a tap on an enabled text field (Game Setup "Name of the Game", player names, ...) shows the keyboard;
//! - the serial number field shows it as soon as it gets the focus (when its dialog opens);
//! - when the field the keyboard was shown for loses the focus (another control tapped, Enter, dialog closed)
//!   or is destroyed, the keyboard is hidden.
//!
//! The hooks are installed for every exe whose code at these addresses matches the GOG 1.60 build (the GOG,
//! Settlers United and multiplayer exes are that build), not only for the GOG sha256: a non-GOG copy is the one
//! that asks for the serial.

use s3_runtime::{HookFlow, LogLevel, Machine};

use crate::KeyboardRequest;

/// `CEditControl` mouse handler (vtable slot 12, event 0x10 = left button down on the control).
const EDIT_MOUSE_DOWN: u32 = 0x0041_9b30;
/// `CEditControl` focus handler (vtable slot 25, `bool` argument: 1 = gets the focus, 0 = loses it).
const EDIT_FOCUS: u32 = 0x0041_9b10;
/// `CEditControl` destructor body (frees the text, then the `CControl` part).
const EDIT_DTOR: u32 = 0x0042_1f50;
// All three are entered with `this` = the `CScreenElement` virtual base at object +0x70 ([`ELEM`]).

/// First bytes of the three functions in the 1.60 build.
const SIGS: [(u32, [u8; 12]); 3] = [
    (EDIT_MOUSE_DOWN, [0x8b, 0xc1, 0x56, 0x57, 0x8a, 0x48, 0x9c, 0x84, 0xc9, 0x74, 0x24, 0x83]),
    (EDIT_FOCUS, [0x8b, 0x41, 0x94, 0x8b, 0x50, 0x04, 0x8b, 0x44, 0x0a, 0x94, 0x8d, 0x4c]),
    (EDIT_DTOR, [0x56, 0x8b, 0xf1, 0x8b, 0x4e, 0xe0, 0x85, 0xc9, 0x74, 0x1e, 0x8a, 0x41]),
];
/// `CEditControl` vtable of its `CScreenElement` base: slots 12 / 25 are thunks to the handlers above.
const EDIT_VTABLE: u32 = 0x0065_5d38;
const VTABLE_SLOTS: [(u32, u32); 2] = [(12, 0x0042_ad70), (25, 0x0042_ade0)];

/// Offset of the `CScreenElement` virtual base in a `CEditControl`.
const ELEM: u32 = 0x70;
/// Object `+0x0c`: read-only (Game Setup shows map and team values in disabled edit controls).
const OFF_DISABLED: u32 = 0x0c;
/// Object `+0x45`: number field (OnChar passes digits only; the serial number field).
const OFF_NUMBER: u32 = 0x45;
/// Object `+0x78..+0x84`: screen rect left, top, right, bottom (`CScreenElement +8..+0x14`).
const OFF_RECT: u32 = 0x78;

#[derive(Default)]
struct TextInputState {
    /// The field the keyboard was shown for.
    owner: Option<u32>,
}

/// Is the code the 1.60 build the hooks were made for?
fn code_matches(m: &mut Machine) -> bool {
    SIGS.iter().all(|(a, sig)| m.mem.read_bytes(*a, sig.len()) == sig[..])
        && VTABLE_SLOTS.iter().all(|&(i, v)| m.mem.read_u32(EDIT_VTABLE + 4 * i) == v)
}

fn editable(m: &mut Machine, obj: u32) -> bool {
    m.mem.read_u8(obj + OFF_DISABLED) == 0
}

fn show_for(m: &mut Machine, obj: u32, why: &str) {
    let r: [i32; 4] = std::array::from_fn(|i| m.mem.read_u32(obj + OFF_RECT + 4 * i as u32) as i32);
    m.log(LogLevel::Info, &format!("text input: {why} (edit {obj:#x}, rect {r:?}) -> show keyboard"));
    m.state::<TextInputState>().borrow_mut().owner = Some(obj);
    crate::request_keyboard(m, KeyboardRequest::Show);
}

/// Hide the keyboard if it was shown for `obj`.
fn release(m: &mut Machine, obj: u32, why: &str) {
    let st = m.state::<TextInputState>();
    let owned = st.borrow_mut().owner == Some(obj);
    if owned {
        st.borrow_mut().owner = None;
        m.log(LogLevel::Info, &format!("text input: edit {obj:#x} {why} -> hide keyboard"));
        crate::request_keyboard(m, KeyboardRequest::Hide);
    }
}

/// Install the hooks when the exe is the 1.60 build. Returns whether they were installed.
pub fn install(m: &mut Machine) -> bool {
    if !code_matches(m) {
        m.log(LogLevel::Info, "text input: exe code differs from the 1.60 build; no automatic soft keyboard");
        return false;
    }
    crate::add_hook(m, EDIT_MOUSE_DOWN, "text-input", |c| {
        let obj = c.reg(s3_cpu::ECX).wrapping_sub(ELEM);
        if editable(c.m, obj) {
            show_for(c.m, obj, "tap on a text field");
        }
        HookFlow::Continue
    });
    crate::add_hook(m, EDIT_FOCUS, "text-input", |c| {
        let obj = c.reg(s3_cpu::ECX).wrapping_sub(ELEM);
        // A tap on the field itself first takes the focus away and then gives it back (the tap shows the
        // keyboard in between, the newest request wins).
        if c.arg(0) & 0xff == 0 {
            release(c.m, obj, "lost the focus");
        } else if editable(c.m, obj) && c.m.mem.read_u8(obj + OFF_NUMBER) != 0 {
            show_for(c.m, obj, "number field got the focus (serial number dialog)");
        }
        HookFlow::Continue
    });
    crate::add_hook(m, EDIT_DTOR, "text-input", |c| {
        let obj = c.reg(s3_cpu::ECX).wrapping_sub(ELEM);
        release(c.m, obj, "destroyed");
        HookFlow::Continue
    });
    m.log(LogLevel::Info, "text input: soft keyboard hooks on the game's text fields installed");
    true
}

/// Tests: write the 1.60 signature bytes and vtable slots (the bodies are stubbed by the test); returns the
/// handlers as (address, stdcall argument dwords).
#[cfg(test)]
pub(crate) fn write_test_signatures(m: &mut Machine) -> [(u32, u32); 3] {
    for (a, sig) in SIGS {
        m.mem.write_bytes(a, &sig);
    }
    for (i, v) in VTABLE_SLOTS {
        m.mem.write_u32(EDIT_VTABLE + 4 * i, v);
    }
    [(EDIT_MOUSE_DOWN, 5), (EDIT_FOCUS, 1), (EDIT_DTOR, 0)]
}
