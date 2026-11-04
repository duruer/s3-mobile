# Journal

Newest first. One entry per session: what was done, what was learned, links to docs updated.

## 2026-11-05 (c) — tooling inventory

- Documented RE tools (`~/Projects/reverse-engineering`, `/opt/ghidra`) and the Android SDK/NDK/AVDs → [tooling](tooling.md).
- REA in this session is our own stdio server, so open/close is safe. In the shared multi-agent setup it is not (see tooling).

## 2026-11-05 (b) — static RE with REA/Ghidra

- Opened S3.EXE in REA (Ghidra 12.1.2 auto-analysis, ephemeral DB, nothing saved).
- XOR at 0xFE72 = decoder for a registry-stored blob (HKLM value + HKCR key) → [exe-variants](game-files/exe-variants.md), [unknowns](reverse-engineering/unknowns.md) #1.
- `ProtectCD.dll`: no code xrefs → dead string (static view).
- DirectX surface mapped: IDirectDraw2 via CoCreateInstance, DirectSound via DirectSoundCreate, DirectInput 3.0, DirectPlay 6; full method list from the game's own error strings → [directx-surface](reverse-engineering/directx-surface.md).
- VirtualProtect only in a page-touch helper, no self-modifying code seen → good for an interpreter/JIT.

## 2026-11-05 — project bootstrap

- Created `~/Projects/settlers-3` with umbrella `AGENTS.md` (index only), `docs/`, `tools/`, gitignored `game-files/`.
- Found the GOG install on Windows C: (`/dev/nvme0n1p2`, `Loki`) at `C:\GOG Games\Settlers 3 Ultimate`; Settlers United at
  `C:\Program Files\Settlers United`. Not on the `Luna` disk. Copied both (game 1.6 GB, SU resources 385 MB) → [inventory](game-files/inventory.md).
- `S3.EXE` vs `s3_united.exe`: 7-byte diff (NOPed XOR decoder at 0xFE72; `SingleNumber`→`SerialNumber`) → [exe-variants](game-files/exe-variants.md).
- `S3.EXE`: MSVC 6, base 0x400000, no relocs, 219 static imports; DirectX/winsock/MCI loaded dynamically → [s3-exe](reverse-engineering/s3-exe.md).
- Registry defaults from `regs.cmd` → [registry](reverse-engineering/registry.md).
- Installed rizin, wine, wine-mono, winetricks, python-pefile (via kdesu). Added `tools/pe_imports.py`.
- Draft strategy: own PE loader + x86 core + Win32/DirectX HLE (blockheads-pc pattern) → [strategy](porting/strategy.md).
- Next: run under Wine with relay trace (strategy "Next steps").
