# perf-cpu (workflow wf_a446e364-b12, 2026-11-05) — FINAL

Goal: >= 60 fps on the phone (target 60-120). Owner: s3-cpu, s3-mods/src/fast*.rs, ddraw pacing, runtime scheduler.
Full write-up: docs/porting/performance.md.

## Done
- Diagnosis: 60 Hz vblank grid quantised frames (17 ms -> 30 fps); then CPU bound (~2.5-2.9 M guest instr/frame).
- s3-dx ddraw: WaitForVerticalBlank = frame limiter, cap default 60, `S3_FPS_CAP`, `ddraw::set_fps_cap/fps_cap`.
- s3-cpu: Op records (pre-decoded operands, EA const generics), 8/16-bit fast forms, cold fault paths, lazy flags,
  cond_fast, block chaining, pointer loop, NativeBlock infrastructure (code-hash matched, budget-exact).
  bench 178 -> ~285 MIPS; s3det old vs new CPU identical (90 000 vms).
- s3-runtime: `Machine::set_native_blocks`, `Stats::cpu_ns`.
- s3-mods: fast.rs (registry, `S3_FAST=0`), fast_sprite.rs, fast_blit.rs, fast_tests.rs (differential vs real exe).
  s3det fast on/off identical. Desktop uncapped 33 -> 81 fps (640x480).
- s3run: S3_SAMPLE `fps:` `cpu:`; S3_PROFILE(+_FROM_MS) -> .disasm/.forms; tools/perf_profile.py; tools/perf/code_hash.py.
- Cargo.toml: `[profile.release.package.s3-cpu] codegen-units = 1`.
- Docs: performance.md (new, AGENTS row), cpu.md, directx-hle.md, status.md perf rows.

## Verify
- `CARGO_TARGET_DIR=port/target cargo test --release -p s3-cpu -p s3-mods -p s3-runtime -p s3-dx -p s3-desktop -j 4`
  (all green 2026-11-05; s3-mods fast_tests needs game-files/gog-s3-ultimate/S3.EXE).
- Deterministic: s3det with/without `S3_FAST=0` -> identical hash files (scratch/perf/det5).
- In game headless default settings: scratch/perf/def4.png (800x600, 53-55 fps at cap 60).

## Next (not done)
- Phone measurement in the Verify phase (expect ~2-2.4x: ~55-65 fps at 640x480).
- More fast paths (fog sprite loop 0x5fc41a, 0x5fd120 run loop), superinstructions, multi-core draw thread, arm64 JIT.
