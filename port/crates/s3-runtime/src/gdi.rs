//! Device-context bridge between DirectDraw (s3-dx) and GDI (s3-win32 gdi32).
//!
//! `IDirectDrawSurface::GetDC` must return an HDC whose drawing (TextOutA, DrawTextA, LineTo, ...) lands in the
//! surface pixels. s3-dx describes the surface memory with a [`DcTarget`] and calls [`create_dc`]; gdi32
//! (s3-win32) installs the implementation with [`install`] at registration time. `ReleaseDC` on the surface
//! calls [`release_dc`]. Without gdi32 installed, `create_dc` returns 0.

use crate::Machine;

/// Pixel memory a DC draws into (guest memory, top-down rows).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DcTarget {
    /// Guest address of the first (top) row.
    pub addr: u32,
    /// Bytes per row.
    pub pitch: u32,
    pub width: u32,
    pub height: u32,
    /// Bits per pixel: 16 (RGB565/RGB555), 24, 32 (and 8, drawn as palette index, rarely used).
    pub bpp: u32,
    /// For 16 bpp: true = RGB565 (F800/07E0/001F), false = RGB555 (7C00/03E0/001F).
    pub rgb565: bool,
}

/// Functions installed by gdi32.
#[derive(Clone, Copy)]
pub struct DcHooks {
    /// Create a DC drawing into `target`; returns the HDC (0 on failure).
    pub create: fn(&mut Machine, DcTarget) -> u32,
    /// Destroy a DC created by `create`.
    pub release: fn(&mut Machine, u32),
}

static HOOKS: std::sync::OnceLock<DcHooks> = std::sync::OnceLock::new();

/// Install the gdi32 implementation (called by `s3_win32::gdi32::register`; process-global, first call wins).
pub fn install(hooks: DcHooks) {
    let _ = HOOKS.set(hooks);
}

/// Create an HDC drawing into `target` (0 if no GDI implementation is installed).
pub fn create_dc(m: &mut Machine, target: DcTarget) -> u32 {
    match HOOKS.get() {
        Some(h) => (h.create)(m, target),
        None => 0,
    }
}

/// Release an HDC obtained from [`create_dc`].
pub fn release_dc(m: &mut Machine, hdc: u32) {
    if let Some(h) = HOOKS.get() {
        (h.release)(m, hdc)
    }
}
