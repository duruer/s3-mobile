# Settlers III port — host runtime (Rust)

Runs the user's own GOG `S3.EXE` with an x86-32 interpreter plus high-level emulation of Win32/DirectX.
No game files are included or needed to build; tests use hand-assembled snippets.

- Architecture, memory map, thunk ABI, how to add an HLE function: [`../docs/porting/architecture.md`](../docs/porting/architecture.md)
- Project index: [`../AGENTS.md`](../AGENTS.md)

```sh
export CARGO_TARGET_DIR=$PWD/target
cargo build -j 4 && cargo test -j 4
./target/debug/s3run --headless --max-seconds 20 --game-dir <gog install> --data-dir <writable dir>
```

License: GPL-3.0-or-later. Not affiliated with Ubisoft / Blue Byte.
