//! x87 FPU (and the small MMX subset the game can use) state and instruction semantics.
//!
//! See `docs/porting/x87-fpu.md` for the design and precision limits. Summary:
//! - Registers are `f64` (80-bit precision is not modelled; `m80` loads/stores convert exactly / round to nearest).
//!   `st(i)` = `regs[(top + i) & 7]`.
//! - Control word: rounding control is honoured by `fist`/`fistp`/`fisttp`/`frndint`/`fbstp`
//!   (MSVC6 `_ftol` sets RC=truncate via `fldcw` before `fistp`); precision control PC=24 bits rounds
//!   arithmetic results to a 24-bit significand, PC=53/64 both give f64 precision. Arithmetic itself always
//!   rounds to nearest. Exception masks are stored but unmasked exceptions are not delivered (only the
//!   status-word flags and ES are set).
//! - Status word: C0/C1/C2/C3 from `fcom*`/`fucom*`/`ftst`/`fxam`/`fprem*`/trig, IE/ZE/SF flags, TOP.
//! - Tag word: only "empty / not empty" is tracked; the full 2-bit tags are computed for `fnstenv`/`fnsave`.
//! - MMX (`movd`/`movq`/`emms`) uses a separate `mmx` array (aliasing with the x87 registers is not modelled).

use iced_x86::{Instruction, MemorySize, Mnemonic, OpKind, Register};

#[path = "fpu_conv.rs"]
pub mod conv;
#[path = "fpu_math.rs"]
pub mod math;
#[cfg(test)]
#[path = "fpu_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "fpu_golden.rs"]
mod golden;

use conv::INDEFINITE;

/// 2^63 (range limit for `fistp m64` and the trig instructions).
const TWO63: f64 = (1u64 << 63) as f64;

use crate::exec::R;
use crate::{fl, Cpu, CpuExit, GuestMemory};

/// Status-word bits.
pub mod sw {
    pub const IE: u16 = 1 << 0;
    pub const DE: u16 = 1 << 1;
    pub const ZE: u16 = 1 << 2;
    pub const OE: u16 = 1 << 3;
    pub const UE: u16 = 1 << 4;
    pub const PE: u16 = 1 << 5;
    pub const SF: u16 = 1 << 6;
    pub const ES: u16 = 1 << 7;
    pub const C0: u16 = 1 << 8;
    pub const C1: u16 = 1 << 9;
    pub const C2: u16 = 1 << 10;
    pub const TOP: u16 = 7 << 11;
    pub const C3: u16 = 1 << 14;
    pub const B: u16 = 1 << 15;
    /// All condition codes.
    pub const CC: u16 = C0 | C1 | C2 | C3;
    /// Exception flags (IE..PE).
    pub const EXC: u16 = 0x3F;
}

/// x87 state.
#[derive(Debug, Clone)]
pub struct Fpu {
    /// Physical registers R0..R7.
    pub regs: [f64; 8],
    /// Top-of-stack index (0..7).
    pub top: u8,
    /// Control word (default 0x037F after `fninit`).
    pub control: u16,
    /// Status word without TOP (TOP is merged in from `top` when stored).
    pub status: u16,
    /// Tag word (2 bits per physical register; 3 = empty, anything else = in use).
    pub tags: u16,
    /// MMX registers (aliasing is not modelled exactly).
    pub mmx: [u64; 8],
    /// Last instruction / operand pointers (for fnstenv/fnsave).
    pub last_ip: u32,
    pub last_dp: u32,
}

impl Default for Fpu {
    fn default() -> Self {
        Self::new()
    }
}

impl Fpu {
    /// State after `fninit`.
    pub fn new() -> Self {
        Fpu { regs: [0.0; 8], top: 0, control: 0x037F, status: 0, tags: 0xFFFF, mmx: [0; 8], last_ip: 0, last_dp: 0 }
    }

    /// `fninit` (MMX registers are left alone).
    pub fn reset(&mut self) {
        let mmx = self.mmx;
        *self = Fpu::new();
        self.mmx = mmx;
    }

    /// Status word including TOP.
    pub fn status_word(&self) -> u16 {
        (self.status & !sw::TOP) | ((self.top as u16 & 7) << 11)
    }

    /// Set the status word (including TOP).
    pub fn set_status_word(&mut self, v: u16) {
        self.status = v & !sw::TOP;
        self.top = ((v >> 11) & 7) as u8;
    }

    #[inline(always)]
    fn phys(&self, i: u8) -> usize {
        ((self.top + i) & 7) as usize
    }

    /// Is physical register `p` empty?
    #[inline(always)]
    pub fn is_empty_phys(&self, p: usize) -> bool {
        (self.tags >> (p * 2)) & 3 == 3
    }

    /// Is `st(i)` empty?
    #[inline(always)]
    pub fn is_empty(&self, i: u8) -> bool {
        self.is_empty_phys(self.phys(i))
    }

    #[inline(always)]
    fn set_tag_phys(&mut self, p: usize, empty: bool) {
        if empty {
            self.tags |= 3 << (p * 2);
        } else {
            self.tags &= !(3 << (p * 2));
        }
    }

    /// `st(i)` without stack checks.
    pub fn st(&self, i: u8) -> f64 {
        self.regs[self.phys(i)]
    }

    /// Set `st(i)` (marks it in use).
    pub fn set_st(&mut self, i: u8, v: f64) {
        let p = self.phys(i);
        self.regs[p] = v;
        self.set_tag_phys(p, false);
    }

    /// Push a value (marks the new top valid). No overflow check (see [`Fpu::push_checked`]).
    pub fn push(&mut self, v: f64) {
        self.top = (self.top + 7) & 7;
        self.regs[self.top as usize] = v;
        self.set_tag_phys(self.top as usize, false);
    }

    /// Pop (marks the old top empty) and return it.
    pub fn pop(&mut self) -> f64 {
        let v = self.regs[self.top as usize];
        self.set_tag_phys(self.top as usize, true);
        self.top = (self.top + 1) & 7;
        v
    }

    /// Record exception flags; sets ES/B if any of them is unmasked.
    #[inline(always)]
    pub fn raise(&mut self, flags: u16) {
        self.status |= flags;
        if flags & !self.control & sw::EXC != 0 {
            self.status |= sw::ES | sw::B;
        }
    }

    /// `st(i)` with the stack-underflow check (empty => IE|SF, C1=0, returns the indefinite NaN).
    pub fn get(&mut self, i: u8) -> f64 {
        if self.is_empty(i) {
            self.status &= !sw::C1;
            self.raise(sw::IE | sw::SF);
            return INDEFINITE;
        }
        self.st(i)
    }

    /// Push with the stack-overflow check (target in use => IE|SF|C1 and the indefinite NaN is pushed).
    pub fn push_checked(&mut self, v: f64) {
        let p = ((self.top + 7) & 7) as usize;
        if !self.is_empty_phys(p) {
            self.raise(sw::IE | sw::SF);
            self.status |= sw::C1;
            self.push(INDEFINITE);
        } else {
            self.status &= !sw::C1;
            self.push(v);
        }
    }

    /// Full 2-bit tag word as stored by `fnstenv`/`fnsave` (00 valid, 01 zero, 10 special, 11 empty).
    pub fn tag_word(&self) -> u16 {
        let mut t = 0u16;
        for p in 0..8 {
            let tag = if self.is_empty_phys(p) {
                3
            } else {
                let v = self.regs[p];
                if v == 0.0 {
                    1
                } else if !v.is_normal() {
                    2
                } else {
                    0
                }
            };
            t |= tag << (p * 2);
        }
        t
    }

    /// Load a tag word (only empty / non-empty is kept).
    pub fn set_tag_word(&mut self, t: u16) {
        self.tags = 0;
        for p in 0..8 {
            if (t >> (p * 2)) & 3 == 3 {
                self.tags |= 3 << (p * 2);
            }
        }
    }

    #[inline(always)]
    fn set_cc(&mut self, c3: bool, c2: bool, c1: bool, c0: bool) {
        self.status &= !sw::CC;
        if c3 {
            self.status |= sw::C3;
        }
        if c2 {
            self.status |= sw::C2;
        }
        if c1 {
            self.status |= sw::C1;
        }
        if c0 {
            self.status |= sw::C0;
        }
    }

    /// Rounding control (0 nearest, 1 down, 2 up, 3 truncate).
    #[inline(always)]
    pub fn rc(&self) -> u16 {
        (self.control >> 10) & 3
    }

    /// Finish an arithmetic result: NaN from non-NaN inputs => IE + indefinite; precision control.
    /// NaN results never come from the host (its NaN sign/payload/propagation rules differ between x86 and
    /// aarch64): NaN operands go through [`math::propagate_nan`] (IE for SNaN), new NaNs become the indefinite.
    fn finish(&mut self, r: f64, a: f64, b: f64) -> f64 {
        if a.is_nan() || b.is_nan() {
            if math::is_snan(a) || math::is_snan(b) {
                self.raise(sw::IE);
            }
            return math::propagate_nan(a, b);
        }
        if r.is_nan() {
            self.raise(sw::IE);
            return INDEFINITE;
        }
        if r.is_infinite() && a.is_finite() && b.is_finite() {
            self.raise(sw::OE | sw::PE);
        }
        conv::apply_pc(r, self.control)
    }

    fn arith(&mut self, op: Mnemonic, d: f64, s: f64) -> f64 {
        use Mnemonic as M;
        self.status &= !sw::C1;
        let (a, b) = match op {
            M::Fsubr | M::Fsubrp | M::Fisubr | M::Fdivr | M::Fdivrp | M::Fidivr => (s, d),
            _ => (d, s),
        };
        let r = match op {
            M::Fadd | M::Faddp | M::Fiadd => a + b,
            M::Fsub | M::Fsubp | M::Fisub | M::Fsubr | M::Fsubrp | M::Fisubr => a - b,
            M::Fmul | M::Fmulp | M::Fimul => a * b,
            _ => {
                if b == 0.0 && a.is_finite() && a != 0.0 {
                    self.raise(sw::ZE);
                }
                a / b
            }
        };
        self.finish(r, a, b)
    }

    /// Compare `a` with `b`, set C3/C2/C0 (C1 = 0). `quiet` = fucom semantics (only SNaN raises IE).
    fn compare(&mut self, a: f64, b: f64, quiet: bool) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match a.partial_cmp(&b) {
            None => {
                let snan = |x: f64| x.is_nan() && (x.to_bits() >> 51) & 1 == 0;
                if !quiet || snan(a) || snan(b) {
                    self.raise(sw::IE);
                }
                self.set_cc(true, true, false, true);
                Ordering::Equal // unused for unordered
            }
            Some(o) => {
                self.set_cc(o == Ordering::Equal, false, false, o == Ordering::Less);
                o
            }
        }
    }

    /// `fxam` classification into C3/C2/C1/C0.
    fn fxam(&mut self) {
        let v = self.st(0);
        let neg = v.is_sign_negative();
        if self.is_empty(0) {
            self.set_cc(true, false, neg, true);
        } else if v.is_nan() {
            self.set_cc(false, false, neg, true);
        } else if v.is_infinite() {
            self.set_cc(false, true, neg, true);
        } else if v == 0.0 {
            self.set_cc(true, false, neg, false);
        } else if v.is_subnormal() {
            // NOTE: f64 subnormals are normal numbers in 80-bit format; report "normal" like real hardware would
            // for the same value loaded from an f64.
            self.set_cc(false, true, neg, false);
        } else {
            self.set_cc(false, true, neg, false);
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Memory helpers
// ---------------------------------------------------------------------------------------------------------------

fn ld_u16(cpu: &Cpu, mem: &GuestMemory, a: u32) -> Result<u16, CpuExit> {
    cpu.check_addr(a, false)?;
    Ok(mem.read_u16(a))
}
fn ld_u32(cpu: &Cpu, mem: &GuestMemory, a: u32) -> Result<u32, CpuExit> {
    cpu.check_addr(a, false)?;
    Ok(mem.read_u32(a))
}
fn ld_u64(cpu: &Cpu, mem: &GuestMemory, a: u32) -> Result<u64, CpuExit> {
    cpu.check_addr(a, false)?;
    Ok(mem.read_u64(a))
}
fn st_u16(cpu: &Cpu, mem: &mut GuestMemory, a: u32, v: u16) -> R {
    cpu.check_addr(a, true)?;
    mem.write_u16(a, v);
    Ok(())
}
fn st_u32(cpu: &Cpu, mem: &mut GuestMemory, a: u32, v: u32) -> R {
    cpu.check_addr(a, true)?;
    mem.write_u32(a, v);
    Ok(())
}
fn st_u64(cpu: &Cpu, mem: &mut GuestMemory, a: u32, v: u64) -> R {
    cpu.check_addr(a, true)?;
    mem.write_u64(a, v);
    Ok(())
}
fn ld_f80(cpu: &Cpu, mem: &GuestMemory, a: u32) -> Result<f64, CpuExit> {
    let m = ld_u64(cpu, mem, a)?;
    let se = ld_u16(cpu, mem, a.wrapping_add(8))?;
    Ok(conv::f80_to_f64(m, se))
}
fn st_f80(cpu: &Cpu, mem: &mut GuestMemory, a: u32, v: f64) -> R {
    let (m, se) = conv::f64_to_f80(v);
    st_u64(cpu, mem, a, m)?;
    st_u16(cpu, mem, a.wrapping_add(8), se)
}

/// Load a floating/integer memory operand as f64 according to its iced memory size.
fn load_mem(cpu: &Cpu, mem: &GuestMemory, ins: &Instruction, a: u32) -> Result<Option<f64>, CpuExit> {
    Ok(Some(match ins.memory_size() {
        MemorySize::Float32 => conv::f32_to_f64(f32::from_bits(ld_u32(cpu, mem, a)?)),
        MemorySize::Float64 => f64::from_bits(ld_u64(cpu, mem, a)?),
        MemorySize::Float80 => ld_f80(cpu, mem, a)?,
        MemorySize::Int16 => ld_u16(cpu, mem, a)? as i16 as f64,
        MemorySize::Int32 => ld_u32(cpu, mem, a)? as i32 as f64,
        MemorySize::Int64 => ld_u64(cpu, mem, a)? as i64 as f64,
        MemorySize::Bcd => {
            cpu.check_addr(a, false)?;
            let mut b = [0u8; 10];
            b.copy_from_slice(mem.slice(a, 10));
            conv::bcd_to_f64(&b)
        }
        _ => return Ok(None),
    }))
}

#[inline(always)]
fn st_index(r: Register) -> Option<u8> {
    let n = r as u32;
    let b = Register::ST0 as u32;
    if (b..b + 8).contains(&n) {
        Some((n - b) as u8)
    } else {
        None
    }
}

#[inline(always)]
fn mm_index(r: Register) -> Option<usize> {
    let n = r as u32;
    let b = Register::MM0 as u32;
    if (b..b + 8).contains(&n) {
        Some((n - b) as usize)
    } else {
        None
    }
}

/// Store `v` as an integer of `bytes` bytes with the current rounding mode (indefinite + IE on overflow/NaN).
fn store_int(cpu: &mut Cpu, mem: &mut GuestMemory, a: u32, bytes: u32, v: f64, rc: u16) -> R {
    let r = conv::round_rc(v, rc);
    if r != v && !v.is_nan() {
        cpu.fpu.raise(sw::PE);
    }
    match bytes {
        2 => {
            let x = if r.is_nan() || !(-32768.0..=32767.0).contains(&r) {
                cpu.fpu.raise(sw::IE);
                0x8000u16
            } else {
                r as i16 as u16
            };
            st_u16(cpu, mem, a, x)
        }
        4 => {
            let x = if r.is_nan() || !(-2147483648.0..=2147483647.0).contains(&r) {
                cpu.fpu.raise(sw::IE);
                0x8000_0000u32
            } else {
                r as i32 as u32
            };
            st_u32(cpu, mem, a, x)
        }
        _ => {
            let x = if r.is_nan() || !(-TWO63..TWO63).contains(&r) {
                cpu.fpu.raise(sw::IE);
                0x8000_0000_0000_0000u64
            } else {
                r as i64 as u64
            };
            st_u64(cpu, mem, a, x)
        }
    }
}

/// Source operand of a compare: 0 operands => st(1); otherwise the last operand (memory or st(i)).
fn compare_src(cpu: &mut Cpu, mem: &GuestMemory, ins: &Instruction) -> Result<f64, CpuExit> {
    let n = ins.op_count();
    if n == 0 {
        return Ok(cpu.fpu.get(1));
    }
    let k = n - 1;
    match ins.op_kind(k) {
        OpKind::Memory => {
            let a = cpu.ea(ins);
            cpu.fpu.last_dp = a;
            Ok(load_mem(cpu, mem, ins, a)?.unwrap_or(INDEFINITE))
        }
        _ => {
            let i = st_index(ins.op_register(k)).unwrap_or(1);
            Ok(cpu.fpu.get(i))
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Environment / state save
// ---------------------------------------------------------------------------------------------------------------

/// Store the FPU environment (14- or 28-byte protected-mode format) at `a`. Returns the env size.
fn store_env(cpu: &Cpu, mem: &mut GuestMemory, a: u32, small: bool) -> Result<u32, CpuExit> {
    let f = &cpu.fpu;
    let fields: [u32; 7] =
        [f.control as u32, f.status_word() as u32, f.tag_word() as u32, f.last_ip, 0x1B, f.last_dp, 0x23];
    if small {
        for (k, v) in fields.iter().enumerate() {
            st_u16(cpu, mem, a + 2 * k as u32, *v as u16)?;
        }
        Ok(14)
    } else {
        for (k, v) in fields.iter().enumerate() {
            // Reserved upper halves of CW/SW/TW read as 0xFFFF on real hardware.
            let hi = if k < 3 || k == 6 { 0xFFFF_0000 } else { 0 };
            st_u32(cpu, mem, a + 4 * k as u32, *v | hi)?;
        }
        Ok(28)
    }
}

fn load_env(cpu: &mut Cpu, mem: &GuestMemory, a: u32, small: bool) -> Result<u32, CpuExit> {
    let mut fields = [0u32; 7];
    for (k, f) in fields.iter_mut().enumerate() {
        *f = if small { ld_u16(cpu, mem, a + 2 * k as u32)? as u32 } else { ld_u32(cpu, mem, a + 4 * k as u32)? };
    }
    let f = &mut cpu.fpu;
    f.control = fields[0] as u16;
    f.set_status_word(fields[1] as u16);
    f.set_tag_word(fields[2] as u16);
    f.last_ip = fields[3];
    f.last_dp = fields[5];
    // Re-evaluate ES/B against the new masks.
    if f.status & !f.control & sw::EXC != 0 {
        f.status |= sw::ES | sw::B;
    } else {
        f.status &= !(sw::ES | sw::B);
    }
    Ok(if small { 14 } else { 28 })
}

// ---------------------------------------------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------------------------------------------

/// Execute an x87/MMX instruction. Returns `None` if the instruction is not an FPU instruction or is not
/// implemented (the caller then raises `FaultKind::Unimplemented`).
pub fn exec(cpu: &mut Cpu, mem: &mut GuestMemory, ins: &Instruction) -> Option<R> {
    use Mnemonic as M;
    let m = ins.mnemonic();
    match m {
        M::Movq | M::Movd | M::Emms => return exec_mmx(cpu, mem, ins),
        _ => {}
    }
    // Control instructions do not update the last-instruction pointers.
    let control = matches!(
        m,
        M::Fninit
            | M::Finit
            | M::Fnclex
            | M::Fclex
            | M::Fldcw
            | M::Fnstcw
            | M::Fstcw
            | M::Fnstsw
            | M::Fstsw
            | M::Fnstenv
            | M::Fstenv
            | M::Fldenv
            | M::Fnsave
            | M::Fsave
            | M::Frstor
    );
    let ip = cpu.fpu.last_ip;
    if !control {
        cpu.fpu.last_ip = cpu.cur_ip;
    }
    match exec_x87(cpu, mem, ins, m) {
        Err(CpuExit::Halt) => {
            cpu.fpu.last_ip = ip;
            None
        }
        r => Some(r),
    }
}

/// Marker used internally: `Err(Halt)` from `exec_x87` means "not handled" (mapped to `None`).
const NOT_HANDLED: R = Err(CpuExit::Halt);

fn exec_x87(cpu: &mut Cpu, mem: &mut GuestMemory, ins: &Instruction, m: Mnemonic) -> R {
    use Mnemonic as M;
    let has_mem = ins.op_count() > 0 && (0..ins.op_count()).any(|k| ins.op_kind(k) == OpKind::Memory);
    let ea = if has_mem { cpu.ea(ins) } else { 0 };
    if has_mem
        && !matches!(m, M::Fldcw | M::Fnstcw | M::Fstcw | M::Fnstsw | M::Fstsw)
        && !matches!(m, M::Fnstenv | M::Fstenv | M::Fldenv | M::Fnsave | M::Fsave | M::Frstor)
    {
        cpu.fpu.last_dp = ea;
    }
    match m {
        // ---- loads ----
        M::Fld | M::Fild | M::Fbld => {
            let v = if has_mem {
                match load_mem(cpu, mem, ins, ea)? {
                    Some(v) => v,
                    None => return NOT_HANDLED,
                }
            } else {
                let i = st_index(ins.op_register(ins.op_count() - 1)).unwrap_or(0);
                cpu.fpu.get(i)
            };
            cpu.fpu.push_checked(v);
        }
        M::Fld1 | M::Fldz | M::Fldpi | M::Fldl2e | M::Fldl2t | M::Fldlg2 | M::Fldln2 => {
            use std::f64::consts as c;
            let v = match m {
                M::Fld1 => 1.0,
                M::Fldz => 0.0,
                M::Fldpi => c::PI,
                M::Fldl2e => c::LOG2_E,
                M::Fldl2t => c::LOG2_10,
                M::Fldlg2 => c::LOG10_2,
                _ => c::LN_2,
            };
            cpu.fpu.push_checked(v);
        }

        // ---- stores ----
        M::Fst | M::Fstp => {
            let v = cpu.fpu.get(0);
            cpu.fpu.status &= !sw::C1;
            if has_mem {
                match ins.memory_size() {
                    MemorySize::Float32 => {
                        let f = conv::f64_to_f32(v);
                        if f.is_infinite() && v.is_finite() {
                            cpu.fpu.raise(sw::OE | sw::PE);
                        }
                        st_u32(cpu, mem, ea, f.to_bits())?
                    }
                    MemorySize::Float64 => st_u64(cpu, mem, ea, v.to_bits())?,
                    MemorySize::Float80 => st_f80(cpu, mem, ea, v)?,
                    _ => return NOT_HANDLED,
                }
            } else {
                let i = st_index(ins.op_register(0)).unwrap_or(0);
                cpu.fpu.set_st(i, v);
            }
            if m == M::Fstp {
                cpu.fpu.pop();
            }
        }
        M::Fist | M::Fistp | M::Fisttp => {
            let v = cpu.fpu.get(0);
            cpu.fpu.status &= !sw::C1;
            let rc = if m == M::Fisttp { 3 } else { cpu.fpu.rc() };
            let bytes = ins.memory_size().size() as u32;
            store_int(cpu, mem, ea, bytes, v, rc)?;
            if m != M::Fist {
                cpu.fpu.pop();
            }
        }
        M::Fbstp => {
            let v = cpu.fpu.get(0);
            let r = conv::round_rc(v, cpu.fpu.rc());
            let b = match conv::f64_to_bcd(r) {
                Some(b) => b,
                None => {
                    cpu.fpu.raise(sw::IE);
                    conv::BCD_INDEFINITE
                }
            };
            cpu.check_addr(ea, true)?;
            mem.write_bytes(ea, &b);
            cpu.fpu.pop();
        }

        // ---- register moves ----
        M::Fxch => {
            let n = ins.op_count();
            let i = if n == 0 { 1 } else { st_index(ins.op_register(n - 1)).unwrap_or(1) };
            let i = if i == 0 && n == 2 { st_index(ins.op_register(0)).unwrap_or(1) } else { i };
            let a = cpu.fpu.get(0);
            let b = cpu.fpu.get(i);
            cpu.fpu.set_st(0, b);
            cpu.fpu.set_st(i, a);
            cpu.fpu.status &= !sw::C1;
        }
        M::Ffree | M::Ffreep => {
            let i = st_index(ins.op_register(0)).unwrap_or(0);
            let p = cpu.fpu.phys(i);
            cpu.fpu.set_tag_phys(p, true);
            if m == M::Ffreep {
                cpu.fpu.pop();
            }
        }
        M::Fincstp => {
            cpu.fpu.top = (cpu.fpu.top + 1) & 7;
            cpu.fpu.status &= !sw::C1;
        }
        M::Fdecstp => {
            cpu.fpu.top = (cpu.fpu.top + 7) & 7;
            cpu.fpu.status &= !sw::C1;
        }
        M::Fcmovb | M::Fcmove | M::Fcmovbe | M::Fcmovu | M::Fcmovnb | M::Fcmovne | M::Fcmovnbe | M::Fcmovnu => {
            let (cf, zf, pf) = (cpu.flag(fl::CF), cpu.flag(fl::ZF), cpu.flag(fl::PF));
            let cond = match m {
                M::Fcmovb => cf,
                M::Fcmove => zf,
                M::Fcmovbe => cf || zf,
                M::Fcmovu => pf,
                M::Fcmovnb => !cf,
                M::Fcmovne => !zf,
                M::Fcmovnbe => !cf && !zf,
                _ => !pf,
            };
            if cond {
                let i = st_index(ins.op_register(ins.op_count() - 1)).unwrap_or(1);
                let v = cpu.fpu.get(i);
                cpu.fpu.set_st(0, v);
            }
        }

        // ---- arithmetic ----
        M::Fadd | M::Faddp | M::Fiadd | M::Fsub | M::Fsubp | M::Fisub | M::Fsubr | M::Fsubrp | M::Fisubr | M::Fmul
        | M::Fmulp | M::Fimul | M::Fdiv | M::Fdivp | M::Fidiv | M::Fdivr | M::Fdivrp | M::Fidivr => {
            let n = ins.op_count();
            let (di, s) = if has_mem {
                match load_mem(cpu, mem, ins, ea)? {
                    Some(v) => (0u8, v),
                    None => return NOT_HANDLED,
                }
            } else if n == 2 {
                let d = st_index(ins.op_register(0)).unwrap_or(0);
                let s = st_index(ins.op_register(1)).unwrap_or(0);
                (d, cpu.fpu.get(s))
            } else if n == 1 {
                let s = st_index(ins.op_register(0)).unwrap_or(1);
                (0, cpu.fpu.get(s))
            } else {
                (1, cpu.fpu.get(0))
            };
            let d = cpu.fpu.get(di);
            let r = cpu.fpu.arith(m, d, s);
            cpu.fpu.set_st(di, r);
            if matches!(m, M::Faddp | M::Fsubp | M::Fsubrp | M::Fmulp | M::Fdivp | M::Fdivrp) {
                cpu.fpu.pop();
            }
        }
        M::Fchs => {
            let v = cpu.fpu.get(0);
            cpu.fpu.set_st(0, -v);
            cpu.fpu.status &= !sw::C1;
        }
        M::Fabs => {
            let v = cpu.fpu.get(0);
            cpu.fpu.set_st(0, v.abs());
            cpu.fpu.status &= !sw::C1;
        }
        M::Fsqrt => {
            let v = cpu.fpu.get(0);
            let r = math::sqrt(v);
            let r = cpu.fpu.finish(r, v, 0.0);
            cpu.fpu.set_st(0, r);
        }
        M::Frndint => {
            let v = cpu.fpu.get(0);
            let r = conv::round_rc(v, cpu.fpu.rc());
            if r != v && !v.is_nan() {
                cpu.fpu.raise(sw::PE);
            }
            cpu.fpu.set_st(0, r);
        }
        M::Fscale => {
            let a = cpu.fpu.get(0);
            let b = cpu.fpu.get(1);
            let r = if a.is_nan() || b.is_nan() {
                if math::is_snan(a) || math::is_snan(b) {
                    cpu.fpu.raise(sw::IE);
                }
                math::propagate_nan(a, b)
            } else if b.is_infinite() {
                if b > 0.0 {
                    if a == 0.0 {
                        cpu.fpu.raise(sw::IE);
                        INDEFINITE
                    } else {
                        a * f64::INFINITY
                    }
                } else if a.is_infinite() {
                    cpu.fpu.raise(sw::IE);
                    INDEFINITE
                } else {
                    a * 0.0
                }
            } else {
                conv::ldexp(a, b.trunc().clamp(-1e5, 1e5) as i64)
            };
            cpu.fpu.status &= !sw::C1;
            cpu.fpu.set_st(0, r);
        }
        M::Fxtract => {
            let v = cpu.fpu.get(0);
            if v == 0.0 {
                cpu.fpu.raise(sw::ZE);
                cpu.fpu.set_st(0, f64::NEG_INFINITY);
                cpu.fpu.push_checked(v);
            } else if v.is_infinite() {
                cpu.fpu.set_st(0, f64::INFINITY);
                cpu.fpu.push_checked(v);
            } else if v.is_nan() {
                cpu.fpu.push_checked(v);
            } else {
                let e = conv::exponent_of(v);
                cpu.fpu.set_st(0, e as f64);
                cpu.fpu.push_checked(conv::ldexp(v, -e));
            }
        }
        M::Fprem | M::Fprem1 => {
            let a = cpu.fpu.get(0);
            let b = cpu.fpu.get(1);
            if a.is_nan() || b.is_nan() || a.is_infinite() || b == 0.0 {
                if !(a.is_nan() || b.is_nan()) || math::is_snan(a) || math::is_snan(b) {
                    cpu.fpu.raise(sw::IE);
                }
                cpu.fpu.set_st(0, if a.is_nan() || b.is_nan() { math::propagate_nan(a, b) } else { INDEFINITE });
                cpu.fpu.set_cc(false, false, false, false);
            } else if b.is_infinite() {
                cpu.fpu.set_cc(false, false, false, false);
            } else {
                // Exact integer long division (no host fmod); q = low 3 bits of the quotient.
                let math::Rem { r, q } = math::fprem(a, b, m == M::Fprem1);
                cpu.fpu.set_st(0, r);
                cpu.fpu.set_cc((q >> 1) & 1 == 1, false, q & 1 == 1, (q >> 2) & 1 == 1);
            }
        }
        M::Fsin | M::Fcos | M::Fptan | M::Fsincos => {
            let v = cpu.fpu.get(0);
            if v.is_finite() && v.abs() >= TWO63 {
                // Out of range: C2=1, operand unchanged.
                cpu.fpu.set_cc(false, true, false, false);
                return Ok(());
            }
            if v.is_infinite() || math::is_snan(v) {
                cpu.fpu.raise(sw::IE);
            }
            cpu.fpu.status &= !(sw::C2 | sw::C1);
            match m {
                // Deterministic (pure-Rust fdlibm) — never the host libm. See fpu_math.rs.
                M::Fsin => cpu.fpu.set_st(0, math::sin(v)),
                M::Fcos => cpu.fpu.set_st(0, math::cos(v)),
                M::Fptan => {
                    cpu.fpu.set_st(0, math::tan(v));
                    cpu.fpu.push_checked(1.0);
                }
                _ => {
                    let (s, c) = (math::sin(v), math::cos(v));
                    cpu.fpu.set_st(0, s);
                    cpu.fpu.push_checked(c);
                }
            }
        }
        M::Fpatan => {
            let x = cpu.fpu.get(0);
            let y = cpu.fpu.get(1);
            cpu.fpu.status &= !sw::C1;
            if math::is_snan(x) || math::is_snan(y) {
                cpu.fpu.raise(sw::IE);
            }
            cpu.fpu.set_st(1, math::atan2(y, x));
            cpu.fpu.pop();
        }
        M::F2xm1 => {
            let x = cpu.fpu.get(0);
            cpu.fpu.status &= !sw::C1;
            if math::is_snan(x) {
                cpu.fpu.raise(sw::IE);
            }
            cpu.fpu.set_st(0, math::exp2m1(x));
        }
        M::Fyl2x | M::Fyl2xp1 => {
            let x = cpu.fpu.get(0);
            let y = cpu.fpu.get(1);
            cpu.fpu.status &= !sw::C1;
            let r = if m == M::Fyl2x {
                if x == 0.0 && y.is_finite() && y != 0.0 {
                    cpu.fpu.raise(sw::ZE);
                }
                y * math::log2(x)
            } else {
                y * math::log2_1p(x)
            };
            let r = cpu.fpu.finish(r, x, y);
            cpu.fpu.set_st(1, r);
            cpu.fpu.pop();
        }

        // ---- compares ----
        M::Fcom | M::Fcomp | M::Fcompp | M::Fucom | M::Fucomp | M::Fucompp | M::Ficom | M::Ficomp => {
            let a = cpu.fpu.get(0);
            let b = compare_src(cpu, mem, ins)?;
            let quiet = matches!(m, M::Fucom | M::Fucomp | M::Fucompp);
            cpu.fpu.compare(a, b, quiet);
            match m {
                M::Fcomp | M::Fucomp | M::Ficomp => {
                    cpu.fpu.pop();
                }
                M::Fcompp | M::Fucompp => {
                    cpu.fpu.pop();
                    cpu.fpu.pop();
                }
                _ => {}
            }
        }
        M::Fcomi | M::Fcomip | M::Fucomi | M::Fucomip => {
            let a = cpu.fpu.get(0);
            let b = compare_src(cpu, mem, ins)?;
            let quiet = matches!(m, M::Fucomi | M::Fucomip);
            let saved = cpu.fpu.status & sw::CC;
            cpu.fpu.compare(a, b, quiet);
            let c = cpu.fpu.status;
            cpu.fpu.status = (cpu.fpu.status & !sw::CC) | (saved & !sw::C1);
            cpu.set_eflags(cpu.eflags() & !(fl::OF | fl::SF | fl::AF | fl::ZF | fl::PF | fl::CF));
            cpu.set_flag(fl::ZF, c & sw::C3 != 0);
            cpu.set_flag(fl::PF, c & sw::C2 != 0);
            cpu.set_flag(fl::CF, c & sw::C0 != 0);
            if matches!(m, M::Fcomip | M::Fucomip) {
                cpu.fpu.pop();
            }
        }
        M::Ftst | M::Ftstp => {
            let a = cpu.fpu.get(0);
            cpu.fpu.compare(a, 0.0, false);
            if m == M::Ftstp {
                cpu.fpu.pop();
            }
        }
        M::Fxam => cpu.fpu.fxam(),

        // ---- control ----
        M::Fninit | M::Finit => cpu.fpu.reset(),
        M::Fnclex | M::Fclex => cpu.fpu.status &= !(sw::EXC | sw::SF | sw::ES | sw::B),
        M::Fldcw => {
            cpu.fpu.control = ld_u16(cpu, mem, ea)? | 0x0040;
            let f = &mut cpu.fpu;
            if f.status & !f.control & sw::EXC != 0 {
                f.status |= sw::ES | sw::B;
            } else {
                f.status &= !(sw::ES | sw::B);
            }
        }
        M::Fnstcw | M::Fstcw => st_u16(cpu, mem, ea, cpu.fpu.control)?,
        M::Fnstsw | M::Fstsw => {
            let v = cpu.fpu.status_word();
            if has_mem {
                st_u16(cpu, mem, ea, v)?;
            } else {
                cpu.set_reg(Register::AX, v as u32);
            }
        }
        M::Fnstenv | M::Fstenv => {
            store_env(cpu, mem, ea, ins.memory_size() == MemorySize::FpuEnv14)?;
            // fnstenv masks all exceptions after storing.
            cpu.fpu.control |= sw::EXC;
        }
        M::Fldenv => {
            load_env(cpu, mem, ea, ins.memory_size() == MemorySize::FpuEnv14)?;
        }
        M::Fnsave | M::Fsave => {
            let small = ins.memory_size() == MemorySize::FpuState94;
            let off = store_env(cpu, mem, ea, small)?;
            for i in 0..8u8 {
                let v = cpu.fpu.st(i);
                st_f80(cpu, mem, ea + off + 10 * i as u32, v)?;
            }
            cpu.fpu.reset();
        }
        M::Frstor => {
            let small = ins.memory_size() == MemorySize::FpuState94;
            let off = load_env(cpu, mem, ea, small)?;
            for i in 0..8u8 {
                let v = ld_f80(cpu, mem, ea + off + 10 * i as u32)?;
                let p = cpu.fpu.phys(i);
                cpu.fpu.regs[p] = v;
            }
        }
        M::Fnop | M::Fneni | M::Feni | M::Fndisi | M::Fdisi | M::Fnsetpm | M::Fsetpm | M::Frstpm => {}
        _ => return NOT_HANDLED,
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// MMX subset
// ---------------------------------------------------------------------------------------------------------------

fn exec_mmx(cpu: &mut Cpu, mem: &mut GuestMemory, ins: &Instruction) -> Option<R> {
    if ins.mnemonic() == Mnemonic::Emms {
        cpu.fpu.tags = 0xFFFF;
        return Some(Ok(()));
    }
    let n = ins.op_count();
    if n != 2 {
        return None;
    }
    let dst_mm = (ins.op_kind(0) == OpKind::Register).then(|| mm_index(ins.op_register(0))).flatten();
    let src_mm = (ins.op_kind(1) == OpKind::Register).then(|| mm_index(ins.op_register(1))).flatten();
    if dst_mm.is_none() && src_mm.is_none() {
        return None; // SSE/XMM forms are not supported.
    }
    let is_q = ins.mnemonic() == Mnemonic::Movq;
    let r = (|| -> R {
        // Read the source.
        let v: u64 = if let Some(s) = src_mm {
            cpu.fpu.mmx[s]
        } else if ins.op_kind(1) == OpKind::Memory {
            let a = cpu.ea(ins);
            if is_q {
                ld_u64(cpu, mem, a)?
            } else {
                ld_u32(cpu, mem, a)? as u64
            }
        } else if ins.op_kind(1) == OpKind::Register {
            cpu.get_reg(ins.op_register(1)) as u64
        } else {
            return NOT_HANDLED;
        };
        if let Some(d) = dst_mm {
            cpu.fpu.mmx[d] = if is_q { v } else { v & 0xFFFF_FFFF };
        } else if ins.op_kind(0) == OpKind::Memory {
            let a = cpu.ea(ins);
            if is_q {
                st_u64(cpu, mem, a, v)?;
            } else {
                st_u32(cpu, mem, a, v as u32)?;
            }
        } else if ins.op_kind(0) == OpKind::Register {
            cpu.set_reg(ins.op_register(0), v as u32);
        } else {
            return NOT_HANDLED;
        }
        // Any MMX instruction: TOP=0, all tags valid.
        cpu.fpu.top = 0;
        cpu.fpu.tags = 0;
        Ok(())
    })();
    match r {
        Err(CpuExit::Halt) => None,
        r => Some(r),
    }
}
