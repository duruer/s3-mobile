//! Native fast paths for hot guest loops (perf-cpu, docs/porting/performance.md).
//!
//! Each fast path replaces a fixed region of the GOG S3.EXE code with Rust that leaves *exactly* the same guest
//! state (registers, EFLAGS, memory, EIP, retired-instruction count) as the original code, so it is safe for
//! lockstep multiplayer and invisible to the `s3det` state hashes. The CPU runs it at the start of the region
//! (`s3_cpu::NativeBlock`, matched by a hash of the code bytes) instead of exiting to the host like a hook, so it
//! costs nothing per call. Disable with `S3_FAST=0`.

use s3_cpu::{NativeBlock, NativeBlocks};
use s3_runtime::{LogLevel, Machine};

use crate::{fast_blit, fast_land, fast_obj, fast_shadow, fast_snap, fast_sprite, fast_tile};

/// All fast paths for the GOG exe: (start, end, block).
pub fn gog_blocks() -> Vec<(u32, u32, NativeBlock)> {
    fn nb(name: &'static str, start: u32, end: u32, hash: u64, f: s3_cpu::NativeFn) -> (u32, u32, NativeBlock) {
        (start, end, NativeBlock { name, len: end - start, hash, f })
    }
    vec![
        nb("sprite_rows", fast_sprite::START, fast_sprite::EXIT, fast_sprite::HASH, fast_sprite::sprite_rows),
        nb("palette_row", fast_blit::PAL_START, fast_blit::PAL_EXIT, fast_blit::PAL_HASH, fast_blit::palette_row),
        nb("darken", fast_blit::DARK_START, fast_blit::DARK_EXIT, fast_blit::DARK_HASH, fast_blit::darken),
        nb("dither", fast_blit::DITHER_START, fast_blit::DITHER_EXIT, fast_blit::DITHER_HASH, fast_blit::dither),
        nb("obj_empty", fast_obj::START, fast_obj::END, fast_obj::HASH, fast_obj::obj_empty),
        nb("land_clean", fast_land::START, fast_land::END, fast_land::HASH, fast_land::land_clean),
        // shadow sprites (fast_shadow.rs): one function per loop, several entries (each hash covers entry..exit)
        nb("shadow_zero", fast_shadow::ZERO_ROW, fast_shadow::ZERO_EXIT, fast_shadow::ZERO_ROW_HASH, fast_shadow::shadow_zero),
        nb("shadow_zero_run", fast_shadow::ZERO_RUN, fast_shadow::ZERO_EXIT, fast_shadow::ZERO_RUN_HASH, fast_shadow::shadow_zero),
        nb("shadow_rows", fast_shadow::ROWS_FIRST, fast_shadow::ROWS_EXIT, fast_shadow::ROWS_FIRST_HASH, fast_shadow::shadow_rows),
        nb("shadow_rows_row", fast_shadow::ROWS_ROW, fast_shadow::ROWS_EXIT, fast_shadow::ROWS_ROW_HASH, fast_shadow::shadow_rows),
        nb("shadow_rows_run", fast_shadow::ROWS_RUN, fast_shadow::ROWS_EXIT, fast_shadow::ROWS_RUN_HASH, fast_shadow::shadow_rows),
        nb("snap_tiles", fast_snap::START, fast_snap::EXIT, fast_snap::HASH, fast_snap::snap_tiles),
        nb("frame_tiles", fast_tile::START, fast_tile::END, fast_tile::HASH, fast_tile::frame_tiles),
        nb("land_mark", fast_tile::MARK_START, fast_tile::MARK_EXIT, fast_tile::MARK_HASH, fast_tile::land_mark),
    ]
}

/// Code outside its registered region that a fast path also stands for (inlined calls, a loop head before the
/// region): a hook anywhere in these ranges (start included) disables it.
fn extra_ranges(name: &str) -> Vec<(u32, u32)> {
    match name {
        "frame_tiles" => vec![fast_tile::HEAD, (fast_obj::START, fast_obj::END), (0x005f_c540, 0x005f_c54b)],
        _ => Vec::new(),
    }
}

/// Install the fast paths for the GOG exe (after the hooks are committed: a region containing a hook address
/// is skipped, the hook must run).
pub fn install(m: &mut Machine, exe_sha256: &str) {
    if std::env::var("S3_FAST").map(|v| v == "0").unwrap_or(false) {
        m.log(LogLevel::Info, "mods: native fast paths disabled (S3_FAST=0)");
        return;
    }
    if exe_sha256 != crate::patchset::GOG_SHA256 {
        return;
    }
    let hooks: Vec<u32> = m.hook_addrs().map(|h| h.iter().copied().collect()).unwrap_or_default();
    let mut map = NativeBlocks::new();
    let mut names = Vec::new();
    // `S3_FAST_OFF=name,name`: leave single fast paths out (bisecting a divergence, measuring one path).
    let off = std::env::var("S3_FAST_OFF").unwrap_or_default();
    for (start, end, b) in gog_blocks() {
        if off.split(',').any(|n| n.trim() == b.name) {
            m.log(LogLevel::Info, &format!("mods: fast path {} disabled (S3_FAST_OFF)", b.name));
            continue;
        }
        let extra = extra_ranges(b.name);
        if let Some(h) = hooks.iter().find(|&&h| (h > start && h < end) || extra.iter().any(|&(a, e)| h >= a && h < e)) {
            m.log(LogLevel::Warn, &format!("mods: fast path {} skipped: hook at {h:#x} inside it", b.name));
            continue;
        }
        names.push(b.name);
        map.insert(start, b);
    }
    m.log(LogLevel::Info, &format!("mods: native fast paths: {}", names.join(", ")));
    m.set_native_blocks(Some(map));
}
