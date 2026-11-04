# Port architecture (binding contract)

Status: foundation in place (2026-11-05). Code: `port/` (Cargo workspace, GPL-3.0-or-later).
Approach: our own host = PE loader + x86-32 **interpreter** (iced-x86 decoder) + **HLE** of the Win32/DirectX
APIs S3.EXE uses. The user's own `S3.EXE` and data run unmodified; nothing of the game is shipped.

## Crates

| Crate | What | Owner after foundation |
| --- | --- | --- |
| `s3-mem` | `GuestMemory`: 4 GiB host reservation, typed reads/writes, strings, `Regions` (VirtualAlloc bookkeeping), `Heaps` (HeapAlloc) | runtime |
| `s3-cpu` | `Cpu`: x86-32 interpreter, decoded-block cache, eager EFLAGS, `fpu.rs` (x87 + MMX movq/movd/emms, see [x87-fpu.md](x87-fpu.md)) | cpu |
| `s3-runtime` | `Machine` (memory + threads + scheduler), thunk `Registry`, `ThunkResult`, `call_guest`, `HandleTable`, SEH, COM helpers, `Vfs`, `Host` trait, `FrameBuffer`, `HostEvent`, input state | runtime |
| `s3-pe` | PE32 parse/load, imports via the registry, `find_resource` / `load_string` | runtime |
| `s3-win32` | one module per DLL: kernel32, user32, gdi32, advapi32, shell32, rpcrt4, ole32, winmm (`_inmm` alias), comctl32, comdlg32, wsock32, wininet | per-DLL agents |
| `s3-dx` | ddraw, dsound, dinput, dplay: COM objects, blitter, software audio mixer, input pump, CLSID factories ([directx-hle.md](directx-hle.md)) | dx agents |
| `s3-engine` | `Engine::new(host, exe)`: registers modules, loads PE, creates the main thread; `run_slice`, `run_for_ms`, `summary` | runtime |
| `s3-desktop` | `s3run` binary: headless or minifb window, PNG screenshots, input injection | desktop |
| `s3-android` | `libs3android.so` (NativeActivity via android-activity): engine thread, letterboxed RGB565 window, touch -> mouse, AAudio. See [android.md](android.md) | android |

Dependency direction: `s3-mem` <- `s3-cpu` <- `s3-runtime` <- `s3-pe`, `s3-win32`, `s3-dx` (s3-dx does not depend on s3-win32; they meet through `s3_runtime::gdi` and `s3_runtime::audio`) <- `s3-engine` <- frontends.

## Memory map (guest addresses, `s3_mem::layout`)

| Range | Use |
| --- | --- |
| `0x00000000-0x0000FFFF` | NULL guard: the CPU raises `AccessViolation` (-> guest exception 0xC0000005) for any access here |
| `0x00010000-0x003FFFFF` | low arena: process parameters, command line, environment blocks |
| `0x00400000-0x007ECFFF` | S3.EXE image (fixed base, no relocations), region kind `Image` |
| `0x10000000-0x6FFFFFFF` | VirtualAlloc / heap segments (first fit, 64 KiB granularity) |
| `0x70000000-0x7EFFFFFF` | thread stacks (default size = PE stack reserve, 1 MiB) |
| `0x78000000+i*0x10000` | fake HMODULEs of HLE DLLs (not mapped; only handle values) |
| `0x7F000000-0x7FFEFFFF` | system area: TEB of thread slot `i` at `0x7FFDE000 - i*0x1000`, PEB at `0x7FFDF000` |
| `0xFFF00000-0xFFFFEFFF` | thunk page: each HLE function is one address (`THUNK_BASE + index`) |
| `0xFFFFF000` | `MAGIC_RETURN`: return address used by host->guest calls |

The whole 4 GiB window is mapped read/write (`MAP_NORESERVE`), so guest accesses never fault on the host.
`Regions` only answers Win32 questions (VirtualQuery, overlap, free space). Released/decommitted ranges are
zeroed with `madvise(MADV_DONTNEED)` on host-page-aligned parts (Android may use 16 KiB host pages).
Heaps keep their metadata on the host side (guest overruns cannot corrupt it); blocks are 8-byte aligned,
heap handle = guest address of the first segment, `GetProcessHeap` = `mem.process_heap()`.

## CPU (`s3-cpu`)

> Details of the integer interpreter (decode cache, threaded dispatch, exact semantics, tests, MIPS):
> [cpu.md](cpu.md). Changes since the foundation: cpuid reports **no MMX** (family 5 model 2), `jcxz`/CX loops,
> BCD ops, the popfd mask, and privileged instructions raise `GeneralProtection`.

- `Cpu { regs: [u32; 8] (EAX..EDI in encoding order, consts EAX..EDI), eip, eflags, fs_base, gs_base, fpu, instructions, cache }`.
- `run(&mut mem, max_instructions) -> CpuExit`:
  `Thunk(addr)` (EIP entered the thunk page; ESP points at the return address), `ReturnMagic`, `Budget`,
  `Halt`, `Fault { eip, kind, detail }` (EIP rewound to the faulting instruction), `Interrupt(n)` (`int3` = 3).
- Decode cache: basic blocks (<= 64 instructions, end at any control flow) keyed by start EIP, one cache per
  `Cpu` (i.e. per guest thread; TODO share it if memory becomes an issue). `invalidate_range` /
  `Machine::invalidate_code` after code writes. S3.EXE does not modify its code (see s3-exe.md).
- Implemented: full i386/i486/Pentium integer set used by MSVC6 code (mov/movzx/movsx/lea/xchg, push/pop(ad/fd),
  call/ret/jmp/jcc/loop*/jecxz, ALU + inc/dec/neg/not, all shifts/rotates incl. rcl/rcr/shld/shrd, mul/imul
  (1/2/3 op)/div/idiv (#DE), cbw/cwde/cwd/cdq, setcc/cmovcc, bt*/bsf/bsr/bswap, xadd/cmpxchg/cmpxchg8b,
  enter/leave, string ops with rep/repe/repne (fast paths for forward movs/stos), xlat, lahf/sahf, flag ops,
  cpuid (Pentium, no MMX/SSE; see cpu.md), rdtsc (host time at 200 MHz), int/int3/into/hlt/ud2).
- Anything else goes to `fpu::exec` (x87 + MMX subset, see [x87-fpu.md](x87-fpu.md)); it returns `None` for anything
  it does not handle -> `FaultKind::Unimplemented` with the mnemonic and disassembly.
- Helpers for new instructions: `read_op/write_op(ins, i, size)`, `op_size`, `ea`, `rd/wr` (NULL check),
  `alu_add/alu_sub/alu_logic/set_szp/cond`, `get_reg/set_reg` (iced `Register`).

## Thunk ABI

1. The PE loader writes a thunk address into every IAT slot (`Registry::resolve_import`). Unknown imports get an
   *auto stub* (no handler).
2. Guest `call [IAT]` pushes the return address and jumps to the thunk; the CPU exits with `Thunk(addr)`.
3. `Machine::dispatch_thunk` counts the call, traces it, and runs the handler `fn(&mut Machine) -> ThunkResult`.
   No handler => machine stops with `UNIMPLEMENTED dll!name (called from 0x...)`.
4. `ThunkResult`:
   - `Return { eax, pop_bytes }` — EAX = value, pop return address, then `pop_bytes` (stdcall `4*nargs`, cdecl 0).
     Helpers: `ThunkResult::stdcall(eax, nargs)`, `ThunkResult::cdecl(eax)`.
   - `Return64 { eax, edx, pop_bytes }`.
   - `Done` — the handler already set EIP/ESP (e.g. RaiseException redirecting into SEH dispatch).
   - `Block` — the thread waits; EIP stays on the thunk and **the handler is called again** on the next
     scheduling round. Handlers that may block must not have side effects before deciding to block.
     `m.thread().block_since` holds the host ms when the wait started (for timeouts).
   - `Exit(code)` (ExitProcess), `ExitThread(code)`, `Error(msg)` (stop the machine).
5. COM methods are ordinary thunks named `Interface::Method` in the implementing DLL
   (`ddraw.dll!IDirectDrawSurface::Flip`); `this` is `m.arg_u32(0)`.

Registry API: `register(dll, name, handler)` (replaces), `declare(dll, name)` (stub, keeps a handler),
`declare_all`, `set_ordinal`, `lookup`, `lookup_ordinal`, `alias("_inmm.dll", "winmm.dll")`,
`module_handle(dll)` / `module_name(hmod)` (fake HMODULEs for LoadLibraryA/GetProcAddress), `describe(addr)`.
DLL names are normalized (lower case, `.dll` added, path stripped).

## Calling-convention helpers (on `Machine`)

`arg_u32(i)`, `arg_i32(i)`, `arg_ptr(i)` (= `[esp + 4 + 4*i]`), `arg_str(i)` (ANSI, `None` for NULL),
`arg_wstr(i)`, `return_address()`, `set_last_error(v)` / `last_error()` (TEB+0x34), `cpu()`, `thread()`,
`current_tid()`, `now_ms()`, `log(level, msg)`, `present(&fb)`.
Guest memory: `m.mem.read_u32/write_u32/...`, `read_cstr/write_cstr`, `read_wstr/write_wstr`, `slice/slice_mut`,
`alloc(size)` / `free(p)` (process heap, zeroed) for runtime-owned guest buffers.

Module state: `let st = m.state::<MyState>();` then `st.borrow_mut()` (type must be `Default + Send`). The guard
must not be held across `call_guest` (re-entry panics with a clear message).

## Host -> guest calls

`m.call_guest(addr, &[args]) -> u32` (WndProc, enum callbacks, timer callbacks, SEH unwind handlers):
pushes args + `MAGIC_RETURN`, runs the current thread until `ReturnMagic`, then restores **all** registers
(including ESP), so stdcall and cdecl callees both work. Nested calls are fine. While the callee blocks, the
other threads run (`run_others`). A thread inside `call_guest` (`nested > 0`) is skipped by the top-level
scheduler. Limitation: if thread B, run from inside A's nested wait, itself blocks inside a nested call waiting
for A, they deadlock (logged as no progress; not seen yet).

## Threads and scheduling

Cooperative green threads on one host thread. `Machine::create_thread(entry, param, stack, suspended)` builds
the stack (`[esp] = s3port.dll!thread_exit`, `[esp+4] = param`), the TEB (SEH list = -1, stack base/limit,
self, ids, TLS pointer = TEB+0xE10 with 64 slots, PEB, LastError) and a `Thread` kernel object handle.
`run_slice(max_instructions)`: polls host input into `m.input`, runs services (`add_service`, host-side
periodic hooks such as an audio mixer), then round-robins runnable threads with `quantum` instructions each
(default 200k). Blocked threads are retried each round. Returns `Ran(n)`, `Idle` (everything blocked; the
frontend sleeps) or `Stopped` (`stop_reason()`: `Exited`, `Unimplemented`, `Fault`, `Error`, `Quit`).
Waits: `m.is_signaled(h)` / `m.try_acquire(h)` implement Event (manual/auto), Mutex (owner + recursion),
Semaphore, Thread (signaled when exited). Critical sections are implemented by kernel32 on the guest struct
(owner/recursion), blocking when contended, with direct hand-off on Leave.
Generic waits: `m.poll_wait(&handles, wait_all, timeout_ms) -> WaitPoll` (Ready/Timeout/Failed/Pending; return
`ThunkResult::Block` on Pending) — use it for any waiting API (user32 MsgWaitForMultipleObjects too).
`m.pulse_event(h)`; waitable timers are `KernelObject::Timer`. Details: [kernel32.md](kernel32.md).

## Exceptions (SEH)

CPU faults become guest exceptions (0xC0000005 with [write, addr], 0xC0000094, 0xC000001D, 0xC0000096,
int3 -> 0x80000003). `seh::raise_exception` writes EXCEPTION_RECORD + CONTEXT (0x2CC bytes) below ESP and calls
the FS:[0] chain handlers **in the guest** with return address `s3port.dll!seh_handler_return`
(ContinueExecution resumes from the CONTEXT, ContinueSearch goes to the next frame; no handler = machine stops
with "unhandled exception"). MSVC `__except` handlers unwind via `RtlUnwind` (implemented in kernel32 with
`seh::rtl_unwind`, which calls handlers with EXCEPTION_UNWINDING via `call_guest`) and jump away; stale
dispatch records are dropped on the next raise. `kernel32!RaiseException` is implemented (C++ `throw`).
Unimplemented instructions are **not** turned into exceptions; they stop the machine.

## Handles

`m.handles`: `insert(KernelObject)`, `insert_named`, `open_named`, `get/get_mut`, `get_other::<T>()` for
module-owned objects (`KernelObject::Other("file", Box<T>)`), `duplicate`, `close`. Handles start at 0x100,
step 4. Pseudo handles: `CURRENT_PROCESS = -1`, `CURRENT_THREAD = -2`.

## VFS

The guest sees the game at `C:\S3` (`GUEST_GAME_DIR`), module path `C:\S3\S3.EXE`, command line `S3.EXE`.
`m.vfs.components(path)` normalizes (drive letters, `\`/`/`, `.`/`..`, relative to `cwd`); any other drive root
(CD paths) maps to the game dir too. `resolve_read` checks `data_dir/overlay/` first, then `game_dir`, both
case-insensitive. `resolve_write` returns an overlay path (creating parents); `copy_up` copies a game file into
the overlay before read/write opens. `list_dir` merges both; `wildcard_match` for FindFirstFile; `set_cwd`.
The game dir is never written; deletes/renames of game-dir files are recorded as whiteouts
(`data_dir/overlay-whiteouts.txt`). File ops: `remove_file`, `rename`, `copy`, `create_dir`, `remove_dir`, `create_path`.
Memory-mapped file views: `s3_runtime::mapfile` (mmap into the guest window). PE resources: `s3_runtime::resources`.

## Host trait (frontends)

```rust
pub trait Host: Send {
    fn present(&mut self, fb: &FrameBuffer);            // RGB565 width*height
    fn poll_events(&mut self) -> Vec<HostEvent>;        // MouseMove{x,y} MouseButton{button,down} Key{vk,down} Char Quit Pause Resume
    fn queue_audio(&mut self, samples: &[i16], rate: u32, channels: u16) {}
    fn audio_queued_frames(&self) -> usize { 0 }
    fn now_ms(&self) -> u64;
    fn game_dir(&self) -> PathBuf;
    fn data_dir(&self) -> PathBuf;
    fn log(&self, level: LogLevel, msg: &str) {}
    fn idle(&mut self, ms: u32) {}                      // all threads blocked
}
```
Input consumers (USER32 queue, DirectInput) read `m.input` (current mouse/buttons/keys) and
`m.input.events_since(&mut cursor)` (each keeps its own cursor; start with `cursor_now()`).
`NullHost` is the headless implementation used by tests.

## COM

`m.com.register_class(clsid, name, factory)`; `ole32!CoCreateInstance` looks the CLSID up and calls
`factory(m, iid) -> Result<iface_ptr, HRESULT>`. Helpers: `m.com_vtable(dll, iface, &METHODS)` (cached),
`m.com_new_object(vtable, size)` (layout `[vtbl][refcount][impl...]`), `Machine::com_addref/com_release`
handlers, `Guid::parse("...")` (const), `Guid::read/write`, Display as `{XXXXXXXX-...}`.
Registered: CLSID_DirectDraw (IDirectDraw/IDirectDraw2), CLSID_DirectPlay, CLSID_DirectPlayLobby; all DirectX
methods are implemented, see [directx-hle.md](directx-hle.md).

## Audio output

The DirectSound mixer service in s3-dx is the only caller of `Host::queue_audio` (44100 Hz stereo i16). Other
producers (winmm waveOut / CD music) push PCM into `m.state::<s3_runtime::audio::AudioStreams>()`; the mixer
resamples and mixes them (details in [directx-hle.md](directx-hle.md)).

## Adding an HLE function

Example (`s3-win32/src/kernel32.rs`):

```rust
pub fn register(reg: &mut Registry) {
    reg.declare_all(DLL, NAMES);                // keeps UNIMPLEMENTED reporting for the rest
    reg.register(DLL, "GetTickCount", get_tick_count);
    reg.register(DLL, "HeapAlloc", heap_alloc);
}

fn get_tick_count(m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(m.now_ms() as u32, 0)  // stdcall, no args
}

fn heap_alloc(m: &mut Machine) -> ThunkResult {
    let (h, flags, size) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    let p = m.mem.heap_alloc(h, size, flags).unwrap_or(0);
    ThunkResult::stdcall(p, 3)                  // pops 3 args
}
```
Rules: get the arg count right (wrong `pop_bytes` corrupts the guest stack; MSVC `wsprintfA` etc. are cdecl);
set LastError on failure where the game may check it; mark guesses as HYPOTHESIS in the docs; add a unit test
with a hand-assembled snippet when behaviour is subtle (see `s3-runtime/src/tests.rs`).

KERNEL32 is complete (all static imports + GetProcAddress targets such as the waitable-timer API); see
[kernel32.md](kernel32.md) for the semantics (scheduler waits, critical sections, timers, VFS file ops, mapped files,
locale). Older note: implemented in the foundation were kernel32 GetVersion (Win98 value 0xC0000A04), GetCurrentProcess/Thread/ThreadId,
GetTickCount, Get/SetLastError, ExitProcess, GetCommandLineA, GetModuleHandleA, LoadLibraryA/GetProcAddress/
FreeLibrary (HLE modules only; GetProcAddress returns 0 for undeclared names and logs), Heap*, Virtual*,
Interlocked*, RaiseException, RtlUnwind; ole32 CoInitialize/CoUninitialize/CoCreateInstance; winmm timeGetTime.

## USER32 / GDI32 / registry / WINMM (summary)

Details: [user32-gdi32.md](user32-gdi32.md). Interfaces for other crates:
- **Surface DCs:** `s3_runtime::gdi::{DcTarget, create_dc, release_dc}` (`s3-runtime/src/gdi.rs`); s3-dx calls
  `create_dc` in `IDirectDrawSurface::GetDC` with the surface memory and `release_dc` in `ReleaseDC`. gdi32 installs
  the implementation in `gdi32::register`.
- **Display mode:** s3-dx calls `s3_win32::user32::set_screen_size(m, w, h, bpp)` from `SetDisplayMode`
  (GetSystemMetrics, full-screen windows, WM_DISPLAYCHANGE).
- **Music:** `s3_win32::winmm::set_external_music_mixer(m, true)` + `winmm::mix_music(m, buf, rate, ch)` if a
  DirectSound mixer owns `Host::queue_audio`; otherwise winmm queues 44100 Hz stereo itself.
- **Messages from other modules:** `s3_win32::user32::post_message(m, hwnd, msg, wp, lp)`.

## Building, testing, running

Always share the one target dir and cap jobs:

```sh
cd port
export CARGO_TARGET_DIR=$PWD/target
cargo build -j 4
cargo test -j 4                      # all crates; s3-pe also loads the local S3.EXE if present
cargo test -p s3-cpu -j 4            # one crate
./target/debug/s3run --headless --max-seconds 20 \
    --game-dir ../game-files/gog-s3-ultimate --data-dir ../scratch/data
S3_TRACE=calls S3_TRACE_LIMIT=500 ./target/debug/s3run --headless ...   # trace HLE calls with 4 args
./target/debug/s3run --game-dir ... --data-dir ... --screenshot out.png --inject "3000:click 400 300;9000:quit"
```

`profile.dev` uses opt-level 1 (deps opt-level 3) because the interpreter is too slow unoptimized.
`s3run` prints a summary at exit (instructions, thunk calls, presents, top called functions, stop reason);
exit code 0 = guest exited 0 or time limit, 3 = stopped with an error. `--screenshot-every-present` writes at
most 300 PNGs.

Android: `scripts/build-apk.sh` (APK, both ABIs) and `scripts/run-emulator.sh start install push launch shot`;
details in [android.md](android.md). cargo-ndk lives in `~/.cargo/bin`, which is not on the default PATH.

## First integration result (2026-11-05)

`s3run --headless` against the GOG S3.EXE: loads (entry 0x621BC6, 219 imports, all declared), runs the CRT
startup: GetVersion, HeapCreate, HeapAlloc, then stops at
`UNIMPLEMENTED kernel32.dll!InitializeCriticalSection (called from 0x624a11)` after 67 instructions.
Next gaps are expected to be the rest of kernel32 CRT init (critical sections, TLS, environment, GetStartupInfoA,
GetStdHandle/GetFileType, SetHandleCount, GetCPInfo/GetACP, GetModuleFileNameA, ...).
