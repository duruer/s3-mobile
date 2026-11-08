//! `user32.dll` HLE: window classes, windows, per-thread message queues, input -> window messages, timers,
//! cursor, rectangles, `wsprintfA`, `DrawTextA` (via gdi32), `MessageBoxA` (logs, auto-answers).
//!
//! Model (see docs/porting/user32-gdi32.md):
//! - Every window belongs to the guest thread that created it; messages for it go to that thread's queue.
//!   Queues are created on demand (also by `PeekMessage(PM_NOREMOVE)`, the classic trick S3 uses).
//! - Host input (`m.input` event log) is turned into WM_MOUSEMOVE/LBUTTON*/RBUTTON*/KEY*/CHAR messages and posted to
//!   the thread owning the capture/active window. Mouse moves are coalesced. Coordinates: the screen is the
//!   display mode, windows sit at (0,0), client area = window rect (no non-client area; HYPOTHESIS that S3 does
//!   not care about its caption under exclusive full-screen DirectDraw).
//! - Sent messages (SendMessage, CreateWindowEx's WM_CREATE etc.) call the WndProc directly on the calling thread
//!   with `Machine::call_guest`, also for windows of other threads (HYPOTHESIS: good enough for S3).
//! - `GetMessageA`/`WaitMessage`/`MsgWaitForMultipleObjects` block (ThunkResult::Block) and are retried by the
//!   scheduler. Retrieval order: posted messages, WM_QUIT, WM_PAINT, WM_TIMER.
//! - The first visible top-level window gets WM_ACTIVATEAPP/WM_NCACTIVATE/WM_ACTIVATE/WM_SETFOCUS when shown, so the
//!   DirectX code believes it is in the foreground.

use std::collections::{BTreeMap, HashMap, VecDeque};

use s3_runtime::{LogLevel, Machine, MouseButton, Registry, ThunkResult};
use s3_runtime::cursor::{self, CursorState};
use s3_runtime::resources::ResId;
use std::sync::Arc;

use crate::gdi32::{self, Rect};

/// DLL name as registered.
pub const DLL: &str = "user32.dll";

/// Known exports (static imports of S3.EXE first).
pub const NAMES: &[&str] = &[
    "SetCursor", "GetMessagePos", "GetDC", "LoadCursorA", "ClientToScreen", "ClipCursor",
    "GetCursorPos", "MoveWindow", "GetClientRect", "wsprintfA", "SendMessageTimeoutA", "ReleaseDC",
    "MsgWaitForMultipleObjects", "PostThreadMessageA", "DispatchMessageA", "TranslateMessage",
    "DrawTextA", "PostMessageA", "SendMessageA", "ShowCursor", "IntersectRect", "IsRectEmpty",
    "PtInRect", "GetSystemMetrics", "LoadIconA", "wvsprintfA", "RegisterClassExA", "GetWindowLongA",
    "PostQuitMessage", "MessageBoxA", "DefWindowProcA", "EndPaint", "BeginPaint", "SetRect",
    "PeekMessageA", "SetWindowLongA", "GetMessageA", "CreateWindowExA", "ShowWindow",
    "DestroyWindow", "RegisterClassA", "RegisterWindowMessageA", "GetActiveWindow", "SetFocus",
    "GetFocus", "SetForegroundWindow", "UpdateWindow", "InvalidateRect", "ScreenToClient",
    "GetWindowRect", "SetCursorPos", "GetAsyncKeyState", "GetKeyState", "SetTimer", "KillTimer",
    "LoadStringA", "IsIconic", "GetForegroundWindow", "SystemParametersInfoA", "FindWindowA",
    "SetWindowPos", "AdjustWindowRect", "AdjustWindowRectEx", "ReleaseCapture", "SetCapture",
    "MessageBeep", "LoadImageA", "DestroyCursor", "DestroyIcon", "CallWindowProcA", "EnumWindows",
    "GetWindowTextA", "SetWindowTextA", "WaitMessage",
    "FillRect", "ValidateRect", "GetCapture", "IsWindow", "IsWindowVisible", "GetParent",
    "GetDesktopWindow", "GetSysColor", "SetRectEmpty", "CopyRect", "OffsetRect", "InflateRect",
    "UnionRect", "EqualRect", "UnregisterClassA", "GetMessageTime", "GetKeyboardState",
    "MapVirtualKeyA", "GetQueueStatus", "GetInputState", "IsWindowEnabled", "EnableWindow",
    "GetUpdateRect", "SetActiveWindow", "BringWindowToTop", "GetClassNameA", "SendNotifyMessageA",
    "GetKeyboardLayout", "GetDoubleClickTime", "SetWindowTextW", "CharUpperA", "CharLowerA",
    "WindowFromPoint", "ChildWindowFromPoint", "OpenClipboard", "CloseClipboard", "GetClipboardData",
    "LoadBitmapA", "DestroyMenu", "GetMenu", "SetMenu", "DrawIcon", "GetWindowPlacement",
];

// ---------------------------------------------------------------------------------------------- messages

pub const WM_CREATE: u32 = 0x0001;
pub const WM_DESTROY: u32 = 0x0002;
pub const WM_MOVE: u32 = 0x0003;
pub const WM_SIZE: u32 = 0x0005;
pub const WM_ACTIVATE: u32 = 0x0006;
pub const WM_SETFOCUS: u32 = 0x0007;
pub const WM_KILLFOCUS: u32 = 0x0008;
pub const WM_PAINT: u32 = 0x000F;
pub const WM_CLOSE: u32 = 0x0010;
pub const WM_QUIT: u32 = 0x0012;
pub const WM_ERASEBKGND: u32 = 0x0014;
pub const WM_SHOWWINDOW: u32 = 0x0018;
pub const WM_ACTIVATEAPP: u32 = 0x001C;
pub const WM_SETCURSOR: u32 = 0x0020;
pub const WM_MOUSEACTIVATE: u32 = 0x0021;
pub const WM_GETMINMAXINFO: u32 = 0x0024;
pub const WM_DISPLAYCHANGE: u32 = 0x007E;
pub const WM_NCCREATE: u32 = 0x0081;
pub const WM_NCDESTROY: u32 = 0x0082;
pub const WM_NCCALCSIZE: u32 = 0x0083;
pub const WM_NCHITTEST: u32 = 0x0084;
pub const WM_NCACTIVATE: u32 = 0x0086;
pub const WM_KEYDOWN: u32 = 0x0100;
pub const WM_KEYUP: u32 = 0x0101;
pub const WM_CHAR: u32 = 0x0102;
pub const WM_SYSKEYDOWN: u32 = 0x0104;
pub const WM_SYSKEYUP: u32 = 0x0105;
pub const WM_SYSCHAR: u32 = 0x0106;
pub const WM_SYSCOMMAND: u32 = 0x0112;
pub const WM_TIMER: u32 = 0x0113;
pub const WM_MOUSEMOVE: u32 = 0x0200;
pub const WM_LBUTTONDOWN: u32 = 0x0201;
pub const WM_LBUTTONUP: u32 = 0x0202;
pub const WM_LBUTTONDBLCLK: u32 = 0x0203;
pub const WM_RBUTTONDOWN: u32 = 0x0204;
pub const WM_RBUTTONUP: u32 = 0x0205;
pub const WM_RBUTTONDBLCLK: u32 = 0x0206;
pub const WM_MBUTTONDOWN: u32 = 0x0207;
pub const WM_MBUTTONUP: u32 = 0x0208;
pub const WM_MBUTTONDBLCLK: u32 = 0x0209;

const WS_CHILD: u32 = 0x4000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const CS_DBLCLKS: u32 = 0x0008;
const CW_USEDEFAULT: u32 = 0x8000_0000;
const HWND_BROADCAST: u32 = 0xFFFF;

/// A queued message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Msg {
    pub hwnd: u32,
    pub message: u32,
    pub wparam: u32,
    pub lparam: u32,
    pub time: u32,
    pub pt: (i32, i32),
}

impl Msg {
    fn write(&self, m: &mut Machine, p: u32) {
        m.mem.write_u32(p, self.hwnd);
        m.mem.write_u32(p + 4, self.message);
        m.mem.write_u32(p + 8, self.wparam);
        m.mem.write_u32(p + 12, self.lparam);
        m.mem.write_u32(p + 16, self.time);
        m.mem.write_u32(p + 20, self.pt.0 as u32);
        m.mem.write_u32(p + 24, self.pt.1 as u32);
    }
    fn read(m: &Machine, p: u32) -> Msg {
        Msg {
            hwnd: m.mem.read_u32(p),
            message: m.mem.read_u32(p + 4),
            wparam: m.mem.read_u32(p + 8),
            lparam: m.mem.read_u32(p + 12),
            time: m.mem.read_u32(p + 16),
            pt: (m.mem.read_u32(p + 20) as i32, m.mem.read_u32(p + 24) as i32),
        }
    }
}

/// Per-thread message queue.
#[derive(Debug, Default)]
pub struct Queue {
    pub msgs: VecDeque<Msg>,
    pub quit: Option<u32>,
    pub last_pos: (i32, i32),
    pub last_time: u32,
}

#[derive(Debug, Clone)]
pub struct WndClass {
    pub name: String,
    pub atom: u32,
    pub style: u32,
    pub wndproc: u32,
    pub wnd_extra: u32,
    pub hinstance: u32,
    pub cursor: u32,
    pub icon: u32,
    pub background: u32,
}

#[derive(Debug, Clone)]
pub struct Window {
    pub hwnd: u32,
    pub class: String,
    pub class_style: u32,
    pub wndproc: u32,
    pub style: u32,
    pub ex_style: u32,
    pub title: String,
    pub rect: Rect,
    pub parent: u32,
    pub id: u32,
    pub hinstance: u32,
    pub user_data: u32,
    pub extra: Vec<u8>,
    pub owner_tid: u32,
    pub visible: bool,
    pub needs_paint: bool,
}

#[derive(Debug, Clone)]
pub struct Timer {
    pub hwnd: u32,
    pub id: u32,
    pub period: u32,
    pub next_due: u64,
    pub proc_: u32,
    pub tid: u32,
}

/// user32 state.
#[derive(Debug)]
pub struct UserState {
    pub classes: HashMap<String, WndClass>,
    pub windows: BTreeMap<u32, Window>,
    next_hwnd: u32,
    next_atom: u32,
    pub queues: HashMap<u32, Queue>,
    pub timers: Vec<Timer>,
    next_timer_id: u32,
    input_cursor: Option<u64>,
    pub active: u32,
    pub focus: u32,
    pub capture: u32,
    pub cursor: u32,
    pub cursor_count: i32,
    pub clip: Option<Rect>,
    pub screen: (u32, u32),
    reg_msgs: HashMap<String, u32>,
    pub activated: bool,
    /// The host sends WM_CHAR text itself (HostEvent::Char) -> TranslateMessage does not synthesize chars.
    pub host_chars: bool,
    toggles: [bool; 256],
    last_click: Option<(u32, u64, i32, i32)>,
    window_dcs: Vec<u32>,
    service_added: bool,
    /// MK_* state as seen by the message stream (input events are applied to it in order).
    mk_state: u32,
    alt_state: bool,
}

impl Default for UserState {
    fn default() -> Self {
        UserState {
            classes: HashMap::new(),
            windows: BTreeMap::new(),
            next_hwnd: 0x0001_0010,
            next_atom: 0xC000,
            queues: HashMap::new(),
            timers: Vec::new(),
            next_timer_id: 0x7000,
            input_cursor: None,
            active: 0,
            focus: 0,
            capture: 0,
            cursor: 0,
            cursor_count: 0,
            clip: None,
            screen: (800, 600),
            reg_msgs: HashMap::new(),
            activated: false,
            host_chars: false,
            toggles: [false; 256],
            last_click: None,
            window_dcs: Vec::new(),
            service_added: false,
            mk_state: 0,
            alt_state: false,
        }
    }
}

fn us(m: &mut Machine) -> s3_runtime::Shared<UserState> {
    m.state::<UserState>()
}

/// Current screen (display mode) size.
pub fn screen_size(m: &mut Machine) -> (u32, u32) {
    us(m).borrow_mut().screen
}

/// Set the screen size (called by ddraw SetDisplayMode). Full-screen top-level windows are resized and every
/// top-level window gets WM_DISPLAYCHANGE (posted; HYPOTHESIS: Windows sends it synchronously).
pub fn set_screen_size(m: &mut Machine, w: u32, h: u32, bpp: u32) {
    let st = us(m);
    let mut s = st.borrow_mut();
    let old = s.screen;
    s.screen = (w, h);
    if old == (w, h) {
        return;
    }
    let now = m.now_ms() as u32;
    let mut posts = Vec::new();
    for win in s.windows.values_mut() {
        if win.style & WS_CHILD == 0 {
            if win.rect == Rect::new(0, 0, old.0 as i32, old.1 as i32) {
                win.rect = Rect::new(0, 0, w as i32, h as i32);
            }
            posts.push((win.owner_tid, win.hwnd));
        }
    }
    for (tid, hwnd) in posts {
        let q = s.queues.entry(tid).or_default();
        q.msgs.push_back(Msg { hwnd, message: WM_DISPLAYCHANGE, wparam: bpp, lparam: (h << 16) | (w & 0xFFFF), time: now, pt: (0, 0) });
    }
}

/// Post a message to a window's owner queue (for other HLE modules, e.g. MCI notifications). Returns false if the
/// window does not exist.
pub fn post_message(m: &mut Machine, hwnd: u32, msg: u32, wparam: u32, lparam: u32) -> bool {
    let now = m.now_ms() as u32;
    let pt = (m.input.mouse_x, m.input.mouse_y);
    let tid = m.current_tid();
    let st = us(m);
    let mut s = st.borrow_mut();
    let target = if hwnd == 0 {
        Some(tid)
    } else {
        s.windows.get(&hwnd).map(|w| w.owner_tid)
    };
    let Some(t) = target else { return false };
    s.queues.entry(t).or_default().msgs.push_back(Msg { hwnd, message: msg, wparam, lparam, time: now, pt });
    true
}

// ---------------------------------------------------------------------------------------------- registration

/// Declare all names and register implemented handlers.
pub fn register(reg: &mut Registry) {
    reg.declare_all(DLL, NAMES);
    let h: &[(&str, s3_runtime::Handler)] = &[
        ("RegisterClassA", register_class_a),
        ("RegisterClassExA", register_class_ex_a),
        ("UnregisterClassA", unregister_class_a),
        ("CreateWindowExA", create_window_ex_a),
        ("DestroyWindow", destroy_window),
        ("ShowWindow", show_window),
        ("UpdateWindow", update_window),
        ("DefWindowProcA", def_window_proc_a),
        ("CallWindowProcA", call_window_proc_a),
        ("GetWindowLongA", get_window_long_a),
        ("SetWindowLongA", set_window_long_a),
        ("GetClientRect", get_client_rect),
        ("GetWindowRect", get_window_rect),
        ("MoveWindow", move_window),
        ("SetWindowPos", set_window_pos),
        ("ClientToScreen", client_to_screen),
        ("ScreenToClient", screen_to_client),
        ("AdjustWindowRect", adjust_window_rect),
        ("AdjustWindowRectEx", adjust_window_rect_ex),
        ("GetWindowTextA", get_window_text_a),
        ("SetWindowTextA", set_window_text_a),
        ("FindWindowA", find_window_a),
        ("IsIconic", ret0_1),
        ("IsWindow", is_window),
        ("IsWindowVisible", is_window_visible),
        ("IsWindowEnabled", is_window),
        ("EnableWindow", ret0_2),
        ("GetParent", get_parent),
        ("GetDesktopWindow", get_desktop_window),
        ("GetClassNameA", get_class_name_a),
        ("EnumWindows", enum_windows),
        ("GetActiveWindow", get_active_window),
        ("GetLastActivePopup", get_last_active_popup),
        ("GetForegroundWindow", get_active_window),
        ("SetActiveWindow", set_active_window),
        ("SetForegroundWindow", set_foreground_window),
        ("BringWindowToTop", set_foreground_window),
        ("GetFocus", get_focus),
        ("SetFocus", set_focus),
        ("SetCapture", set_capture),
        ("ReleaseCapture", release_capture),
        ("GetCapture", get_capture),
        ("GetMessageA", get_message_a),
        ("PeekMessageA", peek_message_a),
        ("WaitMessage", wait_message),
        ("MsgWaitForMultipleObjects", msg_wait_for_multiple_objects),
        ("TranslateMessage", translate_message),
        ("DispatchMessageA", dispatch_message_a),
        ("PostMessageA", post_message_a),
        ("SendNotifyMessageA", post_message_a),
        ("PostThreadMessageA", post_thread_message_a),
        ("SendMessageA", send_message_a),
        ("SendMessageTimeoutA", send_message_timeout_a),
        ("PostQuitMessage", post_quit_message),
        ("GetMessagePos", get_message_pos),
        ("GetMessageTime", get_message_time),
        ("GetQueueStatus", get_queue_status),
        ("GetInputState", get_input_state),
        ("RegisterWindowMessageA", register_window_message_a),
        ("SetTimer", set_timer),
        ("KillTimer", kill_timer),
        ("BeginPaint", begin_paint),
        ("EndPaint", end_paint),
        ("InvalidateRect", invalidate_rect),
        ("ValidateRect", validate_rect),
        ("GetUpdateRect", get_update_rect),
        ("GetDC", get_dc),
        ("ReleaseDC", release_dc),
        ("FillRect", gdi32::fill_rect),
        ("DrawTextA", draw_text_a),
        ("GetSystemMetrics", get_system_metrics),
        ("SystemParametersInfoA", system_parameters_info_a),
        ("GetSysColor", get_sys_color),
        ("GetDoubleClickTime", get_double_click_time),
        ("LoadCursorA", load_cursor_a),
        ("LoadIconA", load_icon_a),
        ("LoadImageA", load_image_a),
        ("LoadBitmapA", load_icon_a),
        ("DestroyCursor", ret1_1),
        ("DestroyIcon", ret1_1),
        ("DestroyMenu", ret1_1),
        ("GetMenu", ret0_1),
        ("SetMenu", ret1_2),
        ("SetCursor", set_cursor),
        ("ShowCursor", show_cursor),
        ("GetCursorPos", get_cursor_pos),
        ("SetCursorPos", set_cursor_pos),
        ("ClipCursor", clip_cursor),
        ("GetAsyncKeyState", get_async_key_state),
        ("GetKeyState", get_key_state),
        ("GetKeyboardState", get_keyboard_state),
        ("MapVirtualKeyA", map_virtual_key_a),
        ("GetKeyboardLayout", get_keyboard_layout),
        ("MessageBoxA", message_box_a),
        ("MessageBeep", ret1_1),
        ("wsprintfA", wsprintf_a),
        ("wvsprintfA", wvsprintf_a),
        ("SetRect", set_rect),
        ("SetRectEmpty", set_rect_empty),
        ("CopyRect", copy_rect),
        ("IntersectRect", intersect_rect),
        ("UnionRect", union_rect),
        ("OffsetRect", offset_rect),
        ("InflateRect", inflate_rect),
        ("IsRectEmpty", is_rect_empty),
        ("PtInRect", pt_in_rect),
        ("EqualRect", equal_rect),
        ("CharUpperA", char_upper_a),
        ("CharLowerA", char_lower_a),
        ("WindowFromPoint", window_from_point),
        ("OpenClipboard", ret0_1),
        ("CloseClipboard", ret1_0),
        ("GetClipboardData", ret0_1),
    ];
    for (n, f) in h {
        reg.register(DLL, n, *f);
    }
}

fn ret0_1(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(0, 1)
}
fn ret0_2(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(0, 2)
}
fn ret1_0(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(1, 0)
}
fn ret1_1(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(1, 1)
}
fn ret1_2(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(1, 2)
}

fn ensure_service(m: &mut Machine) {
    let st = us(m);
    let add = {
        let mut s = st.borrow_mut();
        !std::mem::replace(&mut s.service_added, true)
    };
    if add {
        m.add_service("user32-input", pump_input);
    }
}

// ---------------------------------------------------------------------------------------------- classes/windows

fn class_name_arg(m: &Machine, p: u32) -> String {
    if p < 0x1_0000 {
        format!("#atom{p:x}")
    } else {
        m.mem.read_cstr(p)
    }
}

fn add_class(m: &mut Machine, mut c: WndClass) -> u32 {
    let st = us(m);
    let mut s = st.borrow_mut();
    let key = c.name.to_ascii_lowercase();
    if s.classes.contains_key(&key) {
        return 0; // ERROR_CLASS_ALREADY_EXISTS
    }
    c.atom = s.next_atom;
    s.next_atom += 1;
    let atom = c.atom;
    s.classes.insert(format!("#atom{atom:x}"), c.clone());
    s.classes.insert(key, c);
    atom
}

/// RegisterClassA(const WNDCLASSA*)
fn register_class_a(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    let c = WndClass {
        style: m.mem.read_u32(p),
        wndproc: m.mem.read_u32(p + 4),
        wnd_extra: m.mem.read_u32(p + 12),
        hinstance: m.mem.read_u32(p + 16),
        icon: m.mem.read_u32(p + 20),
        cursor: m.mem.read_u32(p + 24),
        background: m.mem.read_u32(p + 28),
        name: class_name_arg(m, m.mem.read_u32(p + 36)),
        atom: 0,
    };
    m.log(LogLevel::Debug, &format!("RegisterClassA {:?} wndproc {:#x}", c.name, c.wndproc));
    let a = add_class(m, c);
    if a == 0 {
        m.set_last_error(1410);
    }
    ThunkResult::stdcall(a, 1)
}

/// RegisterClassExA(const WNDCLASSEXA*)
fn register_class_ex_a(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    let c = WndClass {
        style: m.mem.read_u32(p + 4),
        wndproc: m.mem.read_u32(p + 8),
        wnd_extra: m.mem.read_u32(p + 16),
        hinstance: m.mem.read_u32(p + 20),
        icon: m.mem.read_u32(p + 24),
        cursor: m.mem.read_u32(p + 28),
        background: m.mem.read_u32(p + 32),
        name: class_name_arg(m, m.mem.read_u32(p + 40)),
        atom: 0,
    };
    m.log(LogLevel::Debug, &format!("RegisterClassExA {:?} wndproc {:#x}", c.name, c.wndproc));
    let a = add_class(m, c);
    if a == 0 {
        m.set_last_error(1410);
    }
    ThunkResult::stdcall(a, 1)
}

fn unregister_class_a(m: &mut Machine) -> ThunkResult {
    let name = class_name_arg(m, m.arg_u32(0)).to_ascii_lowercase();
    let st = us(m);
    let mut s = st.borrow_mut();
    let ok = if let Some(c) = s.classes.remove(&name) {
        s.classes.remove(&format!("#atom{:x}", c.atom));
        s.classes.remove(&c.name.to_ascii_lowercase());
        true
    } else {
        false
    };
    ThunkResult::stdcall(ok as u32, 2)
}

/// Call a window's procedure (or our DefWindowProc if it has none). Must not be called with the state borrowed.
pub fn send(m: &mut Machine, hwnd: u32, msg: u32, wparam: u32, lparam: u32) -> u32 {
    let proc_ = us(m).borrow_mut().windows.get(&hwnd).map(|w| w.wndproc);
    match proc_ {
        Some(p) if p != 0 => m.call_guest(p, &[hwnd, msg, wparam, lparam]),
        Some(_) => def_window_proc(m, hwnd, msg, wparam, lparam),
        None => 0,
    }
}

/// CreateWindowExA(exStyle, class, name, style, x, y, w, h, parent, menu, hinst, param)
fn create_window_ex_a(m: &mut Machine) -> ThunkResult {
    ensure_service(m);
    let ex_style = m.arg_u32(0);
    let class_p = m.arg_u32(1);
    let class = class_name_arg(m, class_p);
    let title = m.arg_str(2).unwrap_or_default();
    let style = m.arg_u32(3);
    let (mut x, mut y, mut w, mut h) = (m.arg_u32(4), m.arg_u32(5), m.arg_u32(6), m.arg_u32(7));
    let (parent, menu, hinst, param) = (m.arg_u32(8), m.arg_u32(9), m.arg_u32(10), m.arg_u32(11));
    let tid = m.current_tid();
    let (sw, sh) = screen_size(m);
    if x == CW_USEDEFAULT {
        x = 0;
        y = 0;
    }
    if w == CW_USEDEFAULT {
        w = sw;
        h = sh;
    }
    let st = us(m);
    let (hwnd, cls) = {
        let mut s = st.borrow_mut();
        let Some(cls) = s.classes.get(&class.to_ascii_lowercase()).cloned() else {
            drop(s);
            m.log(LogLevel::Warn, &format!("CreateWindowExA: unknown class {class:?}"));
            m.set_last_error(1407);
            return ThunkResult::stdcall(0, 12);
        };
        let hwnd = s.next_hwnd;
        s.next_hwnd += 0x10;
        let win = Window {
            hwnd,
            class: cls.name.clone(),
            class_style: cls.style,
            wndproc: cls.wndproc,
            style: style & !WS_VISIBLE,
            ex_style,
            title: title.clone(),
            rect: Rect::new(x as i32, y as i32, (x as i32).wrapping_add(w as i32), (y as i32).wrapping_add(h as i32)),
            parent,
            id: if style & WS_CHILD != 0 { menu } else { 0 },
            hinstance: hinst,
            user_data: 0,
            extra: vec![0; cls.wnd_extra.min(4096) as usize],
            owner_tid: tid,
            visible: false,
            needs_paint: true,
        };
        s.windows.insert(hwnd, win);
        s.queues.entry(tid).or_default();
        (hwnd, cls)
    };
    m.log(
        LogLevel::Info,
        &format!("CreateWindowExA class {:?} title {title:?} style {style:#x} ex {ex_style:#x} {x},{y} {w}x{h} -> hwnd {hwnd:#x} (wndproc {:#x}, tid {tid:#x})", cls.name, cls.wndproc),
    );
    // CREATESTRUCTA + MINMAXINFO + NCCALCSIZE rect in guest memory.
    let cs = m.mem.alloc(48 + 40 + 16);
    let mmi = cs + 48;
    let rc = mmi + 40;
    let name_p = m.arg_u32(2);
    for (i, v) in [param, hinst, menu, parent, h, w, y, x, style, name_p, class_p, ex_style].iter().enumerate() {
        m.mem.write_u32(cs + 4 * i as u32, *v);
    }
    for (i, v) in [0, 0, sw, sh, 0, 0, 1, 1, sw + 8, sh + 8].iter().enumerate() {
        m.mem.write_u32(mmi + 4 * i as u32, *v);
    }
    let mut ok = true;
    if style & WS_CHILD == 0 {
        send(m, hwnd, WM_GETMINMAXINFO, 0, mmi);
    }
    if send(m, hwnd, WM_NCCREATE, 0, cs) == 0 {
        m.log(LogLevel::Warn, "CreateWindowExA: WM_NCCREATE returned FALSE");
        ok = false;
    }
    if ok {
        Rect::new(x as i32, y as i32, x as i32 + w as i32, y as i32 + h as i32).write(m, rc);
        send(m, hwnd, WM_NCCALCSIZE, 0, rc);
        if send(m, hwnd, WM_CREATE, 0, cs) as i32 == -1 {
            m.log(LogLevel::Warn, "CreateWindowExA: WM_CREATE returned -1");
            ok = false;
        }
    }
    m.mem.free(cs);
    if !ok || m.is_stopped() {
        destroy_window_internal(m, hwnd);
        return ThunkResult::stdcall(0, 12);
    }
    send(m, hwnd, WM_SIZE, 0, (h << 16) | (w & 0xFFFF));
    send(m, hwnd, WM_MOVE, 0, (y << 16) | (x & 0xFFFF));
    if style & WS_VISIBLE != 0 {
        show_internal(m, hwnd, true);
    }
    ThunkResult::stdcall(hwnd, 12)
}

/// Show/hide; on the first visible top-level window run the activation sequence.
fn show_internal(m: &mut Machine, hwnd: u32, show: bool) -> bool {
    let st = us(m);
    let (was, toplevel, first_activation) = {
        let mut s = st.borrow_mut();
        let Some(w) = s.windows.get_mut(&hwnd) else { return false };
        let was = w.visible;
        w.visible = show;
        if show {
            w.style |= WS_VISIBLE;
            w.needs_paint = true;
        } else {
            w.style &= !WS_VISIBLE;
        }
        let toplevel = w.style & WS_CHILD == 0;
        let first = show && toplevel && !s.activated;
        if first {
            s.activated = true;
            s.active = hwnd;
            s.focus = hwnd;
        }
        (was, toplevel, first)
    };
    if was != show {
        send(m, hwnd, WM_SHOWWINDOW, show as u32, 0);
    }
    if first_activation && toplevel {
        let tid = m.current_tid();
        m.log(LogLevel::Info, &format!("activating window {hwnd:#x} (WM_ACTIVATEAPP/WM_ACTIVATE/WM_SETFOCUS)"));
        send(m, hwnd, WM_ACTIVATEAPP, 1, tid);
        send(m, hwnd, WM_NCACTIVATE, 1, 0);
        send(m, hwnd, WM_ACTIVATE, 1, 0);
        send(m, hwnd, WM_SETFOCUS, 0, 0);
    }
    was
}

fn destroy_window_internal(m: &mut Machine, hwnd: u32) -> bool {
    if !us(m).borrow_mut().windows.contains_key(&hwnd) {
        return false;
    }
    send(m, hwnd, WM_DESTROY, 0, 0);
    send(m, hwnd, WM_NCDESTROY, 0, 0);
    let st = us(m);
    let mut g = st.borrow_mut();
    let s = &mut *g;
    s.windows.remove(&hwnd);
    s.timers.retain(|t| t.hwnd != hwnd);
    for v in [&mut s.active, &mut s.focus, &mut s.capture] {
        if *v == hwnd {
            *v = 0;
        }
    }
    true
}

fn destroy_window(m: &mut Machine) -> ThunkResult {
    let hwnd = m.arg_u32(0);
    let ok = destroy_window_internal(m, hwnd);
    ThunkResult::stdcall(ok as u32, 1)
}

fn show_window(m: &mut Machine) -> ThunkResult {
    let (hwnd, cmd) = (m.arg_u32(0), m.arg_u32(1));
    let was = show_internal(m, hwnd, cmd != 0);
    ThunkResult::stdcall(was as u32, 2)
}

fn update_window(m: &mut Machine) -> ThunkResult {
    let hwnd = m.arg_u32(0);
    let paint = {
        let st = us(m);
        let mut s = st.borrow_mut();
        match s.windows.get_mut(&hwnd) {
            Some(w) if w.visible && w.needs_paint => {
                w.needs_paint = false;
                true
            }
            Some(_) => false,
            None => return ThunkResult::stdcall(0, 1),
        }
    };
    if paint {
        send(m, hwnd, WM_PAINT, 0, 0);
    }
    ThunkResult::stdcall(1, 1)
}

/// Our DefWindowProc.
pub fn def_window_proc(m: &mut Machine, hwnd: u32, msg: u32, wparam: u32, lparam: u32) -> u32 {
    match msg {
        WM_NCCREATE => 1,
        WM_NCCALCSIZE => 0,
        WM_NCACTIVATE => 1,
        WM_ERASEBKGND => 1,
        WM_NCHITTEST => 1, // HTCLIENT
        WM_MOUSEACTIVATE => 1,
        WM_SETCURSOR => 0,
        WM_PAINT => {
            if let Some(w) = us(m).borrow_mut().windows.get_mut(&hwnd) {
                w.needs_paint = false;
            }
            0
        }
        WM_CLOSE => {
            destroy_window_internal(m, hwnd);
            0
        }
        WM_SYSKEYDOWN if wparam == 0x73 && lparam & (1 << 29) != 0 => {
            // Alt+F4
            post_message(m, hwnd, WM_CLOSE, 0, 0);
            0
        }
        WM_SYSCOMMAND => 0,
        _ => 0,
    }
}

fn def_window_proc_a(m: &mut Machine) -> ThunkResult {
    let (h, msg, wp, lp) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_u32(3));
    let r = def_window_proc(m, h, msg, wp, lp);
    ThunkResult::stdcall(r, 4)
}

fn call_window_proc_a(m: &mut Machine) -> ThunkResult {
    let (p, h, msg, wp, lp) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_u32(3), m.arg_u32(4));
    let r = if p == 0 { def_window_proc(m, h, msg, wp, lp) } else { m.call_guest(p, &[h, msg, wp, lp]) };
    ThunkResult::stdcall(r, 5)
}

const GWL_WNDPROC: i32 = -4;
const GWL_HINSTANCE: i32 = -6;
const GWL_HWNDPARENT: i32 = -8;
const GWL_ID: i32 = -12;
const GWL_STYLE: i32 = -16;
const GWL_EXSTYLE: i32 = -20;
const GWL_USERDATA: i32 = -21;

fn window_long(w: &mut Window, idx: i32, set: Option<u32>) -> Option<u32> {
    let field: &mut u32 = match idx {
        GWL_WNDPROC => &mut w.wndproc,
        GWL_HINSTANCE => &mut w.hinstance,
        GWL_HWNDPARENT => &mut w.parent,
        GWL_ID => &mut w.id,
        GWL_STYLE => &mut w.style,
        GWL_EXSTYLE => &mut w.ex_style,
        GWL_USERDATA => &mut w.user_data,
        i if i >= 0 && (i as usize + 4) <= w.extra.len() => {
            let i = i as usize;
            let old = u32::from_le_bytes(w.extra[i..i + 4].try_into().unwrap());
            if let Some(v) = set {
                w.extra[i..i + 4].copy_from_slice(&v.to_le_bytes());
            }
            return Some(old);
        }
        _ => return None,
    };
    let old = *field;
    if let Some(v) = set {
        *field = v;
    }
    Some(old)
}

fn get_window_long_a(m: &mut Machine) -> ThunkResult {
    let (hwnd, idx) = (m.arg_u32(0), m.arg_i32(1));
    let st = us(m);
    let r = st.borrow_mut().windows.get_mut(&hwnd).and_then(|w| window_long(w, idx, None));
    if r.is_none() {
        m.set_last_error(1413);
    }
    ThunkResult::stdcall(r.unwrap_or(0), 2)
}

fn set_window_long_a(m: &mut Machine) -> ThunkResult {
    let (hwnd, idx, v) = (m.arg_u32(0), m.arg_i32(1), m.arg_u32(2));
    let st = us(m);
    let r = st.borrow_mut().windows.get_mut(&hwnd).and_then(|w| window_long(w, idx, Some(v)));
    if r.is_none() {
        m.set_last_error(1413);
    }
    ThunkResult::stdcall(r.unwrap_or(0), 3)
}

fn win_rect(m: &mut Machine, hwnd: u32) -> Option<Rect> {
    if hwnd == 0 || hwnd == DESKTOP_HWND {
        let (w, h) = screen_size(m);
        return Some(Rect::new(0, 0, w as i32, h as i32));
    }
    us(m).borrow_mut().windows.get(&hwnd).map(|w| w.rect)
}

const DESKTOP_HWND: u32 = 0x0001_0008;

fn get_client_rect(m: &mut Machine) -> ThunkResult {
    let (hwnd, p) = (m.arg_u32(0), m.arg_u32(1));
    let Some(r) = win_rect(m, hwnd) else { return ThunkResult::stdcall(0, 2) };
    Rect::new(0, 0, r.r - r.l, r.b - r.t).write(m, p);
    ThunkResult::stdcall(1, 2)
}

fn get_window_rect(m: &mut Machine) -> ThunkResult {
    let (hwnd, p) = (m.arg_u32(0), m.arg_u32(1));
    let Some(r) = win_rect(m, hwnd) else { return ThunkResult::stdcall(0, 2) };
    r.write(m, p);
    ThunkResult::stdcall(1, 2)
}

fn reposition(m: &mut Machine, hwnd: u32, pos: Option<(i32, i32)>, size: Option<(i32, i32)>) -> bool {
    let (moved, sized, r) = {
        let st = us(m);
        let mut s = st.borrow_mut();
        let Some(w) = s.windows.get_mut(&hwnd) else { return false };
        let old = w.rect;
        let (x, y) = pos.unwrap_or((old.l, old.t));
        let (cw, ch) = size.unwrap_or((old.r - old.l, old.b - old.t));
        w.rect = Rect::new(x, y, x + cw, y + ch);
        (pos.is_some() && (x, y) != (old.l, old.t), size.is_some() && (cw, ch) != (old.r - old.l, old.b - old.t), w.rect)
    };
    if sized {
        send(m, hwnd, WM_SIZE, 0, (((r.b - r.t) as u32) << 16) | ((r.r - r.l) as u32 & 0xFFFF));
    }
    if moved {
        send(m, hwnd, WM_MOVE, 0, ((r.t as u32) << 16) | (r.l as u32 & 0xFFFF));
    }
    true
}

/// MoveWindow(hwnd, x, y, w, h, repaint)
fn move_window(m: &mut Machine) -> ThunkResult {
    let hwnd = m.arg_u32(0);
    let (x, y, w, h) = (m.arg_i32(1), m.arg_i32(2), m.arg_i32(3), m.arg_i32(4));
    let ok = reposition(m, hwnd, Some((x, y)), Some((w, h)));
    ThunkResult::stdcall(ok as u32, 6)
}

/// SetWindowPos(hwnd, after, x, y, cx, cy, flags)
fn set_window_pos(m: &mut Machine) -> ThunkResult {
    let hwnd = m.arg_u32(0);
    let (x, y, w, h, flags) = (m.arg_i32(2), m.arg_i32(3), m.arg_i32(4), m.arg_i32(5), m.arg_u32(6));
    let pos = if flags & 2 == 0 { Some((x, y)) } else { None };
    let size = if flags & 1 == 0 { Some((w, h)) } else { None };
    let ok = reposition(m, hwnd, pos, size);
    if flags & 0x40 != 0 {
        show_internal(m, hwnd, true);
    } else if flags & 0x80 != 0 {
        show_internal(m, hwnd, false);
    }
    ThunkResult::stdcall(ok as u32, 7)
}

fn client_to_screen(m: &mut Machine) -> ThunkResult {
    let (hwnd, p) = (m.arg_u32(0), m.arg_u32(1));
    let Some(r) = win_rect(m, hwnd) else { return ThunkResult::stdcall(0, 2) };
    let (x, y) = (m.mem.read_u32(p) as i32, m.mem.read_u32(p + 4) as i32);
    m.mem.write_u32(p, (x + r.l) as u32);
    m.mem.write_u32(p + 4, (y + r.t) as u32);
    ThunkResult::stdcall(1, 2)
}

fn screen_to_client(m: &mut Machine) -> ThunkResult {
    let (hwnd, p) = (m.arg_u32(0), m.arg_u32(1));
    let Some(r) = win_rect(m, hwnd) else { return ThunkResult::stdcall(0, 2) };
    let (x, y) = (m.mem.read_u32(p) as i32, m.mem.read_u32(p + 4) as i32);
    m.mem.write_u32(p, (x - r.l) as u32);
    m.mem.write_u32(p + 4, (y - r.t) as u32);
    ThunkResult::stdcall(1, 2)
}

fn adjust_window_rect(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(1, 3)
}

fn adjust_window_rect_ex(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(1, 4)
}

fn get_window_text_a(m: &mut Machine) -> ThunkResult {
    let (hwnd, buf, n) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    let t = us(m).borrow_mut().windows.get(&hwnd).map(|w| w.title.clone()).unwrap_or_default();
    if buf == 0 || n == 0 {
        return ThunkResult::stdcall(0, 3);
    }
    let b: Vec<u8> = t.bytes().take(n as usize - 1).collect();
    m.mem.write_cstr_bytes(buf, &b);
    ThunkResult::stdcall(b.len() as u32, 3)
}

fn set_window_text_a(m: &mut Machine) -> ThunkResult {
    let (hwnd, t) = (m.arg_u32(0), m.arg_str(1).unwrap_or_default());
    let ok = us(m).borrow_mut().windows.get_mut(&hwnd).map(|w| w.title = t).is_some();
    ThunkResult::stdcall(ok as u32, 2)
}

fn find_window_a(m: &mut Machine) -> ThunkResult {
    let cp = m.arg_u32(0);
    let class = if cp != 0 { Some(class_name_arg(m, cp)) } else { None };
    let title = m.arg_str(1);
    let st = us(m);
    let s = st.borrow_mut();
    let cls_name = class.map(|c| s.classes.get(&c.to_ascii_lowercase()).map(|k| k.name.clone()).unwrap_or(c));
    let h = s
        .windows
        .values()
        .find(|w| {
            cls_name.as_ref().map_or(true, |c| w.class.eq_ignore_ascii_case(c)) && title.as_ref().map_or(true, |t| &w.title == t)
        })
        .map(|w| w.hwnd)
        .unwrap_or(0);
    ThunkResult::stdcall(h, 2)
}

fn is_window(m: &mut Machine) -> ThunkResult {
    let h = m.arg_u32(0);
    let ok = us(m).borrow_mut().windows.contains_key(&h);
    ThunkResult::stdcall(ok as u32, 1)
}

fn is_window_visible(m: &mut Machine) -> ThunkResult {
    let h = m.arg_u32(0);
    let ok = us(m).borrow_mut().windows.get(&h).map(|w| w.visible).unwrap_or(false);
    ThunkResult::stdcall(ok as u32, 1)
}

fn get_parent(m: &mut Machine) -> ThunkResult {
    let h = m.arg_u32(0);
    let p = us(m).borrow_mut().windows.get(&h).map(|w| w.parent).unwrap_or(0);
    ThunkResult::stdcall(p, 1)
}

fn get_desktop_window(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(DESKTOP_HWND, 0)
}

fn get_class_name_a(m: &mut Machine) -> ThunkResult {
    let (h, buf, n) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    let c = us(m).borrow_mut().windows.get(&h).map(|w| w.class.clone());
    let Some(c) = c else { return ThunkResult::stdcall(0, 3) };
    if buf == 0 || n == 0 {
        return ThunkResult::stdcall(0, 3);
    }
    let b: Vec<u8> = c.bytes().take(n as usize - 1).collect();
    m.mem.write_cstr_bytes(buf, &b);
    ThunkResult::stdcall(b.len() as u32, 3)
}

fn enum_windows(m: &mut Machine) -> ThunkResult {
    let (proc_, lp) = (m.arg_u32(0), m.arg_u32(1));
    let list: Vec<u32> = us(m).borrow_mut().windows.values().filter(|w| w.style & WS_CHILD == 0).map(|w| w.hwnd).collect();
    for h in list {
        if m.call_guest(proc_, &[h, lp]) == 0 || m.is_stopped() {
            break;
        }
    }
    ThunkResult::stdcall(1, 2)
}

fn get_active_window(m: &mut Machine) -> ThunkResult {
    let a = us(m).borrow_mut().active;
    ThunkResult::stdcall(a, 0)
}

/// `GetLastActivePopup(hwnd)`: we have no owned popups, so the window itself (the CRT's MessageBox helper looks
/// this up via GetProcAddress).
fn get_last_active_popup(m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(m.arg_u32(0), 1)
}

fn set_active_window(m: &mut Machine) -> ThunkResult {
    let h = m.arg_u32(0);
    let prev = std::mem::replace(&mut us(m).borrow_mut().active, h);
    ThunkResult::stdcall(prev, 1)
}

fn set_foreground_window(m: &mut Machine) -> ThunkResult {
    let h = m.arg_u32(0);
    let st = us(m);
    let mut s = st.borrow_mut();
    let ok = s.windows.contains_key(&h);
    if ok {
        s.active = h;
    }
    ThunkResult::stdcall(ok as u32, 1)
}

fn get_focus(m: &mut Machine) -> ThunkResult {
    let f = us(m).borrow_mut().focus;
    ThunkResult::stdcall(f, 0)
}

fn set_focus(m: &mut Machine) -> ThunkResult {
    let h = m.arg_u32(0);
    let prev = std::mem::replace(&mut us(m).borrow_mut().focus, h);
    if prev != h {
        if prev != 0 {
            send(m, prev, WM_KILLFOCUS, h, 0);
        }
        if h != 0 {
            send(m, h, WM_SETFOCUS, prev, 0);
        }
    }
    ThunkResult::stdcall(prev, 1)
}

fn set_capture(m: &mut Machine) -> ThunkResult {
    let h = m.arg_u32(0);
    let prev = std::mem::replace(&mut us(m).borrow_mut().capture, h);
    ThunkResult::stdcall(prev, 1)
}

fn release_capture(m: &mut Machine) -> ThunkResult {
    us(m).borrow_mut().capture = 0;
    ThunkResult::stdcall(1, 0)
}

fn get_capture(m: &mut Machine) -> ThunkResult {
    let c = us(m).borrow_mut().capture;
    ThunkResult::stdcall(c, 0)
}

// ---------------------------------------------------------------------------------------------- input

/// Scan code for a VK (set 1), for lParam bits 16-23.
fn vk_scan(vk: u32) -> u32 {
    const TABLE: &[(u32, u32)] = &[
        (0x08, 0x0E), (0x09, 0x0F), (0x0D, 0x1C), (0x10, 0x2A), (0x11, 0x1D), (0x12, 0x38), (0x13, 0x45), (0x14, 0x3A),
        (0x1B, 0x01), (0x20, 0x39), (0x21, 0x49), (0x22, 0x51), (0x23, 0x4F), (0x24, 0x47), (0x25, 0x4B), (0x26, 0x48),
        (0x27, 0x4D), (0x28, 0x50), (0x2D, 0x52), (0x2E, 0x53), (0x30, 0x0B), (0x31, 0x02), (0x32, 0x03), (0x33, 0x04),
        (0x34, 0x05), (0x35, 0x06), (0x36, 0x07), (0x37, 0x08), (0x38, 0x09), (0x39, 0x0A), (0x41, 0x1E), (0x42, 0x30),
        (0x43, 0x2E), (0x44, 0x20), (0x45, 0x12), (0x46, 0x21), (0x47, 0x22), (0x48, 0x23), (0x49, 0x17), (0x4A, 0x24),
        (0x4B, 0x25), (0x4C, 0x26), (0x4D, 0x32), (0x4E, 0x31), (0x4F, 0x18), (0x50, 0x19), (0x51, 0x10), (0x52, 0x13),
        (0x53, 0x1F), (0x54, 0x14), (0x55, 0x16), (0x56, 0x2F), (0x57, 0x11), (0x58, 0x2D), (0x59, 0x15), (0x5A, 0x2C),
        (0x6A, 0x37), (0x6B, 0x4E), (0x6D, 0x4A), (0x70, 0x3B), (0x71, 0x3C), (0x72, 0x3D), (0x73, 0x3E), (0x74, 0x3F),
        (0x75, 0x40), (0x76, 0x41), (0x77, 0x42), (0x78, 0x43), (0x79, 0x44), (0x7A, 0x57), (0x7B, 0x58), (0xBA, 0x27),
        (0xBB, 0x0D), (0xBC, 0x33), (0xBD, 0x0C), (0xBE, 0x34), (0xBF, 0x35), (0xC0, 0x29), (0xDB, 0x1A), (0xDC, 0x2B),
        (0xDD, 0x1B), (0xDE, 0x28),
    ];
    TABLE.iter().find(|(v, _)| *v == vk).map(|(_, s)| *s).unwrap_or(0)
}

fn is_extended(vk: u32) -> bool {
    matches!(vk, 0x21..=0x28 | 0x2D | 0x2E | 0x5B | 0x5C | 0x6F | 0x90)
}

/// Character produced by a key press (US layout), for TranslateMessage when the host sends no WM_CHAR itself.
pub fn vk_to_char(vk: u32, shift: bool, caps: bool) -> Option<u32> {
    Some(match vk {
        0x08 | 0x09 | 0x0D | 0x1B | 0x20 => vk,
        0x30..=0x39 => {
            if shift {
                b")!@#$%^&*("[(vk - 0x30) as usize] as u32
            } else {
                vk
            }
        }
        0x41..=0x5A => {
            if shift ^ caps {
                vk
            } else {
                vk + 0x20
            }
        }
        0x60..=0x69 => vk - 0x30,
        0xBA => (if shift { b':' } else { b';' }) as u32,
        0xBB => (if shift { b'+' } else { b'=' }) as u32,
        0xBC => (if shift { b'<' } else { b',' }) as u32,
        0xBD => (if shift { b'_' } else { b'-' }) as u32,
        0xBE => (if shift { b'>' } else { b'.' }) as u32,
        0xBF => (if shift { b'?' } else { b'/' }) as u32,
        _ => return None,
    })
}

/// Convert new host input events into window messages (scheduler service, also called by the message functions).
pub fn pump_input(m: &mut Machine) {
    let st = us(m);
    let mut cursor = {
        let mut s = st.borrow_mut();
        // No window yet: nobody to receive input; keep events for later.
        let target = if s.capture != 0 { s.capture } else { s.active };
        if target == 0 || !s.windows.contains_key(&target) {
            return;
        }
        match s.input_cursor {
            Some(c) => c,
            None => {
                // Input from before the first window existed is dropped.
                let c = m.input.cursor_now();
                s.input_cursor = Some(c);
                c
            }
        }
    };
    if cursor == m.input.cursor_now() {
        return; // nothing new (cheap path: PeekMessage is called in hot loops)
    }
    let events = m.input.events_since(&mut cursor);
    let now_ms = m.now_ms();
    let now = now_ms as u32;
    let mut s = st.borrow_mut();
    s.input_cursor = Some(cursor);
    if events.is_empty() {
        return;
    }
    let target = if s.capture != 0 { s.capture } else { s.active };
    let Some(win) = s.windows.get(&target).cloned() else { return };
    let (mx, my) = (m.input.mouse_x, m.input.mouse_y);
    let mut mk = s.mk_state;
    let mut alt_down = s.alt_state;
    let mut pos = (mx, my);
    let mut out: Vec<Msg> = Vec::new();
    let clip = s.clip;
    let clampf = |x: i32, y: i32| match clip {
        Some(c) => (x.clamp(c.l, (c.r - 1).max(c.l)), y.clamp(c.t, (c.b - 1).max(c.t))),
        None => (x, y),
    };
    // Recompute modifier state progressively from the events (they are already applied to m.input).
    for e in &events {
        match *e {
            s3_runtime::HostEvent::MouseMove { x, y } => {
                pos = clampf(x, y);
                let lp = (((pos.1 - win.rect.t) as u32 & 0xFFFF) << 16) | ((pos.0 - win.rect.l) as u32 & 0xFFFF);
                if let Some(last) = out.last_mut().filter(|l| l.message == WM_MOUSEMOVE) {
                    last.lparam = lp;
                    last.pt = pos;
                    last.wparam = mk;
                } else {
                    out.push(Msg { hwnd: target, message: WM_MOUSEMOVE, wparam: mk, lparam: lp, time: now, pt: pos });
                }
            }
            s3_runtime::HostEvent::MouseButton { button, down } => {
                let (bit, base) = match button {
                    MouseButton::Left => (1, WM_LBUTTONDOWN),
                    MouseButton::Right => (2, WM_RBUTTONDOWN),
                    MouseButton::Middle => (0x10, WM_MBUTTONDOWN),
                };
                if down {
                    mk |= bit;
                } else {
                    mk &= !bit;
                }
                let mut msg = if down { base } else { base + 1 };
                if down && win.class_style & CS_DBLCLKS != 0 {
                    if let Some((b, t, x, y)) = s.last_click {
                        if b == base && now_ms.saturating_sub(t) <= 500 && (x - pos.0).abs() <= 4 && (y - pos.1).abs() <= 4 {
                            msg = base + 2;
                        }
                    }
                    s.last_click = if msg == base + 2 { None } else { Some((base, now_ms, pos.0, pos.1)) };
                }
                let lp = (((pos.1 - win.rect.t) as u32 & 0xFFFF) << 16) | ((pos.0 - win.rect.l) as u32 & 0xFFFF);
                out.push(Msg { hwnd: target, message: msg, wparam: mk, lparam: lp, time: now, pt: pos });
            }
            s3_runtime::HostEvent::Key { vk, down } => {
                let vk = vk & 0xFF;
                if vk == 0x10 {
                    mk = if down { mk | 4 } else { mk & !4 };
                }
                if vk == 0x11 {
                    mk = if down { mk | 8 } else { mk & !8 };
                }
                if vk == 0x12 {
                    alt_down = down;
                }
                if down && matches!(vk, 0x14 | 0x90 | 0x91) {
                    s.toggles[vk as usize] = !s.toggles[vk as usize];
                }
                let sys = alt_down || vk == 0x12 || vk == 0x79;
                let mut lp = 1 | (vk_scan(vk) << 16) | ((is_extended(vk) as u32) << 24);
                if alt_down {
                    lp |= 1 << 29;
                }
                if !down {
                    lp |= 0xC000_0000;
                }
                let msg = match (down, sys) {
                    (true, false) => WM_KEYDOWN,
                    (false, false) => WM_KEYUP,
                    (true, true) => WM_SYSKEYDOWN,
                    (false, true) => WM_SYSKEYUP,
                };
                out.push(Msg { hwnd: s.focus.max(target), message: msg, wparam: vk, lparam: lp, time: now, pt: pos });
            }
            s3_runtime::HostEvent::Char(c) => {
                s.host_chars = true;
                let ch = if c < 0x100 { c } else { b'?' as u32 };
                out.push(Msg { hwnd: s.focus.max(target), message: WM_CHAR, wparam: ch, lparam: 1, time: now, pt: pos });
            }
            _ => {}
        }
    }
    s.mk_state = mk;
    s.alt_state = alt_down;
    let q = s.queues.entry(win.owner_tid).or_default();
    for msg in out {
        if msg.message == WM_MOUSEMOVE {
            if let Some(last) = q.msgs.back_mut().filter(|l| l.message == WM_MOUSEMOVE && l.hwnd == msg.hwnd) {
                *last = msg;
                continue;
            }
        }
        q.msgs.push_back(msg);
    }
}

// ---------------------------------------------------------------------------------------------- message retrieval

fn filter_match(msg: &Msg, hwnd: u32, min: u32, max: u32) -> bool {
    let hwnd_ok = match hwnd {
        0 => true,
        0xFFFF_FFFF => msg.hwnd == 0,
        h => msg.hwnd == h,
    };
    let range_ok = (min == 0 && max == 0) || (msg.message >= min && msg.message <= max);
    hwnd_ok && range_ok
}

/// Find (and optionally remove) the next message for thread `tid`. Order: posted, WM_QUIT, WM_PAINT, WM_TIMER.
fn next_message(m: &mut Machine, tid: u32, hwnd: u32, min: u32, max: u32, remove: bool) -> Option<Msg> {
    pump_input(m);
    let now_ms = m.now_ms();
    let st = us(m);
    let mut s = st.borrow_mut();
    let q = s.queues.entry(tid).or_default();
    if let Some(i) = q.msgs.iter().position(|msg| filter_match(msg, hwnd, min, max)) {
        let msg = if remove { q.msgs.remove(i).unwrap() } else { q.msgs[i] };
        q.last_pos = msg.pt;
        q.last_time = msg.time;
        return Some(msg);
    }
    if let Some(code) = q.quit {
        if remove {
            q.quit = None;
        }
        return Some(Msg { hwnd: 0, message: WM_QUIT, wparam: code, lparam: 0, time: now_ms as u32, pt: q.last_pos });
    }
    let pt = q.last_pos;
    let range = |msgid: u32| (min == 0 && max == 0) || (msgid >= min && msgid <= max);
    if range(WM_PAINT) {
        if let Some(w) = s.windows.values_mut().find(|w| w.owner_tid == tid && w.visible && w.needs_paint && (hwnd == 0 || w.hwnd == hwnd)) {
            if remove {
                // Cleared here as well as in BeginPaint/DefWindowProc so a WndProc that ignores WM_PAINT cannot
                // cause an endless WM_PAINT stream.
                w.needs_paint = false;
            }
            return Some(Msg { hwnd: w.hwnd, message: WM_PAINT, wparam: 0, lparam: 0, time: now_ms as u32, pt });
        }
    }
    if range(WM_TIMER) {
        if let Some(t) = s.timers.iter_mut().filter(|t| t.tid == tid && (hwnd == 0 || t.hwnd == hwnd)).find(|t| t.next_due <= now_ms) {
            if remove {
                t.next_due = now_ms + t.period.max(1) as u64;
            }
            return Some(Msg { hwnd: t.hwnd, message: WM_TIMER, wparam: t.id, lparam: t.proc_, time: now_ms as u32, pt });
        }
    }
    None
}

/// Does the thread have anything retrievable? (for MsgWait/WaitMessage)
fn has_message(m: &mut Machine, tid: u32) -> bool {
    next_message(m, tid, 0, 0, 0, false).is_some()
}

/// GetMessageA(lpMsg, hwnd, min, max)
fn get_message_a(m: &mut Machine) -> ThunkResult {
    ensure_service(m);
    let (p, hwnd, min, max) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_u32(3));
    let tid = m.current_tid();
    match next_message(m, tid, hwnd, min, max, true) {
        Some(msg) => {
            msg.write(m, p);
            ThunkResult::stdcall((msg.message != WM_QUIT) as u32, 4)
        }
        None => {
            if m.is_stopped() || m.input.quit_requested {
                // Host quit: give the game a WM_QUIT so it can shut down.
                let msg = Msg { hwnd: 0, message: WM_QUIT, wparam: 0, lparam: 0, time: m.now_ms() as u32, pt: (0, 0) };
                msg.write(m, p);
                return ThunkResult::stdcall(0, 4);
            }
            ThunkResult::Block
        }
    }
}

/// PeekMessageA(lpMsg, hwnd, min, max, flags)
fn peek_message_a(m: &mut Machine) -> ThunkResult {
    ensure_service(m);
    let (p, hwnd, min, max, flags) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_u32(3), m.arg_u32(4));
    let tid = m.current_tid();
    match next_message(m, tid, hwnd, min, max, flags & 1 != 0) {
        Some(msg) => {
            msg.write(m, p);
            ThunkResult::stdcall(1, 5)
        }
        None => ThunkResult::stdcall(0, 5),
    }
}

fn wait_message(m: &mut Machine) -> ThunkResult {
    let tid = m.current_tid();
    if has_message(m, tid) || m.is_stopped() {
        ThunkResult::stdcall(1, 0)
    } else {
        ThunkResult::Block
    }
}

/// MsgWaitForMultipleObjects(nCount, pHandles, fWaitAll, dwMilliseconds, dwWakeMask)
fn msg_wait_for_multiple_objects(m: &mut Machine) -> ThunkResult {
    ensure_service(m);
    let (n, ph, all, ms, mask) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_u32(3), m.arg_u32(4));
    let handles: Vec<u32> = (0..n.min(64)).map(|i| m.mem.read_u32(ph + 4 * i)).collect();
    if all != 0 {
        if !handles.is_empty() && handles.iter().all(|&h| m.is_signaled(h) == Some(true)) {
            for &h in &handles {
                m.try_acquire(h);
            }
            return ThunkResult::stdcall(0, 5);
        }
    } else {
        for (i, &h) in handles.iter().enumerate() {
            if m.try_acquire(h) {
                return ThunkResult::stdcall(i as u32, 5);
            }
        }
    }
    let tid = m.current_tid();
    if mask != 0 && has_message(m, tid) {
        return ThunkResult::stdcall(n, 5);
    }
    let waited = m.thread().block_since.map(|t| m.now_ms().saturating_sub(t)).unwrap_or(0);
    if ms == 0 || (ms != 0xFFFF_FFFF && waited >= ms as u64) || m.is_stopped() {
        return ThunkResult::stdcall(0x102, 5); // WAIT_TIMEOUT
    }
    ThunkResult::Block
}

/// TranslateMessage(const MSG*): synthesizes WM_CHAR from WM_KEYDOWN unless the host sends chars itself.
fn translate_message(m: &mut Machine) -> ThunkResult {
    let msg = Msg::read(m, m.arg_u32(0));
    if msg.message != WM_KEYDOWN && msg.message != WM_SYSKEYDOWN {
        return ThunkResult::stdcall(0, 1);
    }
    let (shift, ctrl) = (m.input.keys[0x10], m.input.keys[0x11]);
    let tid = m.current_tid();
    let st = us(m);
    let mut s = st.borrow_mut();
    if s.host_chars {
        return ThunkResult::stdcall(0, 1);
    }
    let caps = s.toggles[0x14];
    let Some(mut c) = vk_to_char(msg.wparam, shift, caps) else { return ThunkResult::stdcall(0, 1) };
    if ctrl && (0x41..=0x5A).contains(&msg.wparam) {
        c = msg.wparam - 0x40;
    }
    let cm = if msg.message == WM_SYSKEYDOWN { WM_SYSCHAR } else { WM_CHAR };
    s.queues.entry(tid).or_default().msgs.push_front(Msg { message: cm, wparam: c, ..msg });
    ThunkResult::stdcall(1, 1)
}

/// DispatchMessageA(const MSG*)
fn dispatch_message_a(m: &mut Machine) -> ThunkResult {
    let msg = Msg::read(m, m.arg_u32(0));
    if msg.message == WM_TIMER && msg.lparam != 0 {
        let r = m.call_guest(msg.lparam, &[msg.hwnd, WM_TIMER, msg.wparam, m.now_ms() as u32]);
        return ThunkResult::stdcall(r, 1);
    }
    if msg.hwnd == 0 {
        return ThunkResult::stdcall(0, 1);
    }
    let r = send(m, msg.hwnd, msg.message, msg.wparam, msg.lparam);
    ThunkResult::stdcall(r, 1)
}

fn post_message_a(m: &mut Machine) -> ThunkResult {
    let (hwnd, msg, wp, lp) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_u32(3));
    if hwnd == HWND_BROADCAST {
        let list: Vec<u32> = us(m).borrow_mut().windows.values().filter(|w| w.style & WS_CHILD == 0).map(|w| w.hwnd).collect();
        for h in list {
            post_message(m, h, msg, wp, lp);
        }
        return ThunkResult::stdcall(1, 4);
    }
    let ok = post_message(m, hwnd, msg, wp, lp);
    if !ok {
        m.set_last_error(1400); // ERROR_INVALID_WINDOW_HANDLE
    }
    ThunkResult::stdcall(ok as u32, 4)
}

/// PostThreadMessageA(idThread, msg, wParam, lParam)
fn post_thread_message_a(m: &mut Machine) -> ThunkResult {
    let (tid, msg, wp, lp) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_u32(3));
    if m.thread_by_tid(tid).is_none() {
        m.set_last_error(1444); // ERROR_INVALID_THREAD_ID
        return ThunkResult::stdcall(0, 4);
    }
    let now = m.now_ms() as u32;
    let pt = (m.input.mouse_x, m.input.mouse_y);
    us(m).borrow_mut().queues.entry(tid).or_default().msgs.push_back(Msg { hwnd: 0, message: msg, wparam: wp, lparam: lp, time: now, pt });
    ThunkResult::stdcall(1, 4)
}

fn send_message_a(m: &mut Machine) -> ThunkResult {
    let (hwnd, msg, wp, lp) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_u32(3));
    let r = if hwnd == HWND_BROADCAST {
        let list: Vec<u32> = us(m).borrow_mut().windows.values().filter(|w| w.style & WS_CHILD == 0).map(|w| w.hwnd).collect();
        for h in list {
            send(m, h, msg, wp, lp);
        }
        0
    } else {
        send(m, hwnd, msg, wp, lp)
    };
    ThunkResult::stdcall(r, 4)
}

/// SendMessageTimeoutA(hwnd, msg, wParam, lParam, flags, timeout, lpdwResult)
fn send_message_timeout_a(m: &mut Machine) -> ThunkResult {
    let (hwnd, msg, wp, lp, out) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_u32(3), m.arg_u32(6));
    let exists = hwnd == HWND_BROADCAST || us(m).borrow_mut().windows.contains_key(&hwnd);
    if !exists {
        return ThunkResult::stdcall(0, 7);
    }
    let r = if hwnd == HWND_BROADCAST { 0 } else { send(m, hwnd, msg, wp, lp) };
    if out != 0 {
        m.mem.write_u32(out, r);
    }
    ThunkResult::stdcall(1, 7)
}

fn post_quit_message(m: &mut Machine) -> ThunkResult {
    let code = m.arg_u32(0);
    let tid = m.current_tid();
    us(m).borrow_mut().queues.entry(tid).or_default().quit = Some(code);
    m.log(LogLevel::Info, &format!("PostQuitMessage({code}) on thread {tid:#x}"));
    ThunkResult::stdcall(0, 1)
}

fn get_message_pos(m: &mut Machine) -> ThunkResult {
    let tid = m.current_tid();
    let p = us(m).borrow_mut().queues.get(&tid).map(|q| q.last_pos).unwrap_or((m.input.mouse_x, m.input.mouse_y));
    ThunkResult::stdcall(((p.1 as u32 & 0xFFFF) << 16) | (p.0 as u32 & 0xFFFF), 0)
}

fn get_message_time(m: &mut Machine) -> ThunkResult {
    let tid = m.current_tid();
    let t = us(m).borrow_mut().queues.get(&tid).map(|q| q.last_time).unwrap_or(0);
    ThunkResult::stdcall(t, 0)
}

fn get_queue_status(m: &mut Machine) -> ThunkResult {
    let tid = m.current_tid();
    pump_input(m);
    let n = us(m).borrow_mut().queues.get(&tid).map(|q| q.msgs.len()).unwrap_or(0);
    let bits = if n > 0 { 0x0008 } else { 0 }; // QS_POSTMESSAGE
    ThunkResult::stdcall(bits << 16 | bits, 1)
}

fn get_input_state(m: &mut Machine) -> ThunkResult {
    let tid = m.current_tid();
    pump_input(m);
    let n = us(m).borrow_mut().queues.get(&tid).map(|q| q.msgs.iter().any(|x| (0x100..=0x209).contains(&x.message))).unwrap_or(false);
    ThunkResult::stdcall(n as u32, 0)
}

fn register_window_message_a(m: &mut Machine) -> ThunkResult {
    let name = m.arg_str(0).unwrap_or_default().to_ascii_lowercase();
    let st = us(m);
    let mut s = st.borrow_mut();
    let n = s.reg_msgs.len() as u32;
    let id = *s.reg_msgs.entry(name).or_insert(0xC100 + n);
    ThunkResult::stdcall(id, 1)
}

/// SetTimer(hwnd, id, elapse, proc)
fn set_timer(m: &mut Machine) -> ThunkResult {
    ensure_service(m);
    let (hwnd, id, ms, proc_) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_u32(3));
    let now = m.now_ms();
    let tid = m.current_tid();
    let st = us(m);
    let mut s = st.borrow_mut();
    let owner = if hwnd != 0 {
        match s.windows.get(&hwnd) {
            Some(w) => w.owner_tid,
            None => return ThunkResult::stdcall(0, 4),
        }
    } else {
        tid
    };
    let id = if hwnd == 0 {
        s.next_timer_id += 1;
        s.next_timer_id
    } else {
        id
    };
    s.timers.retain(|t| !(t.hwnd == hwnd && t.id == id));
    let period = ms.max(10);
    s.timers.push(Timer { hwnd, id, period, next_due: now + period as u64, proc_, tid: owner });
    ThunkResult::stdcall(if hwnd == 0 { id } else { id.max(1) }, 4)
}

fn kill_timer(m: &mut Machine) -> ThunkResult {
    let (hwnd, id) = (m.arg_u32(0), m.arg_u32(1));
    let st = us(m);
    let mut s = st.borrow_mut();
    let before = s.timers.len();
    s.timers.retain(|t| !(t.hwnd == hwnd && t.id == id));
    ThunkResult::stdcall((s.timers.len() != before) as u32, 2)
}

// ---------------------------------------------------------------------------------------------- painting / DCs

fn window_dc(m: &mut Machine) -> u32 {
    let hdc = gdi32::create_dc_for(m, None);
    us(m).borrow_mut().window_dcs.push(hdc);
    hdc
}

/// BeginPaint(hwnd, PAINTSTRUCT*)
fn begin_paint(m: &mut Machine) -> ThunkResult {
    let (hwnd, ps) = (m.arg_u32(0), m.arg_u32(1));
    let Some(r) = win_rect(m, hwnd) else { return ThunkResult::stdcall(0, 2) };
    if let Some(w) = us(m).borrow_mut().windows.get_mut(&hwnd) {
        w.needs_paint = false;
    }
    let hdc = window_dc(m);
    m.mem.fill(ps, 64, 0);
    m.mem.write_u32(ps, hdc);
    m.mem.write_u32(ps + 4, 1);
    Rect::new(0, 0, r.r - r.l, r.b - r.t).write(m, ps + 8);
    ThunkResult::stdcall(hdc, 2)
}

fn end_paint(m: &mut Machine) -> ThunkResult {
    let ps = m.arg_u32(1);
    let hdc = m.mem.read_u32(ps);
    release_window_dc(m, hdc);
    ThunkResult::stdcall(1, 2)
}

fn invalidate_rect(m: &mut Machine) -> ThunkResult {
    let hwnd = m.arg_u32(0);
    let st = us(m);
    let mut s = st.borrow_mut();
    if hwnd == 0 {
        for w in s.windows.values_mut() {
            w.needs_paint = true;
        }
    } else if let Some(w) = s.windows.get_mut(&hwnd) {
        w.needs_paint = true;
    }
    ThunkResult::stdcall(1, 3)
}

fn validate_rect(m: &mut Machine) -> ThunkResult {
    let hwnd = m.arg_u32(0);
    if let Some(w) = us(m).borrow_mut().windows.get_mut(&hwnd) {
        w.needs_paint = false;
    }
    ThunkResult::stdcall(1, 2)
}

fn get_update_rect(m: &mut Machine) -> ThunkResult {
    let (hwnd, p) = (m.arg_u32(0), m.arg_u32(1));
    let need = us(m).borrow_mut().windows.get(&hwnd).map(|w| w.needs_paint).unwrap_or(false);
    if p != 0 {
        let r = if need { win_rect(m, hwnd).map(|r| Rect::new(0, 0, r.r - r.l, r.b - r.t)).unwrap_or_default() } else { Rect::default() };
        r.write(m, p);
    }
    ThunkResult::stdcall(need as u32, 3)
}

fn get_dc(m: &mut Machine) -> ThunkResult {
    let hdc = window_dc(m);
    ThunkResult::stdcall(hdc, 1)
}

fn release_window_dc(m: &mut Machine, hdc: u32) -> bool {
    let st = us(m);
    let ok = {
        let mut s = st.borrow_mut();
        let before = s.window_dcs.len();
        s.window_dcs.retain(|&d| d != hdc);
        s.window_dcs.len() != before
    };
    if ok {
        gdi32::delete_dc_handle(m, hdc);
    }
    ok
}

fn release_dc(m: &mut Machine) -> ThunkResult {
    let hdc = m.arg_u32(1);
    let ok = release_window_dc(m, hdc);
    ThunkResult::stdcall(ok as u32, 2)
}

const DT_CENTER: u32 = 0x1;
const DT_RIGHT: u32 = 0x2;
const DT_VCENTER: u32 = 0x4;
const DT_BOTTOM: u32 = 0x8;
const DT_WORDBREAK: u32 = 0x10;
const DT_SINGLELINE: u32 = 0x20;
const DT_NOCLIP: u32 = 0x100;
const DT_CALCRECT: u32 = 0x400;
const DT_NOPREFIX: u32 = 0x800;

/// Split text into lines for DrawText (word wrap at `width` if DT_WORDBREAK). Pure layout, for tests too.
pub fn layout_lines(text: &[u8], fmt: u32, width: i32, adv: &mut dyn FnMut(&[u8]) -> i32) -> Vec<Vec<u8>> {
    let single = fmt & DT_SINGLELINE != 0;
    let mut paragraphs: Vec<Vec<u8>> = Vec::new();
    if single {
        paragraphs.push(text.iter().copied().filter(|&c| c != b'\r' && c != b'\n').collect());
    } else {
        let mut cur = Vec::new();
        let mut i = 0;
        while i < text.len() {
            match text[i] {
                b'\r' => {
                    paragraphs.push(std::mem::take(&mut cur));
                    if text.get(i + 1) == Some(&b'\n') {
                        i += 1;
                    }
                }
                b'\n' => paragraphs.push(std::mem::take(&mut cur)),
                c => cur.push(c),
            }
            i += 1;
        }
        paragraphs.push(cur);
    }
    if fmt & DT_WORDBREAK == 0 || single {
        return paragraphs;
    }
    let mut out = Vec::new();
    for p in paragraphs {
        let mut line: Vec<u8> = Vec::new();
        for word in p.split(|&c| c == b' ') {
            let candidate: Vec<u8> = if line.is_empty() { word.to_vec() } else { [line.as_slice(), b" ", word].concat() };
            if !line.is_empty() && adv(&candidate) > width {
                out.push(std::mem::take(&mut line));
                line = word.to_vec();
            } else {
                line = candidate;
            }
            // A single word wider than the line: break by characters.
            while adv(&line) > width && line.len() > 1 {
                let mut k = line.len() - 1;
                while k > 1 && adv(&line[..k]) > width {
                    k -= 1;
                }
                out.push(line[..k].to_vec());
                line = line[k..].to_vec();
            }
        }
        out.push(line);
    }
    out
}

fn strip_prefix_amp(line: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(line.len());
    let mut i = 0;
    while i < line.len() {
        if line[i] == b'&' {
            if line.get(i + 1) == Some(&b'&') {
                out.push(b'&');
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        out.push(line[i]);
        i += 1;
    }
    out
}

/// One laid-out line of a `DrawTextA` call: text (prefix `&` removed), left x, top y and width in DC pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct DrawTextLine {
    pub text: Vec<u8>,
    pub x: i32,
    pub y: i32,
    pub w: i32,
}

/// Layout of a `DrawTextA` call in the DC's font, exactly as `DrawTextA` draws it. Host mods use it to redraw the
/// game's texts at another resolution (`s3-mods/src/panel_text.rs`).
#[derive(Debug, Clone, PartialEq)]
pub struct DrawTextLayout {
    pub lines: Vec<DrawTextLine>,
    /// Line height in DC pixels.
    pub line_h: i32,
    /// fontdue em size of the DC's font (pixels) and whether it uses the bold face.
    pub px: f32,
    pub bold: bool,
}

/// Lay out `text` in `rect` with the DT_* flags `fmt` (DT_CALCRECT is not looked at) in the font selected into
/// `hdc`. `None` for an unknown DC.
pub fn draw_text_layout(m: &mut Machine, hdc: u32, text: &[u8], rect: Rect, fmt: u32) -> Option<DrawTextLayout> {
    let mt = m.state::<gdi32::GdiState>().borrow_mut().dc_metrics(hdc)?;
    let line_h = mt.height;
    let width = rect.r - rect.l;
    let mut measure = |s: &[u8]| -> i32 { gdi32::text_extent(m, hdc, s).map(|e| e.0).unwrap_or(0) };
    let mut lines = layout_lines(text, fmt, width, &mut measure);
    if fmt & DT_NOPREFIX == 0 {
        lines = lines.iter().map(|l| strip_prefix_amp(l)).collect();
    }
    let mut y = rect.t;
    if fmt & DT_SINGLELINE != 0 {
        if fmt & DT_VCENTER != 0 {
            y = rect.t + (rect.b - rect.t - line_h) / 2;
        } else if fmt & DT_BOTTOM != 0 {
            y = rect.b - line_h;
        }
    }
    let mut out = Vec::with_capacity(lines.len());
    for text in lines {
        let w = gdi32::text_extent(m, hdc, &text).map(|e| e.0).unwrap_or(0);
        let x = if fmt & DT_CENTER != 0 {
            rect.l + (width - w) / 2
        } else if fmt & DT_RIGHT != 0 {
            rect.r - w
        } else {
            rect.l
        };
        out.push(DrawTextLine { text, x, y, w });
        y += line_h;
    }
    Some(DrawTextLayout { lines: out, line_h, px: mt.px, bold: mt.bold })
}

/// DrawTextA(hdc, str, count, rect, format) -> height of the text.
fn draw_text_a(m: &mut Machine) -> ThunkResult {
    let (hdc, sp, n, prc, fmt) = (m.arg_u32(0), m.arg_u32(1), m.arg_i32(2), m.arg_u32(3), m.arg_u32(4));
    let text = if n < 0 { m.mem.read_cstr_bytes(sp) } else { m.mem.read_bytes(sp, (n as usize).min(0x10000)) };
    let mut rect = Rect::read(m, prc);
    let Some(lay) = draw_text_layout(m, hdc, &text, rect, fmt) else { return ThunkResult::stdcall(0, 5) };
    let width = rect.r - rect.l;
    let total_h = lay.line_h * lay.lines.len() as i32;
    if fmt & DT_CALCRECT != 0 {
        let maxw = lay.lines.iter().map(|l| l.w).max().unwrap_or(0);
        if fmt & DT_SINGLELINE != 0 || fmt & DT_WORDBREAK == 0 || maxw > width {
            rect.r = rect.l + maxw;
        }
        rect.b = rect.t + total_h;
        rect.write(m, prc);
        return ThunkResult::stdcall(total_h as u32, 5);
    }
    let clip = if fmt & DT_NOCLIP != 0 { None } else { Some(rect) };
    // DrawText ignores TA_* alignment: temporarily force TA_LEFT|TA_TOP.
    let saved_align = {
        let st = m.state::<gdi32::GdiState>();
        let mut g = st.borrow_mut();
        g.dc(hdc).map(|d| std::mem::replace(&mut d.text_align, 0))
    };
    for line in &lay.lines {
        gdi32::draw_text_at(m, hdc, line.x, line.y, &line.text, None, clip);
    }
    if let Some(a) = saved_align {
        let st = m.state::<gdi32::GdiState>();
        let mut g = st.borrow_mut();
        if let Some(d) = g.dc(hdc) {
            d.text_align = a;
        }
    }
    ThunkResult::stdcall(total_h as u32, 5)
}

// ---------------------------------------------------------------------------------------------- system info

fn get_system_metrics(m: &mut Machine) -> ThunkResult {
    let i = m.arg_u32(0);
    let (w, h) = screen_size(m);
    let v = match i {
        0 | 16 | 61 | 78 => w,
        1 | 62 | 79 => h,
        17 => h.saturating_sub(18),
        2 | 3 | 20 | 21 => 16,
        4 => 18,
        5 | 6 => 1,
        7 | 8 => 3,
        11 | 12 | 13 | 14 => 32,
        15 => 19,
        19 => 1,
        23 => 0,
        32 | 33 => 4,
        36 | 37 => 4,
        43 => 3,
        80 => 1,
        _ => 0,
    };
    ThunkResult::stdcall(v, 1)
}

/// SystemParametersInfoA(action, uiParam, pvParam, fWinIni)
fn system_parameters_info_a(m: &mut Machine) -> ThunkResult {
    let (action, pv) = (m.arg_u32(0), m.arg_u32(2));
    match action {
        0x30 => {
            // SPI_GETWORKAREA
            let (w, h) = screen_size(m);
            if pv != 0 {
                Rect::new(0, 0, w as i32, h as i32).write(m, pv);
            }
        }
        _ => {
            if pv != 0 && action & 0x1 == 0 && action < 0x2000 {
                // Most SPI_GET* write a BOOL/UINT.
                m.mem.write_u32(pv, 0);
            }
        }
    }
    ThunkResult::stdcall(1, 4)
}

fn get_sys_color(m: &mut Machine) -> ThunkResult {
    let i = m.arg_u32(0);
    ThunkResult::stdcall(gdi32::sys_color(i), 1)
}

fn get_double_click_time(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(500, 0)
}

// ---------------------------------------------------------------------------------------------- cursor / keys

const CURSOR_BASE: u32 = 0x00CC_0000;

fn res_handle(m: &Machine, name: u32) -> u32 {
    if name < 0x1_0000 {
        CURSOR_BASE + name
    } else {
        let s = m.mem.read_cstr(name);
        CURSOR_BASE + 0x1_0000 + (s.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32)) & 0xFFFF)
    }
}

/// Decode the cursor image for `(hinst, name)` and remember it under handle `h` (for drawing at present time,
/// `s3_runtime::cursor`). `hinst == 0` = system cursor (IDC_*): our built-in arrow. A resource that cannot be
/// found/decoded keeps the handle (the game treats NULL as fatal) but draws nothing.
fn register_cursor_image(m: &mut Machine, h: u32, hinst: u32, name: u32) {
    let cs = m.state::<CursorState>();
    if cs.borrow_mut().images.contains_key(&h) {
        return;
    }
    let img = if hinst == 0 {
        Some(cursor::builtin_arrow())
    } else {
        let id = ResId::from_guest_a(m, name);
        let img = cursor::load_cursor_resource(m, hinst, &id);
        if img.is_none() {
            m.log(LogLevel::Warn, &format!("LoadCursor: cursor {id:?} not found/decodable in module {hinst:#x}"));
        }
        img
    };
    if let Some(img) = img {
        cs.borrow_mut().images.insert(h, Arc::new(img));
    }
}

fn load_cursor_a(m: &mut Machine) -> ThunkResult {
    let (hinst, name) = (m.arg_u32(0), m.arg_u32(1));
    let h = res_handle(m, name);
    register_cursor_image(m, h, hinst, name);
    ThunkResult::stdcall(h, 2)
}

fn load_icon_a(m: &mut Machine) -> ThunkResult {
    let h = res_handle(m, m.arg_u32(1)) + 0x2_0000;
    ThunkResult::stdcall(h, 2)
}

/// LoadImageA(hinst, name, type, cx, cy, flags)
fn load_image_a(m: &mut Machine) -> ThunkResult {
    let (hinst, name, ty, flags) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_u32(5));
    let h = res_handle(m, name) + 0x4_0000;
    // IMAGE_CURSOR from a resource (LR_LOADFROMFILE = 0x10 not supported).
    if ty == 2 && flags & 0x10 == 0 {
        register_cursor_image(m, h, hinst, name);
    }
    ThunkResult::stdcall(h, 6)
}

fn set_cursor(m: &mut Machine) -> ThunkResult {
    let h = m.arg_u32(0);
    let prev = std::mem::replace(&mut us(m).borrow_mut().cursor, h);
    m.state::<CursorState>().borrow_mut().current = h;
    m.add_service("cursor", Machine::refresh_cursor);
    ThunkResult::stdcall(prev, 1)
}

fn show_cursor(m: &mut Machine) -> ThunkResult {
    let show = m.arg_u32(0) != 0;
    let st = us(m);
    let mut s = st.borrow_mut();
    s.cursor_count += if show { 1 } else { -1 };
    let count = s.cursor_count;
    drop(s);
    m.state::<CursorState>().borrow_mut().show_count = count;
    ThunkResult::stdcall(count as u32, 1)
}

fn get_cursor_pos(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    let (mut x, mut y) = (m.input.mouse_x, m.input.mouse_y);
    if let Some(c) = us(m).borrow_mut().clip {
        x = x.clamp(c.l, (c.r - 1).max(c.l));
        y = y.clamp(c.t, (c.b - 1).max(c.t));
    }
    if p != 0 {
        m.mem.write_u32(p, x as u32);
        m.mem.write_u32(p + 4, y as u32);
    }
    ThunkResult::stdcall(1, 1)
}

fn set_cursor_pos(m: &mut Machine) -> ThunkResult {
    let (x, y) = (m.arg_i32(0), m.arg_i32(1));
    // HYPOTHESIS: the host pointer cannot be warped; only the emulated position changes.
    m.input.mouse_x = x;
    m.input.mouse_y = y;
    ThunkResult::stdcall(1, 2)
}

fn clip_cursor(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    let r = if p == 0 { None } else { Some(Rect::read(m, p)) };
    us(m).borrow_mut().clip = r;
    m.state::<CursorState>().borrow_mut().clip = r.map(|c| (c.l, c.t, c.r, c.b));
    ThunkResult::stdcall(1, 1)
}

fn key_down(m: &Machine, vk: u32) -> bool {
    match vk & 0xFF {
        0x01 => m.input.buttons[0],
        0x02 => m.input.buttons[1],
        0x04 => m.input.buttons[2],
        0xA0 | 0xA1 => m.input.keys[0x10],
        0xA2 | 0xA3 => m.input.keys[0x11],
        0xA4 | 0xA5 => m.input.keys[0x12],
        v => m.input.keys[v as usize],
    }
}

fn get_async_key_state(m: &mut Machine) -> ThunkResult {
    let vk = m.arg_u32(0);
    let v = if key_down(m, vk) { 0xFFFF_8000 } else { 0 };
    ThunkResult::stdcall(v, 1)
}

fn get_key_state(m: &mut Machine) -> ThunkResult {
    let vk = m.arg_u32(0) & 0xFF;
    let toggled = us(m).borrow_mut().toggles[vk as usize];
    let v = if key_down(m, vk) { 0xFFFF_8000 } else { 0 } | toggled as u32;
    ThunkResult::stdcall(v, 1)
}

fn get_keyboard_state(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    let toggles = us(m).borrow_mut().toggles;
    for vk in 0..256u32 {
        let b = if key_down(m, vk) { 0x80u8 } else { 0 } | toggles[vk as usize] as u8;
        m.mem.write_u8(p + vk, b);
    }
    ThunkResult::stdcall(1, 1)
}

/// MapVirtualKeyA(code, type): 0 = VK -> scan, 1 = scan -> VK, 2 = VK -> char.
fn map_virtual_key_a(m: &mut Machine) -> ThunkResult {
    let (code, ty) = (m.arg_u32(0), m.arg_u32(1));
    let r = match ty {
        0 => vk_scan(code),
        1 => (1..256).find(|&v| vk_scan(v) == code).unwrap_or(0),
        2 => vk_to_char(code, false, false).map(|c| if (0x61..=0x7A).contains(&c) { c - 0x20 } else { c }).unwrap_or(0),
        _ => 0,
    };
    ThunkResult::stdcall(r, 2)
}

fn get_keyboard_layout(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(0x0409_0409, 1)
}

/// MessageBoxA(hwnd, text, caption, type): logged; answers OK/YES/IGNORE/CANCEL so the game continues
/// (retry/cancel boxes get CANCEL so no endless retry loop).
fn message_box_a(m: &mut Machine) -> ThunkResult {
    let text = m.arg_str(1).unwrap_or_default();
    let cap = m.arg_str(2).unwrap_or_default();
    let ty = m.arg_u32(3);
    let answer = match ty & 0xF {
        0 | 1 => 1, // MB_OK / MB_OKCANCEL -> IDOK
        2 => 5,     // MB_ABORTRETRYIGNORE -> IDIGNORE
        3 | 4 => 6, // MB_YESNOCANCEL / MB_YESNO -> IDYES
        5 => 2,     // MB_RETRYCANCEL -> IDCANCEL
        _ => 1,
    };
    m.log(LogLevel::Warn, &format!("MessageBoxA [{cap}] {text:?} (type {ty:#x}) -> {answer}"));
    ThunkResult::stdcall(answer, 4)
}

// ---------------------------------------------------------------------------------------------- wsprintf

/// Format with Windows `wsprintf` rules; `next_arg()` returns the next 32-bit argument, `read_str` reads an ANSI
/// string, `read_wstr` a UTF-16 string. Output is truncated to 1023 chars like Windows.
pub fn format_wsprintf(fmt: &[u8], mut next_arg: impl FnMut() -> u32, read_str: impl Fn(u32) -> Vec<u8>, read_wstr: impl Fn(u32) -> Vec<u8>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < fmt.len() {
        let c = fmt[i];
        i += 1;
        if c != b'%' {
            out.push(c);
            continue;
        }
        let start = i - 1;
        let (mut left, mut zero, mut hash) = (false, false, false);
        while i < fmt.len() {
            match fmt[i] {
                b'-' => left = true,
                b'0' => zero = true,
                b'#' => hash = true,
                _ => break,
            }
            i += 1;
        }
        let mut width = 0usize;
        while i < fmt.len() && fmt[i].is_ascii_digit() {
            width = width * 10 + (fmt[i] - b'0') as usize;
            i += 1;
        }
        let mut prec: Option<usize> = None;
        if i < fmt.len() && fmt[i] == b'.' {
            i += 1;
            let mut p = 0;
            while i < fmt.len() && fmt[i].is_ascii_digit() {
                p = p * 10 + (fmt[i] - b'0') as usize;
                i += 1;
            }
            prec = Some(p);
        }
        let mut wide: Option<bool> = None;
        while i < fmt.len() && matches!(fmt[i], b'l' | b'h' | b'L' | b'I') {
            wide = Some(fmt[i] == b'l');
            i += 1;
        }
        let Some(&ty) = fmt.get(i) else { break };
        i += 1;
        let body: Vec<u8> = match ty {
            b'%' => {
                out.push(b'%');
                continue;
            }
            b'd' | b'i' => {
                let v = next_arg() as i32;
                let mut s = v.unsigned_abs().to_string().into_bytes();
                if let Some(p) = prec {
                    while s.len() < p {
                        s.insert(0, b'0');
                    }
                }
                if v < 0 {
                    if zero && !left && prec.is_none() {
                        while s.len() + 1 < width {
                            s.insert(0, b'0');
                        }
                    }
                    s.insert(0, b'-');
                }
                s
            }
            b'u' => {
                let mut s = next_arg().to_string().into_bytes();
                if let Some(p) = prec {
                    while s.len() < p {
                        s.insert(0, b'0');
                    }
                }
                s
            }
            b'x' | b'X' => {
                let v = next_arg();
                let mut s = if ty == b'x' { format!("{v:x}") } else { format!("{v:X}") }.into_bytes();
                if let Some(p) = prec {
                    while s.len() < p {
                        s.insert(0, b'0');
                    }
                }
                if hash && v != 0 {
                    let pre: &[u8] = if ty == b'x' { b"0x" } else { b"0X" };
                    s.splice(0..0, pre.iter().copied());
                }
                s
            }
            b'c' | b'C' => {
                let v = next_arg();
                let is_w = wide.unwrap_or(ty == b'C');
                let ch = if is_w { if v < 0x100 { v as u8 } else { b'?' } } else { v as u8 };
                vec![ch]
            }
            b's' | b'S' => {
                let p = next_arg();
                let is_w = wide.unwrap_or(ty == b'S');
                let mut s = if p == 0 { b"(null)".to_vec() } else if is_w { read_wstr(p) } else { read_str(p) };
                if let Some(pr) = prec {
                    s.truncate(pr);
                }
                s
            }
            _ => {
                out.extend_from_slice(&fmt[start..i]);
                continue;
            }
        };
        let pad = width.saturating_sub(body.len());
        if left {
            out.extend_from_slice(&body);
            out.extend(std::iter::repeat(b' ').take(pad));
        } else {
            let fill = if zero && matches!(ty, b'd' | b'i' | b'u' | b'x' | b'X') && prec.is_none() { b'0' } else { b' ' };
            if fill == b'0' && body.first() == Some(&b'-') {
                out.push(b'-');
                out.extend(std::iter::repeat(b'0').take(pad));
                out.extend_from_slice(&body[1..]);
            } else {
                out.extend(std::iter::repeat(fill).take(pad));
                out.extend_from_slice(&body);
            }
        }
    }
    out.truncate(1023);
    out
}

fn do_wsprintf(m: &mut Machine, buf: u32, fmt_p: u32, mut ap: u32) -> u32 {
    let fmt = m.mem.read_cstr_bytes(fmt_p);
    let out = {
        let mem = &m.mem;
        format_wsprintf(
            &fmt,
            || {
                let v = mem.read_u32(ap);
                ap = ap.wrapping_add(4);
                v
            },
            |p| mem.read_cstr_bytes(p),
            |p| mem.read_wstr(p).chars().map(|c| if (c as u32) < 0x100 { c as u8 } else { b'?' }).collect(),
        )
    };
    m.mem.write_cstr_bytes(buf, &out);
    out.len() as u32
}

/// wsprintfA(buf, fmt, ...) — cdecl varargs.
fn wsprintf_a(m: &mut Machine) -> ThunkResult {
    let (buf, fmt) = (m.arg_u32(0), m.arg_u32(1));
    let ap = m.cpu().regs[s3_runtime::cpu::ESP].wrapping_add(12);
    let n = do_wsprintf(m, buf, fmt, ap);
    ThunkResult::cdecl(n)
}

/// wvsprintfA(buf, fmt, va_list) — stdcall.
fn wvsprintf_a(m: &mut Machine) -> ThunkResult {
    let (buf, fmt, ap) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    let n = do_wsprintf(m, buf, fmt, ap);
    ThunkResult::stdcall(n, 3)
}

// ---------------------------------------------------------------------------------------------- rectangles

fn set_rect(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    if p == 0 {
        return ThunkResult::stdcall(0, 5);
    }
    Rect::new(m.arg_i32(1), m.arg_i32(2), m.arg_i32(3), m.arg_i32(4)).write(m, p);
    ThunkResult::stdcall(1, 5)
}

fn set_rect_empty(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    Rect::default().write(m, p);
    ThunkResult::stdcall(1, 1)
}

fn copy_rect(m: &mut Machine) -> ThunkResult {
    let (d, s) = (m.arg_u32(0), m.arg_u32(1));
    let r = Rect::read(m, s);
    r.write(m, d);
    ThunkResult::stdcall(1, 2)
}

fn intersect_rect(m: &mut Machine) -> ThunkResult {
    let (d, a, b) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    let r = Rect::read(m, a).intersect(&Rect::read(m, b));
    r.write(m, d);
    ThunkResult::stdcall(!r.is_empty() as u32, 3)
}

fn union_rect(m: &mut Machine) -> ThunkResult {
    let (d, a, b) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    let (ra, rb) = (Rect::read(m, a), Rect::read(m, b));
    let r = if ra.is_empty() {
        rb
    } else if rb.is_empty() {
        ra
    } else {
        Rect::new(ra.l.min(rb.l), ra.t.min(rb.t), ra.r.max(rb.r), ra.b.max(rb.b))
    };
    let r = if r.is_empty() { Rect::default() } else { r };
    r.write(m, d);
    ThunkResult::stdcall(!r.is_empty() as u32, 3)
}

fn offset_rect(m: &mut Machine) -> ThunkResult {
    let (p, dx, dy) = (m.arg_u32(0), m.arg_i32(1), m.arg_i32(2));
    let r = Rect::read(m, p);
    Rect::new(r.l + dx, r.t + dy, r.r + dx, r.b + dy).write(m, p);
    ThunkResult::stdcall(1, 3)
}

fn inflate_rect(m: &mut Machine) -> ThunkResult {
    let (p, dx, dy) = (m.arg_u32(0), m.arg_i32(1), m.arg_i32(2));
    let r = Rect::read(m, p);
    Rect::new(r.l - dx, r.t - dy, r.r + dx, r.b + dy).write(m, p);
    ThunkResult::stdcall(1, 3)
}

fn is_rect_empty(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    let e = p == 0 || Rect::read(m, p).is_empty();
    ThunkResult::stdcall(e as u32, 1)
}

/// PtInRect(lprc, POINT pt) — the POINT is passed by value (2 dwords).
fn pt_in_rect(m: &mut Machine) -> ThunkResult {
    let (p, x, y) = (m.arg_u32(0), m.arg_i32(1), m.arg_i32(2));
    let inside = p != 0 && Rect::read(m, p).contains(x, y);
    ThunkResult::stdcall(inside as u32, 3)
}

fn equal_rect(m: &mut Machine) -> ThunkResult {
    let (a, b) = (m.arg_u32(0), m.arg_u32(1));
    let eq = Rect::read(m, a) == Rect::read(m, b);
    ThunkResult::stdcall(eq as u32, 2)
}

fn case_map(m: &mut Machine, upper: bool) -> u32 {
    let p = m.arg_u32(0);
    let conv = |b: u8| -> u8 {
        if upper {
            match b {
                b'a'..=b'z' => b - 32,
                0xE0..=0xFE if b != 0xF7 => b - 32,
                _ => b,
            }
        } else {
            match b {
                b'A'..=b'Z' => b + 32,
                0xC0..=0xDE if b != 0xD7 => b + 32,
                _ => b,
            }
        }
    };
    if p < 0x1_0000 {
        return conv(p as u8) as u32;
    }
    let s: Vec<u8> = m.mem.read_cstr_bytes(p).into_iter().map(conv).collect();
    m.mem.write_cstr_bytes(p, &s);
    p
}

fn char_upper_a(m: &mut Machine) -> ThunkResult {
    let r = case_map(m, true);
    ThunkResult::stdcall(r, 1)
}

fn char_lower_a(m: &mut Machine) -> ThunkResult {
    let r = case_map(m, false);
    ThunkResult::stdcall(r, 1)
}

fn window_from_point(m: &mut Machine) -> ThunkResult {
    let (x, y) = (m.arg_i32(0), m.arg_i32(1));
    let h = us(m).borrow_mut().windows.values().rev().find(|w| w.visible && w.rect.contains(x, y)).map(|w| w.hwnd).unwrap_or(0);
    ThunkResult::stdcall(h, 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wsprintf_formats() {
        let args = [42u32, (-7i32) as u32, 0xBEEF, 0x1000, b'Z' as u32, 5];
        let mut i = 0;
        let out = format_wsprintf(
            b"a%d b%i c%x d%#X e%-4c| f%05d g%s %% %q",
            || {
                let v = *args.get(i).unwrap_or(&0x1000);
                i += 1;
                v
            },
            |_| b"str".to_vec(),
            |_| b"w".to_vec(),
        );
        assert_eq!(String::from_utf8(out).unwrap(), "a42 b-7 cbeef d0X1000 eZ   | f00005 gstr % %q");
        let mut j = 0;
        let out = format_wsprintf(b"%.3d|%3s|%-3s|%.2s", || { j += 1; 1 }, |_| b"abcdef"[..1].to_vec(), |_| vec![]);
        assert_eq!(String::from_utf8(out).unwrap(), "001|  a|a  |a");
    }

    #[test]
    fn word_wrap() {
        let mut adv = |s: &[u8]| s.len() as i32 * 10;
        let l = layout_lines(b"hello big world", DT_WORDBREAK, 100, &mut adv);
        assert_eq!(l, vec![b"hello big".to_vec(), b"world".to_vec()]);
        let l = layout_lines(b"a\r\nb\nc", 0, 100, &mut adv);
        assert_eq!(l.len(), 3);
        let l = layout_lines(b"a\r\nb", DT_SINGLELINE, 100, &mut adv);
        assert_eq!(l, vec![b"ab".to_vec()]);
        let l = layout_lines(b"abcdefghijkl", DT_WORDBREAK, 50, &mut adv);
        assert!(l.iter().all(|x| x.len() <= 5), "{l:?}");
        assert_eq!(strip_prefix_amp(b"&File && more"), b"File & more".to_vec());
    }

    #[test]
    fn vk_chars() {
        assert_eq!(vk_to_char(0x41, false, false), Some(b'a' as u32));
        assert_eq!(vk_to_char(0x41, true, false), Some(b'A' as u32));
        assert_eq!(vk_to_char(0x31, true, false), Some(b'!' as u32));
        assert_eq!(vk_to_char(0x70, false, false), None);
        assert_eq!(vk_scan(0x1B), 1);
    }
}
