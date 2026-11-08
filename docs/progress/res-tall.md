# Progress: res-tall (zoom out further: heights above 780)

Goal: raise `res::MAX_H` (780) to >= ~1200 (and maybe `MAX_W` > 2560) by patching the render snapshot grid row
stride (130) everywhere, plus every fixed-size buffer a bigger view overflows. Deterministic for MP.
Branch: worktree-agent-acca53771b1483890 (worktree of main c3cec1d). Scratch: `scratch/res-tall/` (local only).

## Status
- 2026-11-08 start: read resolution-zoom.md, res.rs. Grid index = `row + col*130` computed as
  `(col<<6 + col)*2` (`0x52240f`, `0x522f66`), constants 0x82 / 0x186 / 0x1860 (`0x4b275b..0x4b27af`).
  Native fast paths (fast_land/obj/snap/tile.rs) hard-code 130 and hash the code -> must follow the patch.
- In progress: TEMP memtrace instrumentation (s3-cpu/src/memtrace.rs + hit() in fast.rs `ea`, exec.rs
  `ea_noseg`, env `S3_MEMTRACE` in res.rs switch_hook) to list every instruction that touches the grid.
  MUST be removed before commit.

- 2026-11-08 trace done: there are **3 snapshots** (triple buffer: render snap != the one the logic fills); trace
  with `S3_MEMTRACE=abs S3_MEMTRACE_RANGE=1a08f920,1ac8ecc0` (heap addresses stable between runs). Result
  `scratch/res-tall/trace_hud_ins.txt` (classified by `tools/res-tall/map_trace.py s3.asm trace 187660 <3 bases>`):
  210 grid sites in functions 0x4a9960, 0x4abd.., 0x4b26c0 (frame renderer), 0x4c86.., 0x4c92.., 0x4cc4.., 0x4cf3..,
  0x4d3050 (obj drawer), 0x4d5a.., 0x51f110 (fill), 0x54cbd0 (rasteriser). Header arrays are player-sized, not rows.
- Latent bug found: `0x51f1f8 mov eax,0xc636c` (+0x254 = tail 0xc65c0) is a tail reference the tail-site list
  misses (branch `[gd+0x2d284]==7 && [gd+0x2d298]==2`). Add to patch.
- `tools/res-tall/stride_scan.py` lists candidate stride constants (k*0x1860+d, k*130+d, shl 6, imul).

- 2026-11-08 RE done (static review of all 7 grid functions + scans): stride sites = 10 `shl r,6` (x65 of
  x130 -> shl 7 = x258), imm 0x186/0x1860/0x82 (0x4b275b/0x4b279f/0x4b27af), 29 column-neighbour displacements
  (0x4b2ff6, 0x4b3003, 12 in 0x4c85a0, 15 in rasteriser 0x54cbd0). Extra tail sites missed before: 0x51f1f8
  (0xc636c), unwind funclets 0x642cdc/0x642ceb/0x642d02. Sprite lists: bucket = 2*row, 250 buckets, arrays in gfx
  (alloc `push 0x5f700` 0x42c6b9): 62 array refs + 12 bound/init consts -> relocate arrays to gfx+0x5f700 (bigger
  alloc). Tables in `scratch/res-tall/sprite_sites.txt` (offsets via `tools/res-tall/sites.py`).
- Plan: SNAP_ROWS 258, SNAP_COLS 320, MAX_H 1200, MAX_W 3200, buckets 360. Fast paths need stride const + new
  hashes; fast_tests must load the patched code.

- 2026-11-08 IMPLEMENTED (commit 27632af WIP): res.rs `code_patches()` (tail + stride + sprite sites + gfx size),
  `apply_code_patches(mem)` (fast_tests load patched code), fast paths use `res::SNAP_ROWS`/`SNAP_COL_BYTES_NEW`,
  new hashes, test `fast_path_hashes_match_patched_code`. zoom LEVEL_HEIGHTS +840..1200. All s3-mods tests pass
  with `S3_EXE=.../S3.EXE` (fast_tests are silently skipped without it in a worktree!).
- In-game 3072x1200 HUD (S3_RES): full landscape, objects in bottom rows, right/left edges OK (scratch c_*.png).
  Lists at 3072x1200 start scene: sprite 728/4000 751/8000 627/4000, hits 22/10000 (S3_RES_TRACE "res: lists").

- 2026-11-08: memtrace removed (s3-cpu identical to base). s3det 90000 x2 + S3_FAST=0: IDENTICAL
  (`scratch/res-tall/det`, `tools/res-tall/det3.sh`); hashes differ from main's s3det from vms 1000 (code bytes +
  heap layout), guest bytes 64.9 -> 81.3 MB. MP mp1 (Classic, both peers pinch 640x480..1600x1200): IDENTICAL 1226
  ticks. Docs: resolution-zoom.md section "Taller views (res-tall)", AGENTS/touch/game-state-ui limits. Line endings:
  python text-mode writes made CRLF -> fixed back to LF (check `git ls-files --eol`).
- fps run: `tools/res-tall/fps_sizes.ps1` -> scratch/res-tall/fps/.

- 2026-11-08 FINAL: fps (desktop HUD uncapped) 1536x600 440, 2000x780 345, 2304x900 274, 2664x1040 235,
  3072x1200 176; same-size cost vs main within random-map noise. Docs + journal done.

## Next (open)
- Phone fps of the new top levels (expect < 60 fps at 3072x1200); maybe cap the HUD top level per device.
- Display mode = top level of the aspect (3072x1200 surfaces on the phone) even at small zooms: memory only.
- Sprite pools (4000/8000/4000 nodes) and hit list (10000) unchanged: watch `S3_RES_TRACE` "res: lists" in a
  late-game scene at 3072x1200; overflow drops sprites / clickability, no crash.
- Merge: res.rs MAX_W/MAX_H/SNAP_* consts; zoom.rs LEVEL_HEIGHTS top end + 3 test lines.

## How to verify
- `S3_EXE=<GOG S3.EXE> cargo test --release -p s3-mods` (fast_tests skip silently without S3_EXE in a worktree).
- `tools/res-tall/det3.sh <s3det> <game dir> <out>`: run1 = run2 = S3_FAST=0.
- `S3_RES=3072x1200 s3run --headless --display native --window 2316x904 ...` + `t:shot`.
- Run trace, group sites per function, RE each function for stride arithmetic, choose new stride.
