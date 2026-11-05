//! Native fast paths for hot guest loops (perf-cpu, docs/porting/performance.md).
//!
//! Each fast path replaces a fixed region of the GOG S3.EXE code with Rust that leaves *exactly* the same guest
//! state (registers, EFLAGS, memory, EIP, retired-instruction count) as the original code, so it is safe for
//! lockstep multiplayer and invisible to the `s3det` state hashes. The CPU runs it at the start of the region
//! (`s3_cpu::NativeBlock`, matched by a hash of the code bytes) instead of exiting to the host like a hook, so it
//! costs nothing per call. Disable with `S3_FAST=0`.

use s3_cpu::{NativeBlock, NativeBlocks};
use s3_runtime::{LogLevel, Machine};

use crate::{fast_blit, fast_land, fast_obj, fast_snap, fast_sprite};

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
        nb("snap_tiles", fast_snap::START, fast_snap::EXIT, fast_snap::HASH, fast_snap::snap_tiles),
    ]
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
    for (start, end, b) in gog_blocks() {
        if let Some(h) = hooks.iter().find(|&&h| h > start && h < end) {
            m.log(LogLevel::Warn, &format!("mods: fast path {} skipped: hook at {h:#x} inside it", b.name));
            continue;
        }
        names.push(b.name);
        map.insert(start, b);
    }
    m.log(LogLevel::Info, &format!("mods: native fast paths: {}", names.join(", ")));
    m.set_native_blocks(Some(map));
}
