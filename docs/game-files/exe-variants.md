# S3.EXE variants

All four are 3,842,048 bytes, same base build (PE timestamp 2000-03-31 17:09:52, i.e. v1.60 era — HYPOTHESIS on exact version).

| File | Used by | Diff vs `S3.EXE` |
| --- | --- | --- |
| `S3.EXE` | GOG "Settlers 3" play task | reference |
| `S3_multi.EXE` | GOG "Settlers 3 [Multiplayer]" task | identical to `s3_united.exe` |
| `s3_united.exe` | Settlers United | 7 bytes (below) |
| `s3_alobby.exe` | old aLobby | 488 bytes (not analysed) |

## S3.EXE → s3_united.exe (7 bytes)

| File offset | GOG `S3.EXE` | `s3_united.exe` | Meaning |
| --- | --- | --- | --- |
| `0xFE72` (2 B) | `90 90` (nop nop) | `32 D1` (`xor dl, cl`) | GOG NOPed the XOR of a rolling-key byte decoder: `mov dl,[edi]; mov ecx,[ebp-18h]; xor dl,cl; imul ecx,ecx,0FED42531h; mov [eax],dl` (key = LCG-style multiply). With the NOP, bytes are copied unchanged. OBSERVED (2026-11-05, Ghidra `FUN_0040fb40`→`FUN_0040fdec`): decodes a **registry-stored blob**. The blob comes from an HKLM value and the key/seed from an HKCR value (`rand()` if the HKCR value is missing). GOG's NOP turns it into a plain copy, probably so the value written by `regs.cmd` is accepted without its HKCR partner. Exact key names are still open ([unknowns](../reverse-engineering/unknowns.md) #1). |
| `0x295A1C` (5 B) | `"Single"Number` | `"Serial"Number` | Registry value name under `HKLM\SOFTWARE\Bluebyte\Siedler3\1.0\General`. GOG's `regs.cmd` writes `SingleNumber`; the multiplayer exe reads `SerialNumber` (CD key for online play). |

Port implication: we should support at least the GOG `S3.EXE` hash. Detect by sha256 and apply our own
in-memory patches if needed (never ship patched exes).
