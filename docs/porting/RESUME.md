# Resuming interrupted work

Work may stop at any time: power off, sleep, network loss or session end. Nothing is lost if these steps are followed.

## State lives in

1. **git** in `settlers-3/`: phase checkpoints are committed on `main`.
2. **Autosave ref** `refs/wip/autosave`: a background job snapshots the whole working tree (minus gitignored files)
   every 20 minutes without touching HEAD or the index. See the latest snapshot: `git log refs/wip/autosave -3 --stat`.
   Restore a file from it: `git checkout refs/wip/autosave -- <path>`.
   Script: `tools/autosave.sh`. Restart it after a reboot:
   `setsid nohup tools/autosave.sh >/dev/null 2>&1 &`
3. **Progress notes** `docs/progress/<label>.md`: one per agent (what's done, what's next).
4. **Docs** (`docs/`, index in `AGENTS.md`) and `docs/porting/integration-log.md`.

## Overnight workflow (started 2026-11-05 ~01:12)

- Script: [`tools/workflows/s3-android-port-overnight.js`](../../tools/workflows/s3-android-port-overnight.js)
  (copy of the run script). Phases: Foundation → Build → Integrate (loop) → Android → Wrap-up.
- Run ID `wf_4e45ee56-35e`, transcript dir
  `~/.claude/projects/-home-<user>-Projects/2be752db-61d9-5841-8e86-e7b4a514fc21/subagents/workflows/wf_4e45ee56-35e/`
  (`journal.jsonl` records which agents started and finished, with their results).

### Same Claude session still alive

Ask Claude to resume: `Workflow({scriptPath: "<session workflows/scripts/...js>", resumeFromRunId: "wf_4e45ee56-35e"})`.
Finished agents return cached results; unfinished ones start again. They see the files already on disk and the progress notes.

### New Claude session (after reboot)

The workflow cache does not carry over to a new session. Do this instead:
1. Read `AGENTS.md`, this file, `docs/journal.md`, `docs/progress/*.md`, `git log --oneline` and `git log refs/wip/autosave -1`.
2. Find the phase that was reached. Use the old `journal.jsonl` above if it still exists; otherwise use the progress notes and checkpoint commits.
3. Edit a copy of `tools/workflows/s3-android-port-overnight.js` so it starts at the first unfinished phase.
   Remove finished phases, and pass a short summary of earlier results in the prompts.
   Then run it with the Workflow tool (the user has opted in to workflows).
4. Restart the autosave job (see above). Check that the Android emulator `s3_api35` (port 5610) is not left in a bad state.

## Polish workflow (started 2026-11-06 ~03:05)

- First workflow `wf_4e45ee56-35e` **finished**: the game runs in game on desktop and on Android. Commits `b2d6707`, `15bf17a`, `39f0731`.
- Second workflow `wf_3257fcea-201` (polish): cursor, save/load, soft keyboard, players-screen OK bug, audio verification, Android UX.
  Script copy: `tools/workflows/s3-port-polish-night.js`. Same resume rules as above.
  If it is interrupted, the last good state is commit `39f0731` (working APK build).

## Mobile UX + online workflow (started 2026-11-06)

- The polish workflow `wf_3257fcea-201` **finished** (commit `1ffc264`).
- Third workflow `wf_0ab9b0c8-5e1`: hook framework, game-state RE, determinism, multiplayer protocol, then context touch,
  UI mode A/B and DirectPlay over relay, then verify and wrap-up. Script copy: `tools/workflows/s3-mobile-ux-and-online.js`.
  Goals and decisions: [mobile-vision.md](mobile-vision.md). Last known-good state before it: commit `64e681d`.
- **Stopped by the user on 2026-11-06 (PC going to sleep)** during the Foundation phase. Snapshot: `refs/wip/autosave` commit
  "workflow wf_0ab9b0c8-5e1 stopped by user". Finished Foundation agents return cached results on resume (same session).
  Unfinished agents restart and should read their `docs/progress/<label>.md` first.
  Resume (same session): `Workflow({scriptPath: <session copy of s3-mobile-ux-and-online js>, resumeFromRunId: "wf_0ab9b0c8-5e1"})`.

## FPS / zoom / full screen / HUD workflow (started 2026-11-05 ~14:40)

- The mobile UX + online workflow `wf_0ab9b0c8-5e1` **finished** (commit `e200858`). The real phone now runs it (Galaxy Z Fold4).
- Fourth workflow `wf_a446e364-b12`: 60+ fps, FPS counter, full-screen display modes, custom resolution + zoom, HUD panel
  hiding + minimap with a full-screen map. Script copy: `tools/workflows/s3-fps-zoom-fullscreen-hud.js`.
  Last known-good state: commit `0c571e8`.
