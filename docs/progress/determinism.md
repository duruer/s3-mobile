# Progress: determinism (W5 prerequisite) — FINAL 2026-11-05

Goal: bit-identical simulation across runs and hosts (x86_64 / aarch64). Owns `port/crates/s3-cpu/src/fpu*.rs`,
`docs/porting/determinism.md` (full results there).

## Done
- `s3-cpu/src/fpu_math.rs` (module `fpu::math`): pinned pure-Rust `libm =0.2.16` (no default features) for
  fsin/fcos/fptan/fsincos/fpatan/fyl2x/fyl2xp1; f2xm1 with exact Dekker x*ln2; exact integer fprem/fprem1 (exact
  quotient bits); x87 NaN propagation; `host_float_env_check()`.
- fpu.rs routed through it; `finish()` never returns host NaNs; fscale/fprem NaN paths fixed.
- fpu_conv.rs: bit-exact f32<->f64 NaN conversion; round_rc NaN guard.
- `s3-cpu/src/fpu_golden.rs`: golden hashes (interpreter path, 27 cases + int conversions + pure layer).
  Same on x86_64 debug/release and i686 (pure). aarch64-android test binary builds, 0 FMA instrs; NOT run (no device).
- `s3-desktop/src/bin/s3det.rs` + `[[bin]] s3det`: virtual clock, fixed epoch/TZ, deterministic GUIDs, per-second
  memory+CPU hashes, `--dump-at`. `tools/memdiff.py`, `tools/determinism-test.sh`.
- Found + fixed (in s3det) divergence: CoCreateGuid GUID at 0x7a7ac8.
- Result: two parallel 75 000 vms runs (menu -> in game -> build menu) IDENTICAL, final
  `mem=a68d8effad5b92f2 insns=7699350378`; reproduced with a second build later.
- REA: CRT rand 0x61f802 / srand 0x61f7f5 seeded with time(NULL) in 0x431780 and 0x4932e0.
- Docs: determinism.md (new), x87-fpu.md updated, AGENTS.md map row.

## Next (for others)
- Run golden vectors + s3det on arm64 device; compare hashes.
- Frontends call host_float_env_check(); install s3-mods patch set in s3det once mods change behaviour.
- W2: find sim RNG and confirm logic thread never uses x87 for synced state.

## Verify
`cd port && CARGO_TARGET_DIR=$PWD/target cargo test -p s3-cpu golden` and
`cargo build --release -p s3-desktop --bin s3det && ../tools/determinism-test.sh target/release/s3det 75000`.
