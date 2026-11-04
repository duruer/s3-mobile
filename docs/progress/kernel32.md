# Progress: kernel32 (+ s3-runtime scheduler/sync/SEH/VFS, s3-pe)

Goal: implement all KERNEL32 imports of S3.EXE with real semantics, complete PE resources API, unit tests,
then run s3run --headless and fix kernel32-side blockers.

## Done
- s3-runtime: KernelObject::Timer, HandleTable::object_id/name_of, Machine::poll_wait/wait_timed_out/pulse_event
  (WaitPoll), Thread fields wait_objs/pulsed/wait_cs/cs_granted/priority, resources.rs (moved from s3-pe, re-exported),
  mapfile.rs (mmap file views), hosttime.rs, VFS whiteouts + remove_file/rename/copy/create_dir/remove_dir/create_path.
- s3-win32: kernel32.rs (process/module/memory/errors/resources) + kernel32_{sync,file,time,locale}.rs + kernel32_tests.rs
- Tests: 13 kernel32 tests pass (checked via a temp crate /tmp/k32check because s3-win32 did not compile at the time
  due to gdi32 work in progress by another agent).

- docs/porting/kernel32.md written; architecture.md + AGENTS.md updated.
- Headless: menu, Game Setup and an in-game map reached (with temporary stubs for other modules' functions, removed).
- Tests pass in the workspace: s3-win32 31, s3-runtime 9, s3-pe 2.

## Next (open)
- Abandoned mutexes on thread exit; APC completion routines for SetWaitableTimer; writable file views write-back.

## How to verify
- cargo test -p s3-win32 -j 4 (kernel32_tests), cargo test -p s3-runtime -p s3-pe
