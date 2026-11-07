# perf-shadow (native fast paths for the shadow sprite functions)

Goal: native fast paths for the run/row loops of the shadow sprite functions `0x5fd120` (calls `[0x7e8e20]` =
`0x5fd080` dither / `0x5fd0c0` darken) and `0x5fc160` (second loop, calls `[0x7e8c74]` = `0x5f48e0` zero fill).

## Done
- `port/crates/s3-mods/src/fast_shadow.rs`: `shadow_zero` (entries 0x5fc2cd, 0x5fc2d4) and `shadow_rows` (entries
  0x5fd24f, 0x5fd256, 0x5fd25d), registered in `fast.rs`.
- Differential tests `differential_shadow_zero`, `differential_shadow_rows` in `fast_tests.rs`: pass (288/300 and
  198+176/400 ran natively); mutation check (wrong cost / wrong pushed EBX) fails as expected.
- Profile (HUD, 2316x904): `0x5fc160`+`0x5f4830` 21 % -> 1.5 %, `0x5fd120`+`0x5fd080` 7.6 % -> 0.8 %
  (`scratch/perf-shadow/base.prof`, `new.prof`).
- Not done: fog loop `0x5fc41a` (not executed in the in-game profile), first loop of `0x5fc160` (`[0x7e8c70]`,
  not executed either).

## Build note
- The worktree base commit does not build on Windows (s3-mem without the VirtualAlloc port, which is uncommitted in
  the main checkout). For building, `port/crates/s3-mem/{Cargo.toml,src/sys.rs}` and `port/Cargo.lock` were copied
  from the main checkout; they are NOT part of this branch's commit.

- A/B fps (alternating, S3_FAST default, cap 0, HUD 2316x904): base ~35 / ~31, new ~40 / ~38.
- s3det 90000 vms fast vs S3_FAST=0: IDENTICAL plain (`insns=11817005518`) and HUD (`insns=12256208149`),
  `scratch/perf-shadow/det-plain`, `det-hud`. All 68 s3-mods tests pass.

## Status: DONE (committed on branch worktree-agent-aa41e0cf3a6c5d1ed)

## Verify
- `S3_EXE=... cargo test --release -p s3-mods differential_shadow`
