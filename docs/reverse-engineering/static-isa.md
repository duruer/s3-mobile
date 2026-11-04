# S3.EXE instruction-set census (static)

Date: 2026-11-05. Target: GOG `S3.EXE` (sha256 `00a925cf…`). Audience: the CPU-interpreter agents.
Question answered: which x86 instructions must the interpreter implement, and which must be bit-exact.

## Method

- Tool: [`tools/isa-census`](../../tools/isa-census) (our Rust code, iced-x86 decoder, prints statistics only).
  Build/run: `CARGO_TARGET_DIR=port/target cargo build --release -j 4 --manifest-path tools/isa-census/Cargo.toml`,
  then `port/target/release/isa-census game-files/gog-s3-ultimate/S3.EXE --data-seeds --list-rare`.
- **Recursive descent**, not linear sweep. Seeds: entry point, every 32-bit immediate that points into `.text`
  (callbacks, thread procs, `atexit` funcs), every dword in `.rdata`/`.data` that points into `.text` (vtables,
  jump tables, static-init tables). `jmp [idx*4+table]` tables are followed. Result: **663 233 instructions,
  2 268 855 of 2 439 158 `.text` bytes (93 %)**, 186 jump tables, 355 overlap conflicts and 103 invalid stops
  (these come from data dwords that only look like code pointers; a handful of the "rare" instructions below come
  from them).
- Linear sweep (`objdump -d`) over-counts badly: `.text` has 118 501 bytes of `0x90` padding between game
  functions and 17 609 bytes of `0xCC` padding between CRT functions, plus switch jump tables and byte index
  tables (MSVC `mov dl,[esi+table]; jmp [edx*4+table2]`) inside `.text`. That is where objdump's `in`, `out`,
  `bound`, `retf`, `(bad)` come from. They are **data, not code**.
- Seeding only from the entry point reaches just 11 042 instructions: almost all game code is reached through
  vtables and function pointers, so a decoder must decode lazily at whatever EIP it lands on.
- Helper scripts: [`tools/iat_xrefs.py`](../../tools/iat_xrefs.py) lists every instruction that references an
  IAT slot. [`tools/dis.py`](../../tools/dis.py) disassembles a VA range and shows API names in place of IAT slots.

## CPUID feature classes (recursive descent, data seeds)

| Class (iced `CpuidFeature`) | Count | Meaning |
| --- | --- | --- |
| INTEL386 | 594 658 | plain 386 integer code |
| INTEL8086 | 63 397 | 8086-era encodings (string ops, `cdq`, `xchg`, …) |
| FPU (8087) | 4 993 | x87 |
| INTEL186 | 93 | `enter`/`leave`/`push imm`/`imul r,rm,imm`… |
| FPU287 | 84 | `fnstsw ax` etc. |
| MMX | 4 | `movq` ×2, `emms` ×2 (see below) |
| CPUID | 2 | CPU detection (see below) |
| FPU387 | 2 | `fsin`/`fcos`/`fprem1`-class (CRT math) |
| INTEL486 (`bswap`/`xadd`/`cmpxchg`) | **0** | |
| SSE / 3DNow! / CMOV / RDTSC | **0** | |

So: **i386 + x87 is enough.** No SSE, no 3DNow!, no `cmov`, no `rdtsc`, no `bswap`/`cmpxchg`.

## Ranked mnemonics (top part, recursive descent)

`mov` 206 102 · `push` 108 423 · `lea` 58 086 · `call` 57 364 · `jmp` 29 293 · `cmp` 24 448 · `pop` 21 890 ·
`je` 18 786 · `add` 18 176 · `xor` 17 731 · `test` 17 151 · `ret` 13 632 · `sub` 12 597 · `jne` 11 799 ·
`and` 5 660 · `dec` 4 876 · `shl` 3 683 · `inc` 3 477 · `or` 2 716 · `jbe` 2 672 · `jl` 2 066 · `jae` 1 921 ·
`fstp` 1 551 · `shr` 1 489 · `not` 1 429 · `fild` 1 426 · **`repne scasb` 1 373** (inlined `strlen`) ·
`fidiv` 1 361 · `jle` 1 354 · `jge` 1 156 · `jb` 1 130 · `jg` 827 · `imul` 721 · `nop` 705 · `ja` 642 · `sar` 518 ·
**`rep movsd` 511** · `cdq` 489 · `movzx` 347 · `movsx` 333 · `fld` 287 · **`rep movsb` 263** · `neg` 241 ·
`sbb` 227 · `sete` 225 · `idiv` 217 · `setne` 171 · `leave` 167 · **`rep stosd` 159** · `div` 155 · `xchg` 151 ·
`fmul` 86 · `fnstsw` 86 · `jns` 72 · `fcomp` 67 · `mul` 46 · **`repe cmpsb` 45** · `setge` 42 · `setl` 36 ·
`fadd` 35 · `adc` 31 · `js` 29 · `sahf` 26 · `fabs`/`movsd`/`movsw`/`wait` 19 · `fsub` 17 · `fdivr` 16 ·
`movsb` 16 · **`repe cmpsd` 16** · `fst` 15 · `stosb` 14 · `fchs` 13 · `fiadd` 12 · `setg` 11 · `fdiv` 10 ·
`rep stosb` 10 · `setle` 10.
Tail (≤ 9 each): `cld`, `fcom`, `stosd`, `fldcw` 8, `jcxz` 8, `fcompp`, `fldz`, `rcr`, `setb`, `stosw`, `fxch`, `std` 5,
`fistp`, `fld1`, `fnstcw`, `frndint`, `fsqrt`, `lock dec` 4, `xlatb` 4, `faddp`, `fscale`, `ror`, `sal`, `scasb`, `sets`,
`aam`, `bt`/`bts` 2, `cpuid` 2, `emms` 2, `enter` 2, `fist`, `fnclex`, `fnop`, `fsubr`, `jp`, `lock inc` 2, `loop` 2,
`movq` 2, `pushfd` 2, `scasd`, `seta`, `shld` 2, `stc`, `cwde`, `f2xm1`, `fdivp`, `fdivrp`, `fimul`, `fldpi`,
`fnsave`/`frstor` 1, `fyl2x` 1, `jecxz`, `lahf`, `lodsb`, `loope`, `pushad`, `rep stosw`, `setae`, `setns`.
(`in`/`out`/`iretd`/`int1`/`into`/`bound`/`arpl`/`retf`/`salc`/`insb`/`sti`/`cli`, 1–4 each, and the 5 `gs:` and 4 `es:`/`cs:`
overrides are decodes of misidentified data; they do not appear in real functions. Treat them as "raise #UD".)

## What must be exact (priority list for the interpreter)

1. **Integer core + EFLAGS**: `add/sub/adc/sbb/cmp/test/and/or/xor/inc/dec/neg/not`, shifts (`shl/shr/sar/rol/ror/rcr`,
   `shld` 2×), `imul` (1-, 2-, 3-operand), `mul`, `div`, `idiv`, `cdq`, `cwde`, `setcc`, `jcc`, `movzx/movsx`, `xchg`,
   `lea`, `leave`, `enter` (2×). `sahf`/`lahf` are used after `fnstsw ax` for float compares → PF/ZF/CF from AH matter.
   `jp`/`jnp` exist (float compare unordered checks).
2. **String ops with prefixes**: `repne scasb` (strlen, 1 373×), `rep movsd`/`rep movsb` (memcpy), `rep stosd`/`stosb`/
   `stosw` (memset), `repe cmpsb`/`cmpsd` (memcmp/strcmp), single `movs*`/`stos*`/`lods*`/`scas*`. **DF**: `std` appears
   5× (backwards `memmove` in the CRT); `cld` 9×. Respect DF.
3. **Address-size prefix `0x67` on `jcxz`**: the hand-written blitters at `0x5c3e27`, `0x5c3e2f`, `0x5c3e57`,
   `0x5c3fc5`, `0x5c3fcd`, `0x5c3fe1`… use `67 E3 xx` = **`jcxz` (tests CX, not ECX)**. iced decodes it as `Jcxz`
   (8×) vs `Jecxz` (1×). Implement both. `loop` (ECX) is used right after (`0x5c3fdc`).
4. **x87** (≈5 000 instrs). Most are static initialisers of float constants (`fild/fidiv/fstp` in ~1 360 tiny functions,
   `0x54f2xx`–`0x58xxxx`) and game math (`fmul/fadd/fcomp/fnstsw/sahf`). CRT `_ftol` = **`0x61fe98`**
   (`fnstcw`, `or ah,0x0c`, `fldcw`, `fistp qword`, `fldcw`), called 44×: **rounding control must be honoured**
   (truncate inside `_ftol`, round-to-nearest otherwise). CRT math uses `fyl2x`, `f2xm1`, `fscale`, `frndint`, `fprem`,
   `fsqrt`, `fxam`, `fldpi/fldln2/fldl2e/fldlg2`, `fnsave/frstor`, `fnstenv/fldenv`, `fnclex`. A 64-bit-double
   implementation of the x87 stack is very likely good enough (HYPOTHESIS: no code relies on 80-bit precision; game
   logic is integer — Settlers sync is lockstep, check that float results do not feed network-synced state).
5. **`fs:` segment** (5 489 uses): every C++/SEH function does `mov eax,fs:[0]; push eax; mov fs:[0],esp`
   (1 661 frames). Only `fs:[0]` (SEH chain head) and `fs:[4]` (stack base, 1 site) are read; no `fs:[0x18]`/
   `fs:[0x2c]` (TLS goes through `TlsGetValue`). FS base must point to a per-thread TIB.
6. **`lock` prefix**: `lock dec` 4×, `lock inc` 2× (plus `InterlockedIncrement/Decrement/Exchange` imports).
   With a single-threaded interpreter (one guest thread at a time) atomicity is free.
7. **CPUID**: only in the CPU detector `0x5c3900` / `0x5c3970` (leaf 1). Both run inside an SEH `__try`
   (`_except_handler3`): if `cpuid` faults, the `__except` path returns 0 ("no CPUID"). If implemented, return
   a Pentium-class family 5/6 and **EDX bit 23 (MMX) = 0** – see next point.
8. **MMX**: only one routine, `0x5c3ee0`→`0x5c3f00` (an 8-byte `movq` memcpy-to-surface loop, `emms` at `0x5c405a`),
   plus a probe `emms` at `0x5c39f0`. `0x5c3740` benchmarks five surface-copy routines (`0x5c3a40`, `0x5c3d40`,
   `0x5c4070`, `0x5c40f0`, and the MMX one **only if** CPUID says MMX) with `QueryPerformanceCounter` and keeps the
   fastest. **Report no MMX and no MMX code ever runs.** The timing loop at `0x5c4280`-`0x5c4300` calls each
   routine repeatedly and adds `(QPC delta)/QPF` until the total passes a constant: **`QueryPerformanceCounter` must
   advance between calls** or the loop never ends. If `QueryPerformanceFrequency` returns FALSE the benchmark is
   skipped (`je 0x5c4310`).
9. **No self-modifying code**: no instruction writes to an absolute `.text` address (census "store to .text" = 0).
   `VirtualProtect` is only used by the page-touch helper `0x61ab60` on memory-mapped GFX files. A decode cache per
   EIP is safe. `.text` contains jump tables and byte tables, so never pre-decode linearly.
10. **Calls**: 57 k direct `call rel32`, 1 679 indirect `call [reg+off]` (vtables), imports via `call [IAT]` **and**
    `mov reg,[IAT]; call reg` (e.g. `PeekMessageA` at `0x60d6d5`, `GetProcAddress` at `0x60ceb5`). 6 import thunks
    `jmp [IAT]` (e.g. `RtlUnwind` at `0x62ef78`). So HLE must trap on *executing* the IAT target address, not on
    the call instruction pattern.

## Not present

`rdtsc`, SSE (the 7 `movups/movlps/divps/orps/movmskps` hits in linear sweep are data), 3DNow!, `cmov`, `bswap`,
`cmpxchg`, `xadd`, `int 0x2e`/`sysenter` (no direct syscalls), `int 3` in reached code (9 hits are padding at the
end of functions after no-return calls).
