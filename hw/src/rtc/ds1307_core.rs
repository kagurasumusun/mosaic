//! DS1307-family I2C RTC core (DS1307/1308/1338/1340/1341/1342, M41T00/
//! M41T0/M41T11, BQ32000) — QEMU's `hw/timer/mc146818rtc.c` cousin with the
//! DS1307 register layout.
//!
//! A 64-byte register file: BCD clock in `00..=06`, control in `07`, and
//! 56 bytes of battery-backed RAM in `08..=3F`. The CH (clock halt) bit is
//! bit 7 of the seconds register. Boards wire [`Ds1307Core::tick_second`]
//! to a 1 Hz source; the BCD carry chain (seconds → minutes → hours →
//! day/date → month → year, with the 12/24-hour mode respected) is the
//! model's own knowledge.
//!
//! The DS1337 alarm registers and the M41T00's absent RAM are approximated
//! by the DS1307 layout (functional fidelity, declared per-manifest).

use mosaic_contract::i2c::{Ack, I2cAddr, I2cError, I2cTarget};
use mosaic_contract::model::Ctx;

/// Register offsets.
mod reg {
    pub const SECONDS: usize = 0x00;
    pub const MINUTES: usize = 0x01;
    pub const HOURS: usize = 0x02;
    pub const DAY_OF_WEEK: usize = 0x03;
    pub const DATE: usize = 0x04;
    pub const MONTH: usize = 0x05;
    pub const YEAR: usize = 0x06;
    pub const CONTROL: usize = 0x07;
    pub const RAM_BASE: usize = 0x08;
    pub const SIZE: usize = 0x40;
}

/// CH (clock halt) bit in the seconds register.
pub const CLOCK_HALT: u8 = 1 << 7;
/// 12-hour mode bit in the hours register.
pub const H12_MODE: u8 = 1 << 6;
/// PM flag in the hours register (12-hour mode only).
pub const PM_FLAG: u8 = 1 << 5;

/// One DS1307-family RTC.
pub struct Ds1307Core {
    base: u8,
    pointer: usize,
    regs: [u8; reg::SIZE],
}

impl Default for Ds1307Core {
    fn default() -> Self {
        Self::new()
    }
}

impl Ds1307Core {
    pub fn new() -> Self {
        Self {
            base: 0x68,
            pointer: 0,
            regs: [0; reg::SIZE],
        }
    }

    /// Strap address (OS pins), `0x68..=0x6F`.
    pub fn at(mut self, base: u8) -> Self {
        self.base = base;
        self
    }

    /// The raw register file.
    pub fn regs(&self) -> &[u8; reg::SIZE] {
        &self.regs
    }

    /// Program the clock (BCD encoded by the core) and clear CH.
    #[allow(clippy::too_many_arguments)]
    pub fn set_time(
        &mut self,
        seconds: u8,
        minutes: u8,
        hours24: u8,
        day_of_week: u8,
        date: u8,
        month: u8,
        year: u8,
    ) {
        self.regs[reg::SECONDS] = to_bcd(seconds.min(59));
        self.regs[reg::MINUTES] = to_bcd(minutes.min(59));
        self.regs[reg::HOURS] = to_bcd(hours24.min(23)); // 24-hour mode
        self.regs[reg::DAY_OF_WEEK] = day_of_week.clamp(1, 7);
        self.regs[reg::DATE] = to_bcd(date.clamp(1, 31));
        self.regs[reg::MONTH] = to_bcd(month.clamp(1, 12));
        self.regs[reg::YEAR] = to_bcd(year);
    }

    /// Whether the clock oscillator runs (CH clear).
    pub fn running(&self) -> bool {
        self.regs[reg::SECONDS] & CLOCK_HALT == 0
    }

    /// Advance the BCD clock by one second (carry chain, 12/24-hour mode).
    pub fn tick_second(&mut self) {
        if !self.running() {
            return;
        }
        let (seconds, carry) = bcd_step(self.regs[reg::SECONDS], 0x59);
        self.regs[reg::SECONDS] = seconds | (self.regs[reg::SECONDS] & CLOCK_HALT);
        if !carry {
            return;
        }
        let (minutes, carry) = bcd_step(self.regs[reg::MINUTES], 0x59);
        self.regs[reg::MINUTES] = minutes;
        if !carry {
            return;
        }
        let (hours, wrapped) = self.step_hours();
        self.regs[reg::HOURS] = hours;
        if !wrapped {
            return;
        }
        // Day of week cycles 1..=7.
        self.regs[reg::DAY_OF_WEEK] = self.regs[reg::DAY_OF_WEEK] % 7 + 1;
        // Date and month roll over at 01, not 00.
        self.regs[reg::DATE] = roll_at_one(self.regs[reg::DATE], 0x31);
        let date_wrapped = self.regs[reg::DATE] == to_bcd(1);
        if !date_wrapped {
            return;
        }
        self.regs[reg::MONTH] = roll_at_one(self.regs[reg::MONTH], 0x12);
        if self.regs[reg::MONTH] != to_bcd(1) {
            return;
        }
        let (year, _) = bcd_step(self.regs[reg::YEAR], 0x99);
        self.regs[reg::YEAR] = year;
    }

    /// Increment the hours register; returns the new value and whether the
    /// day wrapped.
    fn step_hours(&mut self) -> (u8, bool) {
        let hours = self.regs[reg::HOURS];
        if hours & H12_MODE == 0 {
            let (value, carry) = bcd_step(hours, 0x23);
            return (value, carry);
        }
        let pm = hours & PM_FLAG;
        let bcd = hours & 0x1F;
        if bcd == to_bcd(12) {
            // 12 AM -> 1 AM, 12 PM -> 1 PM: keep the flag, same day.
            (to_bcd(1) | H12_MODE | pm, false)
        } else if bcd == to_bcd(11) {
            // 11 -> 12 flips the AM/PM flag; 11:59:59 PM crosses midnight.
            (to_bcd(12) | H12_MODE | (pm ^ PM_FLAG), pm == PM_FLAG)
        } else {
            let (value, _) = bcd_step(bcd, to_bcd(10));
            (value | H12_MODE | pm, false)
        }
    }
}

/// Increment a BCD value; wrap to 0 (carry) at `max_bcd`.
fn bcd_step(value: u8, max_bcd: u8) -> (u8, bool) {
    let next = if value & 0x0F == 0x09 {
        (value & 0xF0).wrapping_add(0x10)
    } else {
        value + 1
    };
    if next > max_bcd {
        (0, true)
    } else {
        (next, false)
    }
}

/// Increment a BCD counter that rolls over at 01 (dates, months).
fn roll_at_one(value: u8, max_bcd: u8) -> u8 {
    let (next, carry) = bcd_step(value, max_bcd);
    if carry {
        to_bcd(1)
    } else {
        next
    }
}

/// Binary → BCD.
pub fn to_bcd(value: u8) -> u8 {
    ((value / 10) << 4) | (value % 10)
}

/// BCD → binary.
pub fn from_bcd(value: u8) -> u8 {
    (value >> 4) * 10 + (value & 0x0F)
}

impl I2cTarget for Ds1307Core {
    fn matches(&self, addr: &I2cAddr) -> bool {
        matches!(addr, I2cAddr::Seven(a) if *a == self.base)
    }

    fn target_write(
        &mut self,
        _ctx: &mut dyn Ctx,
        addr: &I2cAddr,
        bytes: &[u8],
    ) -> Result<Ack, I2cError> {
        if !self.matches(addr) {
            return Err(I2cError::Nack);
        }
        let Some((&pointer, payload)) = bytes.split_first() else {
            return Ok(Ack::Ack);
        };
        self.pointer = (pointer as usize) % reg::SIZE;
        for &value in payload {
            self.regs[self.pointer] = value;
            self.pointer = (self.pointer + 1) % reg::SIZE;
        }
        Ok(Ack::Ack)
    }

    fn target_read(
        &mut self,
        _ctx: &mut dyn Ctx,
        addr: &I2cAddr,
        buf: &mut [u8],
    ) -> Result<Ack, I2cError> {
        if !self.matches(addr) {
            return Err(I2cError::Nack);
        }
        for byte in buf.iter_mut() {
            *byte = self.regs[self.pointer];
            self.pointer = (self.pointer + 1) % reg::SIZE;
        }
        Ok(Ack::Ack)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestCtx;

    const AT: I2cAddr = I2cAddr::Seven(0x68);

    #[test]
    fn write_sets_the_pointer_and_ram_round_trips() {
        let mut part = Ds1307Core::new();
        let mut ctx = TestCtx::default();
        part.target_write(&mut ctx, &AT, &[0x08, 0xDE, 0xAD]).unwrap();
        let mut buf = [0u8; 2];
        part.target_write(&mut ctx, &AT, &[0x08]).unwrap();
        part.target_read(&mut ctx, &AT, &mut buf).unwrap();
        assert_eq!(buf, [0xDE, 0xAD]);
    }

    #[test]
    fn bcd_carry_chain_rolls_the_whole_clock() {
        let mut part = Ds1307Core::new();
        part.set_time(59, 59, 23, 7, 31, 12, 0x99);
        part.tick_second();
        // 23:59:59 31 Dec 99 -> 00:00:00 01 Jan 00, day of week wraps 7 -> 1.
        assert_eq!(part.regs()[reg::SECONDS], to_bcd(0));
        assert_eq!(part.regs()[reg::MINUTES], to_bcd(0));
        assert_eq!(part.regs()[reg::HOURS], to_bcd(0));
        assert_eq!(part.regs()[reg::DAY_OF_WEEK], 1);
        assert_eq!(part.regs()[reg::DATE], to_bcd(1));
        assert_eq!(part.regs()[reg::MONTH], to_bcd(1));
        assert_eq!(part.regs()[reg::YEAR], to_bcd(0));
    }

    #[test]
    fn bcd_increment_follows_the_bcd_digit_rules() {
        assert_eq!(bcd_step(0x08, 0x59), (0x09, false));
        assert_eq!(bcd_step(0x09, 0x59), (0x10, false)); // low-nibble BCD carry
        assert_eq!(bcd_step(0x59, 0x59), (0x00, true));
        assert_eq!(bcd_step(0x19, 0x59), (0x20, false));
    }

    #[test]
    fn clock_halt_freezes_the_tick() {
        let mut part = Ds1307Core::new();
        let mut ctx = TestCtx::default();
        part.target_write(&mut ctx, &AT, &[reg::SECONDS as u8, to_bcd(30) | CLOCK_HALT])
            .unwrap();
        assert!(!part.running());
        part.tick_second();
        assert_eq!(part.regs()[reg::SECONDS], to_bcd(30) | CLOCK_HALT);
    }

    #[test]
    fn twelve_hour_mode_flips_pm_at_the_noon_and_midnight_boundaries() {
        let mut part = Ds1307Core::new();
        part.regs[reg::HOURS] = to_bcd(11) | H12_MODE; // 11:59:59 AM
        part.regs[reg::MINUTES] = to_bcd(59);
        part.regs[reg::SECONDS] = to_bcd(59);
        part.tick_second();
        // 11:59:59 AM -> 12:00:00 PM (noon): flag flips, same day.
        assert_eq!(part.regs()[reg::HOURS], to_bcd(12) | H12_MODE | PM_FLAG);
        part.regs[reg::MINUTES] = to_bcd(59);
        part.regs[reg::SECONDS] = to_bcd(59);
        part.tick_second();
        // 12:59:59 PM -> 01:00:00 PM: hour rolls to 01, flag stays.
        assert_eq!(part.regs()[reg::HOURS], to_bcd(1) | H12_MODE | PM_FLAG);
        // Walk to 11:59:59 PM and cross midnight: flag clears, day wraps.
        part.regs[reg::HOURS] = to_bcd(11) | H12_MODE | PM_FLAG;
        part.regs[reg::MINUTES] = to_bcd(59);
        part.regs[reg::SECONDS] = to_bcd(59);
        part.regs[reg::DAY_OF_WEEK] = 2;
        part.tick_second();
        assert_eq!(part.regs()[reg::HOURS], to_bcd(12) | H12_MODE); // 12 AM
        assert_eq!(part.regs()[reg::DAY_OF_WEEK], 3); // day wrapped
    }

    #[test]
    fn read_wraps_at_the_register_file_end() {
        let mut part = Ds1307Core::new();
        let mut ctx = TestCtx::default();
        part.target_write(&mut ctx, &AT, &[0x3F, 0x5A]).unwrap();
        part.target_write(&mut ctx, &AT, &[0x3F]).unwrap();
        let mut buf = [0u8; 2];
        part.target_read(&mut ctx, &AT, &mut buf).unwrap();
        assert_eq!(buf, [0x5A, 0x00]);
    }
}
