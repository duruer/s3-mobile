# Progress: cpu-fpu (x87 FPU + MMX subset)

Goal: implement x87 in `port/crates/s3-cpu/src/fpu.rs` (+ `fpu_conv.rs`, `fpu_tests.rs`).

- DONE: full x87 + MMX movq/movd/emms; 13 unit tests (11 snippet tests + 2 conversion tests) pass; clippy-clean in fpu files.
  Doc: docs/porting/x87-fpu.md (AGENTS.md row added, architecture.md lines updated).
  Changed `tests.rs::unimplemented_reports_mnemonic` (cpu agent's file) to use `addps` because fld1 is implemented now.
- Next (optional): CONTEXT.FloatSave helper for SEH; deliver unmasked FPU exceptions if ever needed.
- Verify: `CARGO_TARGET_DIR=port/target cargo test -p s3-cpu -j 4`.
