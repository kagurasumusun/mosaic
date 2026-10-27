//! ARMv7-M Thumb execution.
//!
//! The encoding and flag rules follow the ARMv7-M architecture (16-bit Thumb
//! plus the 32-bit BL form). Unknown 32-bit encodings fault as undefined
//! rather than being skipped. This is not a firmware-shaped interpreter.

use mosaic_exec::BusFault;

/// Architectural guest state the memory port does not own.
pub struct Guest {
    pub r: [u32; 16],
    pub n: bool,
    pub z: bool,
    pub c: bool,
    pub v: bool,
    pub primask: bool,
    pub it: u8,
    pub vtor: u32,
    pub ipsr: u32,
    pub irq_line: bool,
}

impl Guest {
    pub fn new() -> Self {
        Self {
            r: [0; 16],
            n: false,
            z: false,
            c: false,
            v: false,
            primask: false,
            it: 0,
            vtor: 0,
            ipsr: 0,
            irq_line: false,
        }
    }

    fn apsr(&self) -> u32 {
        (u32::from(self.n) << 31)
            | (u32::from(self.z) << 30)
            | (u32::from(self.c) << 29)
            | (u32::from(self.v) << 28)
    }

    fn set_apsr(&mut self, xpsr: u32) {
        self.n = xpsr & (1 << 31) != 0;
        self.z = xpsr & (1 << 30) != 0;
        self.c = xpsr & (1 << 29) != 0;
        self.v = xpsr & (1 << 28) != 0;
    }
}

/// A guest memory fault, already in bus vocabulary so the engine can record it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemFault {
    pub address: u64,
    pub width: u8,
    pub fault: BusFault,
}

pub trait GuestMem {
    fn load(&mut self, addr: u32, size: u8, execute: bool) -> Result<u32, MemFault>;
    fn store(&mut self, addr: u32, size: u8, value: u32) -> Result<(), MemFault>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecStop {
    WaitingForInterrupt,
    Breakpoint,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecFault {
    Mem(MemFault),
    Undefined,
    InvState,
}

/// Execute one instruction at `guest.r[15]`.
///
/// `Ok(None)` retires the instruction. `Ok(Some)` retires it and asks the
/// quantum to stop. `Err` does not retire: PC still names the faulting
/// instruction.
pub fn step(guest: &mut Guest, mem: &mut dyn GuestMem) -> Result<Option<ExecStop>, ExecFault> {
    let pc = guest.r[15];
    if pc & 1 != 0 {
        return Err(ExecFault::InvState);
    }
    let hw1 = load_u16(mem, pc).map_err(ExecFault::Mem)?;
    let wide = is_wide(hw1);
    if wide {
        let _hw2 = load_u16(mem, pc.wrapping_add(2)).map_err(ExecFault::Mem)?;
    }
    let in_it = guest.it != 0;
    if in_it && !condition(guest.it >> 4, guest) {
        guest.r[15] = pc.wrapping_add(if wide { 4 } else { 2 });
        it_advance(guest);
        return Ok(None);
    }
    let stop = if wide {
        let hw2 = load_u16(mem, pc.wrapping_add(2)).map_err(ExecFault::Mem)?;
        exec_wide(guest, mem, pc, hw1, hw2)?
    } else {
        exec_narrow(guest, mem, pc, hw1)?
    };
    // IT itself installs the mask; advancing here would consume the first
    // conditional instruction before it runs.
    if in_it {
        it_advance(guest);
    }
    Ok(stop)
}

/// Stack an 8-word frame and vector to `number`. Called between instructions.
pub fn enter_exception(
    guest: &mut Guest,
    mem: &mut dyn GuestMem,
    number: u32,
) -> Result<(), ExecFault> {
    let mut sp = guest.r[13] & !0b11;
    let mut xpsr = guest.apsr() | (1 << 24) | (guest.ipsr & 0x1ff);
    if sp & 4 != 0 {
        sp -= 4;
        xpsr |= 1 << 9;
    }
    sp = sp.wrapping_sub(32);
    let frame = [
        guest.r[0],
        guest.r[1],
        guest.r[2],
        guest.r[3],
        guest.r[12],
        guest.r[14],
        guest.r[15],
        xpsr,
    ];
    for (index, word) in frame.iter().enumerate() {
        let addr = sp.wrapping_add((index as u32) * 4);
        mem.store(addr, 4, *word).map_err(ExecFault::Mem)?;
    }
    let vector_addr = guest.vtor.wrapping_add(number.wrapping_mul(4));
    let vector = mem.load(vector_addr, 4, false).map_err(ExecFault::Mem)?;
    if vector & 1 == 0 {
        return Err(ExecFault::InvState);
    }
    guest.r[13] = sp;
    guest.r[14] = 0xFFFF_FFF9;
    guest.r[15] = vector & !1;
    guest.ipsr = number & 0x1ff;
    Ok(())
}

fn is_wide(hw: u16) -> bool {
    hw >> 11 >= 0b11101
}

fn it_advance(guest: &mut Guest) {
    if guest.it & 0x07 == 0 {
        guest.it = 0;
    } else {
        guest.it = (guest.it & 0xe0) | ((guest.it << 1) & 0x1f);
    }
}

fn condition(cond: u8, guest: &Guest) -> bool {
    match cond & 0xf {
        0x0 => guest.z,
        0x1 => !guest.z,
        0x2 => guest.c,
        0x3 => !guest.c,
        0x4 => guest.n,
        0x5 => !guest.n,
        0x6 => guest.v,
        0x7 => !guest.v,
        0x8 => guest.c && !guest.z,
        0x9 => !guest.c || guest.z,
        0xA => guest.n == guest.v,
        0xB => guest.n != guest.v,
        0xC => !guest.z && guest.n == guest.v,
        0xD => guest.z || guest.n != guest.v,
        0xE => true,
        _ => false,
    }
}

fn load_u16(mem: &mut dyn GuestMem, addr: u32) -> Result<u16, MemFault> {
    if addr & 1 != 0 {
        return Err(MemFault {
            address: u64::from(addr),
            width: 2,
            fault: BusFault::Misaligned,
        });
    }
    Ok(mem.load(addr, 2, true)? as u16)
}

fn exec_narrow(
    guest: &mut Guest,
    mem: &mut dyn GuestMem,
    pc: u32,
    hw: u16,
) -> Result<Option<ExecStop>, ExecFault> {
    let op = hw >> 11;
    match op {
        0b00000..=0b00010 => shift_imm(guest, pc, hw),
        0b00011 => add_sub(guest, pc, hw),
        0b00100..=0b00111 => dp_imm(guest, pc, hw),
        0b01000 => {
            if hw & (1 << 10) == 0 {
                dp_reg(guest, pc, hw)
            } else {
                special(guest, mem, pc, hw)
            }
        }
        0b01001 => ldr_lit(guest, mem, pc, hw),
        0b01010 | 0b01011 => ld_st_reg(guest, mem, pc, hw),
        0b01100..=0b01111 => ld_st_imm(guest, mem, pc, hw),
        0b10000 | 0b10001 => ld_st_half(guest, mem, pc, hw),
        0b10010 | 0b10011 => sp_rel(guest, mem, pc, hw),
        0b10100 | 0b10101 => adr_add_sp(guest, pc, hw),
        0b10110 | 0b10111 => misc(guest, mem, pc, hw),
        0b11000 | 0b11001 => ldm_stm(guest, mem, pc, hw),
        0b11010 | 0b11011 => b_cond(guest, pc, hw),
        0b11100 => b_uncond(guest, pc, hw),
        _ => Err(ExecFault::Undefined),
    }
}

fn exec_wide(
    guest: &mut Guest,
    _mem: &mut dyn GuestMem,
    pc: u32,
    hw1: u16,
    hw2: u16,
) -> Result<Option<ExecStop>, ExecFault> {
    // BL: 11110 S imm10 / 11 J1 1 J2 imm11. BLX (bit 12 clear) has no A32
    // state on M-profile and is undefined.
    if hw1 >> 11 == 0b11110 && hw2 >> 14 == 0b11 && hw2 & (1 << 12) != 0 {
        let s = (hw1 >> 10) & 1;
        let imm10 = hw1 & 0x3ff;
        let j1 = (hw2 >> 13) & 1;
        let j2 = (hw2 >> 11) & 1;
        let imm11 = hw2 & 0x7ff;
        let i1 = u16::from(j1 ^ s == 0);
        let i2 = u16::from(j2 ^ s == 0);
        let packed = (u32::from(s) << 24)
            | (u32::from(i1) << 23)
            | (u32::from(i2) << 22)
            | (u32::from(imm10) << 12)
            | (u32::from(imm11) << 1);
        let imm = sign_extend(packed, 25);
        guest.r[14] = pc.wrapping_add(4) | 1;
        guest.r[15] = pc.wrapping_add(4).wrapping_add(imm);
        return Ok(None);
    }
    let _ = pc;
    Err(ExecFault::Undefined)
}

fn shift_imm(guest: &mut Guest, pc: u32, hw: u16) -> Result<Option<ExecStop>, ExecFault> {
    let kind = (hw >> 11) & 0b11;
    let mut imm = u32::from((hw >> 6) & 0x1f);
    let rm = reg3(hw, 3);
    let rd = reg3(hw, 0);
    let value = guest.r[rm];
    if kind != 0 && imm == 0 {
        imm = 32;
    }
    let (result, carry) = shift(kind, value, imm);
    write_flags_nzc(guest, result, carry);
    guest.r[rd] = result;
    advance(guest, pc, 2);
    Ok(None)
}

fn add_sub(guest: &mut Guest, pc: u32, hw: u16) -> Result<Option<ExecStop>, ExecFault> {
    let rn = guest.r[reg3(hw, 3)];
    let sub = hw & (1 << 9) != 0;
    let operand = if hw & (1 << 10) != 0 {
        u32::from((hw >> 6) & 0x7)
    } else {
        guest.r[reg3(hw, 6)]
    };
    let (result, c, v) = if sub {
        add_carry(rn, !operand, true)
    } else {
        add_carry(rn, operand, false)
    };
    write_flags(guest, result, c, v);
    guest.r[reg3(hw, 0)] = result;
    advance(guest, pc, 2);
    Ok(None)
}

fn dp_imm(guest: &mut Guest, pc: u32, hw: u16) -> Result<Option<ExecStop>, ExecFault> {
    let op = (hw >> 11) & 0b11;
    let rd = usize::from((hw >> 8) & 0x7);
    let imm = u32::from(hw & 0xff);
    let base = guest.r[rd];
    match op {
        0 => {
            write_flags_nz(guest, imm);
            guest.r[rd] = imm;
        }
        1 => {
            let (result, c, v) = add_carry(base, !imm, true);
            write_flags(guest, result, c, v);
        }
        2 => {
            let (result, c, v) = add_carry(base, imm, false);
            write_flags(guest, result, c, v);
            guest.r[rd] = result;
        }
        _ => {
            let (result, c, v) = add_carry(base, !imm, true);
            write_flags(guest, result, c, v);
            guest.r[rd] = result;
        }
    }
    advance(guest, pc, 2);
    Ok(None)
}

fn dp_reg(guest: &mut Guest, pc: u32, hw: u16) -> Result<Option<ExecStop>, ExecFault> {
    let op = (hw >> 6) & 0xf;
    let rm = reg3(hw, 3);
    let rd = reg3(hw, 0);
    let a = guest.r[rd];
    let b = guest.r[rm];
    let mut write = true;
    let (result, carry, overflow, touch_cv) = match op {
        0x0 => (a & b, None, None, false),
        0x1 => (a ^ b, None, None, false),
        0x2 => {
            let (value, c) = shift(0, a, b & 0xff);
            (value, c, None, true)
        }
        0x3 => {
            let (value, c) = shift(1, a, b & 0xff);
            (value, c, None, true)
        }
        0x4 => {
            let (value, c) = shift(2, a, b & 0xff);
            (value, c, None, true)
        }
        0x5 => {
            let (value, c, v) = add_carry(a, b, guest.c);
            (value, Some(c), Some(v), true)
        }
        0x6 => {
            let (value, c, v) = add_carry(a, !b, guest.c);
            (value, Some(c), Some(v), true)
        }
        0x7 => {
            let (value, c) = ror(a, b & 0xff);
            (value, c, None, true)
        }
        0x8 => {
            write = false;
            (a & b, None, None, false)
        }
        0x9 => {
            let (value, c, v) = add_carry(0, !b, true);
            (value, Some(c), Some(v), true)
        }
        0xA => {
            write = false;
            let (value, c, v) = add_carry(a, !b, true);
            (value, Some(c), Some(v), true)
        }
        0xB => {
            write = false;
            let (value, c, v) = add_carry(a, b, false);
            (value, Some(c), Some(v), true)
        }
        0xC => (a | b, None, None, false),
        0xD => (a.wrapping_mul(b), None, None, false),
        0xE => (a & !b, None, None, false),
        _ => (!b, None, None, false),
    };
    if touch_cv {
        write_flags(
            guest,
            result,
            carry.unwrap_or(guest.c),
            overflow.unwrap_or(guest.v),
        );
    } else {
        write_flags_nzc(guest, result, carry);
    }
    if write {
        guest.r[rd] = result;
    }
    advance(guest, pc, 2);
    Ok(None)
}

fn special(
    guest: &mut Guest,
    mem: &mut dyn GuestMem,
    pc: u32,
    hw: u16,
) -> Result<Option<ExecStop>, ExecFault> {
    let op = (hw >> 8) & 0b11;
    let rm = high_reg(hw, 3);
    let rd = high_reg(hw, 0);
    match op {
        0 => {
            let base = if rd == 15 {
                pc.wrapping_add(4)
            } else {
                guest.r[rd]
            };
            let addend = if rm == 15 {
                pc.wrapping_add(4)
            } else {
                guest.r[rm]
            };
            write_reg(guest, rd, base.wrapping_add(addend))?;
        }
        1 => {
            let left = if rd == 15 {
                pc.wrapping_add(4)
            } else {
                guest.r[rd]
            };
            let right = if rm == 15 {
                pc.wrapping_add(4)
            } else {
                guest.r[rm]
            };
            let (result, c, v) = add_carry(left, !right, true);
            write_flags(guest, result, c, v);
        }
        2 => {
            let value = if rm == 15 {
                pc.wrapping_add(4)
            } else {
                guest.r[rm]
            };
            write_reg(guest, rd, value)?;
        }
        _ => {
            if hw & 0x7 != 0 {
                return Err(ExecFault::Undefined);
            }
            let link = hw & (1 << 7) != 0;
            if link {
                guest.r[14] = pc.wrapping_add(2) | 1;
            }
            return branch_exchange(guest, mem, guest.r[rm]);
        }
    }
    if guest.r[15] == pc {
        advance(guest, pc, 2);
    }
    Ok(None)
}

fn ldr_lit(
    guest: &mut Guest,
    mem: &mut dyn GuestMem,
    pc: u32,
    hw: u16,
) -> Result<Option<ExecStop>, ExecFault> {
    let rt = usize::from((hw >> 8) & 0x7);
    let imm = u32::from(hw & 0xff) << 2;
    let address = (pc.wrapping_add(4) & !0b11).wrapping_add(imm);
    guest.r[rt] = mem.load(address, 4, false).map_err(ExecFault::Mem)?;
    advance(guest, pc, 2);
    Ok(None)
}

fn ld_st_reg(
    guest: &mut Guest,
    mem: &mut dyn GuestMem,
    pc: u32,
    hw: u16,
) -> Result<Option<ExecStop>, ExecFault> {
    let kind = (hw >> 9) & 0x7;
    let address = guest.r[reg3(hw, 3)].wrapping_add(guest.r[reg3(hw, 6)]);
    let rt = reg3(hw, 0);
    match kind {
        0 => mem.store(address, 4, guest.r[rt]).map_err(ExecFault::Mem)?,
        1 => mem.store(address, 2, guest.r[rt]).map_err(ExecFault::Mem)?,
        2 => mem.store(address, 1, guest.r[rt]).map_err(ExecFault::Mem)?,
        3 => {
            let raw = mem.load(address, 1, false).map_err(ExecFault::Mem)?;
            guest.r[rt] = sign_extend(raw, 8);
        }
        4 => guest.r[rt] = mem.load(address, 4, false).map_err(ExecFault::Mem)?,
        5 => guest.r[rt] = mem.load(address, 2, false).map_err(ExecFault::Mem)?,
        6 => guest.r[rt] = mem.load(address, 1, false).map_err(ExecFault::Mem)?,
        _ => {
            let raw = mem.load(address, 2, false).map_err(ExecFault::Mem)?;
            guest.r[rt] = sign_extend(raw, 16);
        }
    }
    advance(guest, pc, 2);
    Ok(None)
}

fn ld_st_imm(
    guest: &mut Guest,
    mem: &mut dyn GuestMem,
    pc: u32,
    hw: u16,
) -> Result<Option<ExecStop>, ExecFault> {
    let word = hw & (1 << 12) == 0;
    let load = hw & (1 << 11) != 0;
    let imm = u32::from((hw >> 6) & 0x1f);
    let address = guest.r[reg3(hw, 3)].wrapping_add(if word { imm << 2 } else { imm });
    let rt = reg3(hw, 0);
    transfer(guest, mem, rt, address, if word { 4 } else { 1 }, load)?;
    advance(guest, pc, 2);
    Ok(None)
}

fn ld_st_half(
    guest: &mut Guest,
    mem: &mut dyn GuestMem,
    pc: u32,
    hw: u16,
) -> Result<Option<ExecStop>, ExecFault> {
    let load = hw & (1 << 11) != 0;
    let imm = u32::from((hw >> 6) & 0x1f) << 1;
    let address = guest.r[reg3(hw, 3)].wrapping_add(imm);
    transfer(guest, mem, reg3(hw, 0), address, 2, load)?;
    advance(guest, pc, 2);
    Ok(None)
}

fn sp_rel(
    guest: &mut Guest,
    mem: &mut dyn GuestMem,
    pc: u32,
    hw: u16,
) -> Result<Option<ExecStop>, ExecFault> {
    let load = hw & (1 << 11) != 0;
    let rt = usize::from((hw >> 8) & 0x7);
    let address = guest.r[13].wrapping_add(u32::from(hw & 0xff) << 2);
    transfer(guest, mem, rt, address, 4, load)?;
    advance(guest, pc, 2);
    Ok(None)
}

fn adr_add_sp(guest: &mut Guest, pc: u32, hw: u16) -> Result<Option<ExecStop>, ExecFault> {
    let rd = usize::from((hw >> 8) & 0x7);
    let imm = u32::from(hw & 0xff) << 2;
    guest.r[rd] = if hw & (1 << 11) != 0 {
        guest.r[13].wrapping_add(imm)
    } else {
        (pc.wrapping_add(4) & !0b11).wrapping_add(imm)
    };
    advance(guest, pc, 2);
    Ok(None)
}

fn misc(
    guest: &mut Guest,
    mem: &mut dyn GuestMem,
    pc: u32,
    hw: u16,
) -> Result<Option<ExecStop>, ExecFault> {
    if hw & 0xf500 == 0xb100 {
        return cbz(guest, pc, hw);
    }
    match hw >> 8 {
        0xb0 => {
            let imm = u32::from(hw & 0x7f) << 2;
            if hw & (1 << 7) != 0 {
                guest.r[13] = guest.r[13].wrapping_sub(imm) & !0b11;
            } else {
                guest.r[13] = guest.r[13].wrapping_add(imm) & !0b11;
            }
        }
        0xb2 => extend_or_rev(guest, hw)?,
        0xb4 | 0xb5 => return push(guest, mem, pc, hw),
        0xb6 if hw & 0xffe0 == 0xb660 => {
            // CPSID/CPSIE i. Other CPS forms are undefined on this core.
            if hw & 0b10 == 0 {
                return Err(ExecFault::Undefined);
            }
            guest.primask = hw & (1 << 4) != 0;
        }
        0xba => extend_or_rev(guest, hw)?,
        0xbc | 0xbd => return pop(guest, mem, pc, hw),
        0xbe => {
            advance(guest, pc, 2);
            return Ok(Some(ExecStop::Breakpoint));
        }
        0xbf => return hint_or_it(guest, pc, hw),
        _ => return Err(ExecFault::Undefined),
    }
    advance(guest, pc, 2);
    Ok(None)
}

fn cbz(guest: &mut Guest, pc: u32, hw: u16) -> Result<Option<ExecStop>, ExecFault> {
    let nonzero = hw & (1 << 11) != 0;
    let imm = u32::from((hw >> 3) & 0x1f) | (u32::from((hw >> 9) & 1) << 5);
    let rn = guest.r[reg3(hw, 0)];
    let taken = if nonzero { rn != 0 } else { rn == 0 };
    if taken {
        guest.r[15] = pc.wrapping_add(4).wrapping_add(imm << 1);
    } else {
        advance(guest, pc, 2);
    }
    Ok(None)
}

fn push(
    guest: &mut Guest,
    mem: &mut dyn GuestMem,
    pc: u32,
    hw: u16,
) -> Result<Option<ExecStop>, ExecFault> {
    let mut list = hw & 0xff;
    if hw & (1 << 8) != 0 {
        list |= 1 << 14;
    }
    if list == 0 {
        return Err(ExecFault::Undefined);
    }
    let count = list.count_ones();
    let mut address = guest.r[13].wrapping_sub(count * 4);
    guest.r[13] = address & !0b11;
    for reg in 0..16 {
        if list & (1 << reg) != 0 {
            mem.store(address, 4, guest.r[reg])
                .map_err(ExecFault::Mem)?;
            address = address.wrapping_add(4);
        }
    }
    advance(guest, pc, 2);
    Ok(None)
}

fn pop(
    guest: &mut Guest,
    mem: &mut dyn GuestMem,
    pc: u32,
    hw: u16,
) -> Result<Option<ExecStop>, ExecFault> {
    let mut list = hw & 0xff;
    if hw & (1 << 8) != 0 {
        list |= 1 << 15;
    }
    if list == 0 {
        return Err(ExecFault::Undefined);
    }
    let mut address = guest.r[13];
    let mut values = [0u32; 16];
    for (reg, slot) in values.iter_mut().enumerate() {
        if list & (1 << reg) != 0 {
            *slot = mem.load(address, 4, false).map_err(ExecFault::Mem)?;
            address = address.wrapping_add(4);
        }
    }
    guest.r[13] = address & !0b11;
    for (reg, slot) in values.iter().enumerate().take(15) {
        if list & (1 << reg) != 0 {
            guest.r[reg] = *slot;
        }
    }
    guest.r[13] &= !0b11;
    if list & (1 << 15) != 0 {
        return branch_exchange(guest, mem, values[15]);
    }
    advance(guest, pc, 2);
    Ok(None)
}

fn extend_or_rev(guest: &mut Guest, hw: u16) -> Result<(), ExecFault> {
    let op = (hw >> 6) & 0b11;
    let value = guest.r[reg3(hw, 3)];
    let rd = reg3(hw, 0);
    let result = if hw >> 8 == 0xb2 {
        match op {
            0 => sign_extend(value, 16),
            1 => sign_extend(value, 8),
            2 => value & 0xffff,
            _ => value & 0xff,
        }
    } else if hw >> 8 == 0xba {
        match op {
            0 => value.swap_bytes(),
            1 => value.rotate_right(16),
            2 => return Err(ExecFault::Undefined),
            _ => sign_extend(((value & 0xff) << 8) | ((value >> 8) & 0xff), 16),
        }
    } else {
        return Err(ExecFault::Undefined);
    };
    guest.r[rd] = result;
    Ok(())
}

fn hint_or_it(guest: &mut Guest, pc: u32, hw: u16) -> Result<Option<ExecStop>, ExecFault> {
    if hw & 0xf != 0 {
        if guest.it != 0 {
            return Err(ExecFault::Undefined);
        }
        guest.it = hw as u8;
        advance(guest, pc, 2);
        return Ok(None);
    }
    match hw & 0xff {
        0x00 | 0x10 | 0x20 | 0x40 => {
            advance(guest, pc, 2);
            Ok(None)
        }
        // A pending line wakes WFI. PRIMASK masks taking the exception, not
        // the wake, so a masked core with no pending line still sleeps.
        0x30 if guest.irq_line => {
            advance(guest, pc, 2);
            Ok(None)
        }
        0x30 => Ok(Some(ExecStop::WaitingForInterrupt)),
        _ => Err(ExecFault::Undefined),
    }
}

fn ldm_stm(
    guest: &mut Guest,
    mem: &mut dyn GuestMem,
    pc: u32,
    hw: u16,
) -> Result<Option<ExecStop>, ExecFault> {
    let load = hw & (1 << 11) != 0;
    let rn = usize::from((hw >> 8) & 0x7);
    let list = hw & 0xff;
    if list == 0 {
        return Err(ExecFault::Undefined);
    }
    let base = guest.r[rn];
    let mut address = base;
    if load {
        let mut values = [0u32; 8];
        for (reg, slot) in values.iter_mut().enumerate() {
            if list & (1 << reg) != 0 {
                *slot = mem.load(address, 4, false).map_err(ExecFault::Mem)?;
                address = address.wrapping_add(4);
            }
        }
        for (reg, slot) in values.iter().enumerate() {
            if list & (1 << reg) != 0 {
                guest.r[reg] = *slot;
            }
        }
        if list & (1 << rn) == 0 {
            guest.r[rn] = address;
        }
    } else {
        let lowest = list.trailing_zeros() as usize;
        for reg in 0..8 {
            if list & (1 << reg) != 0 {
                let stored = if reg == rn && reg != lowest {
                    address
                } else {
                    guest.r[reg]
                };
                mem.store(address, 4, stored).map_err(ExecFault::Mem)?;
                address = address.wrapping_add(4);
            }
        }
        guest.r[rn] = address;
    }
    advance(guest, pc, 2);
    Ok(None)
}

fn b_cond(guest: &mut Guest, pc: u32, hw: u16) -> Result<Option<ExecStop>, ExecFault> {
    let cond = (hw >> 8) & 0xf;
    if cond == 0xE {
        return Err(ExecFault::Undefined);
    }
    if cond == 0xF {
        // SVC has no vectored delivery on this core yet.
        return Err(ExecFault::Undefined);
    }
    let imm = sign_extend(u32::from(hw & 0xff) << 1, 9);
    if condition(cond as u8, guest) {
        guest.r[15] = pc.wrapping_add(4).wrapping_add(imm);
    } else {
        advance(guest, pc, 2);
    }
    Ok(None)
}

fn b_uncond(guest: &mut Guest, pc: u32, hw: u16) -> Result<Option<ExecStop>, ExecFault> {
    let imm = sign_extend(u32::from(hw & 0x7ff) << 1, 12);
    guest.r[15] = pc.wrapping_add(4).wrapping_add(imm);
    Ok(None)
}

fn transfer(
    guest: &mut Guest,
    mem: &mut dyn GuestMem,
    rt: usize,
    address: u32,
    size: u8,
    load: bool,
) -> Result<(), ExecFault> {
    if load {
        guest.r[rt] = mem.load(address, size, false).map_err(ExecFault::Mem)?;
    } else {
        mem.store(address, size, guest.r[rt])
            .map_err(ExecFault::Mem)?;
    }
    Ok(())
}

fn branch_exchange(
    guest: &mut Guest,
    mem: &mut dyn GuestMem,
    target: u32,
) -> Result<Option<ExecStop>, ExecFault> {
    if guest.ipsr != 0 && target & 0xFFFF_FFF0 == 0xFFFF_FFF0 {
        return exception_return(guest, mem, target);
    }
    if target & 1 == 0 {
        return Err(ExecFault::InvState);
    }
    guest.r[15] = target & !1;
    Ok(None)
}

fn exception_return(
    guest: &mut Guest,
    mem: &mut dyn GuestMem,
    exc: u32,
) -> Result<Option<ExecStop>, ExecFault> {
    // Thread/Handler return on MSP. PSP (EXC_RETURN bit 2) is not modelled.
    if exc & (1 << 2) != 0 {
        return Err(ExecFault::Undefined);
    }
    let mut address = guest.r[13];
    let mut frame = [0u32; 8];
    for word in &mut frame {
        *word = mem.load(address, 4, false).map_err(ExecFault::Mem)?;
        address = address.wrapping_add(4);
    }
    let xpsr = frame[7];
    if xpsr & (1 << 9) != 0 {
        address = address.wrapping_add(4);
    }
    guest.r[0] = frame[0];
    guest.r[1] = frame[1];
    guest.r[2] = frame[2];
    guest.r[3] = frame[3];
    guest.r[12] = frame[4];
    guest.r[14] = frame[5];
    guest.r[13] = address & !0b11;
    guest.set_apsr(xpsr);
    guest.ipsr = xpsr & 0x1ff;
    if xpsr & (1 << 24) == 0 {
        return Err(ExecFault::InvState);
    }
    guest.r[15] = frame[6] & !1;
    Ok(None)
}

fn write_reg(guest: &mut Guest, index: usize, value: u32) -> Result<(), ExecFault> {
    if index == 15 {
        if value & 1 == 0 {
            return Err(ExecFault::InvState);
        }
        guest.r[15] = value & !1;
    } else if index == 13 {
        guest.r[13] = value & !0b11;
    } else {
        guest.r[index] = value;
    }
    Ok(())
}

fn advance(guest: &mut Guest, pc: u32, size: u32) {
    if guest.r[15] == pc {
        guest.r[15] = pc.wrapping_add(size);
    }
}

fn reg3(hw: u16, shift: u32) -> usize {
    usize::from((hw >> shift) & 0x7)
}

fn high_reg(hw: u16, low_shift: u32) -> usize {
    let low = usize::from((hw >> low_shift) & 0x7);
    let high = if low_shift == 0 {
        usize::from((hw >> 7) & 1) << 3
    } else {
        usize::from((hw >> 6) & 1) << 3
    };
    low | high
}

fn write_flags(guest: &mut Guest, result: u32, carry: bool, overflow: bool) {
    guest.n = result & 0x8000_0000 != 0;
    guest.z = result == 0;
    guest.c = carry;
    guest.v = overflow;
}

fn write_flags_nz(guest: &mut Guest, result: u32) {
    guest.n = result & 0x8000_0000 != 0;
    guest.z = result == 0;
}

fn write_flags_nzc(guest: &mut Guest, result: u32, carry: Option<bool>) {
    write_flags_nz(guest, result);
    if let Some(carry) = carry {
        guest.c = carry;
    }
}

fn add_carry(a: u32, b: u32, carry: bool) -> (u32, bool, bool) {
    let (sum, c1) = a.overflowing_add(b);
    let (sum, c2) = sum.overflowing_add(u32::from(carry));
    let overflow = (a ^ sum) & (b ^ sum) & 0x8000_0000 != 0;
    (sum, c1 || c2, overflow)
}

/// `kind` 0 = LSL, 1 = LSR, 2 = ASR. A zero shift leaves C unchanged.
fn shift(kind: u16, value: u32, amount: u32) -> (u32, Option<bool>) {
    if amount == 0 {
        return (value, None);
    }
    match kind {
        0 if amount < 32 => (value << amount, Some((value >> (32 - amount)) & 1 == 1)),
        0 if amount == 32 => (0, Some(value & 1 == 1)),
        0 => (0, Some(false)),
        1 if amount < 32 => (value >> amount, Some((value >> (amount - 1)) & 1 == 1)),
        1 if amount == 32 => (0, Some(value & 0x8000_0000 != 0)),
        1 => (0, Some(false)),
        _ if amount < 32 => (
            ((value as i32) >> amount) as u32,
            Some((value >> (amount - 1)) & 1 == 1),
        ),
        _ => (
            if value & 0x8000_0000 != 0 {
                u32::MAX
            } else {
                0
            },
            Some(value & 0x8000_0000 != 0),
        ),
    }
}

fn ror(value: u32, amount: u32) -> (u32, Option<bool>) {
    if amount == 0 {
        return (value, None);
    }
    let amount = amount % 32;
    if amount == 0 {
        return (value, Some(value & 0x8000_0000 != 0));
    }
    let result = value.rotate_right(amount);
    (result, Some(result & 0x8000_0000 != 0))
}

fn sign_extend(value: u32, bits: u32) -> u32 {
    let shift = 32 - bits;
    ((value << shift) as i32 >> shift) as u32
}
