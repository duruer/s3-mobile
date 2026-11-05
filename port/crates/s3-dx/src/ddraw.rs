//! DirectDraw HLE (`ddraw.dll`). S3.EXE creates it with `CoCreateInstance(CLSID_DirectDraw, IID_IDirectDraw2)` +
//! `IDirectDraw2::Initialize`, and calls `DirectDrawEnumerateA` via GetProcAddress (see
//! docs/reverse-engineering/directx-surface.md, wine-trace.md, docs/porting/directx-hle.md).
//!
//! Surfaces are plain guest memory (allocated with VirtualAlloc-style regions). Blt/BltFast/Unlock/ReleaseDC
//! targeting the visible primary surface, and Flip, present the primary through `Machine::present`.
//! Display modes: EnumDisplayModes offers 640x480 / 800x600 / 1024x768 at 8 or 16 bpp (16-bit = RGB565);
//! SetDisplayMode accepts any size up to 4096x4096 (custom resolutions, docs/porting/resolution-zoom.md).

use std::collections::HashMap;

use s3_mem::{MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE};
use s3_runtime::com::Guid;
use s3_runtime::gdi::{self, DcTarget};
use s3_runtime::{FrameBuffer, LogLevel, Machine, Registry, ThunkResult};

use crate::blit::{self, BltOpts, ColorKey, Rect, Surf};
use crate::hr::*;
use crate::{declare_interface, put_u32, reg_method, ret};

pub const DLL: &str = "ddraw.dll";

pub const CLSID_DIRECTDRAW: Guid = Guid::parse("D7B70EE0-4340-11CF-B063-0020AFC2CD35");
pub const IID_IUNKNOWN: Guid = Guid::parse("00000000-0000-0000-C000-000000000046");
pub const IID_IDIRECTDRAW: Guid = Guid::parse("6C14DB80-A733-11CE-A521-0020AF0BE560");
pub const IID_IDIRECTDRAW2: Guid = Guid::parse("B3A6F3E0-2B43-11CF-A2DE-00AA00B93356");
pub const IID_IDIRECTDRAWSURFACE: Guid = Guid::parse("6C14DB81-A733-11CE-A521-0020AF0BE560");
pub const IID_IDIRECTDRAWSURFACE2: Guid = Guid::parse("57805885-6EEC-11CF-9441-A82303C10E27");
pub const IID_IDIRECTDRAWSURFACE3: Guid = Guid::parse("DA044E00-69B2-11D0-A1D5-00AA00B8DFBB");

pub const EXPORTS: &[&str] = &["DirectDrawCreate", "DirectDrawEnumerateA", "DirectDrawCreateClipper"];

/// IDirectDraw2 vtable (IDirectDraw + GetAvailableVidMem). `Initialize` is slot 18 (+0x48).
pub const IDIRECTDRAW2: &[&str] = &[
    "QueryInterface", "AddRef", "Release", "Compact", "CreateClipper", "CreatePalette", "CreateSurface",
    "DuplicateSurface", "EnumDisplayModes", "EnumSurfaces", "FlipToGDISurface", "GetCaps", "GetDisplayMode",
    "GetFourCCCodes", "GetGDISurface", "GetMonitorFrequency", "GetScanLine", "GetVerticalBlankStatus",
    "Initialize", "RestoreDisplayMode", "SetCooperativeLevel", "SetDisplayMode", "WaitForVerticalBlank",
    "GetAvailableVidMem",
];

/// IDirectDraw (v1) vtable: the first 23 entries of IDirectDraw2. `SetDisplayMode` takes 3 args (no refresh
/// rate / flags), so it has its own handler (`IDirectDraw::SetDisplayMode`).
pub const IDIRECTDRAW: &[&str] = IDIRECTDRAW2.split_at(23).0;

/// IDirectDrawSurface3 vtable (v1 = first 36 entries, v2 adds 3, v3 adds SetSurfaceDesc). All three interfaces
/// share one vtable (prefix compatible).
pub const IDIRECTDRAWSURFACE3: &[&str] = &[
    "QueryInterface", "AddRef", "Release", "AddAttachedSurface", "AddOverlayDirtyRect", "Blt", "BltBatch",
    "BltFast", "DeleteAttachedSurface", "EnumAttachedSurfaces", "EnumOverlayZOrders", "Flip",
    "GetAttachedSurface", "GetBltStatus", "GetCaps", "GetClipper", "GetColorKey", "GetDC", "GetFlipStatus",
    "GetOverlayPosition", "GetPalette", "GetPixelFormat", "GetSurfaceDesc", "Initialize", "IsLost", "Lock",
    "ReleaseDC", "Restore", "SetClipper", "SetColorKey", "SetOverlayPosition", "SetPalette", "Unlock",
    "UpdateOverlay", "UpdateOverlayDisplay", "UpdateOverlayZOrder", "GetDDInterface", "PageLock",
    "PageUnlock", "SetSurfaceDesc",
];

pub const IDIRECTDRAWPALETTE: &[&str] =
    &["QueryInterface", "AddRef", "Release", "GetCaps", "GetEntries", "Initialize", "SetEntries"];

pub const IDIRECTDRAWCLIPPER: &[&str] = &[
    "QueryInterface", "AddRef", "Release", "GetClipList", "GetHWnd", "Initialize", "IsClipListChanged",
    "SetClipList", "SetHWnd",
];

// ---------------------------------------------------------------------------------------------- constants

pub const DD_OK: u32 = 0;
const fn ddhr(code: u32) -> u32 {
    0x8876_0000 | code
}
pub const DDERR_INVALIDPARAMS: u32 = E_INVALIDARG;
pub const DDERR_UNSUPPORTED: u32 = E_NOTIMPL;
pub const DDERR_ALREADYINITIALIZED: u32 = ddhr(5);
pub const DDERR_NOCOLORKEY: u32 = ddhr(215);
pub const DDERR_NOCLIPLIST: u32 = ddhr(205);
pub const DDERR_NOTFOUND: u32 = ddhr(255);
pub const DDERR_NOCLIPPERATTACHED: u32 = ddhr(568);
/// HYPOTHESIS: value from memory of ddraw.h (MAKE_DDHRESULT(...)); never seen used by S3.
pub const DDERR_NOPALETTEATTACHED: u32 = ddhr(572);
pub const DDERR_NOTLOCKED: u32 = ddhr(584);
pub const DDERR_CANTCREATEDC: u32 = ddhr(589);
pub const DDERR_WASSTILLDRAWING: u32 = ddhr(540);
pub const DDERR_INVALIDMODE: u32 = ddhr(70);

pub const DDSD_CAPS: u32 = 0x1;
pub const DDSD_HEIGHT: u32 = 0x2;
pub const DDSD_WIDTH: u32 = 0x4;
pub const DDSD_PITCH: u32 = 0x8;
pub const DDSD_BACKBUFFERCOUNT: u32 = 0x20;
pub const DDSD_LPSURFACE: u32 = 0x800;
pub const DDSD_PIXELFORMAT: u32 = 0x1000;
pub const DDSD_CKSRCBLT: u32 = 0x10000;
pub const DDSD_CKDESTBLT: u32 = 0x4000;
pub const DDSD_REFRESHRATE: u32 = 0x40000;

pub const DDSCAPS_BACKBUFFER: u32 = 0x4;
pub const DDSCAPS_COMPLEX: u32 = 0x8;
pub const DDSCAPS_FLIP: u32 = 0x10;
pub const DDSCAPS_FRONTBUFFER: u32 = 0x20;
pub const DDSCAPS_OFFSCREENPLAIN: u32 = 0x40;
pub const DDSCAPS_PRIMARYSURFACE: u32 = 0x200;
pub const DDSCAPS_SYSTEMMEMORY: u32 = 0x800;
pub const DDSCAPS_VIDEOMEMORY: u32 = 0x4000;
pub const DDSCAPS_VISIBLE: u32 = 0x8000;
pub const DDSCAPS_LOCALVIDMEM: u32 = 0x1000_0000;

pub const DDPF_PALETTEINDEXED8: u32 = 0x20;
pub const DDPF_RGB: u32 = 0x40;

pub const DDBLT_ASYNC: u32 = 0x200;
pub const DDBLT_COLORFILL: u32 = 0x400;
pub const DDBLT_DDFX: u32 = 0x800;
pub const DDBLT_KEYDEST: u32 = 0x2000;
pub const DDBLT_KEYDESTOVERRIDE: u32 = 0x4000;
pub const DDBLT_KEYSRC: u32 = 0x8000;
pub const DDBLT_KEYSRCOVERRIDE: u32 = 0x10000;
pub const DDBLT_ROP: u32 = 0x20000;
pub const DDBLT_DEPTHFILL: u32 = 0x0200_0000;
pub const DDBLT_WAIT: u32 = 0x0100_0000;
pub const DDBLTFX_MIRRORLEFTRIGHT: u32 = 0x2;
pub const DDBLTFX_MIRRORUPDOWN: u32 = 0x4;
pub const DDBLTFAST_SRCCOLORKEY: u32 = 0x1;
pub const DDBLTFAST_DESTCOLORKEY: u32 = 0x2;
pub const DDCKEY_COLORSPACE: u32 = 0x1;
pub const DDCKEY_DESTBLT: u32 = 0x2;
pub const DDCKEY_SRCBLT: u32 = 0x8;
pub const DDWAITVB_BLOCKBEGIN: u32 = 0x1;
pub const DDWAITVB_BLOCKBEGINEVENT: u32 = 0x2;
pub const DDWAITVB_BLOCKEND: u32 = 0x4;
pub const DDPCAPS_4BIT: u32 = 0x1;
pub const DDPCAPS_8BITENTRIES: u32 = 0x2;
pub const DDPCAPS_8BIT: u32 = 0x4;
pub const DDPCAPS_1BIT: u32 = 0x100;
pub const DDPCAPS_2BIT: u32 = 0x200;
const ROP_BLACKNESS: u32 = 0x0000_0042;
const ROP_WHITENESS: u32 = 0x00FF_0062;

/// DDSURFACEDESC size (DDSURFACEDESC2 is 0x7C with the same prefix).
pub const DDSD_SIZE: u32 = 0x6C;
/// Simulated refresh rate.
pub const REFRESH_HZ: u32 = 60;
/// Mode reported before the game calls SetDisplayMode.
pub const DEFAULT_MODE: (u32, u32, u32) = (800, 600, 16);
/// Modes offered by EnumDisplayModes (S3 uses 640x480, 800x600, 1024x768 at 16 bpp).
pub const MODES: &[(u32, u32)] = &[(640, 480), (800, 600), (1024, 768)];
pub const MODE_BPPS: &[u32] = &[8, 16];
const VIDMEM: u32 = 64 << 20;

// ---------------------------------------------------------------------------------------------- state

/// Host-side state of one surface (keyed by its interface pointer).
#[derive(Debug, Clone, Default)]
pub struct Surface {
    pub surf: Surf,
    /// DDSCAPS as reported.
    pub caps: u32,
    /// 16-bit X1R5G5B5 instead of RGB565.
    pub rgb555: bool,
    pub ck_src: Option<ColorKey>,
    pub ck_dst: Option<ColorKey>,
    pub palette: u32,
    pub clipper: u32,
    /// Attached surfaces (back buffer of a flip chain, or AddAttachedSurface).
    pub attached: Vec<u32>,
    /// Flip chain `[front, back1, ...]`, stored on the front surface only.
    pub chain: Vec<u32>,
    /// Pixel memory allocated by us (freed on final Release); false for SetSurfaceDesc(lpSurface) and chain
    /// members whose memory is owned by the front.
    pub owns_bits: bool,
    /// Size of the allocation in `surf.bits` (if `owns_bits`).
    pub alloc_size: u32,
    pub lock_count: u32,
    /// HDC from GetDC (0 = none).
    pub hdc: u32,
    /// Owning IDirectDraw interface pointer.
    pub dd: u32,
}

impl Surface {
    pub fn is_front(&self) -> bool {
        self.caps & DDSCAPS_PRIMARYSURFACE != 0 && self.caps & DDSCAPS_BACKBUFFER == 0
    }
}

/// DirectDraw module state.
#[derive(Default)]
pub struct DdState {
    /// Current display mode (w, h, bpp); `None` = not set yet ([`DEFAULT_MODE`]).
    pub mode: Option<(u32, u32, u32)>,
    pub surfaces: HashMap<u32, Surface>,
    /// Palettes: 256 PALETTEENTRY dwords each.
    pub palettes: HashMap<u32, Vec<u32>>,
    /// Clippers: HWND.
    pub clippers: HashMap<u32, u32>,
    /// The visible primary surface (latest created).
    pub primary: Option<u32>,
    pub coop_hwnd: u32,
    pub coop_flags: u32,
    /// Reused presentation buffer.
    pub fb: FrameBuffer,
    pub presents: u64,
    /// Frame limiter (see [`dd_wait_for_vertical_blank`]): time (ms, fractional) the last wait released.
    pub vblank_last_ms: f64,
    /// Guest strings for DirectDrawEnumerateA (description, name).
    enum_strings: Option<(u32, u32)>,
    /// Port extension (res-zoom): the game draws only the top-left `w x h` of the mode; the primary surface is
    /// presented cropped to it. Set by `SetDisplayMode(w, h, bpp, 0, DDSDM_PORT_LOGICAL)`, cleared by a real
    /// mode change (docs/porting/resolution-zoom.md).
    pub logical: Option<(u32, u32)>,
}

impl DdState {
    pub fn mode(&self) -> (u32, u32, u32) {
        self.mode.unwrap_or(DEFAULT_MODE)
    }
}

fn state(m: &mut Machine) -> s3_runtime::Shared<DdState> {
    m.state::<DdState>()
}

// ---------------------------------------------------------------------------------------------- registration

pub fn register(reg: &mut Registry) {
    reg.declare_all(DLL, EXPORTS);
    declare_interface(reg, DLL, "IDirectDraw", IDIRECTDRAW);
    declare_interface(reg, DLL, "IDirectDraw2", IDIRECTDRAW2);
    declare_interface(reg, DLL, "IDirectDrawSurface", IDIRECTDRAWSURFACE3);
    declare_interface(reg, DLL, "IDirectDrawPalette", IDIRECTDRAWPALETTE);
    declare_interface(reg, DLL, "IDirectDrawClipper", IDIRECTDRAWCLIPPER);

    reg.register(DLL, "DirectDrawCreate", direct_draw_create);
    reg.register(DLL, "DirectDrawEnumerateA", direct_draw_enumerate_a);
    reg.register(DLL, "DirectDrawCreateClipper", direct_draw_create_clipper);

    let dd: &[(&str, s3_runtime::Handler)] = &[
        ("QueryInterface", dd_query_interface),
        ("AddRef", Machine::com_addref),
        ("Release", Machine::com_release),
        ("Compact", dd_ok1),
        ("CreateClipper", dd_create_clipper),
        ("CreatePalette", dd_create_palette),
        ("CreateSurface", dd_create_surface),
        ("DuplicateSurface", dd_duplicate_surface),
        ("EnumDisplayModes", dd_enum_display_modes),
        ("EnumSurfaces", dd_enum_surfaces),
        ("FlipToGDISurface", dd_ok1),
        ("GetCaps", dd_get_caps),
        ("GetDisplayMode", dd_get_display_mode),
        ("GetFourCCCodes", dd_get_fourcc_codes),
        ("GetGDISurface", dd_get_gdi_surface),
        ("GetMonitorFrequency", dd_get_monitor_frequency),
        ("GetScanLine", dd_get_scan_line),
        ("GetVerticalBlankStatus", dd_get_vertical_blank_status),
        ("Initialize", dd_initialize),
        ("RestoreDisplayMode", dd_ok1),
        ("SetCooperativeLevel", dd_set_cooperative_level),
        ("WaitForVerticalBlank", dd_wait_for_vertical_blank),
    ];
    for (n, h) in dd {
        reg_method(reg, DLL, "IDirectDraw", n, *h);
        reg_method(reg, DLL, "IDirectDraw2", n, *h);
    }
    reg_method(reg, DLL, "IDirectDraw", "SetDisplayMode", dd1_set_display_mode);
    reg_method(reg, DLL, "IDirectDraw2", "SetDisplayMode", dd2_set_display_mode);
    reg_method(reg, DLL, "IDirectDraw2", "GetAvailableVidMem", dd_get_available_vid_mem);

    let s: &[(&str, s3_runtime::Handler)] = &[
        ("QueryInterface", surf_query_interface),
        ("AddRef", Machine::com_addref),
        ("Release", surf_release),
        ("AddAttachedSurface", surf_add_attached_surface),
        ("AddOverlayDirtyRect", notimpl2),
        ("Blt", surf_blt),
        ("BltBatch", notimpl4),
        ("BltFast", surf_blt_fast),
        ("DeleteAttachedSurface", surf_delete_attached_surface),
        ("EnumAttachedSurfaces", surf_enum_attached_surfaces),
        ("EnumOverlayZOrders", notimpl4),
        ("Flip", surf_flip),
        ("GetAttachedSurface", surf_get_attached_surface),
        ("GetBltStatus", dd_ok2),
        ("GetCaps", surf_get_caps),
        ("GetClipper", surf_get_clipper),
        ("GetColorKey", surf_get_color_key),
        ("GetDC", surf_get_dc),
        ("GetFlipStatus", dd_ok2),
        ("GetOverlayPosition", notimpl3),
        ("GetPalette", surf_get_palette),
        ("GetPixelFormat", surf_get_pixel_format),
        ("GetSurfaceDesc", surf_get_surface_desc),
        ("Initialize", surf_initialize),
        ("IsLost", dd_ok1),
        ("Lock", surf_lock),
        ("ReleaseDC", surf_release_dc),
        ("Restore", dd_ok1),
        ("SetClipper", surf_set_clipper),
        ("SetColorKey", surf_set_color_key),
        ("SetOverlayPosition", notimpl3),
        ("SetPalette", surf_set_palette),
        ("Unlock", surf_unlock),
        ("UpdateOverlay", notimpl6),
        ("UpdateOverlayDisplay", notimpl2),
        ("UpdateOverlayZOrder", notimpl3),
        ("GetDDInterface", surf_get_dd_interface),
        ("PageLock", dd_ok2),
        ("PageUnlock", dd_ok2),
        ("SetSurfaceDesc", surf_set_surface_desc),
    ];
    for (n, h) in s {
        reg_method(reg, DLL, "IDirectDrawSurface", n, *h);
    }

    let p: &[(&str, s3_runtime::Handler)] = &[
        ("QueryInterface", generic_qi_self),
        ("AddRef", Machine::com_addref),
        ("Release", Machine::com_release),
        ("GetCaps", pal_get_caps),
        ("GetEntries", pal_get_entries),
        ("Initialize", pal_initialize),
        ("SetEntries", pal_set_entries),
    ];
    for (n, h) in p {
        reg_method(reg, DLL, "IDirectDrawPalette", n, *h);
    }
    let c: &[(&str, s3_runtime::Handler)] = &[
        ("QueryInterface", generic_qi_self),
        ("AddRef", Machine::com_addref),
        ("Release", Machine::com_release),
        ("GetClipList", clip_get_clip_list),
        ("GetHWnd", clip_get_hwnd),
        ("Initialize", clip_initialize),
        ("IsClipListChanged", clip_is_clip_list_changed),
        ("SetClipList", notimpl3),
        ("SetHWnd", clip_set_hwnd),
    ];
    for (n, h) in c {
        reg_method(reg, DLL, "IDirectDrawClipper", n, *h);
    }
}

pub fn register_factories(m: &mut Machine) {
    m.com.register_class(CLSID_DIRECTDRAW, "DirectDraw", create_directdraw);
}

crate::stubs! {
    dd_ok1 = (1, DD_OK);
    dd_ok2 = (2, DD_OK);
    notimpl2 = (2, DDERR_UNSUPPORTED);
    notimpl3 = (3, DDERR_UNSUPPORTED);
    notimpl4 = (4, DDERR_UNSUPPORTED);
    notimpl6 = (6, DDERR_UNSUPPORTED);
}

// ---------------------------------------------------------------------------------------------- objects

/// Object layout for every DirectDraw interface object: `[vtbl][refcount][core]`, where `core` is the first
/// interface pointer of the DirectDraw object (all interfaces of one object share it).
const DD_OBJ_SIZE: u32 = 0x10;

fn new_dd_iface(m: &mut Machine, v2: bool, core: u32) -> u32 {
    let vt = if v2 {
        m.com_vtable(DLL, "IDirectDraw2", IDIRECTDRAW2)
    } else {
        m.com_vtable(DLL, "IDirectDraw", IDIRECTDRAW)
    };
    let p = m.com_new_object(vt, DD_OBJ_SIZE);
    m.mem.write_u32(p + 8, if core == 0 { p } else { core });
    p
}

/// CoCreateInstance(CLSID_DirectDraw, ...).
fn create_directdraw(m: &mut Machine, iid: Guid) -> Result<u32, u32> {
    if iid == IID_IDIRECTDRAW2 {
        Ok(new_dd_iface(m, true, 0))
    } else if iid == IID_IDIRECTDRAW || iid == IID_IUNKNOWN {
        Ok(new_dd_iface(m, false, 0))
    } else {
        m.log(LogLevel::Warn, &format!("ddraw: CoCreateInstance for unsupported {iid}"));
        Err(E_NOINTERFACE)
    }
}

/// `DirectDrawCreate(lpGUID, lplpDD, pUnkOuter)`.
fn direct_draw_create(m: &mut Machine) -> ThunkResult {
    let out = m.arg_u32(1);
    if out == 0 {
        return ret(DDERR_INVALIDPARAMS, 3);
    }
    let p = new_dd_iface(m, false, 0);
    m.mem.write_u32(out, p);
    ret(DD_OK, 3)
}

/// `DirectDrawEnumerateA(lpCallback, lpContext)`: one driver, the primary display (GUID NULL).
fn direct_draw_enumerate_a(m: &mut Machine) -> ThunkResult {
    let (cb, ctx) = (m.arg_u32(0), m.arg_u32(1));
    let st = state(m);
    let strings = st.borrow_mut().enum_strings;
    let (desc, name) = match strings {
        Some(s) => s,
        None => {
            let d = m.mem.alloc(32);
            m.mem.write_cstr(d, "Primary Display Driver");
            let n = m.mem.alloc(8);
            m.mem.write_cstr(n, "display");
            st.borrow_mut().enum_strings = Some((d, n));
            (d, n)
        }
    };
    if cb != 0 {
        m.call_guest(cb, &[0, desc, name, ctx]);
    }
    ret(DD_OK, 2)
}

/// `DirectDrawCreateClipper(dwFlags, lplpClipper, pUnkOuter)`.
fn direct_draw_create_clipper(m: &mut Machine) -> ThunkResult {
    let out = m.arg_u32(1);
    let c = new_clipper(m);
    put_u32(m, out, c);
    ret(DD_OK, 3)
}

fn new_clipper(m: &mut Machine) -> u32 {
    let vt = m.com_vtable(DLL, "IDirectDrawClipper", IDIRECTDRAWCLIPPER);
    let c = m.com_new_object(vt, 0x10);
    state(m).borrow_mut().clippers.insert(c, 0);
    c
}

/// QueryInterface that hands out the object itself for any IID (palettes, clippers).
fn generic_qi_self(m: &mut Machine) -> ThunkResult {
    let (this, out) = (m.arg_u32(0), m.arg_u32(2));
    if out == 0 {
        return ret(E_POINTER, 3);
    }
    let n = m.mem.read_u32(this + 4) + 1;
    m.mem.write_u32(this + 4, n);
    m.mem.write_u32(out, this);
    ret(S_OK, 3)
}

// ---------------------------------------------------------------------------------------------- IDirectDraw

fn dd_query_interface(m: &mut Machine) -> ThunkResult {
    let (this, riid, out) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    if out == 0 {
        return ret(E_POINTER, 3);
    }
    let iid = Guid::read(m, riid);
    let core = m.mem.read_u32(this + 8);
    let p = if iid == IID_IDIRECTDRAW2 {
        new_dd_iface(m, true, core)
    } else if iid == IID_IDIRECTDRAW {
        new_dd_iface(m, false, core)
    } else if iid == IID_IUNKNOWN {
        let n = m.mem.read_u32(this + 4) + 1;
        m.mem.write_u32(this + 4, n);
        this
    } else {
        m.log(LogLevel::Warn, &format!("ddraw: IDirectDraw::QueryInterface({iid}) -> E_NOINTERFACE"));
        m.mem.write_u32(out, 0);
        return ret(E_NOINTERFACE, 3);
    };
    m.mem.write_u32(out, p);
    ret(S_OK, 3)
}

fn dd_initialize(_m: &mut Machine) -> ThunkResult {
    // CoCreateInstance'd objects must be initialized; DirectDrawCreate ones return ALREADYINITIALIZED in
    // real DirectDraw. S3 only uses the CoCreateInstance path, so always succeed.
    ret(DD_OK, 2)
}

fn dd_set_cooperative_level(m: &mut Machine) -> ThunkResult {
    let (hwnd, flags) = (m.arg_u32(1), m.arg_u32(2));
    let st = state(m);
    let mut s = st.borrow_mut();
    s.coop_hwnd = hwnd;
    s.coop_flags = flags;
    ret(DD_OK, 3)
}

/// `IDirectDraw2::SetDisplayMode` flag (port extension, not a DirectX flag): change only the presented size
/// ("logical mode") without touching surfaces; `w x h` must fit the current mode. Used by the custom-resolution
/// zoom of s3-mods (`res.rs`) to switch sizes without the game's slow display re-setup.
pub const DDSDM_PORT_LOGICAL: u32 = 0x8000_0000;

fn set_logical(m: &mut Machine, w: u32, h: u32) -> u32 {
    let st = state(m);
    let mut s = st.borrow_mut();
    let (mw, mh, _) = s.mode();
    if w == 0 || h == 0 || w > mw || h > mh {
        return DDERR_INVALIDMODE;
    }
    s.logical = if (w, h) == (mw, mh) { None } else { Some((w, h)) };
    DD_OK
}

fn set_mode(m: &mut Machine, w: u32, h: u32, bpp: u32) -> u32 {
    // Any size (res-zoom: custom game resolutions up to 2560 wide, docs/porting/resolution-zoom.md).
    if !(matches!(bpp, 8 | 16 | 32) && (320..=4096).contains(&w) && (200..=4096).contains(&h)) {
        m.log(LogLevel::Warn, &format!("ddraw: SetDisplayMode({w}x{h}x{bpp}) rejected"));
        return DDERR_INVALIDMODE;
    }
    m.log(LogLevel::Info, &format!("ddraw: SetDisplayMode {w}x{h}x{bpp}"));
    let st = state(m);
    let mut s = st.borrow_mut();
    s.mode = Some((w, h, bpp));
    s.logical = None;
    DD_OK
}

/// IDirectDraw2::SetDisplayMode(this, w, h, bpp, refresh, flags).
fn dd2_set_display_mode(m: &mut Machine) -> ThunkResult {
    if m.arg_u32(5) & DDSDM_PORT_LOGICAL != 0 {
        let r = set_logical(m, m.arg_u32(1), m.arg_u32(2));
        return ret(r, 6);
    }
    let r = set_mode(m, m.arg_u32(1), m.arg_u32(2), m.arg_u32(3));
    ret(r, 6)
}

/// IDirectDraw::SetDisplayMode(this, w, h, bpp).
fn dd1_set_display_mode(m: &mut Machine) -> ThunkResult {
    let r = set_mode(m, m.arg_u32(1), m.arg_u32(2), m.arg_u32(3));
    ret(r, 4)
}

/// Write a DDPIXELFORMAT (32 bytes).
fn write_pixel_format(m: &mut Machine, p: u32, bpp: u32, rgb555: bool) {
    m.mem.fill(p, 32, 0);
    m.mem.write_u32(p, 32);
    let (flags, r, g, b) = match bpp {
        8 => (DDPF_RGB | DDPF_PALETTEINDEXED8, 0, 0, 0),
        16 if rgb555 => (DDPF_RGB, 0x7C00, 0x03E0, 0x001F),
        16 => (DDPF_RGB, 0xF800, 0x07E0, 0x001F),
        _ => (DDPF_RGB, 0x00FF_0000, 0x0000_FF00, 0x0000_00FF),
    };
    m.mem.write_u32(p + 4, flags);
    m.mem.write_u32(p + 0xC, bpp);
    m.mem.write_u32(p + 0x10, r);
    m.mem.write_u32(p + 0x14, g);
    m.mem.write_u32(p + 0x18, b);
}

/// Fill a DDSURFACEDESC for a mode / surface. Keeps the caller's dwSize (0x6C or 0x7C).
fn write_ddsd(m: &mut Machine, p: u32, w: u32, h: u32, pitch: u32, bpp: u32, rgb555: bool, caps: u32, lp: u32) {
    let size = m.mem.read_u32(p);
    let size = if size == 0x7C { 0x7C } else { DDSD_SIZE };
    m.mem.fill(p, size as usize, 0);
    m.mem.write_u32(p, size);
    let mut flags = DDSD_CAPS | DDSD_WIDTH | DDSD_HEIGHT | DDSD_PITCH | DDSD_PIXELFORMAT;
    if lp != 0 {
        flags |= DDSD_LPSURFACE;
    }
    m.mem.write_u32(p + 4, flags);
    m.mem.write_u32(p + 8, h);
    m.mem.write_u32(p + 0xC, w);
    m.mem.write_u32(p + 0x10, pitch);
    m.mem.write_u32(p + 0x24, lp);
    write_pixel_format(m, p + 0x48, bpp, rgb555);
    m.mem.write_u32(p + 0x68, caps);
}

fn write_mode_ddsd(m: &mut Machine, p: u32, w: u32, h: u32, bpp: u32) {
    write_ddsd(m, p, w, h, pitch_for(w, bpp), bpp, false, 0, 0);
    let f = m.mem.read_u32(p + 4) | DDSD_REFRESHRATE;
    m.mem.write_u32(p + 4, f & !DDSD_CAPS);
    m.mem.write_u32(p + 0x18, REFRESH_HZ);
}

fn pitch_for(w: u32, bpp: u32) -> u32 {
    (w * (bpp / 8).max(1) + 3) & !3
}

fn dd_get_display_mode(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(1);
    if p == 0 {
        return ret(DDERR_INVALIDPARAMS, 2);
    }
    let (w, h, bpp) = state(m).borrow_mut().mode();
    write_mode_ddsd(m, p, w, h, bpp);
    ret(DD_OK, 2)
}

/// EnumDisplayModes(this, flags, lpDDSD filter, ctx, callback). Callback: `HRESULT cb(LPDDSURFACEDESC, ctx)`,
/// returns DDENUMRET_OK (1) to continue.
fn dd_enum_display_modes(m: &mut Machine) -> ThunkResult {
    let (filter, ctx, cb) = (m.arg_u32(2), m.arg_u32(3), m.arg_u32(4));
    if cb == 0 {
        return ret(DDERR_INVALIDPARAMS, 5);
    }
    let (mut fw, mut fh, mut fbpp) = (0, 0, 0);
    if filter != 0 {
        let f = m.mem.read_u32(filter + 4);
        if f & DDSD_WIDTH != 0 {
            fw = m.mem.read_u32(filter + 0xC);
        }
        if f & DDSD_HEIGHT != 0 {
            fh = m.mem.read_u32(filter + 8);
        }
        if f & DDSD_PIXELFORMAT != 0 {
            fbpp = m.mem.read_u32(filter + 0x48 + 0xC);
        }
    }
    let buf = m.mem.alloc(DDSD_SIZE);
    'outer: for &bpp in MODE_BPPS {
        for &(w, h) in MODES {
            if (fw != 0 && fw != w) || (fh != 0 && fh != h) || (fbpp != 0 && fbpp != bpp) {
                continue;
            }
            m.mem.write_u32(buf, DDSD_SIZE);
            write_mode_ddsd(m, buf, w, h, bpp);
            if m.call_guest(cb, &[buf, ctx]) == 0 || m.is_stopped() {
                break 'outer;
            }
        }
    }
    m.mem.free(buf);
    ret(DD_OK, 5)
}

fn dd_enum_surfaces(_m: &mut Machine) -> ThunkResult {
    ret(DD_OK, 5)
}

fn dd_duplicate_surface(_m: &mut Machine) -> ThunkResult {
    ret(DDERR_UNSUPPORTED, 3)
}

/// GetCaps(this, lpDDDriverCaps, lpDDHELCaps): DDCAPS, size taken from the caller's dwSize.
fn dd_get_caps(m: &mut Machine) -> ThunkResult {
    for i in 1..=2 {
        let p = m.arg_u32(i);
        if p == 0 {
            continue;
        }
        let size = m.mem.read_u32(p).clamp(0x40, 0x17C);
        m.mem.fill(p + 4, (size - 4) as usize, 0);
        // DDCAPS_BLT | BLTCOLORFILL | BLTSTRETCH | COLORKEY | PALETTE
        m.mem.write_u32(p + 4, 0x40 | 0x0400_0000 | 0x200 | 0x0040_0000 | 0x8000);
        m.mem.write_u32(p + 0xC, 0x200); // dwCKeyCaps: DDCKEYCAPS_SRCBLT
        m.mem.write_u32(p + 0x10, 0x20 | 0x40); // dwFXCaps: stretch x/y
        m.mem.write_u32(p + 0x3C, VIDMEM);
        m.mem.write_u32(p + 0x40, VIDMEM);
    }
    ret(DD_OK, 3)
}

fn dd_get_fourcc_codes(m: &mut Machine) -> ThunkResult {
    let n = m.arg_u32(1);
    put_u32(m, n, 0);
    ret(DD_OK, 3)
}

fn dd_get_gdi_surface(m: &mut Machine) -> ThunkResult {
    let out = m.arg_u32(1);
    let prim = state(m).borrow_mut().primary;
    match prim {
        Some(p) => {
            let n = m.mem.read_u32(p + 4) + 1;
            m.mem.write_u32(p + 4, n);
            put_u32(m, out, p);
            ret(DD_OK, 2)
        }
        None => ret(DDERR_NOTFOUND, 2),
    }
}

fn dd_get_monitor_frequency(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(1);
    put_u32(m, p, REFRESH_HZ);
    ret(DD_OK, 2)
}

/// Position in the simulated 60 Hz frame: (phase 0..1, frame index).
fn frame_phase(now_ms: u64) -> (f64, u64) {
    let t = now_ms as f64 * REFRESH_HZ as f64 / 1000.0;
    (t.fract(), t as u64)
}

fn dd_get_scan_line(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(1);
    let (_, h, _) = state(m).borrow_mut().mode();
    let (ph, _) = frame_phase(m.now_ms());
    let total = h + h / 20;
    put_u32(m, p, ((ph * total as f64) as u32).min(total - 1));
    ret(DD_OK, 2)
}

fn dd_get_vertical_blank_status(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(1);
    let (ph, _) = frame_phase(m.now_ms());
    put_u32(m, p, (ph > 0.95) as u32);
    ret(DD_OK, 2)
}

/// Frame cap applied by `WaitForVerticalBlank` (frames per second; 0 = no wait at all). Default 60; env
/// `S3_FPS_CAP` overrides it at the first wait; frontends / settings may call [`set_fps_cap`].
static FPS_CAP: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(u32::MAX);

/// Set the frame cap (fps; 0 = unlimited). See [`dd_wait_for_vertical_blank`].
pub fn set_fps_cap(fps: u32) {
    FPS_CAP.store(fps, std::sync::atomic::Ordering::Relaxed);
}

/// Current frame cap (fps; 0 = unlimited).
pub fn fps_cap() -> u32 {
    use std::sync::atomic::Ordering::Relaxed;
    let v = FPS_CAP.load(Relaxed);
    if v != u32::MAX {
        return v;
    }
    let v = std::env::var("S3_FPS_CAP").ok().and_then(|s| s.parse().ok()).unwrap_or(REFRESH_HZ);
    FPS_CAP.store(v, Relaxed);
    v
}

/// WaitForVerticalBlank(this, flags, hEvent): S3's frame pacer when `WaitVBlank` = 1 (the draw thread calls it
/// once per frame before presenting).
///
/// It is a **frame limiter**, not a simulated vertical-blank grid: the wait releases `1000 / fps_cap` ms after
/// the previous release, or at once if the frame already took longer. A 60 Hz grid (the original behaviour
/// here, and real vsync) rounds every frame up to a whole number of refresh periods, so a frame needing 17 ms
/// ran at 30 fps; with the limiter it runs at ~58 fps. Rendering only; the logic thread has its own 64 ms timer
/// (docs/porting/performance.md).
fn dd_wait_for_vertical_blank(m: &mut Machine) -> ThunkResult {
    let flags = m.arg_u32(1);
    if flags & DDWAITVB_BLOCKBEGINEVENT != 0 {
        return ret(DDERR_UNSUPPORTED, 3);
    }
    let cap = fps_cap();
    let now = m.now_ms() as f64;
    let st = state(m);
    let mut s = st.borrow_mut();
    if cap == 0 {
        s.vblank_last_ms = now;
        return ret(DD_OK, 3);
    }
    let period = 1000.0 / cap as f64;
    let target = s.vblank_last_ms + period;
    if now + 0.5 >= target {
        // Late frames do not build up credit (no burst of frames after a slow one).
        s.vblank_last_ms = if now - target > period { now } else { target };
        ret(DD_OK, 3)
    } else {
        ThunkResult::Block
    }
}

fn dd_get_available_vid_mem(m: &mut Machine) -> ThunkResult {
    let (t, f) = (m.arg_u32(2), m.arg_u32(3));
    put_u32(m, t, VIDMEM);
    put_u32(m, f, VIDMEM);
    ret(DD_OK, 4)
}

fn dd_create_clipper(m: &mut Machine) -> ThunkResult {
    let out = m.arg_u32(2);
    let c = new_clipper(m);
    put_u32(m, out, c);
    ret(DD_OK, 4)
}

/// CreatePalette(this, flags, lpColorTable, lplpPalette, pUnkOuter).
fn dd_create_palette(m: &mut Machine) -> ThunkResult {
    let (flags, table, out) = (m.arg_u32(1), m.arg_u32(2), m.arg_u32(3));
    if out == 0 {
        return ret(DDERR_INVALIDPARAMS, 5);
    }
    let n = palette_entries(flags);
    let mut entries = vec![0u32; 256];
    if table != 0 {
        for (i, e) in entries.iter_mut().enumerate().take(n) {
            *e = m.mem.read_u32(table + 4 * i as u32);
        }
    }
    let vt = m.com_vtable(DLL, "IDirectDrawPalette", IDIRECTDRAWPALETTE);
    let p = m.com_new_object(vt, 0x10);
    m.mem.write_u32(p + 8, flags);
    state(m).borrow_mut().palettes.insert(p, entries);
    m.mem.write_u32(out, p);
    ret(DD_OK, 5)
}

fn palette_entries(flags: u32) -> usize {
    if flags & DDPCAPS_1BIT != 0 {
        2
    } else if flags & DDPCAPS_2BIT != 0 {
        4
    } else if flags & DDPCAPS_4BIT != 0 {
        16
    } else {
        256
    }
}

/// CreateSurface(this, lpDDSD, lplpSurface, pUnkOuter).
fn dd_create_surface(m: &mut Machine) -> ThunkResult {
    let (this, d, out) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    if d == 0 || out == 0 {
        return ret(DDERR_INVALIDPARAMS, 4);
    }
    let flags = m.mem.read_u32(d + 4);
    let req_caps = if flags & DDSD_CAPS != 0 { m.mem.read_u32(d + 0x68) } else { 0 };
    let (mw, mh, mbpp) = state(m).borrow_mut().mode();
    let primary = req_caps & DDSCAPS_PRIMARYSURFACE != 0;
    let (w, h) = if primary {
        (mw, mh)
    } else if flags & DDSD_WIDTH != 0 && flags & DDSD_HEIGHT != 0 {
        (m.mem.read_u32(d + 0xC), m.mem.read_u32(d + 8))
    } else {
        return ret(DDERR_INVALIDPARAMS, 4);
    };
    if w == 0 || h == 0 || w > 8192 || h > 8192 {
        return ret(DDERR_INVALIDPARAMS, 4);
    }
    let (mut bpp, mut rgb555) = (mbpp, false);
    if flags & DDSD_PIXELFORMAT != 0 && !primary {
        let pf = d + 0x48;
        let b = m.mem.read_u32(pf + 0xC);
        if matches!(b, 8 | 16 | 24 | 32) {
            bpp = b;
        }
        rgb555 = bpp == 16 && m.mem.read_u32(pf + 0x10) == 0x7C00;
    }
    let backs = if flags & DDSD_BACKBUFFERCOUNT != 0 && primary && req_caps & DDSCAPS_FLIP != 0 {
        m.mem.read_u32(d + 0x14).clamp(1, 3)
    } else {
        0
    };
    let mem_caps = if req_caps & DDSCAPS_SYSTEMMEMORY != 0 {
        DDSCAPS_SYSTEMMEMORY
    } else {
        DDSCAPS_VIDEOMEMORY | DDSCAPS_LOCALVIDMEM
    };
    let mut caps = req_caps & !(DDSCAPS_SYSTEMMEMORY | DDSCAPS_VIDEOMEMORY) | mem_caps;
    if primary {
        caps |= DDSCAPS_VISIBLE;
        if backs > 0 {
            caps |= DDSCAPS_FRONTBUFFER;
        }
    }
    let ck_src = (flags & DDSD_CKSRCBLT != 0).then(|| read_ck(m, d + 0x40, false));
    let ck_dst = (flags & DDSD_CKDESTBLT != 0).then(|| read_ck(m, d + 0x30, false));

    let Some(front) = new_surface(m, this, w, h, bpp, rgb555, caps) else {
        return ret(E_OUTOFMEMORY, 4);
    };
    let st = state(m);
    {
        let mut s = st.borrow_mut();
        let f = s.surfaces.get_mut(&front).unwrap();
        f.ck_src = ck_src;
        f.ck_dst = ck_dst;
    }
    if backs > 0 {
        let mut chain = vec![front];
        for i in 0..backs {
            let bc = (caps & !(DDSCAPS_PRIMARYSURFACE | DDSCAPS_VISIBLE | DDSCAPS_FRONTBUFFER))
                | DDSCAPS_FLIP
                | DDSCAPS_COMPLEX
                | if i == 0 { DDSCAPS_BACKBUFFER } else { 0 };
            let Some(b) = new_surface(m, this, w, h, bpp, rgb555, bc) else {
                return ret(E_OUTOFMEMORY, 4);
            };
            chain.push(b);
        }
        let mut s = st.borrow_mut();
        for i in 0..chain.len() {
            let next = chain[(i + 1) % chain.len()];
            let me = s.surfaces.get_mut(&chain[i]).unwrap();
            if next != front {
                me.attached.push(next);
            }
        }
        s.surfaces.get_mut(&front).unwrap().chain = chain;
    }
    if primary {
        st.borrow_mut().primary = Some(front);
    }
    m.mem.write_u32(out, front);
    m.log(
        LogLevel::Debug,
        &format!("ddraw: CreateSurface {w}x{h}x{bpp} caps={caps:#x} backs={backs} -> {front:#x}"),
    );
    ret(DD_OK, 4)
}

/// Allocate pixel memory + a surface object. Returns the interface pointer.
fn new_surface(m: &mut Machine, dd: u32, w: u32, h: u32, bpp: u32, rgb555: bool, caps: u32) -> Option<u32> {
    let pitch = pitch_for(w, bpp);
    let size = pitch * h;
    let bits = m.mem.virtual_alloc(0, size, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE).ok()?;
    let vt = m.com_vtable(DLL, "IDirectDrawSurface", IDIRECTDRAWSURFACE3);
    let p = m.com_new_object(vt, 0x10);
    let s = Surface {
        surf: Surf { bits, width: w, height: h, pitch, bpp },
        caps,
        rgb555,
        owns_bits: true,
        alloc_size: size,
        dd,
        ..Default::default()
    };
    state(m).borrow_mut().surfaces.insert(p, s);
    Some(p)
}

fn read_ck(m: &Machine, p: u32, colorspace: bool) -> ColorKey {
    let low = m.mem.read_u32(p);
    let high = if colorspace { m.mem.read_u32(p + 4) } else { low };
    ColorKey { low, high: high.max(low) }
}

// ---------------------------------------------------------------------------------------------- presentation

/// Present the visible primary surface (if any) through the host.
pub fn present_primary(m: &mut Machine) {
    let st = state(m);
    let mut s = st.borrow_mut();
    let Some(p) = s.primary else { return };
    let Some(sf) = s.surfaces.get(&p) else { return };
    let (mut surf, rgb555, pal) = (sf.surf, sf.rgb555, sf.palette);
    if let Some((lw, lh)) = s.logical {
        // Logical mode (port extension): present only the part the game draws.
        surf.width = surf.width.min(lw);
        surf.height = surf.height.min(lh);
    }
    let palette = if surf.bpp == 8 { s.palettes.get(&pal).cloned() } else { None };
    let mut fb = std::mem::take(&mut s.fb);
    blit::to_framebuffer_into(&m.mem, &surf, rgb555, palette.as_deref(), &mut fb);
    s.presents += 1;
    m.present(&fb);
    s.fb = fb;
}

fn is_visible(m: &mut Machine, surf: u32) -> bool {
    state(m).borrow_mut().primary == Some(surf)
}

// ---------------------------------------------------------------------------------------------- IDirectDrawSurface

fn surf_query_interface(m: &mut Machine) -> ThunkResult {
    let (this, riid, out) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    if out == 0 {
        return ret(E_POINTER, 3);
    }
    let iid = Guid::read(m, riid);
    if [IID_IDIRECTDRAWSURFACE, IID_IDIRECTDRAWSURFACE2, IID_IDIRECTDRAWSURFACE3, IID_IUNKNOWN].contains(&iid) {
        let n = m.mem.read_u32(this + 4) + 1;
        m.mem.write_u32(this + 4, n);
        m.mem.write_u32(out, this);
        ret(S_OK, 3)
    } else {
        m.log(LogLevel::Warn, &format!("ddraw: IDirectDrawSurface::QueryInterface({iid}) -> E_NOINTERFACE"));
        m.mem.write_u32(out, 0);
        ret(E_NOINTERFACE, 3)
    }
}

fn surf_release(m: &mut Machine) -> ThunkResult {
    let this = m.arg_u32(0);
    let n = m.mem.read_u32(this + 4).saturating_sub(1);
    m.mem.write_u32(this + 4, n);
    if n == 0 {
        destroy_surface(m, this);
    }
    ret(n, 1)
}

/// Free a surface (and the rest of its flip chain if it is the front).
fn destroy_surface(m: &mut Machine, p: u32) {
    let st = state(m);
    let mut frees = Vec::new();
    {
        let mut s = st.borrow_mut();
        let Some(sf) = s.surfaces.remove(&p) else { return };
        let mut members = vec![(p, sf)];
        let chain = members[0].1.chain.clone();
        for c in chain.into_iter().skip(1) {
            if let Some(x) = s.surfaces.remove(&c) {
                members.push((c, x));
            }
        }
        for (ptr, x) in members {
            if s.primary == Some(ptr) {
                s.primary = None;
            }
            if x.owns_bits {
                frees.push(x.surf.bits);
            }
            if x.hdc != 0 {
                // A DC left open; nothing else to do (gdi32 owns it).
            }
            if ptr != p {
                frees_obj(&mut frees, ptr);
            }
        }
    }
    for f in frees {
        if f & 1 == 1 {
            m.mem.free(f & !1);
        } else {
            let _ = m.mem.virtual_free(f, 0, MEM_RELEASE);
        }
    }
    m.mem.free(p);
}

/// Mark an object pointer for `m.mem.free` in the frees list (low bit tag; objects are 8-aligned).
fn frees_obj(frees: &mut Vec<u32>, ptr: u32) {
    frees.push(ptr | 1);
}

fn with_surface<R>(m: &mut Machine, p: u32, f: impl FnOnce(&mut Surface) -> R) -> Option<R> {
    let st = state(m);
    let mut s = st.borrow_mut();
    s.surfaces.get_mut(&p).map(f)
}

fn get_surface(m: &mut Machine, p: u32) -> Option<Surface> {
    with_surface(m, p, |s| s.clone())
}

fn surf_add_attached_surface(m: &mut Machine) -> ThunkResult {
    let (this, other) = (m.arg_u32(0), m.arg_u32(1));
    if get_surface(m, other).is_none() {
        return ret(DDERR_INVALIDPARAMS, 2);
    }
    let n = m.mem.read_u32(other + 4) + 1;
    m.mem.write_u32(other + 4, n);
    with_surface(m, this, |s| s.attached.push(other));
    ret(DD_OK, 2)
}

fn surf_delete_attached_surface(m: &mut Machine) -> ThunkResult {
    let (this, other) = (m.arg_u32(0), m.arg_u32(2));
    with_surface(m, this, |s| {
        if other == 0 {
            s.attached.clear()
        } else {
            s.attached.retain(|&a| a != other)
        }
    });
    ret(DD_OK, 3)
}

/// EnumAttachedSurfaces(this, ctx, cb): `HRESULT cb(LPDIRECTDRAWSURFACE, LPDDSURFACEDESC, ctx)`.
fn surf_enum_attached_surfaces(m: &mut Machine) -> ThunkResult {
    let (this, ctx, cb) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    let att = with_surface(m, this, |s| s.attached.clone()).unwrap_or_default();
    if cb == 0 {
        return ret(DDERR_INVALIDPARAMS, 3);
    }
    let buf = m.mem.alloc(DDSD_SIZE);
    for a in att {
        let Some(s) = get_surface(m, a) else { continue };
        m.mem.write_u32(buf, DDSD_SIZE);
        write_ddsd(m, buf, s.surf.width, s.surf.height, s.surf.pitch, s.surf.bpp, s.rgb555, s.caps, 0);
        let n = m.mem.read_u32(a + 4) + 1;
        m.mem.write_u32(a + 4, n);
        if m.call_guest(cb, &[a, buf, ctx]) == 0 {
            break;
        }
    }
    m.mem.free(buf);
    ret(DD_OK, 3)
}

/// GetAttachedSurface(this, lpDDSCaps, lplpSurface).
fn surf_get_attached_surface(m: &mut Machine) -> ThunkResult {
    let (this, pcaps, out) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    if pcaps == 0 || out == 0 {
        return ret(DDERR_INVALIDPARAMS, 3);
    }
    let want = m.mem.read_u32(pcaps);
    let st = state(m);
    let found = {
        let s = st.borrow_mut();
        s.surfaces
            .get(&this)
            .and_then(|me| me.attached.iter().copied().find(|a| s.surfaces.get(a).is_some_and(|x| x.caps & want == want)))
    };
    match found {
        Some(a) => {
            let n = m.mem.read_u32(a + 4) + 1;
            m.mem.write_u32(a + 4, n);
            m.mem.write_u32(out, a);
            ret(DD_OK, 3)
        }
        None => ret(DDERR_NOTFOUND, 3),
    }
}

fn surf_get_caps(m: &mut Machine) -> ThunkResult {
    let (this, p) = (m.arg_u32(0), m.arg_u32(1));
    let Some(caps) = with_surface(m, this, |s| s.caps) else { return ret(DDERR_INVALIDPARAMS, 2) };
    put_u32(m, p, caps);
    ret(DD_OK, 2)
}

fn surf_get_clipper(m: &mut Machine) -> ThunkResult {
    let (this, out) = (m.arg_u32(0), m.arg_u32(1));
    let c = with_surface(m, this, |s| s.clipper).unwrap_or(0);
    if c == 0 {
        return ret(DDERR_NOCLIPPERATTACHED, 2);
    }
    let n = m.mem.read_u32(c + 4) + 1;
    m.mem.write_u32(c + 4, n);
    put_u32(m, out, c);
    ret(DD_OK, 2)
}

fn surf_set_clipper(m: &mut Machine) -> ThunkResult {
    let (this, c) = (m.arg_u32(0), m.arg_u32(1));
    with_surface(m, this, |s| s.clipper = c);
    ret(DD_OK, 2)
}

fn surf_get_color_key(m: &mut Machine) -> ThunkResult {
    let (this, flags, p) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    let ck = with_surface(m, this, |s| if flags & DDCKEY_SRCBLT != 0 { s.ck_src } else { s.ck_dst }).flatten();
    match ck {
        Some(k) if p != 0 => {
            m.mem.write_u32(p, k.low);
            m.mem.write_u32(p + 4, k.high);
            ret(DD_OK, 3)
        }
        _ => ret(DDERR_NOCOLORKEY, 3),
    }
}

fn surf_set_color_key(m: &mut Machine) -> ThunkResult {
    let (this, flags, p) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    let ck = (p != 0).then(|| read_ck(m, p, flags & DDCKEY_COLORSPACE != 0));
    let ok = with_surface(m, this, |s| {
        if flags & DDCKEY_SRCBLT != 0 {
            s.ck_src = ck;
        } else if flags & DDCKEY_DESTBLT != 0 {
            s.ck_dst = ck;
        }
    });
    ret(if ok.is_some() { DD_OK } else { DDERR_INVALIDPARAMS }, 3)
}

fn surf_get_palette(m: &mut Machine) -> ThunkResult {
    let (this, out) = (m.arg_u32(0), m.arg_u32(1));
    let p = with_surface(m, this, |s| s.palette).unwrap_or(0);
    if p == 0 {
        return ret(DDERR_NOPALETTEATTACHED, 2);
    }
    let n = m.mem.read_u32(p + 4) + 1;
    m.mem.write_u32(p + 4, n);
    put_u32(m, out, p);
    ret(DD_OK, 2)
}

fn surf_set_palette(m: &mut Machine) -> ThunkResult {
    let (this, p) = (m.arg_u32(0), m.arg_u32(1));
    let r = with_surface(m, this, |s| {
        s.palette = p;
        s.surf.bpp == 8
    });
    if r == Some(true) && p != 0 && is_visible(m, this) {
        present_primary(m);
    }
    ret(DD_OK, 2)
}

fn surf_get_pixel_format(m: &mut Machine) -> ThunkResult {
    let (this, p) = (m.arg_u32(0), m.arg_u32(1));
    let Some(s) = get_surface(m, this) else { return ret(DDERR_INVALIDPARAMS, 2) };
    if p == 0 {
        return ret(DDERR_INVALIDPARAMS, 2);
    }
    write_pixel_format(m, p, s.surf.bpp, s.rgb555);
    ret(DD_OK, 2)
}

fn surf_get_surface_desc(m: &mut Machine) -> ThunkResult {
    let (this, p) = (m.arg_u32(0), m.arg_u32(1));
    let Some(s) = get_surface(m, this) else { return ret(DDERR_INVALIDPARAMS, 2) };
    if p == 0 {
        return ret(DDERR_INVALIDPARAMS, 2);
    }
    write_ddsd(m, p, s.surf.width, s.surf.height, s.surf.pitch, s.surf.bpp, s.rgb555, s.caps, 0);
    if s.chain.len() > 1 {
        let f = m.mem.read_u32(p + 4) | DDSD_BACKBUFFERCOUNT;
        m.mem.write_u32(p + 4, f);
        m.mem.write_u32(p + 0x14, s.chain.len() as u32 - 1);
    }
    ret(DD_OK, 2)
}

fn surf_initialize(_m: &mut Machine) -> ThunkResult {
    ret(DDERR_ALREADYINITIALIZED, 3)
}

fn surf_get_dd_interface(m: &mut Machine) -> ThunkResult {
    let (this, out) = (m.arg_u32(0), m.arg_u32(1));
    let dd = with_surface(m, this, |s| s.dd).unwrap_or(0);
    if dd != 0 {
        let n = m.mem.read_u32(dd + 4) + 1;
        m.mem.write_u32(dd + 4, n);
    }
    put_u32(m, out, dd);
    ret(DD_OK, 2)
}

/// SetSurfaceDesc(this, lpDDSD, flags): only lpSurface/size/pitch changes (client memory).
fn surf_set_surface_desc(m: &mut Machine) -> ThunkResult {
    let (this, d) = (m.arg_u32(0), m.arg_u32(1));
    if d == 0 {
        return ret(DDERR_INVALIDPARAMS, 3);
    }
    let flags = m.mem.read_u32(d + 4);
    let (w, h, pitch, lp) = (m.mem.read_u32(d + 0xC), m.mem.read_u32(d + 8), m.mem.read_u32(d + 0x10), m.mem.read_u32(d + 0x24));
    let old = with_surface(m, this, |s| {
        let old = s.owns_bits.then_some(s.surf.bits);
        if flags & DDSD_LPSURFACE != 0 {
            s.surf.bits = lp;
            s.owns_bits = false;
        }
        if flags & DDSD_WIDTH != 0 {
            s.surf.width = w;
        }
        if flags & DDSD_HEIGHT != 0 {
            s.surf.height = h;
        }
        if flags & DDSD_PITCH != 0 {
            s.surf.pitch = pitch;
        }
        if flags & DDSD_LPSURFACE != 0 {
            old
        } else {
            None
        }
    });
    if let Some(Some(b)) = old {
        let _ = m.mem.virtual_free(b, 0, MEM_RELEASE);
    }
    ret(DD_OK, 3)
}

/// Lock(this, lpDestRect, lpDDSD, flags, hEvent).
fn surf_lock(m: &mut Machine) -> ThunkResult {
    let (this, prect, d) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    let Some(s) = with_surface(m, this, |s| {
        s.lock_count += 1;
        s.clone()
    }) else {
        return ret(DDERR_INVALIDPARAMS, 5);
    };
    if d == 0 {
        return ret(DDERR_INVALIDPARAMS, 5);
    }
    let mut lp = s.surf.bits;
    if prect != 0 {
        let r = Rect::read(&m.mem, prect);
        let r = r.intersect(&s.surf.full());
        lp = lp.wrapping_add(r.t.max(0) as u32 * s.surf.pitch + r.l.max(0) as u32 * s.surf.bypp());
    }
    write_ddsd(m, d, s.surf.width, s.surf.height, s.surf.pitch, s.surf.bpp, s.rgb555, s.caps, lp);
    ret(DD_OK, 5)
}

fn surf_unlock(m: &mut Machine) -> ThunkResult {
    let this = m.arg_u32(0);
    let r = with_surface(m, this, |s| {
        let was = s.lock_count;
        s.lock_count = 0;
        was
    });
    match r {
        None => ret(DDERR_INVALIDPARAMS, 2),
        Some(_) => {
            if is_visible(m, this) {
                present_primary(m);
            }
            ret(DD_OK, 2)
        }
    }
}

fn surf_get_dc(m: &mut Machine) -> ThunkResult {
    let (this, out) = (m.arg_u32(0), m.arg_u32(1));
    let Some(s) = get_surface(m, this) else { return ret(DDERR_INVALIDPARAMS, 2) };
    if out == 0 {
        return ret(DDERR_INVALIDPARAMS, 2);
    }
    let t = DcTarget {
        addr: s.surf.bits,
        pitch: s.surf.pitch,
        width: s.surf.width,
        height: s.surf.height,
        bpp: s.surf.bpp,
        rgb565: !s.rgb555,
    };
    let hdc = gdi::create_dc(m, t);
    if hdc == 0 {
        m.log(LogLevel::Warn, "ddraw: GetDC failed (no gdi32 DC hook installed)");
        return ret(DDERR_CANTCREATEDC, 2);
    }
    with_surface(m, this, |s| s.hdc = hdc);
    m.mem.write_u32(out, hdc);
    ret(DD_OK, 2)
}

fn surf_release_dc(m: &mut Machine) -> ThunkResult {
    let (this, hdc) = (m.arg_u32(0), m.arg_u32(1));
    if hdc != 0 {
        gdi::release_dc(m, hdc);
    }
    with_surface(m, this, |s| s.hdc = 0);
    if is_visible(m, this) {
        present_primary(m);
    }
    ret(DD_OK, 2)
}

/// Blt(this, lpDestRect, lpDDSrcSurface, lpSrcRect, dwFlags, lpDDBltFx).
fn surf_blt(m: &mut Machine) -> ThunkResult {
    let (this, pdr, src, psr, flags, fx) =
        (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_u32(3), m.arg_u32(4), m.arg_u32(5));
    let Some(d) = get_surface(m, this) else { return ret(DDERR_INVALIDPARAMS, 6) };
    let drect = if pdr != 0 { Rect::read(&m.mem, pdr) } else { d.surf.full() };
    let rop = if flags & DDBLT_ROP != 0 && fx != 0 { m.mem.read_u32(fx + 8) } else { 0 };
    if flags & DDBLT_COLORFILL != 0 {
        if fx == 0 {
            return ret(DDERR_INVALIDPARAMS, 6);
        }
        let c = m.mem.read_u32(fx + 0x50);
        blit::fill(&mut m.mem, &d.surf, drect, c);
    } else if flags & DDBLT_DEPTHFILL != 0 {
        // no z-buffers
    } else if rop == ROP_BLACKNESS || rop == ROP_WHITENESS {
        let c = if rop == ROP_BLACKNESS { 0 } else { u32::MAX };
        blit::fill(&mut m.mem, &d.surf, drect, c);
    } else if src != 0 {
        let Some(s) = get_surface(m, src) else { return ret(DDERR_INVALIDPARAMS, 6) };
        let srect = if psr != 0 { Rect::read(&m.mem, psr) } else { s.surf.full() };
        let mut o = BltOpts::default();
        if flags & DDBLT_KEYSRCOVERRIDE != 0 && fx != 0 {
            o.src_key = Some(read_ck(m, fx + 0x5C, true));
        } else if flags & DDBLT_KEYSRC != 0 {
            o.src_key = s.ck_src;
        }
        if flags & DDBLT_KEYDESTOVERRIDE != 0 && fx != 0 {
            o.dst_key = Some(read_ck(m, fx + 0x54, true));
        } else if flags & DDBLT_KEYDEST != 0 {
            o.dst_key = d.ck_dst;
        }
        if flags & DDBLT_DDFX != 0 && fx != 0 {
            let ddfx = m.mem.read_u32(fx + 4);
            o.mirror_lr = ddfx & DDBLTFX_MIRRORLEFTRIGHT != 0;
            o.mirror_ud = ddfx & DDBLTFX_MIRRORUPDOWN != 0;
        }
        blit::blt(&mut m.mem, &d.surf, drect, &s.surf, srect, &o);
    } else {
        return ret(DDERR_INVALIDPARAMS, 6);
    }
    if is_visible(m, this) {
        present_primary(m);
    }
    ret(DD_OK, 6)
}

/// BltFast(this, x, y, lpDDSrcSurface, lpSrcRect, dwTrans).
fn surf_blt_fast(m: &mut Machine) -> ThunkResult {
    let (this, x, y, src, psr, trans) =
        (m.arg_u32(0), m.arg_i32(1), m.arg_i32(2), m.arg_u32(3), m.arg_u32(4), m.arg_u32(5));
    let (Some(d), Some(s)) = (get_surface(m, this), get_surface(m, src)) else {
        return ret(DDERR_INVALIDPARAMS, 6);
    };
    let srect = if psr != 0 { Rect::read(&m.mem, psr) } else { s.surf.full() };
    let drect = Rect::new(x, y, x + srect.w(), y + srect.h());
    let o = BltOpts {
        src_key: if trans & DDBLTFAST_SRCCOLORKEY != 0 { s.ck_src } else { None },
        dst_key: if trans & DDBLTFAST_DESTCOLORKEY != 0 { d.ck_dst } else { None },
        ..Default::default()
    };
    blit::blt(&mut m.mem, &d.surf, drect, &s.surf, srect, &o);
    if is_visible(m, this) {
        present_primary(m);
    }
    ret(DD_OK, 6)
}

/// Flip(this, lpDDSurfaceTargetOverride, flags): rotate the pixel memory through the chain, then present.
fn surf_flip(m: &mut Machine) -> ThunkResult {
    let this = m.arg_u32(0);
    let st = state(m);
    {
        let mut s = st.borrow_mut();
        let chain = match s.surfaces.get(&this) {
            Some(f) if f.chain.len() > 1 => f.chain.clone(),
            _ => return ret(DDERR_NOTFOUND, 3),
        };
        let bits: Vec<u32> = chain.iter().map(|c| s.surfaces[c].surf.bits).collect();
        let n = chain.len();
        for i in 0..n {
            s.surfaces.get_mut(&chain[i]).unwrap().surf.bits = bits[(i + 1) % n];
        }
    }
    if is_visible(m, this) {
        present_primary(m);
    }
    ret(DD_OK, 3)
}

// ---------------------------------------------------------------------------------------------- palette / clipper

fn pal_get_caps(m: &mut Machine) -> ThunkResult {
    let (this, p) = (m.arg_u32(0), m.arg_u32(1));
    let f = m.mem.read_u32(this + 8);
    put_u32(m, p, f);
    ret(DD_OK, 2)
}

/// GetEntries(this, flags, base, count, lpEntries).
fn pal_get_entries(m: &mut Machine) -> ThunkResult {
    let (this, base, n, p) = (m.arg_u32(0), m.arg_u32(2), m.arg_u32(3), m.arg_u32(4));
    let e = state(m).borrow_mut().palettes.get(&this).cloned();
    let Some(e) = e else { return ret(DDERR_INVALIDPARAMS, 5) };
    for i in 0..n {
        if let Some(v) = e.get((base + i) as usize) {
            m.mem.write_u32(p + 4 * i, *v);
        }
    }
    ret(DD_OK, 5)
}

fn pal_set_entries(m: &mut Machine) -> ThunkResult {
    let (this, base, n, p) = (m.arg_u32(0), m.arg_u32(2), m.arg_u32(3), m.arg_u32(4));
    let vals: Vec<u32> = (0..n).map(|i| m.mem.read_u32(p + 4 * i)).collect();
    let st = state(m);
    let affects_primary = {
        let mut s = st.borrow_mut();
        let Some(e) = s.palettes.get_mut(&this) else { return ret(DDERR_INVALIDPARAMS, 5) };
        for (i, v) in vals.into_iter().enumerate() {
            if let Some(x) = e.get_mut(base as usize + i) {
                *x = v;
            }
        }
        s.primary.and_then(|pr| s.surfaces.get(&pr)).is_some_and(|x| x.palette == this && x.surf.bpp == 8)
    };
    if affects_primary {
        present_primary(m);
    }
    ret(DD_OK, 5)
}

fn pal_initialize(_m: &mut Machine) -> ThunkResult {
    ret(DDERR_ALREADYINITIALIZED, 4)
}

fn clip_get_clip_list(_m: &mut Machine) -> ThunkResult {
    ret(DDERR_NOCLIPLIST, 4)
}

fn clip_get_hwnd(m: &mut Machine) -> ThunkResult {
    let (this, p) = (m.arg_u32(0), m.arg_u32(1));
    let h = state(m).borrow_mut().clippers.get(&this).copied().unwrap_or(0);
    put_u32(m, p, h);
    ret(DD_OK, 2)
}

fn clip_initialize(_m: &mut Machine) -> ThunkResult {
    ret(DDERR_ALREADYINITIALIZED, 3)
}

fn clip_is_clip_list_changed(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(1);
    put_u32(m, p, 0);
    ret(DD_OK, 2)
}

fn clip_set_hwnd(m: &mut Machine) -> ThunkResult {
    let (this, hwnd) = (m.arg_u32(0), m.arg_u32(2));
    state(m).borrow_mut().clippers.insert(this, hwnd);
    ret(DD_OK, 3)
}
