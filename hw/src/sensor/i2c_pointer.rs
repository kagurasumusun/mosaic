//! Pointer-register sensor core — the TMP102/MCP9808/ADS1115 register
//! convention shared by half the I2C sensor and monitor catalog.
//!
//! The first write byte selects the register (masked to the model's
//! pointer width), further bytes fill 16-bit registers MSB-first; reads
//! stream the byte file from the pointer with auto-increment and wrap at
//! the register-file end. The byte cursor persists across transactions,
//! so an odd-length read continues exactly where it stopped. Boards and
//! tests inject readings through [`PointerSensor::set_register`];
//! power-on defaults ride along as builder state.

use mosaic_contract::i2c::{Ack, I2cAddr, I2cError, I2cTarget};
use mosaic_contract::model::Ctx;

/// One pointer-register sensor or monitor.
pub struct PointerSensor {
    base: u8,
    reg_count: usize,
    mask: u8,
    /// Byte cursor into the register file (`0..reg_count * 2`).
    cursor: usize,
    regs: Vec<u8>,
}

impl PointerSensor {
    /// Register-file size (16-bit registers) and the pointer mask
    /// (e.g. `0x03` for a 2-bit pointer). Registers read `0x0000`
    /// unless defaults are layered on.
    pub fn new(reg_count: usize, mask: u8) -> Self {
        let reg_count = reg_count.max(1);
        Self {
            base: 0x48,
            reg_count,
            mask,
            cursor: 0,
            regs: vec![0; reg_count * 2],
        }
    }

    /// Strap address (A1..A0 pins).
    pub fn at(mut self, base: u8) -> Self {
        self.base = base;
        self
    }

    /// Layer a power-on default onto one register (builder style).
    pub fn with_register(mut self, register: u8, msb: u8, lsb: u8) -> Self {
        self.set_register(register, msb, lsb);
        self
    }

    /// Inject one 16-bit register value (the temperature reading path).
    pub fn set_register(&mut self, register: u8, msb: u8, lsb: u8) {
        let index = (register & self.mask) as usize;
        if index < self.reg_count {
            self.regs[index * 2] = msb;
            self.regs[index * 2 + 1] = lsb;
        }
    }

    /// Read one 16-bit register.
    pub fn register(&self, register: u8) -> u16 {
        let index = (register & self.mask) as usize;
        if index < self.reg_count {
            (self.regs[index * 2] as u16) << 8 | self.regs[index * 2 + 1] as u16
        } else {
            0
        }
    }

    fn advance(&mut self) {
        self.cursor = (self.cursor + 1) % (self.reg_count * 2);
    }
}

impl I2cTarget for PointerSensor {
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
        // A dummy write re-seats the cursor at the register's MSB.
        self.cursor = (pointer & self.mask) as usize % self.reg_count * 2;
        for &value in payload {
            self.regs[self.cursor] = value;
            self.advance();
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
            *byte = self.regs[self.cursor];
            self.advance();
        }
        Ok(Ack::Ack)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestCtx;

    const AT: I2cAddr = I2cAddr::Seven(0x48);

    #[test]
    fn write_selects_and_fills_registers_msb_first() {
        let mut part = PointerSensor::new(4, 0x03);
        let mut ctx = TestCtx::default();
        part.target_write(&mut ctx, &AT, &[0x01, 0xAB, 0xCD, 0x12, 0x34])
            .unwrap();
        // Registers 1 and 2 were filled in order.
        assert_eq!(part.register(1), 0xABCD);
        assert_eq!(part.register(2), 0x1234);
    }

    #[test]
    fn write_bursts_wrap_at_the_register_file_end() {
        let mut part = PointerSensor::new(2, 0x01);
        let mut ctx = TestCtx::default();
        // Start at the last register; the burst wraps into register 0's MSB.
        part.target_write(&mut ctx, &AT, &[0x01, 0x11, 0x22, 0x33]).unwrap();
        assert_eq!(part.register(1), 0x1122);
        // The byte stream continued into register 0's MSB lane (0x3300).
        assert_eq!(part.register(0), 0x3300);
    }

    #[test]
    fn reads_stream_bytes_and_wrap() {
        let mut part = PointerSensor::new(4, 0x03)
            .with_register(0, 0x12, 0x34)
            .with_register(1, 0x56, 0x78)
            .with_register(2, 0x9A, 0xBC)
            .with_register(3, 0xDE, 0xF0);
        let mut ctx = TestCtx::default();
        part.target_write(&mut ctx, &AT, &[0x00]).unwrap();
        let mut buf = [0u8; 5];
        part.target_read(&mut ctx, &AT, &mut buf).unwrap();
        assert_eq!(buf, [0x12, 0x34, 0x56, 0x78, 0x9A]);
        // The odd-length read stopped mid-register: the next byte is 0xBC.
        part.target_read(&mut ctx, &AT, &mut buf[..1]).unwrap();
        assert_eq!(buf[0], 0xBC);
    }

    #[test]
    fn only_the_strap_address_answers() {
        let mut part = PointerSensor::new(4, 0x03).at(0x49);
        let mut ctx = TestCtx::default();
        assert_eq!(
            part.target_write(&mut ctx, &AT, &[0x00, 1]),
            Err(I2cError::Nack)
        );
        assert_eq!(
            part.target_read(&mut ctx, &AT, &mut [0u8; 1]),
            Err(I2cError::Nack)
        );
    }

    #[test]
    fn mcp9808_style_manufacturer_and_device_ids() {
        let part = PointerSensor::new(9, 0x0F)
            .with_register(6, 0x00, 0x54) // manufacturer ID
            .with_register(7, 0x04, 0x00); // device ID
        assert_eq!(part.register(6), 0x0054);
        assert_eq!(part.register(7), 0x0400);
    }
}
