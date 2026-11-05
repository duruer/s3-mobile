//! Integer instruction semantics. One `match` on the mnemonic; anything not handled here goes to `fpu::exec`.
//!
//! Conventions for adding instructions:
//! - `self.eip` already points to the next instruction when `step` runs; branches overwrite it.
//! - Use `read_op`/`write_op` with the operand size from `op_size` (bytes) unless the instruction has its own.
//! - Return `Err(self.fault(..))` for CPU exceptions; `run` rewinds EIP to the faulting instruction.

use std::sync::OnceLock;
use std::time::Instant;

use iced_x86::{Code, Instruction, Mnemonic, OpKind, Register};

use crate::flags::{mask, sext, sign_bit};
use crate::{fl, fpu, layout, Cpu, CpuExit, FaultKind, GuestMemory, EAX, EBP, EBX, ECX, EDI, EDX, ESI, ESP};

/// Result of executing one instruction.
pub type R = Result<(), CpuExit>;

const R_AL: u32 = Register::AL as u32;
const R_BL: u32 = Register::BL as u32;
const R_AH: u32 = Register::AH as u32;
const R_BH: u32 = Register::BH as u32;
const R_AX: u32 = Register::AX as u32;
const R_DI: u32 = Register::DI as u32;
const R_EAX: u32 = Register::EAX as u32;
const R_EDI: u32 = Register::EDI as u32;

/// EFLAGS bits writable by popf/popfd in user mode: CF PF AF ZF SF DF OF, AC (bit 18, 486 probe) and ID
/// (bit 21, CPUID probe). TF/IOPL/NT/RF/VM are not modelled; IF stays set.
const POPF_MASK: u32 = fl::STATUS | fl::DF | (1 << 18) | fl::ID;

#[derive(Clone, Copy, PartialEq, Eq)]
enum StrOp {
    Movs,
    Stos,
    Lods,
    Scas,
    Cmps,
}

impl Cpu {
    /// Read a register by iced name (8/16/32-bit GPRs; segment registers return fixed flat selectors).
    #[inline(always)]
    pub fn get_reg(&self, r: Register) -> u32 {
        let n = r as u32;
        if (R_EAX..=R_EDI).contains(&n) {
            return self.regs[(n - R_EAX) as usize];
        }
        if (R_AL..=R_BL).contains(&n) {
            return self.regs[(n - R_AL) as usize] & 0xFF;
        }
        if (R_AH..=R_BH).contains(&n) {
            return (self.regs[(n - R_AH) as usize] >> 8) & 0xFF;
        }
        if (R_AX..=R_DI).contains(&n) {
            return self.regs[(n - R_AX) as usize] & 0xFFFF;
        }
        match r {
            Register::CS => 0x1B,
            Register::DS | Register::ES | Register::SS => 0x23,
            Register::FS => 0x3B,
            _ => 0,
        }
    }

    /// Write a register by iced name (partial registers merge). Writes to segment registers are ignored.
    #[inline(always)]
    pub fn set_reg(&mut self, r: Register, v: u32) {
        let n = r as u32;
        if (R_EAX..=R_EDI).contains(&n) {
            self.regs[(n - R_EAX) as usize] = v;
        } else if (R_AL..=R_BL).contains(&n) {
            let x = &mut self.regs[(n - R_AL) as usize];
            *x = (*x & !0xFF) | (v & 0xFF);
        } else if (R_AH..=R_BH).contains(&n) {
            let x = &mut self.regs[(n - R_AH) as usize];
            *x = (*x & !0xFF00) | ((v & 0xFF) << 8);
        } else if (R_AX..=R_DI).contains(&n) {
            let x = &mut self.regs[(n - R_AX) as usize];
            *x = (*x & !0xFFFF) | (v & 0xFFFF);
        }
    }

    /// Build a fault exit for the current instruction.
    pub fn fault(&self, kind: FaultKind, detail: impl Into<String>) -> CpuExit {
        CpuExit::Fault { eip: self.cur_ip, kind, detail: detail.into() }
    }

    /// Access-violation exit (out of line: keeps the callers' fast paths small).
    #[cold]
    #[inline(never)]
    pub(crate) fn av_exit(&self, addr: u32, write: bool) -> CpuExit {
        let what = if write { "write" } else { "read" };
        self.fault(FaultKind::AccessViolation { addr, write }, format!("{what} {addr:#x}"))
    }

    fn unimplemented(&self, ins: &Instruction) -> CpuExit {
        self.fault(FaultKind::Unimplemented, format!("{:?}: {}", ins.mnemonic(), ins))
    }

    /// Segment base for a memory operand (only FS/GS are non-zero).
    #[inline(always)]
    pub fn seg_base(&self, seg: Register) -> u32 {
        match seg {
            Register::FS => self.fs_base,
            Register::GS => self.gs_base,
            _ => 0,
        }
    }

    /// Effective address of the memory operand without segment base (LEA).
    #[inline(always)]
    pub fn ea_noseg(&self, ins: &Instruction) -> u32 {
        let mut a = ins.memory_displacement32();
        let b = ins.memory_base();
        if b != Register::None {
            a = a.wrapping_add(self.get_reg(b));
        }
        let i = ins.memory_index();
        if i != Register::None {
            a = a.wrapping_add(self.get_reg(i).wrapping_mul(ins.memory_index_scale()));
        }
        // 16-bit addressing (0x67 prefix, e.g. `[bx+si]`) wraps at 64 KiB.
        if b.is_gpr16() || i.is_gpr16() {
            a &= 0xFFFF;
        }
        a
    }

    /// Linear address of the memory operand.
    #[inline(always)]
    pub fn ea(&self, ins: &Instruction) -> u32 {
        self.ea_noseg(ins).wrapping_add(self.seg_base(ins.memory_segment()))
    }

    /// Guest load of `size` bytes (1/2/4) with the NULL-guard check.
    #[inline(always)]
    pub fn rd(&self, mem: &GuestMemory, addr: u32, size: u32) -> Result<u32, CpuExit> {
        if addr < layout::NULL_GUARD_END {
            return Err(self.av_exit(addr, false));
        }
        Ok(match size {
            1 => mem.read_u8(addr) as u32,
            2 => mem.read_u16(addr) as u32,
            _ => mem.read_u32(addr),
        })
    }

    /// Guest store of `size` bytes (1/2/4) with the NULL-guard check.
    #[inline(always)]
    pub fn wr(&self, mem: &mut GuestMemory, addr: u32, size: u32, v: u32) -> R {
        if addr < layout::NULL_GUARD_END {
            return Err(self.av_exit(addr, true));
        }
        match size {
            1 => mem.write_u8(addr, v as u8),
            2 => mem.write_u16(addr, v as u16),
            _ => mem.write_u32(addr, v),
        }
        Ok(())
    }

    /// Check an address for the NULL guard (for multi-byte helpers that access memory directly).
    #[inline(always)]
    pub fn check_addr(&self, addr: u32, write: bool) -> R {
        if addr < layout::NULL_GUARD_END {
            return Err(self.fault(FaultKind::AccessViolation { addr, write }, format!("access {addr:#x}")));
        }
        Ok(())
    }

    /// Operand size in bytes of operand 0 (register size or memory size), 4 otherwise.
    #[inline(always)]
    pub fn op_size(ins: &Instruction) -> u32 {
        Self::op_size_n(ins, 0)
    }

    /// Operand size in bytes of operand `i`.
    #[inline(always)]
    pub fn op_size_n(ins: &Instruction, i: u32) -> u32 {
        match ins.op_kind(i) {
            OpKind::Register => ins.op_register(i).size() as u32,
            OpKind::Memory => ins.memory_size().size() as u32,
            OpKind::Immediate8 => 1,
            OpKind::Immediate16 | OpKind::Immediate8to16 => 2,
            _ => 4,
        }
    }

    /// Read operand `i` (register, memory, immediate, near branch).
    #[inline(always)]
    pub fn read_op(&self, mem: &GuestMemory, ins: &Instruction, i: u32, size: u32) -> Result<u32, CpuExit> {
        match ins.op_kind(i) {
            OpKind::Register => Ok(self.get_reg(ins.op_register(i))),
            OpKind::Memory => self.rd(mem, self.ea(ins), size),
            OpKind::NearBranch32 | OpKind::NearBranch16 => Ok(ins.near_branch_target() as u32),
            OpKind::Immediate8
            | OpKind::Immediate16
            | OpKind::Immediate32
            | OpKind::Immediate8to16
            | OpKind::Immediate8to32
            | OpKind::Immediate8_2nd => Ok(ins.immediate(i) as u32 & mask(size)),
            _ => Err(self.unimplemented(ins)),
        }
    }

    /// Write operand `i` (register or memory).
    #[inline(always)]
    pub fn write_op(&mut self, mem: &mut GuestMemory, ins: &Instruction, i: u32, size: u32, v: u32) -> R {
        match ins.op_kind(i) {
            OpKind::Register => {
                self.set_reg(ins.op_register(i), v);
                Ok(())
            }
            OpKind::Memory => {
                let a = self.ea(ins);
                self.wr(mem, a, size, v)
            }
            _ => Err(self.unimplemented(ins)),
        }
    }

    #[inline(always)]
    fn push(&mut self, mem: &mut GuestMemory, size: u32, v: u32) -> R {
        let sp = self.regs[ESP].wrapping_sub(size);
        self.wr(mem, sp, size, v)?;
        self.regs[ESP] = sp;
        Ok(())
    }

    #[inline(always)]
    fn pop(&mut self, mem: &GuestMemory, size: u32) -> Result<u32, CpuExit> {
        let v = self.rd(mem, self.regs[ESP], size)?;
        self.regs[ESP] = self.regs[ESP].wrapping_add(size);
        Ok(v)
    }

    fn acc(&self, size: u32) -> u32 {
        self.regs[EAX] & mask(size)
    }

    fn set_acc(&mut self, size: u32, v: u32) {
        match size {
            1 => self.set_reg(Register::AL, v),
            2 => self.set_reg(Register::AX, v),
            _ => self.regs[EAX] = v,
        }
    }

    /// Shift/rotate. Returns None when the masked count is zero (no change at all).
    pub(crate) fn shift(&mut self, m: Mnemonic, a: u32, count: u32, size: u32) -> Option<u32> {
        let bits = size * 8;
        let msk = mask(size);
        let sb = sign_bit(size);
        let a = a & msk;
        let c = count & 0x1F;
        if c == 0 {
            return None;
        }
        let cf_in = self.flag(fl::CF) as u32;
        let r;
        match m {
            Mnemonic::Shl | Mnemonic::Sal => {
                r = if c >= 32 { 0 } else { ((a as u64) << c) as u32 & msk };
                let cf = if c <= bits { (a as u64 >> (bits - c)) & 1 != 0 } else { false };
                self.set_szp(r, size);
                self.set_flag(fl::CF, cf);
                self.set_flag(fl::OF, ((r & sb) != 0) != cf);
            }
            Mnemonic::Shr => {
                r = if c >= bits { 0 } else { a >> c };
                let cf = if c <= bits { (a as u64 >> (c - 1)) & 1 != 0 } else { false };
                self.set_szp(r, size);
                self.set_flag(fl::CF, cf);
                self.set_flag(fl::OF, a & sb != 0);
            }
            Mnemonic::Sar => {
                let s = sext(a, size);
                let cc = c.min(31);
                r = (s >> cc) as u32 & msk;
                let cf = (s >> (c.min(bits) - 1).min(31)) & 1 != 0;
                self.set_szp(r, size);
                self.set_flag(fl::CF, cf);
                self.set_flag(fl::OF, false);
            }
            Mnemonic::Rol => {
                let k = c % bits;
                r = if k == 0 { a } else { ((a << k) | (a >> (bits - k))) & msk };
                let cf = r & 1 != 0;
                self.set_flag(fl::CF, cf);
                self.set_flag(fl::OF, ((r & sb) != 0) != cf);
            }
            Mnemonic::Ror => {
                let k = c % bits;
                r = if k == 0 { a } else { ((a >> k) | (a << (bits - k))) & msk };
                let msb = r & sb != 0;
                self.set_flag(fl::CF, msb);
                self.set_flag(fl::OF, msb != ((r & (sb >> 1)) != 0));
            }
            Mnemonic::Rcl => {
                let k = c % (bits + 1);
                let (mut v, mut cf) = (a, cf_in);
                for _ in 0..k {
                    let msb = (v >> (bits - 1)) & 1;
                    v = ((v << 1) | cf) & msk;
                    cf = msb;
                }
                r = v;
                self.set_flag(fl::CF, cf != 0);
                self.set_flag(fl::OF, ((r & sb) != 0) != (cf != 0));
            }
            Mnemonic::Rcr => {
                let k = c % (bits + 1);
                let (mut v, mut cf) = (a, cf_in);
                for _ in 0..k {
                    let lsb = v & 1;
                    v = (v >> 1) | (cf << (bits - 1));
                    cf = lsb;
                }
                r = v & msk;
                self.set_flag(fl::CF, cf != 0);
                self.set_flag(fl::OF, ((r & sb) != 0) != ((r & (sb >> 1)) != 0));
            }
            _ => unreachable!(),
        }
        Some(r)
    }

    fn string_op(&mut self, mem: &mut GuestMemory, ins: &Instruction, op: StrOp, size: u32) -> R {
        // 16-bit address size (SI/DI/CX) never occurs in S3.EXE; refuse loudly instead of running it wrong.
        if (0..ins.op_count()).any(|i| matches!(ins.op_kind(i), OpKind::MemorySegSI | OpKind::MemorySegDI | OpKind::MemoryESDI)) {
            return Err(self.unimplemented(ins));
        }
        let step = if self.flag(fl::DF) { (size as i32).wrapping_neg() as u32 } else { size };
        let rep = ins.has_rep_prefix() || ins.has_repne_prefix();
        let repne = ins.has_repne_prefix();
        let sbase = match op {
            StrOp::Movs | StrOp::Lods | StrOp::Cmps => self.seg_base(ins.memory_segment()),
            _ => 0,
        };
        let mut n = if rep { self.regs[ECX] } else { 1 };
        if n == 0 {
            return Ok(());
        }
        // Fast paths (forward, no overlap hazards).
        if rep && !self.flag(fl::DF) && n > 4 {
            let bytes = n as u64 * size as u64;
            let src = self.regs[ESI].wrapping_add(sbase);
            let dst = self.regs[EDI];
            if bytes < 0x4000_0000 && dst as u64 + bytes <= 1 << 32 && src as u64 + bytes <= 1 << 32 {
                match op {
                    StrOp::Movs if (dst as u64 >= src as u64 + bytes) || (src as u64 >= dst as u64 + bytes) || dst <= src => {
                        // dst <= src overlapping forward copy behaves like memmove for element-wise copies.
                        self.check_addr(src, false)?;
                        self.check_addr(dst, true)?;
                        mem.copy_within(src, dst, bytes as usize);
                        self.regs[ESI] = self.regs[ESI].wrapping_add(bytes as u32);
                        self.regs[EDI] = self.regs[EDI].wrapping_add(bytes as u32);
                        self.regs[ECX] = 0;
                        return Ok(());
                    }
                    StrOp::Stos => {
                        self.check_addr(dst, true)?;
                        let v = self.acc(size);
                        match size {
                            1 => mem.fill(dst, bytes as usize, v as u8),
                            _ => {
                                let mut a = dst;
                                for _ in 0..n {
                                    if size == 2 {
                                        mem.write_u16(a, v as u16)
                                    } else {
                                        mem.write_u32(a, v)
                                    }
                                    a = a.wrapping_add(size);
                                }
                            }
                        }
                        self.regs[EDI] = self.regs[EDI].wrapping_add(bytes as u32);
                        self.regs[ECX] = 0;
                        return Ok(());
                    }
                    _ => {}
                }
            }
        }
        while n > 0 {
            match op {
                StrOp::Movs => {
                    let v = self.rd(mem, self.regs[ESI].wrapping_add(sbase), size)?;
                    self.wr(mem, self.regs[EDI], size, v)?;
                    self.regs[ESI] = self.regs[ESI].wrapping_add(step);
                    self.regs[EDI] = self.regs[EDI].wrapping_add(step);
                }
                StrOp::Stos => {
                    self.wr(mem, self.regs[EDI], size, self.acc(size))?;
                    self.regs[EDI] = self.regs[EDI].wrapping_add(step);
                }
                StrOp::Lods => {
                    let v = self.rd(mem, self.regs[ESI].wrapping_add(sbase), size)?;
                    self.set_acc(size, v);
                    self.regs[ESI] = self.regs[ESI].wrapping_add(step);
                }
                StrOp::Scas => {
                    let v = self.rd(mem, self.regs[EDI], size)?;
                    self.alu_sub(self.acc(size), v, 0, size);
                    self.regs[EDI] = self.regs[EDI].wrapping_add(step);
                }
                StrOp::Cmps => {
                    let a = self.rd(mem, self.regs[ESI].wrapping_add(sbase), size)?;
                    let b = self.rd(mem, self.regs[EDI], size)?;
                    self.alu_sub(a, b, 0, size);
                    self.regs[ESI] = self.regs[ESI].wrapping_add(step);
                    self.regs[EDI] = self.regs[EDI].wrapping_add(step);
                }
            }
            n -= 1;
            if rep {
                self.regs[ECX] = n;
                if matches!(op, StrOp::Scas | StrOp::Cmps) {
                    let zf = self.flag(fl::ZF);
                    if (repne && zf) || (!repne && !zf) {
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    fn rdtsc_value() -> u64 {
        static START: OnceLock<Instant> = OnceLock::new();
        let ns = START.get_or_init(Instant::now).elapsed().as_nanos() as u64;
        ns / 5 // pretend 200 MHz
    }
}

/// Execute one decoded instruction.
#[inline(always)]
pub(crate) fn step(cpu: &mut Cpu, mem: &mut GuestMemory, ins: &Instruction) -> R {
    use Mnemonic as M;
    let m = ins.mnemonic();
    match m {
        M::Mov => {
            let s = Cpu::op_size(ins);
            let v = cpu.read_op(mem, ins, 1, s)?;
            cpu.write_op(mem, ins, 0, s, v)
        }
        M::Movzx | M::Movsx => {
            let ds = Cpu::op_size(ins);
            let ss = Cpu::op_size_n(ins, 1);
            let v = cpu.read_op(mem, ins, 1, ss)?;
            let v = if m == M::Movsx { sext(v, ss) as u32 & mask(ds) } else { v };
            cpu.write_op(mem, ins, 0, ds, v)
        }
        M::Lea => {
            let s = Cpu::op_size(ins);
            let a = cpu.ea_noseg(ins);
            cpu.write_op(mem, ins, 0, s, a & mask(s))
        }
        M::Xchg => {
            let s = Cpu::op_size(ins);
            let a = cpu.read_op(mem, ins, 0, s)?;
            let b = cpu.read_op(mem, ins, 1, s)?;
            cpu.write_op(mem, ins, 0, s, b)?;
            cpu.write_op(mem, ins, 1, s, a)
        }
        M::Push => {
            let s = (-ins.stack_pointer_increment()) as u32;
            let v = cpu.read_op(mem, ins, 0, s)?;
            cpu.push(mem, s, v)
        }
        M::Pop => {
            let s = ins.stack_pointer_increment() as u32;
            let v = cpu.pop(mem, s)?;
            cpu.write_op(mem, ins, 0, s, v)
        }
        M::Pushad => {
            let sp = cpu.regs[ESP];
            for r in [EAX, ECX, EDX, EBX] {
                cpu.push(mem, 4, cpu.regs[r])?;
            }
            cpu.push(mem, 4, sp)?;
            for r in [EBP, ESI, EDI] {
                cpu.push(mem, 4, cpu.regs[r])?;
            }
            Ok(())
        }
        M::Popad => {
            for r in [EDI, ESI, EBP] {
                cpu.regs[r] = cpu.pop(mem, 4)?;
            }
            cpu.pop(mem, 4)?;
            for r in [EBX, EDX, ECX, EAX] {
                cpu.regs[r] = cpu.pop(mem, 4)?;
            }
            Ok(())
        }
        M::Pushfd => cpu.push(mem, 4, (cpu.eflags() & 0x00FC_FFFF) | 2),
        M::Pushf => cpu.push(mem, 2, cpu.eflags() & 0xFFFF),
        M::Popfd => {
            let v = cpu.pop(mem, 4)?;
            cpu.set_eflags((v & POPF_MASK) | fl::IF | 2);
            Ok(())
        }
        M::Popf => {
            let v = cpu.pop(mem, 2)?;
            cpu.set_eflags((cpu.eflags() & 0xFFFF_0000) | (v & POPF_MASK & 0xFFFF) | fl::IF | 2);
            Ok(())
        }
        M::Call => {
            let target = match ins.op0_kind() {
                OpKind::NearBranch32 | OpKind::NearBranch16 => ins.near_branch_target() as u32,
                OpKind::Register | OpKind::Memory => cpu.read_op(mem, ins, 0, 4)?,
                _ => return Err(cpu.fault(FaultKind::GeneralProtection, format!("far call: {ins}"))),
            };
            cpu.push(mem, 4, cpu.eip)?;
            cpu.eip = target;
            Ok(())
        }
        M::Ret => {
            let ret = cpu.pop(mem, 4)?;
            if ins.op_count() == 1 {
                cpu.regs[ESP] = cpu.regs[ESP].wrapping_add(ins.immediate16() as u32);
            }
            cpu.eip = ret;
            Ok(())
        }
        M::Jmp => {
            cpu.eip = match ins.op0_kind() {
                OpKind::NearBranch32 | OpKind::NearBranch16 => ins.near_branch_target() as u32,
                OpKind::Register | OpKind::Memory => cpu.read_op(mem, ins, 0, 4)?,
                _ => return Err(cpu.fault(FaultKind::GeneralProtection, format!("far jmp: {ins}"))),
            };
            Ok(())
        }
        M::Jo | M::Jno | M::Jb | M::Jae | M::Je | M::Jne | M::Jbe | M::Ja | M::Js | M::Jns | M::Jp | M::Jnp
        | M::Jl | M::Jge | M::Jle | M::Jg => {
            if cpu.cond(ins.condition_code()) {
                cpu.eip = ins.near_branch_target() as u32;
            }
            Ok(())
        }
        M::Loop | M::Loope | M::Loopne => {
            // Address-size prefix 0x67 selects CX as the counter.
            let cx = matches!(
                ins.code(),
                Code::Loop_rel8_16_CX | Code::Loop_rel8_32_CX | Code::Loope_rel8_16_CX | Code::Loope_rel8_32_CX
                    | Code::Loopne_rel8_16_CX | Code::Loopne_rel8_32_CX
            );
            let c = if cx {
                let c = cpu.regs[ECX].wrapping_sub(1) & 0xFFFF;
                cpu.set_reg(Register::CX, c);
                c
            } else {
                let c = cpu.regs[ECX].wrapping_sub(1);
                cpu.regs[ECX] = c;
                c
            };
            let zf = cpu.flag(fl::ZF);
            let take = c != 0
                && match m {
                    M::Loope => zf,
                    M::Loopne => !zf,
                    _ => true,
                };
            if take {
                cpu.eip = ins.near_branch_target() as u32;
            }
            Ok(())
        }
        M::Jecxz => {
            if cpu.regs[ECX] == 0 {
                cpu.eip = ins.near_branch_target() as u32;
            }
            Ok(())
        }
        // `67 E3 rel8`: tests CX only (used by the game's blitters, see static-isa.md).
        M::Jcxz => {
            if cpu.regs[ECX] & 0xFFFF == 0 {
                cpu.eip = ins.near_branch_target() as u32;
            }
            Ok(())
        }
        M::Add | M::Adc | M::Sub | M::Sbb | M::Cmp | M::And | M::Or | M::Xor | M::Test => {
            let s = Cpu::op_size(ins);
            let a = cpu.read_op(mem, ins, 0, s)?;
            let b = cpu.read_op(mem, ins, 1, s)?;
            let cf = cpu.flag(fl::CF) as u32;
            let r = match m {
                M::Add => cpu.alu_add(a, b, 0, s),
                M::Adc => cpu.alu_add(a, b, cf, s),
                M::Sub | M::Cmp => cpu.alu_sub(a, b, 0, s),
                M::Sbb => cpu.alu_sub(a, b, cf, s),
                M::And | M::Test => cpu.alu_logic(a & b, s),
                M::Or => cpu.alu_logic(a | b, s),
                _ => cpu.alu_logic(a ^ b, s),
            };
            if m != M::Cmp && m != M::Test {
                cpu.write_op(mem, ins, 0, s, r)?;
            }
            Ok(())
        }
        M::Inc | M::Dec => {
            let s = Cpu::op_size(ins);
            let a = cpu.read_op(mem, ins, 0, s)?;
            let r = if m == M::Inc { cpu.alu_inc(a, s) } else { cpu.alu_dec(a, s) };
            cpu.write_op(mem, ins, 0, s, r)
        }
        M::Neg => {
            let s = Cpu::op_size(ins);
            let a = cpu.read_op(mem, ins, 0, s)?;
            let r = cpu.alu_sub(0, a, 0, s);
            cpu.write_op(mem, ins, 0, s, r)
        }
        M::Not => {
            let s = Cpu::op_size(ins);
            let a = cpu.read_op(mem, ins, 0, s)?;
            cpu.write_op(mem, ins, 0, s, !a & mask(s))
        }
        M::Shl | M::Sal | M::Shr | M::Sar | M::Rol | M::Ror | M::Rcl | M::Rcr => {
            let s = Cpu::op_size(ins);
            let a = cpu.read_op(mem, ins, 0, s)?;
            let c = cpu.read_op(mem, ins, 1, 1)?;
            if let Some(r) = cpu.shift(m, a, c, s) {
                cpu.write_op(mem, ins, 0, s, r)?;
            }
            Ok(())
        }
        M::Shld | M::Shrd => {
            let s = Cpu::op_size(ins);
            let d = cpu.read_op(mem, ins, 0, s)?;
            let src = cpu.read_op(mem, ins, 1, s)?;
            let c = cpu.read_op(mem, ins, 2, 1)? & 0x1F;
            if c == 0 {
                return Ok(());
            }
            let bits = s * 8;
            let (r, cf) = if s == 4 {
                if m == M::Shld {
                    let w = ((d as u64) << 32) | src as u64;
                    (((w << c) >> 32) as u32, (d >> (32 - c)) & 1 != 0)
                } else {
                    let w = ((src as u64) << 32) | d as u64;
                    ((w >> c) as u32, (d >> (c - 1)) & 1 != 0)
                }
            } else {
                // 16-bit: concatenate to 32 bits (count > 16 is undefined on real hardware).
                if m == M::Shld {
                    let w = (d << 16) | src;
                    (((w as u64) << c >> 16) as u32 & 0xFFFF, ((w as u64) << c >> 32) & 1 != 0)
                } else {
                    let w = ((src as u64) << 16) | d as u64;
                    ((w >> c) as u32 & 0xFFFF, (w >> (c - 1)) & 1 != 0)
                }
            };
            cpu.set_szp(r, s);
            cpu.set_flag(fl::CF, cf);
            cpu.set_flag(fl::OF, ((r ^ d) & sign_bit(s)) != 0);
            let _ = bits;
            cpu.write_op(mem, ins, 0, s, r)
        }
        M::Mul | M::Imul if ins.op_count() == 1 => {
            let s = Cpu::op_size(ins);
            let b = cpu.read_op(mem, ins, 0, s)?;
            let a = cpu.acc(s);
            let (lo, hi, of) = if m == M::Mul {
                let r = a as u64 * b as u64;
                match s {
                    1 => ((r & 0xFFFF) as u32, 0, r > 0xFF),
                    2 => ((r & 0xFFFF) as u32, (r >> 16) as u32, r > 0xFFFF),
                    _ => (r as u32, (r >> 32) as u32, r > 0xFFFF_FFFF),
                }
            } else {
                let r = sext(a, s) as i64 * sext(b, s) as i64;
                let fits = match s {
                    1 => r == r as i8 as i64,
                    2 => r == r as i16 as i64,
                    _ => r == r as i32 as i64,
                };
                match s {
                    1 => ((r as u32) & 0xFFFF, 0, !fits),
                    2 => ((r as u32) & 0xFFFF, ((r >> 16) as u32) & 0xFFFF, !fits),
                    _ => (r as u32, (r >> 32) as u32, !fits),
                }
            };
            match s {
                1 => cpu.set_reg(Register::AX, lo),
                2 => {
                    cpu.set_reg(Register::AX, lo);
                    cpu.set_reg(Register::DX, hi);
                }
                _ => {
                    cpu.regs[EAX] = lo;
                    cpu.regs[EDX] = hi;
                }
            }
            cpu.set_szp(lo, s);
            cpu.set_flag(fl::CF, of);
            cpu.set_flag(fl::OF, of);
            Ok(())
        }
        M::Imul => {
            let s = Cpu::op_size(ins);
            let (a, b) = if ins.op_count() == 2 {
                (cpu.read_op(mem, ins, 0, s)?, cpu.read_op(mem, ins, 1, s)?)
            } else {
                (cpu.read_op(mem, ins, 1, s)?, ins.immediate(2) as u32)
            };
            let r = sext(a, s) as i64 * sext(b, s) as i64;
            let lo = r as u32 & mask(s);
            let of = sext(lo, s) as i64 != r;
            cpu.set_szp(lo, s);
            cpu.set_flag(fl::CF, of);
            cpu.set_flag(fl::OF, of);
            cpu.write_op(mem, ins, 0, s, lo)
        }
        M::Div | M::Idiv => {
            let s = Cpu::op_size(ins);
            let d = cpu.read_op(mem, ins, 0, s)?;
            if d & mask(s) == 0 {
                return Err(cpu.fault(FaultKind::DivideError, "divide by zero"));
            }
            let de = || cpu.fault(FaultKind::DivideError, "quotient overflow");
            let (q, r) = if m == M::Div {
                let n: u64 = match s {
                    1 => (cpu.regs[EAX] & 0xFFFF) as u64,
                    2 => (((cpu.regs[EDX] & 0xFFFF) << 16) | (cpu.regs[EAX] & 0xFFFF)) as u64,
                    _ => ((cpu.regs[EDX] as u64) << 32) | cpu.regs[EAX] as u64,
                };
                let q = n / d as u64;
                if q > mask(s) as u64 {
                    return Err(de());
                }
                (q as u32, (n % d as u64) as u32)
            } else {
                let n: i64 = match s {
                    1 => cpu.regs[EAX] as u16 as i16 as i64,
                    2 => ((((cpu.regs[EDX] & 0xFFFF) << 16) | (cpu.regs[EAX] & 0xFFFF)) as i32) as i64,
                    _ => (((cpu.regs[EDX] as u64) << 32) | cpu.regs[EAX] as u64) as i64,
                };
                let dv = sext(d, s) as i64;
                if n == i64::MIN && dv == -1 {
                    return Err(de());
                }
                let q = n / dv;
                let (lo, hi) = match s {
                    1 => (i8::MIN as i64, i8::MAX as i64),
                    2 => (i16::MIN as i64, i16::MAX as i64),
                    _ => (i32::MIN as i64, i32::MAX as i64),
                };
                if q < lo || q > hi {
                    return Err(de());
                }
                (q as u32 & mask(s), (n % dv) as u32 & mask(s))
            };
            match s {
                1 => {
                    cpu.set_reg(Register::AL, q);
                    cpu.set_reg(Register::AH, r);
                }
                2 => {
                    cpu.set_reg(Register::AX, q);
                    cpu.set_reg(Register::DX, r);
                }
                _ => {
                    cpu.regs[EAX] = q;
                    cpu.regs[EDX] = r;
                }
            }
            Ok(())
        }
        M::Cbw => {
            let v = cpu.regs[EAX] as u8 as i8 as i16 as u16 as u32;
            cpu.set_reg(Register::AX, v);
            Ok(())
        }
        M::Cwde => {
            cpu.regs[EAX] = cpu.regs[EAX] as u16 as i16 as i32 as u32;
            Ok(())
        }
        M::Cwd => {
            let v = if cpu.regs[EAX] & 0x8000 != 0 { 0xFFFF } else { 0 };
            cpu.set_reg(Register::DX, v);
            Ok(())
        }
        M::Cdq => {
            cpu.regs[EDX] = if cpu.regs[EAX] & 0x8000_0000 != 0 { 0xFFFF_FFFF } else { 0 };
            Ok(())
        }
        M::Seto | M::Setno | M::Setb | M::Setae | M::Sete | M::Setne | M::Setbe | M::Seta | M::Sets | M::Setns
        | M::Setp | M::Setnp | M::Setl | M::Setge | M::Setle | M::Setg => {
            let v = cpu.cond(ins.condition_code()) as u32;
            cpu.write_op(mem, ins, 0, 1, v)
        }
        M::Cmovo | M::Cmovno | M::Cmovb | M::Cmovae | M::Cmove | M::Cmovne | M::Cmovbe | M::Cmova | M::Cmovs
        | M::Cmovns | M::Cmovp | M::Cmovnp | M::Cmovl | M::Cmovge | M::Cmovle | M::Cmovg => {
            let s = Cpu::op_size(ins);
            let v = cpu.read_op(mem, ins, 1, s)?;
            if cpu.cond(ins.condition_code()) {
                cpu.write_op(mem, ins, 0, s, v)?;
            }
            Ok(())
        }
        M::Bt | M::Bts | M::Btr | M::Btc => {
            let s = Cpu::op_size(ins);
            let bits = s * 8;
            let off = cpu.read_op(mem, ins, 1, s)?;
            let (addr, bit) = if ins.op0_kind() == OpKind::Memory && ins.op1_kind() == OpKind::Register {
                let so = sext(off, s);
                let shift = if s == 4 { 5 } else { 4 };
                (Some(cpu.ea(ins).wrapping_add(((so >> shift) * s as i32) as u32)), off & (bits - 1))
            } else {
                (None, off & (bits - 1))
            };
            let v = match addr {
                Some(a) => cpu.rd(mem, a, s)?,
                None => cpu.read_op(mem, ins, 0, s)?,
            };
            cpu.set_flag(fl::CF, (v >> bit) & 1 != 0);
            let nv = match m {
                M::Bts => v | (1 << bit),
                M::Btr => v & !(1 << bit),
                M::Btc => v ^ (1 << bit),
                _ => return Ok(()),
            };
            match addr {
                Some(a) => cpu.wr(mem, a, s, nv),
                None => cpu.write_op(mem, ins, 0, s, nv),
            }
        }
        M::Bsf | M::Bsr => {
            let s = Cpu::op_size(ins);
            let v = cpu.read_op(mem, ins, 1, s)? & mask(s);
            if v == 0 {
                cpu.set_flag(fl::ZF, true);
                return Ok(());
            }
            cpu.set_flag(fl::ZF, false);
            let r = if m == M::Bsf { v.trailing_zeros() } else { 31 - v.leading_zeros() };
            cpu.write_op(mem, ins, 0, s, r)
        }
        M::Bswap => {
            let r = ins.op0_register();
            let v = cpu.get_reg(r);
            cpu.set_reg(r, v.swap_bytes());
            Ok(())
        }
        M::Xadd => {
            let s = Cpu::op_size(ins);
            let d = cpu.read_op(mem, ins, 0, s)?;
            let src = cpu.read_op(mem, ins, 1, s)?;
            let r = cpu.alu_add(d, src, 0, s);
            cpu.write_op(mem, ins, 1, s, d)?;
            cpu.write_op(mem, ins, 0, s, r)
        }
        M::Cmpxchg => {
            let s = Cpu::op_size(ins);
            let d = cpu.read_op(mem, ins, 0, s)?;
            let a = cpu.acc(s);
            cpu.alu_sub(a, d, 0, s);
            if a == d & mask(s) {
                let src = cpu.read_op(mem, ins, 1, s)?;
                cpu.write_op(mem, ins, 0, s, src)
            } else {
                cpu.set_acc(s, d);
                Ok(())
            }
        }
        M::Cmpxchg8b => {
            let a = cpu.ea(ins);
            cpu.check_addr(a, true)?;
            let v = mem.read_u64(a);
            let cmp = ((cpu.regs[EDX] as u64) << 32) | cpu.regs[EAX] as u64;
            if v == cmp {
                mem.write_u64(a, ((cpu.regs[ECX] as u64) << 32) | cpu.regs[EBX] as u64);
                cpu.set_flag(fl::ZF, true);
            } else {
                cpu.regs[EAX] = v as u32;
                cpu.regs[EDX] = (v >> 32) as u32;
                cpu.set_flag(fl::ZF, false);
            }
            Ok(())
        }
        M::Leave => {
            cpu.regs[ESP] = cpu.regs[EBP];
            cpu.regs[EBP] = cpu.pop(mem, 4)?;
            Ok(())
        }
        M::Enter => {
            let size = ins.immediate16() as u32;
            let level = (ins.immediate8_2nd() & 0x1F) as u32;
            cpu.push(mem, 4, cpu.regs[EBP])?;
            let frame = cpu.regs[ESP];
            if level > 0 {
                let mut bp = cpu.regs[EBP];
                for _ in 1..level {
                    bp = bp.wrapping_sub(4);
                    let v = cpu.rd(mem, bp, 4)?;
                    cpu.push(mem, 4, v)?;
                }
                cpu.push(mem, 4, frame)?;
            }
            cpu.regs[EBP] = frame;
            cpu.regs[ESP] = cpu.regs[ESP].wrapping_sub(size);
            Ok(())
        }
        M::Movsb | M::Movsw | M::Movsd if ins.op0_kind() != OpKind::Register => {
            cpu.string_op(mem, ins, StrOp::Movs, str_size(m))
        }
        M::Stosb | M::Stosw | M::Stosd => cpu.string_op(mem, ins, StrOp::Stos, str_size(m)),
        M::Lodsb | M::Lodsw | M::Lodsd => cpu.string_op(mem, ins, StrOp::Lods, str_size(m)),
        M::Scasb | M::Scasw | M::Scasd => cpu.string_op(mem, ins, StrOp::Scas, str_size(m)),
        M::Cmpsb | M::Cmpsw | M::Cmpsd if ins.op0_kind() != OpKind::Register => {
            cpu.string_op(mem, ins, StrOp::Cmps, str_size(m))
        }
        M::Xlatb => {
            let a = cpu.regs[EBX]
                .wrapping_add(cpu.regs[EAX] & 0xFF)
                .wrapping_add(cpu.seg_base(ins.memory_segment()));
            let v = cpu.rd(mem, a, 1)?;
            cpu.set_reg(Register::AL, v);
            Ok(())
        }
        M::Sahf => {
            let ah = (cpu.regs[EAX] >> 8) & 0xFF;
            let keep = fl::SF | fl::ZF | fl::AF | fl::PF | fl::CF;
            cpu.set_eflags((cpu.eflags() & !keep) | (ah & keep));
            Ok(())
        }
        M::Lahf => {
            let v = (cpu.eflags() & (fl::SF | fl::ZF | fl::AF | fl::PF | fl::CF)) | 2;
            cpu.set_reg(Register::AH, v);
            Ok(())
        }
        M::Clc => {
            cpu.set_flag(fl::CF, false);
            Ok(())
        }
        M::Stc => {
            cpu.set_flag(fl::CF, true);
            Ok(())
        }
        M::Cmc => {
            cpu.set_eflags(cpu.eflags() ^ fl::CF);
            Ok(())
        }
        M::Cld => {
            cpu.set_flag(fl::DF, false);
            Ok(())
        }
        M::Std => {
            cpu.set_flag(fl::DF, true);
            Ok(())
        }
        M::Cli | M::Sti | M::Nop | M::Wait | M::Pause | M::Lfence | M::Sfence | M::Mfence => Ok(()),
        M::Cpuid => {
            match cpu.regs[EAX] {
                0 => {
                    cpu.regs[EAX] = 1;
                    // "GenuineIntel"
                    cpu.regs[EBX] = 0x756E_6547;
                    cpu.regs[EDX] = 0x4965_6E69;
                    cpu.regs[ECX] = 0x6C65_746E;
                }
                1 => {
                    // Family 5 model 2 (Pentium P54C). Deliberately NO MMX (bit 23): S3.EXE then never runs its
                    // MMX surface-copy routine (static-isa.md #8).
                    cpu.regs[EAX] = 0x0000_0525;
                    cpu.regs[EBX] = 0;
                    cpu.regs[ECX] = 0;
                    cpu.regs[EDX] = 1 | (1 << 4) | (1 << 8); // FPU TSC CX8
                }
                _ => {
                    cpu.regs[EAX] = 0;
                    cpu.regs[EBX] = 0;
                    cpu.regs[ECX] = 0;
                    cpu.regs[EDX] = 0;
                }
            }
            Ok(())
        }
        M::Rdtsc => {
            let t = Cpu::rdtsc_value();
            cpu.regs[EAX] = t as u32;
            cpu.regs[EDX] = (t >> 32) as u32;
            Ok(())
        }
        M::Int3 => Err(CpuExit::Interrupt(3)),
        M::Int => Err(CpuExit::Interrupt(ins.immediate8())),
        M::Into => {
            if cpu.flag(fl::OF) {
                Err(CpuExit::Interrupt(4))
            } else {
                Ok(())
            }
        }
        M::Hlt => Err(CpuExit::Halt),
        M::Salc => {
            let v = if cpu.flag(fl::CF) { 0xFF } else { 0 };
            cpu.set_reg(Register::AL, v);
            Ok(())
        }
        M::Aam => {
            let base = ins.immediate8() as u32;
            if base == 0 {
                return Err(cpu.fault(FaultKind::DivideError, "aam 0"));
            }
            let al = cpu.regs[EAX] & 0xFF;
            let (q, r) = (al / base, al % base);
            cpu.set_reg(Register::AX, (q << 8) | r);
            cpu.alu_logic(r, 1);
            Ok(())
        }
        M::Aad => {
            let base = ins.immediate8() as u32;
            let ax = cpu.regs[EAX];
            let al = ((ax & 0xFF) + ((ax >> 8) & 0xFF) * base) & 0xFF;
            cpu.set_reg(Register::AX, al);
            cpu.alu_logic(al, 1);
            Ok(())
        }
        M::Aaa | M::Aas => {
            let ax = cpu.regs[EAX] & 0xFFFF;
            if (ax & 0x0F) > 9 || cpu.flag(fl::AF) {
                let nax = if m == M::Aaa {
                    (ax.wrapping_add(0x106)) & 0xFFFF
                } else {
                    // Intel: AX <- AX - 6 (may borrow from AH); AH <- AH - 1.
                    let t = ax.wrapping_sub(6) & 0xFFFF;
                    (t & 0xFF) | ((((t >> 8).wrapping_sub(1)) & 0xFF) << 8)
                };
                cpu.set_reg(Register::AX, nax & 0xFF0F);
                cpu.set_flag(fl::AF, true);
                cpu.set_flag(fl::CF, true);
            } else {
                cpu.set_reg(Register::AL, ax & 0x0F);
                cpu.set_flag(fl::AF, false);
                cpu.set_flag(fl::CF, false);
            }
            Ok(())
        }
        M::Daa | M::Das => {
            let old_al = cpu.regs[EAX] & 0xFF;
            let old_cf = cpu.flag(fl::CF);
            let mut al = old_al;
            // Intel pseudo-code: the first step's carry can only happen when the second step also runs, so
            // CF is decided by the second step alone.
            let af = (old_al & 0x0F) > 9 || cpu.flag(fl::AF);
            if af {
                al = if m == M::Daa { al.wrapping_add(6) } else { al.wrapping_sub(6) } & 0xFF;
            }
            let cf = old_al > 0x99 || old_cf;
            if cf {
                al = if m == M::Daa { al.wrapping_add(0x60) } else { al.wrapping_sub(0x60) } & 0xFF;
            }
            cpu.set_reg(Register::AL, al);
            cpu.set_szp(al, 1);
            cpu.set_flag(fl::AF, af);
            cpu.set_flag(fl::CF, cf);
            Ok(())
        }
        // Privileged / segment / far control flow: never in reached S3.EXE code (static-isa.md); Windows
        // would raise an access violation / privileged-instruction exception.
        M::In | M::Out | M::Insb | M::Insw | M::Insd | M::Outsb | M::Outsw | M::Outsd | M::Iret | M::Iretd
        | M::Retf | M::Lgdt | M::Lidt | M::Lldt | M::Ltr | M::Lmsw | M::Clts | M::Invd | M::Wbinvd | M::Invlpg
        | M::Rdmsr | M::Wrmsr | M::Bound | M::Arpl | M::Lds | M::Les | M::Lfs | M::Lgs | M::Lss => {
            Err(cpu.fault(FaultKind::GeneralProtection, format!("{:?}: {}", m, ins)))
        }
        M::Ud0 | M::Ud1 | M::Ud2 => Err(cpu.fault(FaultKind::InvalidOpcode, format!("{ins}"))),
        _ => match fpu::exec(cpu, mem, ins) {
            Some(r) => r,
            None => Err(cpu.unimplemented(ins)),
        },
    }
}

fn str_size(m: Mnemonic) -> u32 {
    use Mnemonic as M;
    match m {
        M::Movsb | M::Stosb | M::Lodsb | M::Scasb | M::Cmpsb => 1,
        M::Movsw | M::Stosw | M::Lodsw | M::Scasw | M::Cmpsw => 2,
        _ => 4,
    }
}
