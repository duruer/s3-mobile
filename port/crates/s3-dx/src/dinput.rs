//! DirectInput 3 HLE (`dinput.dll`, `DirectInputCreateA(hinst, 0x0300, ...)`).
//!
//! Devices for `GUID_SysMouse` (S3 uses only this one: c_dfDIMouse, buffer 128, event notification) and
//! `GUID_SysKeyboard`, fed from `Machine::input` (each device keeps its own event cursor). Mouse axes are
//! relative: deltas between successive absolute host positions. [`pump_service`] runs once per scheduler slice:
//! it converts new host events into each acquired device's buffer and signals the device's notification event.

use std::collections::{HashMap, VecDeque};

use s3_runtime::com::Guid;
use s3_runtime::{HostEvent, KernelObject, Machine, MouseButton, Registry, ThunkResult};

use crate::hr::*;
use crate::{declare_interface, reg_method, ret};

pub const DLL: &str = "dinput.dll";

pub const GUID_SYSMOUSE: Guid = Guid::parse("6F1D2B60-D5A0-11CF-BFC7-444553540000");
pub const GUID_SYSKEYBOARD: Guid = Guid::parse("6F1D2B61-D5A0-11CF-BFC7-444553540000");
pub const IID_IDIRECTINPUTA: Guid = Guid::parse("89521360-AA8A-11CF-BFC7-444553540000");
pub const IID_IDIRECTINPUTDEVICEA: Guid = Guid::parse("5944E680-C92E-11CF-BFC7-444553540000");
const IID_IUNKNOWN: Guid = Guid::parse("00000000-0000-0000-C000-000000000046");

pub const EXPORTS: &[&str] = &["DirectInputCreateA"];

pub const IDIRECTINPUTA: &[&str] =
    &["QueryInterface", "AddRef", "Release", "CreateDevice", "EnumDevices", "GetDeviceStatus", "RunControlPanel", "Initialize"];

pub const IDIRECTINPUTDEVICEA: &[&str] = &[
    "QueryInterface", "AddRef", "Release", "GetCapabilities", "EnumObjects", "GetProperty", "SetProperty",
    "Acquire", "Unacquire", "GetDeviceState", "GetDeviceData", "SetDataFormat", "SetEventNotification",
    "SetCooperativeLevel", "GetObjectInfo", "GetDeviceInfo", "RunControlPanel", "Initialize",
];

pub const DI_OK: u32 = 0;
pub const DI_BUFFEROVERFLOW: u32 = 1;
pub const DIERR_NOTACQUIRED: u32 = 0x8007_000C;
pub const DIERR_DEVICENOTREG: u32 = 0x8004_0154;
pub const DIGDD_PEEK: u32 = 1;
pub const DIPROP_BUFFERSIZE: u32 = 1;
pub const DIMOFS_X: u32 = 0;
pub const DIMOFS_Y: u32 = 4;
pub const DIMOFS_Z: u32 = 8;
pub const DIMOFS_BUTTON0: u32 = 12;
const DIDEVTYPE_MOUSE: u32 = 2;
const DIDEVTYPE_KEYBOARD: u32 = 3;
/// sizeof(DIDEVICEINSTANCEA).
const DIDEVICEINSTANCE_SIZE: u32 = 0x230;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    #[default]
    Mouse,
    Keyboard,
}

/// One buffered DIDEVICEOBJECTDATA.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiEvent {
    pub ofs: u32,
    pub data: u32,
    pub time: u32,
    pub seq: u32,
}

/// Host-side state of one device (keyed by interface pointer).
#[derive(Debug, Default)]
pub struct Device {
    pub kind: Kind,
    pub acquired: bool,
    pub event: u32,
    /// DIPROP_BUFFERSIZE (0 = unbuffered: events are not kept).
    pub buffer_size: u32,
    pub buf: VecDeque<DiEvent>,
    pub overflow: bool,
    cursor: u64,
    last_x: i32,
    last_y: i32,
    /// Accumulated deltas for GetDeviceState.
    acc: [i32; 3],
    buttons: [u8; 4],
    keys: Vec<u8>,
}

impl Device {
    fn push(&mut self, e: DiEvent) -> bool {
        if self.buffer_size == 0 {
            return false;
        }
        if self.buf.len() >= self.buffer_size as usize {
            self.buf.pop_front();
            self.overflow = true;
        }
        self.buf.push_back(e);
        true
    }

    /// Convert host events; returns true if anything was buffered.
    fn feed(&mut self, events: &[HostEvent], now: u32, seq: &mut u32) -> bool {
        let mut any = false;
        for e in events {
            match (self.kind, *e) {
                (Kind::Mouse, HostEvent::MouseMove { x, y }) => {
                    let (dx, dy) = (x - self.last_x, y - self.last_y);
                    self.last_x = x;
                    self.last_y = y;
                    self.acc[0] += dx;
                    self.acc[1] += dy;
                    *seq += 1;
                    if dx != 0 {
                        any |= self.push(DiEvent { ofs: DIMOFS_X, data: dx as u32, time: now, seq: *seq });
                    }
                    if dy != 0 {
                        any |= self.push(DiEvent { ofs: DIMOFS_Y, data: dy as u32, time: now, seq: *seq });
                    }
                }
                (Kind::Mouse, HostEvent::MouseButton { button, down }) => {
                    let i = match button {
                        MouseButton::Left => 0,
                        MouseButton::Right => 1,
                        MouseButton::Middle => 2,
                    };
                    let v = if down { 0x80 } else { 0 };
                    self.buttons[i] = v as u8;
                    *seq += 1;
                    any |= self.push(DiEvent { ofs: DIMOFS_BUTTON0 + i as u32, data: v, time: now, seq: *seq });
                }
                (Kind::Keyboard, HostEvent::Key { vk, down }) => {
                    let dik = vk_to_dik(vk);
                    if dik != 0 {
                        let v = if down { 0x80 } else { 0 };
                        self.keys[dik as usize] = v as u8;
                        *seq += 1;
                        any |= self.push(DiEvent { ofs: dik, data: v, time: now, seq: *seq });
                    }
                }
                _ => {}
            }
        }
        any
    }
}

#[derive(Default)]
pub struct DiState {
    pub devices: HashMap<u32, Device>,
    seq: u32,
}

fn state(m: &mut Machine) -> s3_runtime::Shared<DiState> {
    m.state::<DiState>()
}

pub fn register(reg: &mut Registry) {
    reg.declare_all(DLL, EXPORTS);
    declare_interface(reg, DLL, "IDirectInputA", IDIRECTINPUTA);
    declare_interface(reg, DLL, "IDirectInputDeviceA", IDIRECTINPUTDEVICEA);
    reg.register(DLL, "DirectInputCreateA", direct_input_create_a);
    let di: &[(&str, s3_runtime::Handler)] = &[
        ("QueryInterface", di_query_interface),
        ("AddRef", Machine::com_addref),
        ("Release", Machine::com_release),
        ("CreateDevice", di_create_device),
        ("EnumDevices", di_enum_devices),
        ("GetDeviceStatus", ok2),
        ("RunControlPanel", ok3),
        ("Initialize", ok3),
    ];
    for (n, h) in di {
        reg_method(reg, DLL, "IDirectInputA", n, *h);
    }
    let dev: &[(&str, s3_runtime::Handler)] = &[
        ("QueryInterface", dev_query_interface),
        ("AddRef", Machine::com_addref),
        ("Release", dev_release),
        ("GetCapabilities", dev_get_capabilities),
        ("EnumObjects", ok4),
        ("GetProperty", dev_get_property),
        ("SetProperty", dev_set_property),
        ("Acquire", dev_acquire),
        ("Unacquire", dev_unacquire),
        ("GetDeviceState", dev_get_device_state),
        ("GetDeviceData", dev_get_device_data),
        ("SetDataFormat", ok2),
        ("SetEventNotification", dev_set_event_notification),
        ("SetCooperativeLevel", ok3),
        ("GetObjectInfo", notimpl4),
        ("GetDeviceInfo", dev_get_device_info),
        ("RunControlPanel", ok3),
        ("Initialize", ok4),
    ];
    for (n, h) in dev {
        reg_method(reg, DLL, "IDirectInputDeviceA", n, *h);
    }
}

crate::stubs! {
    ok2 = (2, DI_OK);
    ok3 = (3, DI_OK);
    ok4 = (4, DI_OK);
    notimpl4 = (4, E_NOTIMPL);
}

/// `DirectInputCreateA(hinst, dwVersion, lplpDirectInput, pUnkOuter)`.
fn direct_input_create_a(m: &mut Machine) -> ThunkResult {
    let out = m.arg_u32(2);
    if out == 0 {
        return ret(E_INVALIDARG, 4);
    }
    let vt = m.com_vtable(DLL, "IDirectInputA", IDIRECTINPUTA);
    let p = m.com_new_object(vt, 0x10);
    m.mem.write_u32(out, p);
    ret(DI_OK, 4)
}

fn qi_self(m: &mut Machine, ok: &[Guid]) -> ThunkResult {
    let (this, riid, out) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    if out == 0 {
        return ret(E_POINTER, 3);
    }
    let iid = Guid::read(m, riid);
    if ok.contains(&iid) || iid == IID_IUNKNOWN {
        let n = m.mem.read_u32(this + 4) + 1;
        m.mem.write_u32(this + 4, n);
        m.mem.write_u32(out, this);
        ret(S_OK, 3)
    } else {
        m.mem.write_u32(out, 0);
        ret(E_NOINTERFACE, 3)
    }
}

fn di_query_interface(m: &mut Machine) -> ThunkResult {
    qi_self(m, &[IID_IDIRECTINPUTA])
}

fn dev_query_interface(m: &mut Machine) -> ThunkResult {
    qi_self(m, &[IID_IDIRECTINPUTDEVICEA])
}

/// CreateDevice(this, rguid, lplpDevice, pUnkOuter).
fn di_create_device(m: &mut Machine) -> ThunkResult {
    let (rguid, out) = (m.arg_u32(1), m.arg_u32(2));
    if rguid == 0 || out == 0 {
        return ret(E_INVALIDARG, 4);
    }
    let g = Guid::read(m, rguid);
    let kind = if g == GUID_SYSMOUSE {
        Kind::Mouse
    } else if g == GUID_SYSKEYBOARD {
        Kind::Keyboard
    } else {
        return ret(DIERR_DEVICENOTREG, 4);
    };
    let vt = m.com_vtable(DLL, "IDirectInputDeviceA", IDIRECTINPUTDEVICEA);
    let p = m.com_new_object(vt, 0x10);
    m.mem.write_u32(p + 8, kind as u32);
    state(m).borrow_mut().devices.insert(p, Device { kind, keys: vec![0; 256], ..Default::default() });
    m.mem.write_u32(out, p);
    ret(DI_OK, 4)
}

fn write_device_instance(m: &mut Machine, p: u32, kind: Kind) {
    m.mem.fill(p + 4, (DIDEVICEINSTANCE_SIZE - 4) as usize, 0);
    let (g, t, name) = match kind {
        Kind::Mouse => (GUID_SYSMOUSE, DIDEVTYPE_MOUSE, "Mouse"),
        Kind::Keyboard => (GUID_SYSKEYBOARD, DIDEVTYPE_KEYBOARD, "Keyboard"),
    };
    g.write(m, p + 4);
    g.write(m, p + 0x14);
    m.mem.write_u32(p + 0x24, t);
    m.mem.write_cstr(p + 0x28, name);
    m.mem.write_cstr(p + 0x12C, name);
}

/// EnumDevices(this, dwDevType, lpCallback, pvRef, dwFlags): `BOOL cb(LPCDIDEVICEINSTANCEA, pvRef)`.
fn di_enum_devices(m: &mut Machine) -> ThunkResult {
    let (ty, cb, ctx) = (m.arg_u32(1), m.arg_u32(2), m.arg_u32(3));
    if cb == 0 {
        return ret(E_INVALIDARG, 5);
    }
    let buf = m.mem.alloc(DIDEVICEINSTANCE_SIZE);
    for (kind, t) in [(Kind::Mouse, DIDEVTYPE_MOUSE), (Kind::Keyboard, DIDEVTYPE_KEYBOARD)] {
        if ty != 0 && ty & 0xFF != t {
            continue;
        }
        m.mem.write_u32(buf, DIDEVICEINSTANCE_SIZE);
        write_device_instance(m, buf, kind);
        if m.call_guest(cb, &[buf, ctx]) == 0 {
            break;
        }
    }
    m.mem.free(buf);
    ret(DI_OK, 5)
}

fn dev_release(m: &mut Machine) -> ThunkResult {
    let this = m.arg_u32(0);
    let n = m.mem.read_u32(this + 4).saturating_sub(1);
    m.mem.write_u32(this + 4, n);
    if n == 0 {
        state(m).borrow_mut().devices.remove(&this);
        m.mem.free(this);
    }
    ret(n, 1)
}

fn with_dev<R>(m: &mut Machine, p: u32, f: impl FnOnce(&mut Device) -> R) -> Option<R> {
    let st = state(m);
    let mut s = st.borrow_mut();
    s.devices.get_mut(&p).map(f)
}

/// DIDEVCAPS {dwSize, dwFlags, dwDevType, dwAxes, dwButtons, dwPOVs, ...}.
fn dev_get_capabilities(m: &mut Machine) -> ThunkResult {
    let (this, p) = (m.arg_u32(0), m.arg_u32(1));
    let Some(kind) = with_dev(m, this, |d| d.kind) else { return ret(E_INVALIDARG, 2) };
    if p == 0 {
        return ret(E_INVALIDARG, 2);
    }
    let size = m.mem.read_u32(p).clamp(0x18, 0x2C);
    m.mem.fill(p + 4, (size - 4) as usize, 0);
    m.mem.write_u32(p + 4, 1); // DIDC_ATTACHED
    let (t, axes, buttons) = match kind {
        Kind::Mouse => (DIDEVTYPE_MOUSE, 3, 3),
        Kind::Keyboard => (DIDEVTYPE_KEYBOARD, 0, 128),
    };
    m.mem.write_u32(p + 8, t);
    m.mem.write_u32(p + 0xC, axes);
    m.mem.write_u32(p + 0x10, buttons);
    ret(DI_OK, 2)
}

fn dev_get_device_info(m: &mut Machine) -> ThunkResult {
    let (this, p) = (m.arg_u32(0), m.arg_u32(1));
    let Some(kind) = with_dev(m, this, |d| d.kind) else { return ret(E_INVALIDARG, 2) };
    if p == 0 {
        return ret(E_INVALIDARG, 2);
    }
    write_device_instance(m, p, kind);
    ret(DI_OK, 2)
}

/// GetProperty(this, rguidProp, pdiph): only DIPROP_BUFFERSIZE.
fn dev_get_property(m: &mut Machine) -> ThunkResult {
    let (this, prop, p) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    if prop != DIPROP_BUFFERSIZE || p == 0 {
        return ret(E_NOTIMPL, 3);
    }
    let Some(n) = with_dev(m, this, |d| d.buffer_size) else { return ret(E_INVALIDARG, 3) };
    m.mem.write_u32(p + 0x10, n);
    ret(DI_OK, 3)
}

/// SetProperty(this, rguidProp, pdiph): DIPROP_BUFFERSIZE sets the buffer size; others are accepted.
fn dev_set_property(m: &mut Machine) -> ThunkResult {
    let (this, prop, p) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    if prop == DIPROP_BUFFERSIZE && p != 0 {
        let n = m.mem.read_u32(p + 0x10).min(4096);
        with_dev(m, this, |d| {
            d.buffer_size = n;
            while d.buf.len() > n as usize {
                d.buf.pop_front();
            }
        });
    }
    ret(DI_OK, 3)
}

fn dev_acquire(m: &mut Machine) -> ThunkResult {
    let this = m.arg_u32(0);
    let (cur, x, y) = (m.input.cursor_now(), m.input.mouse_x, m.input.mouse_y);
    let r = with_dev(m, this, |d| {
        if d.acquired {
            return S_FALSE;
        }
        d.acquired = true;
        d.cursor = cur;
        d.last_x = x;
        d.last_y = y;
        d.acc = [0; 3];
        DI_OK
    });
    ret(r.unwrap_or(E_INVALIDARG), 1)
}

fn dev_unacquire(m: &mut Machine) -> ThunkResult {
    let this = m.arg_u32(0);
    let r = with_dev(m, this, |d| {
        let was = d.acquired;
        d.acquired = false;
        if was {
            DI_OK
        } else {
            S_FALSE
        }
    });
    ret(r.unwrap_or(E_INVALIDARG), 1)
}

fn dev_set_event_notification(m: &mut Machine) -> ThunkResult {
    let (this, h) = (m.arg_u32(0), m.arg_u32(1));
    with_dev(m, this, |d| d.event = h);
    ret(DI_OK, 2)
}

/// GetDeviceState(this, cbData, lpvData): DIMOUSESTATE (16 bytes) or 256 key bytes.
fn dev_get_device_state(m: &mut Machine) -> ThunkResult {
    let (this, cb, p) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    pump_service(m);
    let st = state(m);
    let mut s = st.borrow_mut();
    let Some(d) = s.devices.get_mut(&this) else { return ret(E_INVALIDARG, 3) };
    if !d.acquired {
        return ret(DIERR_NOTACQUIRED, 3);
    }
    if p == 0 {
        return ret(E_INVALIDARG, 3);
    }
    let mut bytes = vec![0u8; cb as usize];
    match d.kind {
        Kind::Mouse => {
            for (i, v) in d.acc.iter().enumerate() {
                if let Some(dst) = bytes.get_mut(4 * i..4 * i + 4) {
                    dst.copy_from_slice(&v.to_le_bytes());
                }
            }
            d.acc = [0; 3];
            for (i, b) in d.buttons.iter().enumerate() {
                if let Some(x) = bytes.get_mut(12 + i) {
                    *x = *b;
                }
            }
        }
        Kind::Keyboard => {
            let n = bytes.len().min(256);
            bytes[..n].copy_from_slice(&d.keys[..n]);
        }
    }
    m.mem.write_bytes(p, &bytes);
    ret(DI_OK, 3)
}

/// GetDeviceData(this, cbObjectData, rgdod, pdwInOut, dwFlags).
fn dev_get_device_data(m: &mut Machine) -> ThunkResult {
    let (this, cb, out, pn, flags) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_u32(3), m.arg_u32(4));
    if pn == 0 || cb < 16 {
        return ret(E_INVALIDARG, 5);
    }
    pump_service(m);
    let want = m.mem.read_u32(pn);
    let st = state(m);
    let mut s = st.borrow_mut();
    let Some(d) = s.devices.get_mut(&this) else { return ret(E_INVALIDARG, 5) };
    if !d.acquired {
        return ret(DIERR_NOTACQUIRED, 5);
    }
    let n = (want as usize).min(d.buf.len());
    if out != 0 {
        for (i, e) in d.buf.iter().take(n).enumerate() {
            let a = out + i as u32 * cb;
            m.mem.fill(a, cb as usize, 0);
            m.mem.write_u32(a, e.ofs);
            m.mem.write_u32(a + 4, e.data);
            m.mem.write_u32(a + 8, e.time);
            m.mem.write_u32(a + 12, e.seq);
        }
    }
    if flags & DIGDD_PEEK == 0 {
        d.buf.drain(..n);
    }
    let r = if d.overflow {
        if flags & DIGDD_PEEK == 0 {
            d.overflow = false;
        }
        DI_BUFFEROVERFLOW
    } else {
        DI_OK
    };
    m.mem.write_u32(pn, n as u32);
    ret(r, 5)
}

/// Scheduler service (also called from GetDeviceData/GetDeviceState): feed new host input to every acquired
/// device and signal notification events.
pub fn pump_service(m: &mut Machine) {
    let st = state(m);
    let now = m.now_ms() as u32;
    let mut signal = Vec::new();
    {
        let mut s = st.borrow_mut();
        let mut seq = s.seq;
        for d in s.devices.values_mut() {
            if !d.acquired {
                continue;
            }
            let evs = m.input.events_since(&mut d.cursor);
            if evs.is_empty() {
                continue;
            }
            if d.feed(&evs, now, &mut seq) && d.event != 0 {
                signal.push(d.event);
            }
        }
        s.seq = seq;
    }
    for h in signal {
        if let Some(KernelObject::Event { signaled, .. }) = m.handles.get_mut(h) {
            *signaled = true;
        }
    }
}

/// Windows virtual-key code to DirectInput scan code (DIK_*); 0 = unmapped.
pub fn vk_to_dik(vk: u32) -> u32 {
    const LETTERS: [u8; 26] = [
        0x1E, 0x30, 0x2E, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32, 0x31, 0x18, 0x19, 0x10,
        0x13, 0x1F, 0x14, 0x16, 0x2F, 0x11, 0x2D, 0x15, 0x2C,
    ];
    const NUMPAD: [u8; 10] = [0x52, 0x4F, 0x50, 0x51, 0x4B, 0x4C, 0x4D, 0x47, 0x48, 0x49];
    (match vk {
        0x41..=0x5A => LETTERS[(vk - 0x41) as usize],
        0x31..=0x39 => (vk - 0x31 + 2) as u8,
        0x30 => 0x0B,
        0x60..=0x69 => NUMPAD[(vk - 0x60) as usize],
        0x70..=0x79 => (vk - 0x70 + 0x3B) as u8,
        0x7A => 0x57,
        0x7B => 0x58,
        0x1B => 0x01,
        0x08 => 0x0E,
        0x09 => 0x0F,
        0x0D => 0x1C,
        0x10 | 0xA0 => 0x2A,
        0xA1 => 0x36,
        0x11 | 0xA2 => 0x1D,
        0xA3 => 0x9D,
        0x12 | 0xA4 => 0x38,
        0xA5 => 0xB8,
        0x20 => 0x39,
        0x14 => 0x3A,
        0x90 => 0x45,
        0x91 => 0x46,
        0x6A => 0x37,
        0x6B => 0x4E,
        0x6D => 0x4A,
        0x6E => 0x53,
        0x6F => 0xB5,
        0x13 => 0xC5,
        0x24 => 0xC7,
        0x26 => 0xC8,
        0x21 => 0xC9,
        0x25 => 0xCB,
        0x27 => 0xCD,
        0x23 => 0xCF,
        0x28 => 0xD0,
        0x22 => 0xD1,
        0x2D => 0xD2,
        0x2E => 0xD3,
        0xBA => 0x27,
        0xBB => 0x0D,
        0xBC => 0x33,
        0xBD => 0x0C,
        0xBE => 0x34,
        0xBF => 0x35,
        0xC0 => 0x29,
        0xDB => 0x1A,
        0xDC => 0x2B,
        0xDD => 0x1B,
        0xDE => 0x28,
        _ => 0,
    }) as u32
}
