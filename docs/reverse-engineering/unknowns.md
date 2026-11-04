# Open questions

Mark resolved items with date + link to the doc that answers them; do not delete.

1. **PARTLY RESOLVED 2026-11-05**: XOR decoder at file offset `0xFE72` is in `FUN_0040fb40`/`FUN_0040fdec`: it reads a value from an HKLM key and a key/seed from an HKCR key (via `FUN_00616900`/`FUN_00618030`/`FUN_00617c40`), then decodes the blob with a rolling key (`key *= 0xFED42531`). GOG's NOP makes it a plain copy. Still open: which key/value names (`DAT_007a7fbc`, `DAT_007a7f8c`, `DAT_007a7d6c`, `DAT_007a7d5c` are filled at runtime). ([exe-variants](../game-files/exe-variants.md))
2. **RESOLVED (static) 2026-11-05**: the string `ProtectCD.dll` (VA `0x7876f8`) has **no code references** in Ghidra → most likely dead. Confirm with a runtime trace.
3. Minimum file set to run (which of GFX 555/565, S3/S3QOTA/S3MCD, editor, Save are needed).
4. Where are intro/cutscene AVIs? None in the GOG install — does the game skip them gracefully (`Intro` registry)?
5. **RESOLVED 2026-11-05**: yes, DirectInput **3.0** with buffered `GetDeviceData`; see [directx-surface](directx-surface.md). Whether window messages are used as well is still open.
6. **PARTLY RESOLVED 2026-11-05**: IDirectDraw2 via CoCreateInstance; method list in [directx-surface](directx-surface.md). Open: modes requested, pixel format, whether the primary surface is locked directly.
7. Game timing: fixed tick? Can we pause/resume safely on Android lifecycle events?
8. What is `DATA.DAT` (27 KB)?
9. What exactly does Settlers United `s3.dll` hook (D3D9 renderer, bug fixes, AI `s3.ai.dll`)? Which fixes matter for stability?
10. `VirtualProtect`/`VirtualQuery` use: `FUN_0061ab60` makes a memory range writable, touches every page, restores protection (pre-commit / page-in helper). **Not** self-modifying code (2026-11-05). Who calls it is open.
11. Main loop: `PeekMessageA` is called at `0x60d6d5`, `0x60d9f0`, `0x60f452`, `0x60f893`; `CreateThread` only at `0x6227db` (probably CRT `_beginthread`). The game uses several `CGameThread`s. Map the threads and the frame loop.
12. DirectPlay CLSIDs/IIDs used at `0x5c0808` / `0x5c0847`.
