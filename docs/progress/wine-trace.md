# Progress: wine-trace (wine reference run of GOG S3.EXE)

Status: DONE (2026-11-05). Results in [docs/reverse-engineering/wine-trace.md](../reverse-engineering/wine-trace.md).

- Prefix `scratch/wineprefix` (380 MB, kept), `C:\S3` -> symlink to game dir, regs.cmd imported (`scratch/wine-s3.reg`),
  virtual desktop 1024x768 (`scratch/wine-desktop.reg`), HKCR serial `Seed` key now present (written by the game).
- Reached intro, main menu (800x600), serial dialog, game setup, and in-game (640x480) for random 384 and "Beginner I".
- Raw logs deleted; summaries `scratch/wine-sum-all.txt`, `scratch/wine-counts-all.txt`, screenshots `scratch/wine-*.png`.
- Game dir unchanged (mtime listing before/after identical except the top dir mtime from a stray log, since removed).

Re-run: `Xvfb :77 -screen 0 1024x768x16 &` then `scratch/wine-run.sh "<WINEDEBUG>" <log>`; summarize with
`python3 -I tools/wine_relay_summary.py <log> 0 99999999999 out.txt 2> counts.txt`.
Not done: save/load, campaign, multiplayer traces.
