# Progress: mods-w1 (W1 mod/hook framework) — DONE 2026-11-06

Goal: s3-mods crate (hooks, guest struct views, call_guest helpers, exe-hash patch sets, overlay API, gesture
recognizer, settings), CPU block-entry hook point, engine wiring, s3run --inject touch commands, docs/porting/mods.md.

## Done
- s3-cpu: `CpuExit::Hook`, `HookAddrs`, `Cpu::set_hooks/skip_hook_once`, blocks split before hooked addrs (`Block::hooked`).
- s3-runtime: `ext.rs` (MachineExt fn pointers; set_hook_addrs, refresh_overlay, last_frame_size), machine.rs handles
  Hook exits in run_thread/call_guest, present = overlay -> cursor, event filter in run_slice, create_thread inherits hooks;
  `HostEvent::Pointer` + `PointerPhase`; `Stats::hooks`.
- s3-mods: install (sha256, settings, patch set, ext fns, service), Mod trait + capture, hooks (skip/jump/on_return
  trampoline), call (stdcall/cdecl/thiscall/fastcall/virtual), view (guest_struct!), patchset (gog-base safe points),
  overlay (fontdue), gesture, fallback (legacy touch->mouse), settings JSON, debug overlay (S3_MODS_DEBUG=1).
- s3-engine: `s3_mods::install` after PE load; summary prints hooks dispatched.
- s3run: inject `tap`, `tap2`, `drag`, `longpress`, `pinch` (raw pointer events).
- Verified: workspace `cargo test` all green; `cargo check --target aarch64-linux-android -p s3-android` OK;
  headless run reached in game by taps only, drag scrolled the map, tap opened the build menu, debug overlay visible.
- Benchmark: bench_mips no measurable cost; in game parallel A/B 97.2 vs 97.0 and 95.0 vs 94.9 M/s (< 1%).
- Docs: docs/porting/mods.md (new), architecture.md, cpu.md, integration-log.md, AGENTS.md map row.

## Next (for other workstreams)
- W2: per-frame safe point (main-thread safe points hit rarely in game), game structs via guest_struct!.
- Build phase: switch s3-android touch to HostEvent::Pointer (then s3-mods fallback/gestures handle it).

## Verify
CARGO_TARGET_DIR=port/target cargo test -p s3-mods -p s3-cpu -p s3-runtime -p s3-desktop -j 4
