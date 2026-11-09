// SPDX-License-Identifier: GPL-3.0-or-later
//! `cfg(s3_jit)`: the AArch64 JIT is compiled in only with feature `jit` on aarch64, and never on iOS (Apple
//! forbids writable+executable memory for apps; docs/porting/cpu.md).
fn main() {
    println!("cargo::rustc-check-cfg=cfg(s3_jit)");
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if std::env::var_os("CARGO_FEATURE_JIT").is_some() && arch == "aarch64" && os != "ios" {
        println!("cargo::rustc-cfg=s3_jit");
    }
}
