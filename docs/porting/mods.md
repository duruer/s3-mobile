# Mod / hook framework (W1, `s3-mods`)

Status: implemented 2026-11-06 (agent `mods-w1`). Code: `port/crates/s3-mods`, hook point in `s3-cpu`, extension
points in `s3-runtime/src/ext.rs`, wiring in `s3-engine`. Vision and workstreams: [mobile-vision.md](mobile-vision.md).

The framework lets host code change the running game without shipping a modified exe: hooks at guest addresses,
typed views of game memory, calls into game functions, per-exe patch sets, an overlay drawn on every frame, and a
touch pipeline (gesture recognizer -> mods -> fallback touch->mouse) that runs before the game sees input.

## Layers

```
frontend (s3run / Android)  --HostEvent::Pointer{id,phase,x,y}-->  Machine::run_slice
   -> MachineExt::event_filter (s3-mods)
        captured pointer? -> that mod only
        else GestureRecognizer -> TouchEvents -> Mod::observe (all) -> Mod::on_touch (top first)
              unconsumed -> fallback::TouchMouse -> mouse HostEvents -> m.input (USER32 / DirectInput)
guest CPU  --EIP hits hooked addr-->  CpuExit::Hook(eip) -> Machine::dispatch_hook -> hooks::dispatch (closures)
guest Flip/Blt -> Machine::present -> overlay (Mod::draw on a copy) -> cursor compose -> Host::present
service "s3mods" (each run_slice): long-press timing, delayed mouse-ups, Mod::tick, overlay-only refresh
```

## CPU hook point (`s3-cpu`)

- `HookAddrs` (= `HashSet<u32>`) installed per CPU with `Cpu::set_hooks(Option<Arc<HookAddrs>>)` (clears the
  decode cache). `Machine::set_hook_addrs` pushes one set to every thread, and `create_thread` gives it to new threads.
- `decode_block` ends a block **before** any hooked address and flags blocks that start at one (`Block::hooked`), so a
  hook can sit anywhere (function entry, call site, mid-function), not only at block starts.
- At block entry: `if blk.hooked { exit CpuExit::Hook(eip) unless hook_skip == eip }`. One byte test per executed
  block. After a `Continue` hook the runtime calls `Cpu::skip_hook_once(eip)` so the original code runs once.
- Hooks on thunk addresses are not supported (use the thunk registry instead).

## Runtime extension points (`s3-runtime/src/ext.rs`)

`Machine::ext: MachineExt` holds plain function pointers (state lives in `m.state::<T>()`):
`hook_dispatch(m, eip) -> HookFlow {Continue, Redirected}`, `event_filter(m, HostEvent, &mut Vec<HostEvent>)`,
`overlay_wanted(m) -> bool` (cheap, per present) and `overlay_compose(m, &mut FrameBuffer) -> bool`.
`Machine::refresh_overlay()` re-presents the last guest frame with the current overlay (when the game is idle);
`Machine::last_frame_size()`. `Stats::hooks` counts dispatched hooks (printed in the s3run summary).
New host event: `HostEvent::Pointer { id, phase: PointerPhase::{Down,Move,Up,Cancel}, x, y }` in game pixels;
`InputState` ignores it (game consumers never see raw touch).

## Hooks (`s3_mods::hooks`)

```rust
s3_mods::add_hook(m, 0x0060_d9e0, "my-mod", |c| {        // c: HookCtx { m, eip }
    let this = c.reg(s3_cpu::ECX); let a0 = c.arg(0);       // args at a function entry: [esp+4+4i]
    c.on_return(|p| { let v = p.eax; p.set_eax(v + 1); });   // post-call callback (function entry only)
    HookFlow::Continue                                      // or c.return_from_function(eax, stdcall_args) / c.jump(a)
});
s3_mods::hooks::commit(m);   // push the address set to the CPUs (install/patch sets do this for you)
```

- Several hooks per address run in registration order; the first `Redirected` ends the chain.
- `return_from_function(eax, n)`: skip the original function (pop return address + `n` dwords, EAX = value).
- `on_return`: replaces `[esp]` with the trampoline thunk `s3mods.dll!post_call_return` and keeps a per-thread LIFO of
  `(entry_esp, real return)`. On return the newest frame with `entry_esp < esp` is ours; newer frames were abandoned by
  exception unwinding and are dropped. Caveat: code that reads its own return address would see the trampoline
  (HYPOTHESIS: MSVC6 code in S3.EXE does not do that).
- Hooks fire again for guest calls made from inside the hook (recursion: guard with a flag, see tests).
- `remove_hooks(m, name)`, `remove_hooks_all`, `hook_stats(m)` (address, name, hits), `hook_time_ns(m)`.

## Calling game functions (`s3_mods::call`)

`call_stdcall`, `call_cdecl`, `call_thiscall(m, f, this, args)`, `call_fastcall`, `call_virtual(m, this, index, args)`.
All use `Machine::call_guest` (runs on the current thread, restores every register incl. ESP); thiscall/fastcall also
restore ECX/EDX. Call from hooks, or queue with `s3_mods::defer(m, |m| ...)`: deferred work runs at the next **safe
point** hook on the guest main thread (`add_safe_point(m, addr)`; the GOG set registers two, below). Calling from
input/overlay callbacks runs on whatever thread is current: avoid.

## Typed memory views (`s3_mods::view`)

`GuestPtr<T>` (`get/set/index`) for `u8/i8/u16/i16/u32/i32/f32`, and

```rust
s3_mods::guest_struct! { pub struct Player { 0x00 => id / set_id: u32, 0x24 => wood / set_wood: u16 } }
let p = Player::at(addr); let w = p.wood(&m.mem); p.set_wood(&mut m.mem, w + 10);
```

## Patch sets (`s3_mods::patchset`)

`install` hashes S3.EXE (own dependency-free SHA-256, `sha256.rs`) and applies the patch set whose `exe_sha256`
matches; any other exe runs **unpatched** with a warning log (`mods: unknown S3.EXE (sha256 …); game patches
disabled`). Input/overlay/settings stay active for every exe. `patch_bytes(m, addr, expect, new)` writes bytes only if
the original bytes match, then invalidates decoded code. `S3_MODS=0` or settings `patches_enabled: false` disable sets.

| Set | Exe | Contents |
| --- | --- | --- |
| `gog-base` | GOG `00a925cfdcfc3349b5d0f3b8a4c312f01b813e363b4464f6f3d6de17804d950b` | safe points at `0x60d9e0` ("message waiting?" test in Run `0x404860`) and `0x60d740` (GetMessage/Dispatch step), see [static-startup.md](../reverse-engineering/static-startup.md) §3 |

Measured (2026-11-06, desktop): the two safe points were hit 39-123 times in a 40-45 s run (menu + ~25 s in game),
because the main thread mostly sleeps in `GetMessageA` in game. So deferred work can wait up to a few seconds in game.
Next (W2): find a safe point that runs every frame or tick (for example the logic thread's 64 ms tick) on a thread
that may call the game functions we need.
HYPOTHESIS: both addresses run only on the main thread (deferred work also checks the main thread id).

W2/W3/W4 add their hooks to `gog_base` (or new sets) in `patchset.rs`.

## Overlay (`s3_mods::overlay`)

`Canvas` over the presented RGB565 frame (game pixels, alpha blending): `fill_rect`, `stroke_rect`,
`fill_rounded_rect`/`stroke_rounded_rect` (anti-aliased), `fill_circle`, `blit`/`blit_scaled` (`Image` = RGBA8),
`text(x, y, px, Weight, Rgba, str) -> width`, `text_centered`, `measure_text`, `set_clip`. Text uses fontdue with the
bundled Liberation Sans (Regular/Bold, SIL OFL 1.1, files from `s3-win32/assets/fonts`), glyphs cached.
Order on screen: guest frame -> overlay -> mouse cursor. Cost when no mod draws: one `overlay_wanted` check plus a
copy of the guest frame kept for `refresh_overlay` (~1 MB memcpy per present).

## Mods and touch input

```rust
pub trait Mod: Send {
    fn name(&self) -> &str;
    fn observe(&mut self, m, ev: &TouchEvent) {}                     // sees everything, cannot consume
    fn on_touch(&mut self, m, ev: &TouchEvent) -> ModEvent { Pass }   // Consumed | Pass
    fn wants_draw(&self) -> bool { false }  fn draw(&mut self, m, c: &mut Canvas) {}
    fn needs_redraw(&self) -> bool { false } fn tick(&mut self, m, now_ms: u64) {}
}
s3_mods::add_mod(m, Box::new(MyHud::default()));   // later mods are on top (get events first)
```

Mods are stored in `ModsState` and taken out while called, so a mod may call the mods API and guest code.
Consuming a raw `Down` **captures** the pointer: its Move/Up go only to that mod and the recognizer forgets it (HUD buttons).

### Gesture recognizer (`gesture.rs`, pure Rust, unit tested)

Frontends deliver raw pointer events only; desktop inject and Android share this code. Events (game pixels, f32):
raw `Down/Move/Up/Cancel`; `Tap` (one finger, <= `tap_max_ms` 500, moved <= `tap_slop` 8 px); `DoubleTap` (extra
event after the second Tap within 300 ms / 24 px, so taps never wait); `LongPress` (held >= 500 ms without moving, fired
by the service tick); `Pan {phase Begin/Update/End, fingers 1|2, x, y, dx, dy (since last), start, after_long_press}`;
`Pinch {phase, cx, cy, scale}` (with a two-finger Pan); `TwoFingerTap`, `ThreeFingerTap`. A second finger landing
on a running one-finger pan or long press ends that gesture.

### Fallback touch->mouse (`fallback.rs`)

Unconsumed gestures map like the legacy Android mapper (`s3-android/src/touch.rs`), so behaviour does not change until
W3 replaces it: Tap = left click (button-up 60 ms later), LongPress / TwoFingerTap = right click (`long_press_right_click`
setting), ThreeFingerTap = keyboard request (`take_keyboard_request`), one-finger Pan = left drag, or right drag on the
in-game map (x >= 170, not 800x600), mirrored and halved = "grab" scrolling. `set_right_next(m, true)` = RMB button.
Android still uses its own `touch.rs` (sends mouse events); switching it to `HostEvent::Pointer` is the build phase's job.

## Settings (`settings.rs`)

`<data_dir>/mods-settings.json`, loaded at install, saved atomically with `s3_mods::set_settings(m, s)`; missing fields
use defaults, a broken file falls back to defaults with a warning.
`{ version, ui_mode: "ClassicTouch"|"MobileHud", patches_enabled, debug_overlay, hud_scale,
touch: { long_press_ms, tap_max_ms, tap_slop, double_tap_ms, double_tap_slop, long_press_right_click } }`.
`S3_MODS_DEBUG=1` turns on the built-in debug overlay (finger circles + last gesture label).

## Desktop testing (`s3run --inject` touch commands)

Screen coordinates = game pixels (800x600 menus, 640x480 in game): `tap x y`, `tap2 x y`,
`drag x1 y1 x2 y2 [ms=300]` (moves every 16 ms), `longpress x y [ms=800]`, `pinch cx cy scale [ms=300]`
(fingers 80 px apart -> 80*scale). Example (menu -> in game by taps, map drag, build menu tap):

```sh
I="1200:tap 165 235;3200:tap 630 467;5200:tap 730 567;7200:tap 730 567;30000:shot a.png;31000:drag 400 300 300 250 400;33000:shot c.png;34000:tap 25 167;36000:shot e.png"
S3_MODS_DEBUG=1 ./target/release/s3run --headless --game-dir ../game-files/gog-s3-ultimate --data-dir ../scratch/x --max-seconds 40 --inject "$I"
```

Verified 2026-11-06: the game reached the in-game view by taps only. The drag scrolled the map with the finger, the tap
opened the build menu, and the debug overlay showed the finger and the "Pan1 Update" / "Pan2" labels.
Screenshots were in `scratch/mods-w1/` (local only).

## Performance

- CPU microbenchmark `bench_mips` (small blocks, worst case for a per-block check), with vs without the `blk.hooked`
  test (copy of s3-cpu without it), 3 interleaved runs: 189.4/189.0/188.7 vs 185.5/183.4/180.8 MIPS: no measurable
  cost (the difference is code-layout noise).
- In game (`S3_SAMPLE=1`, sum of guest M instr/s over all threads, samples >= 25 s). gog-base hooks on vs `S3_MODS=0`,
  two runs **in parallel** so host noise is shared: 97.2 vs 97.0 and 95.0 vs 94.9 M/s. The overhead is within noise, < 1%
  (target < 2%). Sequential runs differed by up to 10% between runs because of other jobs on the host; do not compare
  them.
- Hook callbacks: ~40-120 dispatches per run in total. The cost is dominated by what a hook does (a `HashMap` lookup
  plus a closure clone per hit).

## Tests

`cargo test -p s3-mods` (19 + doctest): SHA-256 vectors, settings round trip/partial/broken files, gestures (tap,
double tap, long press + drag, slop, pinch, 2/3-finger taps, cancel), fallback mapping, overlay primitives/text/blit,
guest struct views, and machine-level tests on hand-assembled code: skip a stdcall function, post-call trampoline
changing EAX, mid-block hook, guest calls (stdcall/cdecl/thiscall/virtual) from a hook incl. re-entry, deferred work at a
safe point, touch pipeline (fallback click, pointer capture by a mod, overlay on present, key passthrough).
`s3-cpu`: `hook_exits_mid_block_and_skip_once`. `s3-desktop`: `inject_touch_parsing`.
