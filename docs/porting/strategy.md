# Porting strategy

Status: **option B IMPLEMENTED (2026-11-06)**: own host, iced-x86 interpreter + HLE, code in `port/`, contract in
[architecture.md](architecture.md). What works today (details and evidence in [status.md](status.md)):

- The unmodified GOG `S3.EXE` + data runs on desktop (`s3run`) and on Android (APK, emulator `s3_api35`): main menu
  in < 1 s, Single Player scenario -> random map -> in game in ~16-25 s, building placement, map scrolling, pause/resume.
- Speed: ~100 M guest instructions/s in game, about one host core; 48 fps desktop, ~30 fps x86_64 emulator.
  Option C (static recompilation) is not needed for now. A pure interpreter also keeps the iOS path open.
- Not done yet: real arm64 phone test, cursor drawing, video, save/load check, multiplayer (DirectPlay stubs only).

Decided 2026-11-05 (option B). The original draft notes below are kept for history; the "Next steps" list is done
(wine trace: [wine-trace.md](../reverse-engineering/wine-trace.md); instruction rate measured, see status.md).

## Constraints

- Run the **user's own** `S3.EXE` + data. Nothing of the game is shipped, cached in releases or committed.
- Guest = 32-bit x86 Windows PE (MSVC 6, fixed base 0x400000, no relocs). Host = Android arm64 (later iOS arm64).
- **iOS forbids JIT** for App Store / non-debug apps → any iOS build needs an interpreter or ahead-of-time translation.
- Original HW target was a Pentium ~200 MHz; modern phones are far faster, so even an interpreter may be fast enough (to be measured).

## Options

| # | Approach | Pros | Cons |
| --- | --- | --- | --- |
| A | Wine + Box64 container (Winlator-style) | Works soonest, good reference/baseline | Huge, slow start, bad mobile UX, impossible on iOS |
| B | **Own host: PE loader + x86-32 CPU core + HLE of Win32/DirectX** (blockheads-pc pattern) | Small, mobile-native UX (touch, lifecycle, saves), iOS possible with interpreter | We write the HLE (~219 static imports + DDraw/DSound/DPlay COM) and pick a CPU core |
| C | Static recompilation (x86 → C/LLVM) of the user's exe on device | Native speed, iOS-friendly | Indirect calls, jump tables, self-modifying code risk; complex toolchain on device |

Recommended direction: **B**, with C as a later speed path if needed. (Chosen and implemented, see status above.)

### CPU core candidates for B

| Core | License | Notes |
| --- | --- | --- |
| Own x86-32 interpreter (Rust) | ours | **Chosen.** i386 + x87 (MMX not needed: cpuid reports none). Portable incl. iOS. ~170 MIPS benchmark |
| Box64 (incl. 32-bit "box32") dynarec | MIT | ARM64 dynarec; tied to Linux/Wine-ish ABI, would need extraction |
| FEX-Emu | MIT | x86/x86-64 → ARM64 JIT, Linux-host oriented |
| Unicorn | GPL-2.0-only | Rejected in blockheads-pc for license reasons; same here if we go GPL-3.0-or-later |
| dynarmic | 0BSD | ARM guest only — not usable |

### Host layers (B)

- Loader: map PE sections at 0x400000, resolve imports to host thunks, run entry point.
- Win32 HLE: files (VFS over user's install + app data dir for Save/registry), threads/sync, time, messages/window, registry (prefilled from `regs.cmd`, see [registry](../reverse-engineering/registry.md)).
- DirectDraw → present 8/16-bit framebuffer via GLES/Vulkan texture; scale to phone screen. (Implemented without GL: RGB565 copied into the ANativeWindow buffer, the compositor scales.)
- DirectSound → Android AAudio/Oboe (later CoreAudio). MCI music → play `MUSIC/*.ogg` ourselves.
- Input: touch → mouse (tap = left click, long-press / two-finger tap = right click, pinch? = zoom n/a, drag = scroll).
- DirectPlay multiplayer: later, possibly over our own relay.
- App shell: Android app (Kotlin or Tauri like blockheads), launcher asks user to pick the GOG folder/zip (SAF).

## Next steps (DONE 2026-11-05, kept for history; current list in [status.md](status.md#next-steps))

1. Run GOG `S3.EXE` under Wine on this PC (`wine` installed) to get a working reference + API trace (`WINEDEBUG=+relay`).
2. From the trace, list the actually-used APIs and COM methods → size the HLE.
3. Benchmark: how many x86 instructions/sec the game needs in a typical match (count via trace/`perf`), to choose interpreter vs JIT.
4. Write feasibility PLAN with milestones and kill gates.

## Decision: app shell (2026-11-05, user-confirmed)

- The game is **never** rendered inside a WebView. The Rust engine draws straight to a native surface (NativeActivity / ANativeWindow) in the same APK.
- Planned after the core runs: a **Tauri 2 + Svelte 5 + SvelteKit launcher** for the GOG folder picker (SAF), file validation, settings, i18n and saves. Its "Play" button starts the native game activity.
- Tonight's Gradle-less NativeActivity shell (`port/crates/s3-android`) is the game part of that design. It will be embedded later in the Tauri Gradle project.
