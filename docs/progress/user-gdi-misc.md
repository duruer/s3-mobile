# Progress: user-gdi-misc

Goal: USER32, GDI32, ADVAPI32 (registry JSON), SHELL32, RPCRT4, OLE32, WINMM/_INMM (timers, MCI cdaudio -> ogg), stubs
for comctl32/comdlg32/wsock32/wininet. Design doc: [docs/porting/user32-gdi32.md](../porting/user32-gdi32.md).

## Done (2026-11-05)
- `s3-runtime/src/gdi.rs`: `DcTarget` + `create_dc/release_dc` hooks (used by s3-dx GetDC).
- user32.rs, gdi32.rs + font.rs (fontdue, bundled Liberation Sans OFL), advapi32.rs + registry_store.rs,
  winmm.rs + audio_music.rs (lewton), ole32/rpcrt4/shell32/wsock32/wininet/comctl32/comdlg32.
- Tests: `src/user_gdi_tests.rs` + unit tests; all s3-win32 tests pass.
- Integration: headless s3run reaches the main menu (text drawn by us); injected click opens Game Setup without
  serial prompt. Intro video = fake MCI video (notify success) -> no error box.
- Clicking through setup/lobby reaches the in-game map (random 384, 640x480); MCI plays track 7.

## Next
- s3-dx should call `user32::set_screen_size` in SetDisplayMode and (if it owns audio) `winmm::mix_music`.
- Hear music on a real frontend (desktop/Android audio).

## Verify
`cd port && CARGO_TARGET_DIR=$PWD/target cargo test -p s3-win32 -j 4`
`./target/debug/s3run --headless --max-seconds 60 --game-dir ../game-files/gog-s3-ultimate --data-dir ../scratch/data-ugdi --screenshot ../scratch/ugdi-shot.png`
