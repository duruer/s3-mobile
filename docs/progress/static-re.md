# Progress: static-re (static RE of S3.EXE for HLE implementers)

Status: DONE (2026-11-05).

Outputs: docs/reverse-engineering/static-isa.md, static-startup.md, directx-surface.md (appended section
"Static details for the HLE"), unknowns.md (#1, #7, #10, #11, #12 updated; #13, #14 added), registry.md
(Resolution, WaitVBlank), s3-exe.md (TODO), AGENTS.md map rows.

Tools: tools/isa-census (Rust, iced-x86; build with CARGO_TARGET_DIR=port/target), tools/iat_xrefs.py, tools/dis.py.

Next (for whoever continues): unknowns #13 (DirectPlay failure tolerance at startup), #14 (registry value for
presentation flags), exact GFX suffix formatting code in the 0x5fe8xx-0x5ff1xx loader.
