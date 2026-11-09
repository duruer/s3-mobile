# tech-debt (noted 2026-11-09, for later; user: "teknik borçlara sonra gelelim")

Not started. Pick these up when the user asks.

1. **Golden FPU vectors fail on aarch64 (Windows 11 ARM64 host, release):** `s3-cpu` `fpu::golden::golden_pure_functions`
   and `golden_x87_vectors` hash differently than the stored x86_64 constants. Matters for multiplayer between arm64
   phones and x86_64 PCs (lockstep desync). Details + numbers: [determinism.md](../porting/determinism.md) ("2026-11-09").
   Next: run the golden test on an arm64 Android phone (`adb push` the test binary, see determinism.md) to tell a
   Windows/MSVC issue from an aarch64 issue; then bisect the per-case hashes (`golden_x87_vectors` prints them).
2. **`s3-runtime` `tests::vfs_case_insensitive_and_overlay` fails on Windows:** `crates/s3-runtime/src/tests.rs:86`
   checks `p.ends_with("Gfx/Siedler3_00.f8007e01f.DAT")`, the path has `\` on Windows. Test-only (path separator).
3. Small: `@semantic-release/github` warns that `successComment: false` / `failComment: false` are deprecated
   (`.releaserc.json`).

CI (ubuntu x86_64) runs all tests and is green; 1 and 2 only show on this Windows ARM64 dev PC.
