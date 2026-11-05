# Progress: save-load — FINAL (2026-11-05)

Goal: save/load round trip on desktop headless (s3run --inject), fix what breaks, document in kernel32.md.

## Result: DONE
Start random map -> in-game save (slot 1) -> Quit Game -> statistics -> main menu -> Load Game -> entry -> the saved
game continues. Recipe: docs/porting/integration-log.md ("Save/load round trip"); details: docs/porting/kernel32.md
("Save / load"). Evidence (local): scratch/keep/rt1-saved.png, rt2-menu.png, rt3-loadlist.png, rt4-loaded.png.

## Fixes
- s3-win32/src/gdi32.rs `Pixels`: 1 bpp / 4 bpp DIBs (all in-game panel text and tooltips are drawn into a
  640x480x1 DIB; they were invisible -> Save/Quit menu looked empty). Test user_gdi_tests::draw_text_into_monochrome_dib.
- s3-win32/src/kernel32_sync.rs CreateThread: stack arg = commit, size = PE reserve (game passes 0x1000 -> threads had
  64 KiB; the save-slot reader's 256 KiB locals ran off the stack, SEH frame check failed -> crash on "Save Game").
  Test kernel32_tests::create_thread_stack_is_commit_not_reserve.
- s3-win32/src/kernel32_file.rs: `S3_FILES=1` logging of file ops. Test kernel32_tests::save_slots_overlay_and_game_dir.
- s3-desktop/src/main.rs inject: `shot <png>` and `text <str>` commands (parse_inject_all).
- VFS itself needed no change (create/write/find/case-insensitive merge with game dir Save/Multi all worked).

## Verify
cd port; CARGO_TARGET_DIR=$PWD/target cargo test --release -j 4 -p s3-win32 -p s3-runtime -p s3-desktop
Then the round-trip inject from integration-log.md (~95 s run).

## Open
- Android save/load not tried (same code path; overlay is files/data).
- Typing letters in game triggers hotkeys; no name prompt is needed for saves.
