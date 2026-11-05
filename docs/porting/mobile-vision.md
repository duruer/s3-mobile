# Product vision: a real mobile Settlers 3 (not just an emulator)

Set by the user on 2026-11-06. Running S3.EXE is the foundation, not the goal.

## Requirements (user)

1. **Easy start:** the user gives the needed game files (folder or zip) and plays. No adb, no manual copying.
2. **Online play like Settlers United:** lobby, friends, internet matches.
3. **Easy mobile UI like JSettlers:** large touch targets, gestures, readable on a phone.
4. **Patch the game for touch instead of simulating desktop input.** Example: right-click must not need a long press and
   a wait. Rendering and logic may be patched so fingers work naturally. ESC/RMB buttons are a stopgap.

## Technical basis we already have

We own the CPU interpreter and every OS API, so we can do everything Settlers United's `s3.dll` does, plus more,
without shipping a modified exe:

- **Hooks:** run host code when EIP reaches an address (function entry or call site). Read and write game memory, and call
  game functions. Patches are applied in memory at load time, keyed by the S3.EXE sha256 (GOG `00a925cf…`).
- **Render layer:** we compose every presented frame ourselves (the cursor already works this way). A native mobile HUD
  can be drawn on top or replace parts of the original UI. The game frame can be cropped and scaled (e.g. hide the original
  left panel, zoom the map).
- **Network:** DirectPlay is our HLE, so multiplayer sessions can run over our own transport.

## Workstreams (proposed)

| # | Workstream | What | Needs |
| --- | --- | --- | --- |
| W1 | Mod/hook framework | `s3-mods` crate: address hooks, memory views of game structs, calling game functions, per-exe-hash patch sets, debug overlay | CPU hook point (cheap EIP check at block entry) |
| W2 | Game-state RE | Selection list, unit/building types, camera/viewport, cursor mode (build placement, attack…), panel/menu state, player resources, command functions (move, build, destroy, priority) | REA/Ghidra on S3.EXE + runtime memory inspection |
| W3 | Touch input v2 | **Context-aware tap:** units selected + tap on ground = move (right-click semantics); tap on own unit = select; in build mode tap = place, second tap/✕ = cancel. Two-finger pan, pinch = zoom (scaled frame), long-press-drag = box select, double tap = select all of a type | W1 + W2 |
| W4 | Mobile HUD | Native, large-button UI rendered by us: build menu as a bottom sheet with categories, selected-unit card, resource/stats panels, game menu (save/load/quit) — calling game functions instead of clicking the original panel. Option to hide the original left panel and use the full screen for the map | W1 + W2 |
| W5 | Online multiplayer | DirectPlay 6 HLE over our own relay/lobby server (Rust, WebSocket/QUIC); rooms, invites. S3 MP is lockstep, so **determinism across devices** is needed: x87 results must be bit-identical on arm64 and x86_64. That means our own deterministic transcendental functions (no host libm), identical rounding, and desync detection | Server hosting; decision on crossplay |
| W6 | Launcher (Tauri 2 + Svelte 5) | Folder/zip picker via SAF, validate by hash, import into app storage, settings, i18n, account/lobby UI | — |
| W7 | Real-device tuning | arm64 phone performance, battery, thermal, interpreter speedups | a phone |

## Open decisions (ask the user)

- Crossplay target for online: our own network only (mobile + our desktop app), or also existing Windows players
  (Settlers United / aLobby, i.e. real DirectPlay over a VPN)?
- HUD direction: keep the original panel (scaled + gestures), or replace it with a native mobile HUD (JSettlers-like)?
