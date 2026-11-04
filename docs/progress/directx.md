# Progress: directx

Goal: s3-dx crate: DirectDraw (surfaces, Blt/BltFast/Flip/Lock/GetDC -> Host::present), DirectSound (software
mixer -> Host::queue_audio), DirectInput (mouse/keyboard from HostEvents), DirectPlay stubs.

## Done (2026-11-05)
- All four modules implemented, every interface method has a handler; docs/porting/directx-hle.md.
- s3-runtime/src/audio.rs (`AudioStreams`) added: extra PCM streams (winmm music) mixed by the dsound mixer.
- s3-dx no longer depends on s3-win32 (Cargo.toml).
- 13 tests pass: `cargo test -p s3-dx`.

## Next
- Run against S3.EXE once kernel32/user32 let startup reach DirectDraw; check logs for unexpected IIDs/flags.

## Verify
`cd port && CARGO_TARGET_DIR=$PWD/target cargo test -p s3-dx -j 4`
