export const meta = {
  name: 's3-port-polish-night',
  description: 'Polish the working Settlers 3 Android port: cursor, save/load, soft keyboard, input bug, audio verification, Android UX',
  phases: [
    { title: 'Polish', detail: 'parallel fixes in disjoint areas' },
    { title: 'Verify', detail: 'desktop + emulator end-to-end verification and fixes' },
    { title: 'Wrap-up', detail: 'docs, journal, commit' },
  ],
}
const ROOT = '~/Projects/settlers-3'
const PORT = ROOT + '/port'
const GAME = ROOT + '/game-files/gog-s3-ultimate'
const CONTEXT = `
PROJECT ${ROOT}: Rust host that runs the user's ORIGINAL Settlers 3 GOG S3.EXE (PE loader + x86 interpreter + Win32/DirectX HLE) on desktop (s3run) and Android (NativeActivity APK). STATUS: it WORKS — main menu, game setup, in game, building placement by touch on the emulator. Do not break that: keep changes focused, run tests, and re-check the game still reaches in-game.
READ FIRST: ${ROOT}/AGENTS.md (index + rules, especially crash-safe progress notes in docs/progress/<label>.md), docs/porting/status.md, docs/porting/architecture.md, docs/porting/integration-log.md (run/inject recipes, tap coordinates), docs/porting/android.md, plus the doc of the area you touch.
Game files (local only, never commit/embed): ${GAME}. Build: CARGO_TARGET_DIR=${PORT}/target cargo ... -j 4 (shared target dir; other agents build concurrently). Disk/RAM are shared; keep logs and screenshots small and under ${ROOT}/scratch.
Emulator: ONLY an agent whose task says so may use emulator-5610 (AVD s3_api35, scripts in ${PORT}/scripts). Never touch emulator-5600.
Do NOT git commit (wrap-up commits). No sudo. REA MCP (mcp__rea__*) may be used on ${GAME}/S3.EXE only; never close_binary.
Document what you learn in the matching doc; return a journal summary instead of editing docs/journal.md.
`
const RESULT = { type: 'object', properties: {
  status: { type: 'string', enum: ['done', 'partial', 'blocked'] }, summary: { type: 'string' },
  journal: { type: 'string' }, open_issues: { type: 'array', items: { type: 'string' } } },
  required: ['status', 'summary', 'journal', 'open_issues'] }

phase('Polish')
const TASKS = [
  { key: 'cursor', owns: 'cursor parts of s3-win32/src/user32.rs (LoadCursorA/SetCursor/ShowCursor/cursor state), PE resource helpers in s3-pe if needed, and the present/compose path in s3-engine (or s3-runtime FrameBuffer helpers)',
    task: 'Draw the mouse cursor. The game uses the Windows cursor (registry GDIMouse=1). Load real cursor images from S3.EXE .rsrc (RT_GROUP_CURSOR/RT_CURSOR, monochrome AND/XOR or color DIB cursors; system IDC_* cursors -> provide a simple built-in arrow drawn from code, not copied from Windows) at LoadCursorA, track SetCursor/ShowCursor/SetCursorPos/hot spot, and composite the current cursor into the frame at present time for all frontends (do not modify guest surface memory; compose into the host copy). Respect ShowCursor counter. Unit-test cursor decoding with a synthetic cursor resource. Verify on desktop with s3run --headless screenshots (menu + in-game with an injected mouse position) and Read the PNGs.' },
  { key: 'save-load', owns: 's3-win32/src/kernel32*.rs file/VFS parts and s3-runtime VFS',
    task: 'Make save/load work and prove it on desktop: with s3run --headless --inject, start a game (recipe in integration-log.md), open the in-game menu, save a game (the save name may need keyboard input: inject key events; check how inject supports keys and extend s3-desktop inject syntax if needed — you may edit s3-desktop/src/main.rs inject parsing only), quit to the main menu, load the save, verify the screen shows the loaded game. Fix whatever breaks (file create/write/rename/FindFirstFile on the overlay data dir, directory creation, case-insensitive matching between overlay and game dir, file times). Also check that Save/ files from the user\'s game dir (Save/Multi etc.) are visible. Document in docs/porting/kernel32.md.' },
  { key: 'input-ok-bug', owns: 's3-dx/src/dinput*.rs and the input/message parts of s3-win32/src/user32.rs (not cursor code)',
    task: 'Investigate and fix the bug that the first OK click on the players screen (Single Player: Scenario -> OK -> players screen -> OK) is ignored on desktop and Android. Likely causes: DirectInput buffered data vs window messages ordering, button-up/down pairing, mouse position update before click, event notification, double-click timing, or focus messages. Use S3_TRACE and REA on S3.EXE to see how the dialog reads clicks. Fix the root cause, add a test, verify with s3run --headless --inject that one click suffices. Also implement the two missing GetProcAddress names (kernel32 IsProcessorFeaturePresent -> FALSE for unknown features / TRUE for PF_FLOATING_POINT_EMULATED false etc., user32 GetLastActivePopup -> hwnd) — these are tiny; coordinate by only adding new functions.' },
  { key: 'audio-verify', owns: 's3-desktop (except inject parsing owned by save-load), s3-dx/src/dsound*.rs, s3-win32/src/winmm*.rs audio parts',
    task: 'Add an audio sink to s3run: --audio-wav <file> writes the mixed Host audio (effects + MCI ogg music) to a WAV (bounded length), and optionally real playback with the cpal crate behind a cargo feature (default off; only if libasound dev files exist — check pkg-config alsa; do not require root). Run the game to the menu and in game, analyze the WAV with a small python3 -I script (RMS per second, clipping, silence), verify music plays (MCI tracks) and effects occur, and that the mixer timing (GetCurrentPosition) keeps up without gaps or drift. Fix issues (resampling quality, volume dB curve, pan, looping, buffer underruns). Document in docs/porting/directx-hle.md and user32-gdi32.md (MCI).' },
  { key: 'android-ux', owns: 's3-android crate, android/ manifest+res, scripts/*.sh, docs/porting/android.md. You are the ONLY emulator user in this phase.',
    task: 'Android UX: (1) on-screen keyboard: show the IME (JNI ANativeActivity_showSoftInput or InputMethodManager via jni crate) when the user does a three-finger tap or taps a small on-screen keyboard button drawn in the letterbox margin; map key events (incl. Enter/Backspace/Esc) to HostEvent::Key with correct VK codes and WM_CHAR text. (2) immersive fullscreen (hide system bars), keep screen on while the game runs, handle Back button as Esc. (3) use the black letterbox margins for 3 small touch buttons: Esc/menu, keyboard, right-click toggle (next tap = right click) — draw them in the host compose step of s3-android only. (4) show a clear first-run screen when game files are missing, with the exact path to copy files to. Build the APK (scripts/build-apk.sh --abi x86_64 for iteration), install on emulator-5610, verify with screenshots + adb input (adb shell input keyevent / text) that typing reaches the game (e.g. player name field in Game Setup), Back opens the in-game menu, and gameplay still works. Keep the game files already on the device.' },
]
const res = await parallel(TASKS.map(t => () => agent(`${CONTEXT}\nYOUR TASK (${t.key}; you own: ${t.owns}; others edit other parts concurrently — keep your crate compiling and avoid editing files you do not own beyond minimal, compatible additions):\n${t.task}\nWhen done: cargo test for your crates passes, s3run still reaches in-game (short headless run), docs updated, progress note docs/progress/${t.key}.md final.`, { label: t.key, phase: 'Polish', schema: RESULT })))
const journal = []
res.forEach((r, i) => { if (r) journal.push(`### ${TASKS[i].key} [${r.status}]\n${r.journal}`) })
log('Polish: ' + res.map((r, i) => `${TASKS[i].key}=${r ? r.status : 'FAILED'}`).join(', '))

phase('Verify')
let ver = null
for (let i = 1; i <= 3; i++) {
  ver = await agent(`${CONTEXT}\nYOUR TASK (end-to-end verification ${i}; you may fix anything, you are the only emulator user now):
Polish results: ${res.map((r, k) => `${TASKS[k].key}: ${r ? r.status + ' — ' + r.summary.slice(0, 400) : 'FAILED'}`).join('\n')}
${ver ? 'Previous verification: ' + ver.summary.slice(0, 800) : ''}
1. cargo build --workspace + cargo test --workspace (-j 4). Fix breakages from parallel work.
2. Desktop: s3run headless: menu -> single player -> in game -> place building -> save -> load; cursor visible in screenshots; audio WAV non-silent. Read the PNGs.
3. Android: scripts/build-apk.sh (both ABIs, release), install on emulator-5610, run the same flow by adb taps (coordinates in android.md), test keyboard input, Back button, pause/resume (Home + reopen), cursor visible. Take screenshots into ${ROOT}/scratch/android-shots/verify${i}-*.png and Read them.
4. Fix regressions you find (smallest correct fix), rebuild, re-verify. Update docs/porting/status.md (what works now, with screenshot paths, known issues, next steps) and integration-log.md.
Return status done only if the full flow works on Android.`, { label: `verify-${i}`, phase: 'Verify', schema: RESULT })
  if (ver) journal.push(`### verify-${i} [${ver.status}]\n${ver.journal}`)
  if (ver && ver.status === 'done') break
}

phase('Wrap-up')
const wrap = await agent(`${CONTEXT}\nYOUR TASK (wrap-up): make sure AGENTS.md Map lists all docs (incl. new ones) and docs match the code; update docs/porting/status.md; append to docs/journal.md a new top section '## 2026-11-06 — polish night (workflow)' with these entries (concise, merged):\n${journal.join('\n')}\nLegal check: git ls-files contains no game bytes; licenses of any new deps/assets are GPL-compatible. Then git add -A and commit: git -c user.name="Ahmet Enes Duruer" -c user.email="ahmet@duruer.dev" commit -m 'feat(port): cursor, save/load, keyboard and Android UX polish' with a body ending in a blank line and 'Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>'. No push. Leave the app running on emulator-5610 in the main menu. Return a concise Turkish report for the user: what works, APK path, how to install on a phone, known issues.`, { label: 'wrap-up', phase: 'Wrap-up', schema: RESULT })
return { res, ver, wrap }
