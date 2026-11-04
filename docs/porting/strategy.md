# Porting strategy

Status: **DRAFT, no decision yet** (2026-11-05). Next step is a feasibility study like
`~/Projects/blockheads-club/blockheads-pc/docs/feasibility/PLAN.md`.

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

Recommended direction (to confirm in feasibility): **B**, with C as a later speed path if needed.

### CPU core candidates for B

| Core | License | Notes |
| --- | --- | --- |
| Own x86-32 interpreter (Rust) | ours | Needs i386 + x87 FPU + maybe MMX. Portable incl. iOS. Measure speed first |
| Box64 (incl. 32-bit "box32") dynarec | MIT | ARM64 dynarec; tied to Linux/Wine-ish ABI, would need extraction |
| FEX-Emu | MIT | x86/x86-64 → ARM64 JIT, Linux-host oriented |
| Unicorn | GPL-2.0-only | Rejected in blockheads-pc for license reasons; same here if we go GPL-3.0-or-later |
| dynarmic | 0BSD | ARM guest only — not usable |

### Host layers (B)

- Loader: map PE sections at 0x400000, resolve imports to host thunks, run entry point.
- Win32 HLE: files (VFS over user's install + app data dir for Save/registry), threads/sync, time, messages/window, registry (prefilled from `regs.cmd`, see [registry](../reverse-engineering/registry.md)).
- DirectDraw → present 8/16-bit framebuffer via GLES/Vulkan texture; scale to phone screen.
- DirectSound → Android AAudio/Oboe (later CoreAudio). MCI music → play `MUSIC/*.ogg` ourselves.
- Input: touch → mouse (tap = left click, long-press / two-finger tap = right click, pinch? = zoom n/a, drag = scroll).
- DirectPlay multiplayer: later, possibly over our own relay.
- App shell: Android app (Kotlin or Tauri like blockheads), launcher asks user to pick the GOG folder/zip (SAF).

## Next steps

1. Run GOG `S3.EXE` under Wine on this PC (`wine` installed) to get a working reference + API trace (`WINEDEBUG=+relay`).
2. From the trace, list the actually-used APIs and COM methods → size the HLE.
3. Benchmark: how many x86 instructions/sec the game needs in a typical match (count via trace/`perf`), to choose interpreter vs JIT.
4. Write feasibility PLAN with milestones and kill gates.
