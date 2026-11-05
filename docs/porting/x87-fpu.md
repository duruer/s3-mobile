# x87 FPU and MMX emulation (`s3-cpu`)

Code: `port/crates/s3-cpu/src/fpu.rs` (state + dispatch), `fpu_conv.rs` (pure numeric helpers: 80-bit/BCD
conversion, bit-exact f32<->f64, rounding/precision control, `ldexp`), `fpu_math.rs` (deterministic
transcendentals, exact `fprem`, x87 NaN rules, host FP environment check), `fpu_tests.rs` (hand-assembled
snippets, no game bytes), `fpu_golden.rs` (golden-vector determinism hashes). **Determinism contract:
[determinism.md](determinism.md)** — never call host libm / `%` on floats / let the host create NaNs in fpu*.rs.
Entry point: `fpu::exec(cpu, mem, ins) -> Option<R>`, called by `exec.rs` for every mnemonic the integer core does not
handle. `None` = not an FPU instruction / not implemented -> `FaultKind::Unimplemented`.

Instruction census that drove this: [static-isa.md](../reverse-engineering/static-isa.md) (~5 000 x87 instructions,
MMX only in one optional blitter).

## Model

| Part | How it is emulated |
| --- | --- |
| Registers | 8 x `f64` (`regs`, physical order), `top`, `st(i) = regs[(top+i)&7]` |
| Tag word | only empty / in-use is stored (`tags`, 2 bits per reg, 3 = empty). Full tags (valid/zero/special/empty) are computed by `tag_word()` for `fnstenv`/`fnsave` |
| Status word | `status` without TOP; `status_word()` merges TOP. C0-C3, IE/ZE/OE/PE/SF, ES/B |
| Control word | `control`. **RC honoured** by `fist`/`fistp`/`fisttp`(always trunc)/`frndint`/`fbstp`. **PC=24 bit** rounds arithmetic results (`+ - * / sqrt`, fyl2x) to a 24-bit significand (wide exponent kept); PC=53/64 both give f64 precision. Arithmetic itself always rounds to nearest |
| Stack faults | reading an empty `st(i)` -> IE+SF, C1=0, value = indefinite NaN; pushing onto an in-use slot -> IE+SF+C1, indefinite NaN pushed (masked response) |
| Exceptions | flags are set (IE for invalid, ZE for x/0, OE on overflow to inf, PE on inexact integer conversion/frndint). **Unmasked exceptions are not delivered** (no #MF / `EXCEPTION_FLT_*` SEH); only ES/B are set. HYPOTHESIS: the game never unmasks FPU exceptions (MSVC default CW 0x027F, all masked) |
| Last IP/DP | `last_ip` = address of the last non-control x87 instruction, `last_dp` = its memory operand; opcode field stored as 0 |

## Instructions

- Loads: `fld m32/m64/m80/st(i)`, `fild m16/m32/m64`, `fbld`, `fld1/fldz/fldpi/fldl2e/fldl2t/fldlg2/fldln2` (f64-rounded constants).
- Stores: `fst/fstp m32/m64/m80/st(i)`, `fist/fistp m16/m32/m64` (out of range or NaN -> integer indefinite 0x8000.. + IE), `fisttp`, `fbstp` (BCD indefinite on overflow).
- Arithmetic incl. reverse and pop forms and integer-memory forms: `fadd/fsub/fsubr/fmul/fdiv/fdivr` (+`p`, `fi*`). Operand roles come from iced's decoded operands (op0 = destination), so the `DC/DE` reg-reg encodings are correct without special cases.
- `fchs fabs fsqrt frndint fscale fxtract fprem fprem1 fsin fcos fsincos fptan fpatan f2xm1 fyl2x fyl2xp1`.
  - `fprem/fprem1` always produce the complete remainder (C2=0) by exact integer long division (`math::fprem`); quotient bits Q2/Q1/Q0 (exact for any exponent difference) go to C0/C3/C1.
  - Transcendentals use the pinned pure-Rust `libm` crate (fdlibm port), not the host libm; `f2xm1` uses an exact x*ln2 product (<= 1 ulp). See determinism.md.
  - trig: |x| >= 2^63 -> C2=1, operand unchanged (like hardware); inf -> IE + indefinite.
- Compares: `fcom/fcomp/fcompp/ficom/ficomp/fucom/fucomp/fucompp/ftst` (C3 C2 C0: > 000, < 001, = 100, unordered 111; fucom raises IE only for SNaN), `fcomi/fcomip/fucomi/fucomip` (ZF PF CF, OF SF AF cleared), `fxam` (C1 = sign; f64 subnormals report "normal" since they are normal in 80-bit), `fcmovcc`.
- Control: `fninit/finit`, `fnclex/fclex`, `fldcw`, `fnstcw`, `fnstsw ax/m16` (also the `9B` `fstsw` form iced decodes as one instruction), `fnstenv/fldenv` (28-byte and 14-byte protected-mode formats; fnstenv then masks all exceptions), `fnsave/fsave/frstor` (108/94 bytes: env + ST(0)..ST(7) as 80-bit in stack order; fnsave then does fninit), `ffree/ffreep/fincstp/fdecstp/fxch/fnop`, 287 no-ops (`fneni/fndisi/fnsetpm`). `fwait` is handled by the integer core as a no-op.
- MMX: `movq mm,mm/m64`, `movq m64,mm`, `movd mm,r/m32`, `movd r/m32,mm`, `emms`. MMX registers are a separate `mmx[8]`
  array (not aliased with the f64 x87 registers); any MMX op sets TOP=0 and all tags valid, `emms` empties them.
  XMM forms of `movq/movd` return `None`. No other MMX instructions are implemented.

## Precision limits (known differences from real hardware)

- 64-bit significand is reduced to 53 bits on every load from m80 and on every operation. Code that depends on extended
  precision (e.g. compares of `a*b` against a stored double) can differ in the last bit. HYPOTHESIS (from static-isa.md):
  game logic is integer, floats do not feed lockstep-synced state.
- Values outside the f64 exponent range (|x| > 1.8e308 or < 4.9e-324) become inf / 0 when loaded from m80.
- `fsin/fcos/fptan` use fdlibm (exact argument reduction, < 1 ulp), not the x87's 66-bit pi approximation.
- NaN results follow x87 rules (larger significand wins; invalid -> indefinite `0xFFF8...`), never the host's.
- Arithmetic ignores RC (always round-to-nearest).

## Notes for other agents

- STALE (fixed by cpu-int): `cpuid` now reports family 5 with **no MMX** (EDX bit 23 = 0), see [cpu.md](cpu.md).
  Old note: `exec.rs` `cpuid` reported MMX = 1; static-isa.md recommends 0. With MMX reported the
  surface-copy benchmark at 0x5c3740 may pick the MMX `movq` routine (0x5c3ee0); that routine only uses `movq` and
  `emms`, which are implemented, so either choice works.
- The SEH `CONTEXT.FloatSave` (s3-runtime) can be filled with the same layout as `fnsave` (108 bytes + Cr0NpxState);
  there is no helper for that yet.
