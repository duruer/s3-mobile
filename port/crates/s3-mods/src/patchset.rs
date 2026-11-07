//! Per-exe patch sets. A patch set (hooks + byte patches) is applied only when the S3.EXE sha256 matches;
//! any other exe runs unpatched (logged). The generic framework (input gestures, overlay, settings) is
//! exe-independent and always active.

use s3_runtime::{HookFlow, LogLevel, Machine};

use crate::hooks;

/// GOG "Ultimate Collection" S3.EXE (docs/game-files/inventory.md).
pub const GOG_SHA256: &str = "00a925cfdcfc3349b5d0f3b8a4c312f01b813e363b4464f6f3d6de17804d950b";

/// A named set of patches for one exact exe build.
pub struct PatchSet {
    pub name: &'static str,
    pub exe_sha256: &'static str,
    pub apply: fn(&mut Machine) -> Result<(), String>,
}

/// Patch sets shipped with the port. W2/W3/W4 add their hooks to the GOG set (or add sets).
pub fn builtin() -> Vec<PatchSet> {
    vec![PatchSet { name: "gog-base", exe_sha256: GOG_SHA256, apply: gog_base }]
}

/// Patch set for `sha256`, if any.
pub fn find(sha256: &str) -> Option<PatchSet> {
    builtin().into_iter().find(|p| p.exe_sha256.eq_ignore_ascii_case(sha256))
}

/// Write `new` at `addr` after checking the original bytes are `expect` (guards against wrong exe/offsets).
/// Invalidates decoded code in every thread.
pub fn patch_bytes(m: &mut Machine, addr: u32, expect: &[u8], new: &[u8]) -> Result<(), String> {
    let cur = m.mem.read_bytes(addr, expect.len());
    if cur != expect {
        return Err(format!("patch at {addr:#x}: expected {expect:02x?}, found {cur:02x?}"));
    }
    m.mem.write_bytes(addr, new);
    m.invalidate_code(addr, new.len().max(expect.len()) as u32);
    Ok(())
}

/// GOG S3.EXE main-thread message-loop functions (docs/reverse-engineering/static-startup.md §3), used as
/// safe points for deferred host->guest calls:
/// - `0x60d9e0` "message waiting?" test, called once per iteration of `Run` (`0x404860`);
/// - `0x60d740` one GetMessage/Translate/Dispatch step.
/// HYPOTHESIS: both run only on the main thread, between game-logic steps; verified only by hit counts.
pub const GOG_SAFE_POINTS: [u32; 2] = [0x0060_d9e0, 0x0060_d740];

fn gog_base(m: &mut Machine) -> Result<(), String> {
    // Sanity: the hooked addresses must hold code (first bytes of these functions, from the GOG exe).
    for a in GOG_SAFE_POINTS {
        if m.mem.read_bytes(a, 4).iter().all(|&b| b == 0) {
            return Err(format!("gog-base: no code at {a:#x}"));
        }
    }
    for a in GOG_SAFE_POINTS {
        hooks::add_hook(m, a, "gog-base:safe-point", |_| HookFlow::Continue);
        crate::add_safe_point(m, a);
    }
    m.log(LogLevel::Info, "mods: gog-base: safe points 0x60d9e0, 0x60d740");
    // Camera clamp 0x51b4d0: the y limit `9*W + 0x176 - mapW` uses the map view WIDTH (game bug, harmless at
    // 640/800 px). With our wide custom resolutions (~2000 px) it shrinks the y range, on small maps (tutorial) to
    // the top half or nothing. Use the view HEIGHT [0x730f1c] instead (docs/reverse-engineering/game-state-ui.md).
    // View-only state (camera), not simulation: lockstep MP is unaffected.
    patch_bytes(m, 0x0051_b528, &[0x8b, 0x3d, 0x18, 0x0f, 0x73, 0x00], &[0x8b, 0x3d, 0x1c, 0x0f, 0x73, 0x00])?;
    m.log(LogLevel::Info, "mods: gog-base: camera y clamp uses the view height (0x51b528)");
    Ok(())
}
