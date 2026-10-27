//! PCF857x/PCA9555-family I2C GPIO expander cores.
//!
//! Two register models cover the common expanders:
//!
//! * [`Pcf857x`] — the quasi-bidirectional single-register parts
//!   (PCF8574/PCA8574, 8-bit; PCF8575/PCA8575, 16-bit): a write latches the
//!   output port, a read returns `port & inputs` (a latched 0 always reads
//!   0; a latched 1 reads the external pin, undriven pins read 1 — exactly
//!   the open-drain pulls);
//! * [`Pca9555`] — the banked parts (PCA9554/TCA6408 one bank,
//!   PCA9555/PCA9535/PCA9539/TCA6416 two banks): a pointer register
//!   selects input/output/polarity/config per bank; writes to the input
//!   register are ignored, reads stream from the pointer with wrap-around,
//!   and the input register reports `pin ^ polarity` where a configured
//!   output (config bit 0) shows the latched output.

use mosaic_contract::i2c::{Ack, I2cAddr, I2cError, I2cTarget};
use mosaic_contract::model::Ctx;

/// A quasi-bidirectional single-register expander (PCF8574/PCF8575 class).
pub struct Pcf857x {
    base: u8,
    width: usize,
    port: u16,
    inputs: u16,
}

impl Pcf857x {
    /// Byte width of the port: 1 (PCF8574) or 2 (PCF8575).
    pub fn new(width: usize) -> Self {
        Self {
            base: 0x20,
            width: width.clamp(1, 2),
            port: 0xFFFF,
            inputs: 0xFFFF,
        }
    }

    /// Strap address (A2..A0 pins).
    pub fn at(mut self, base: u8) -> Self {
        self.base = base;
        self
    }

    /// Drive the external pin state (bit 1 = externally driven high).
    pub fn set_inputs(&mut self, pins: u16) {
        self.inputs = pins & self.mask();
    }

    /// The latched output port.
    pub fn port(&self) -> u16 {
        self.port & self.mask()
    }

    fn mask(&self) -> u16 {
        if self.width == 1 {
            0x00FF
        } else {
            0xFFFF
        }
    }
}

impl I2cTarget for Pcf857x {
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
        let mask = self.mask();
        for (index, &value) in bytes.iter().enumerate() {
            if index >= self.width {
                break;
            }
            let lane = (value as u16) << (8 * index);
            let lane_mask = 0xFFu16 << (8 * index);
            self.port = (self.port & !lane_mask & mask) | (lane & mask);
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
        let port = self.port & self.inputs;
        for (index, byte) in buf.iter_mut().enumerate() {
            *byte = if index < self.width {
                (port >> (8 * index)) as u8
            } else {
                0xFF
            };
        }
        Ok(Ack::Ack)
    }
}

/// A banked expander (PCA9554/PCA9555/TCA64xx class).
pub struct Pca9555 {
    base: u8,
    banks: usize,
    pointer: usize,
    out: [u8; 2],
    pol: [u8; 2],
    cfg: [u8; 2],
    ext: [u8; 2],
}

impl Pca9555 {
    /// Bank count: 1 (PCA9554-class, 4 registers) or 2 (PCA9555-class,
    /// 8 registers).
    pub fn new(banks: usize) -> Self {
        Self {
            base: 0x20,
            banks: banks.clamp(1, 2),
            pointer: 0,
            out: [0, 0],
            pol: [0, 0],
            cfg: [0xFF, 0xFF],
            ext: [0xFF, 0xFF],
        }
    }

    /// Strap address (A2..A0 pins).
    pub fn at(mut self, base: u8) -> Self {
        self.base = base;
        self
    }

    /// Drive the external pin state (bit 1 = externally driven high).
    pub fn set_inputs(&mut self, bank: usize, pins: u8) {
        if let Some(slot) = self.ext.get_mut(bank) {
            *slot = pins;
        }
    }

    /// The latched output registers.
    pub fn outputs(&self) -> [u8; 2] {
        self.out
    }

    fn register_count(&self) -> usize {
        self.banks * 4
    }

    /// Bank/register decode: the register-type index dominates (PCA9555
    /// register map: 0/1 input, 2/3 output, 4/5 polarity, 6/7 config).
    fn read_register(&self, index: usize) -> u8 {
        let bank = index % self.banks;
        let reg = index / self.banks;
        match reg {
            0 => {
                // A configured output shows its latch; an input shows the pin.
                let pin = !self.cfg[bank] & self.out[bank] | self.cfg[bank] & self.ext[bank];
                pin ^ self.pol[bank]
            }
            1 => self.out[bank],
            2 => self.pol[bank],
            _ => self.cfg[bank],
        }
    }

    fn write_register(&mut self, index: usize, value: u8) {
        let bank = index % self.banks;
        match index / self.banks {
            0 => {} // input register: read-only
            1 => self.out[bank] = value,
            2 => self.pol[bank] = value,
            _ => self.cfg[bank] = value,
        }
    }
}

impl I2cTarget for Pca9555 {
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
        let count = self.register_count();
        self.pointer = (pointer as usize) % count;
        for &value in payload {
            self.write_register(self.pointer, value);
            self.pointer = (self.pointer + 1) % count;
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
        let count = self.register_count();
        for byte in buf.iter_mut() {
            *byte = self.read_register(self.pointer);
            self.pointer = (self.pointer + 1) % count;
        }
        Ok(Ack::Ack)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestCtx;

    #[test]
    fn pcf8574_latches_writes_and_ands_external_inputs() {
        let mut part = Pcf857x::new(1);
        let mut ctx = TestCtx::default();
        let addr = I2cAddr::Seven(0x20);
        part.target_write(&mut ctx, &addr, &[0xF0]).unwrap();
        assert_eq!(part.port(), 0xF0);
        // A latched 1 reads the external pin; a latched 0 always reads 0.
        part.set_inputs(0x3C);
        let mut buf = [0u8; 1];
        part.target_read(&mut ctx, &addr, &mut buf).unwrap();
        assert_eq!(buf[0], 0x30);
        // Undriven pins read 1.
        part.set_inputs(0xFF);
        part.target_read(&mut ctx, &addr, &mut buf).unwrap();
        assert_eq!(buf[0], 0xF0);
    }

    #[test]
    fn pcf8575_moves_sixteen_bits_in_two_bytes() {
        let mut part = Pcf857x::new(2);
        let mut ctx = TestCtx::default();
        let addr = I2cAddr::Seven(0x20);
        part.target_write(&mut ctx, &addr, &[0x34, 0x12]).unwrap();
        assert_eq!(part.port(), 0x1234);
        let mut buf = [0u8; 2];
        part.target_read(&mut ctx, &addr, &mut buf).unwrap();
        assert_eq!(buf, [0x34, 0x12]);
    }

    #[test]
    fn pca9555_pointer_wraps_and_inputs_show_outputs() {
        let mut part = Pca9555::new(2);
        let mut ctx = TestCtx::default();
        let addr = I2cAddr::Seven(0x20);
        // All pins are inputs by default and undriven: both input ports read 0xFF.
        let mut buf = [0u8; 2];
        part.target_read(&mut ctx, &addr, &mut buf).unwrap();
        assert_eq!(buf, [0xFF, 0xFF]);
        // Configure bank 0 pins 0-3 as outputs (config register 6), latch 0xA.
        part.target_write(&mut ctx, &addr, &[6, 0xF0]).unwrap();
        part.target_write(&mut ctx, &addr, &[2, 0x0A]).unwrap();
        assert_eq!(part.outputs(), [0x0A, 0x00]);
        // Input register 0: outputs show 0x0A, inputs (pins 4-7) show pins.
        part.set_inputs(0, 0xC0);
        part.target_write(&mut ctx, &addr, &[0]).unwrap();
        part.target_read(&mut ctx, &addr, &mut buf).unwrap();
        assert_eq!(buf[0], 0xCA);
    }

    #[test]
    fn pca9555_input_register_writes_are_ignored() {
        let mut part = Pca9555::new(1);
        let mut ctx = TestCtx::default();
        let addr = I2cAddr::Seven(0x20);
        part.target_write(&mut ctx, &addr, &[0, 0x00]).unwrap();
        // The write went to the input register: outputs stay 0.
        assert_eq!(part.outputs(), [0, 0]);
        // Pointer wrap: write 4 bytes from register 3, wraps through 0..3.
        // 3 = config 0x00 (all outputs), 0 = input (write ignored),
        // 1 = output 0xAA, 2 = polarity 0x00.
        part.target_write(&mut ctx, &addr, &[3, 0x00, 0x55, 0xAA, 0x00])
            .unwrap();
        // The 0x55 went to the read-only input register; the outputs latched 0xAA.
        assert_eq!(part.outputs(), [0xAA, 0]);
    }
}
