//! 24Cxx I2C serial EEPROM functional core — QEMU `hw/eeprom/at24c.c` role.
//!
//! One shared core behind every 24Cxx/M24C/CAT24C/BR24G (SRAM) and
//! MB85RC/FM24C (FRAM) part: a word address followed by auto-incrementing
//! data. Per-part modules only supply the geometry (capacity, page); the
//! strap address (A2..A0 pins) is board wiring and defaults to `0x50`.
//!
//! * a write burst rolls over **inside the page** of its first byte's
//!   address (AT24Cxx page write, so a burst never crosses the page
//!   boundary); FRAM parts set the page to the full capacity, which
//!   removes the rollover exactly as FRAM behaves;
//! * reads roll over at the device end and continue from address 0;
//! * the address counter persists across transactions (current-address
//!   read), exactly like the datasheet's sequential read;
//! * the erased state is `0xFF`.

use mosaic_contract::i2c::{Ack, I2cAddr, I2cError, I2cTarget};
use mosaic_contract::model::Ctx;

/// One 24Cxx-family EEPROM.
pub struct I2cEeprom {
    base: u8,
    size: usize,
    page: usize,
    addr: usize,
    data: Vec<u8>,
}

impl I2cEeprom {
    /// Capacity and page-write size in bytes. A page equal to the capacity
    /// disables the page rollover (FRAM behavior).
    pub fn new(size: usize, page: usize) -> Self {
        let size = size.max(1);
        let page = page.clamp(1, size);
        Self {
            base: 0x50,
            size,
            page,
            addr: 0,
            data: vec![0xFF; size],
        }
    }

    /// Strap address (A2..A0 pins), `0x50..=0x57`.
    pub fn at(mut self, base: u8) -> Self {
        self.base = base;
        self
    }

    /// Load an image (a programmed EEPROM contents blob).
    pub fn load(&mut self, image: &[u8]) {
        let n = image.len().min(self.size);
        self.data[..n].copy_from_slice(&image[..n]);
    }

    /// Raw contents (save path for boards and tests).
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// The persistent word address (current-address read pointer).
    pub fn pointer(&self) -> usize {
        self.addr
    }
}

impl I2cTarget for I2cEeprom {
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
        let Some((&word, payload)) = bytes.split_first() else {
            // An empty frame touches nothing.
            return Ok(Ack::Ack);
        };
        // The first byte is the word address (dummy write sets the pointer).
        self.addr = word as usize % self.size;
        // The burst rolls over inside the first byte's page.
        let page = self.page;
        let page_base = self.addr - (self.addr % page);
        for &value in payload {
            self.data[self.addr] = value;
            self.addr = page_base + (self.addr + 1 - page_base) % page;
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
            *byte = self.data[self.addr];
            // Reads roll over at the device end, not at the page.
            self.addr = (self.addr + 1) % self.size;
        }
        Ok(Ack::Ack)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestCtx;

    const AT: I2cAddr = I2cAddr::Seven(0x50);

    #[test]
    fn dummy_write_sets_the_pointer_and_sequential_read_wraps() {
        let mut part = I2cEeprom::new(256, 8);
        let mut ctx = TestCtx::default();
        // Program 0..=9 at 0xF8 (crosses the 8-byte page: only 8 bytes stick).
        part.target_write(&mut ctx, &AT, &[0xF8, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9])
            .unwrap();
        // Page write rolled over: the ninth and tenth bytes re-wrote 0xF8/0xF9.
        assert_eq!(&part.data()[0xF8..0x100], &[8, 9, 2, 3, 4, 5, 6, 7]);
        // Sequential read from 0xF8 wraps at the device end.
        let mut buf = [0u8; 4];
        part.target_write(&mut ctx, &AT, &[0xFE]).unwrap();
        part.target_read(&mut ctx, &AT, &mut buf).unwrap();
        assert_eq!(buf, [6, 7, 0xFF, 0xFF]);
    }

    #[test]
    fn current_address_read_continues_where_the_last_one_stopped() {
        let mut part = I2cEeprom::new(16, 16);
        let mut ctx = TestCtx::default();
        let mut buf = [0u8; 2];
        part.target_read(&mut ctx, &AT, &mut buf).unwrap();
        assert_eq!(buf, [0xFF, 0xFF]);
        part.target_read(&mut ctx, &AT, &mut buf).unwrap();
        assert_eq!(part.pointer(), 4);
    }

    #[test]
    fn fram_page_equals_capacity_so_bursts_cross_pages() {
        let mut part = I2cEeprom::new(32, 32); // FRAM geometry
        let mut ctx = TestCtx::default();
        part.target_write(&mut ctx, &AT, &[0x0E, 1, 2, 3, 4, 5])
            .unwrap();
        assert_eq!(&part.data()[0x0E..0x13], &[1, 2, 3, 4, 5]);
    }

    #[test]
    fn only_the_strap_address_answers() {
        let mut part = I2cEeprom::new(256, 8).at(0x51);
        let mut ctx = TestCtx::default();
        assert!(!part.matches(&AT));
        assert_eq!(
            part.target_write(&mut ctx, &AT, &[0x00, 1]),
            Err(I2cError::Nack)
        );
        assert_eq!(
            part.target_read(&mut ctx, &AT, &mut [0u8; 1]),
            Err(I2cError::Nack)
        );
        assert!(part.matches(&I2cAddr::Seven(0x51)));
    }

    #[test]
    fn load_and_pointer_persistence() {
        let mut part = I2cEeprom::new(128, 8);
        part.load(&[0xAA; 4]);
        assert_eq!(&part.data()[..4], &[0xAA; 4]);
        let mut ctx = TestCtx::default();
        part.target_write(&mut ctx, &AT, &[0x7F, 0x55]).unwrap();
        // The single byte stopped at the page boundary: the counter wrapped to
        // the page base 0x78.
        assert_eq!(part.pointer(), 0x78);
        assert_eq!(part.data()[0x7F], 0x55);
    }
}
