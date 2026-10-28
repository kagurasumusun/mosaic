//! 25xx SPI serial EEPROM functional core (25AA/25LC/M95/CAT25C/BR25G).
//!
//! Command set: `WREN(06)`, `WRDI(04)`, `RDSR(05)`, `WRSR(01)`,
//! `READ(03)`, `WRITE(02)`. The address width is 1 byte for parts up to
//! 256 bytes and 2 bytes above. Writes require the write-enable latch and
//! roll over inside a 16-byte page; the latch drops when the frame ends
//! (chip-select deassert), exactly like the datasheet's self-timed cycle
//! completing. The erased state is `0xFF`.

use mosaic_contract::model::Ctx;
use mosaic_contract::spi::{SpiError, SpiMode, SpiTarget};

mod cmd {
    pub const WREN: u8 = 0x06;
    pub const WRDI: u8 = 0x04;
    pub const RDSR: u8 = 0x05;
    pub const WRSR: u8 = 0x01;
    pub const READ: u8 = 0x03;
    pub const WRITE: u8 = 0x02;
}

/// Live WEL bit in the status register.
const STATUS_WEL: u8 = 1 << 1;
/// Writable status bits (block protect + WPEN); WIP/WEL are live.
const STATUS_WRITABLE: u8 = 0xFC;
/// Page-write size (25xx family).
const PAGE: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    Idle,
    /// Collecting address bytes; `data` selects the follow-up phase.
    Address {
        seen: u8,
        write: bool,
    },
    ReadData,
    WriteData,
    ReadStatus,
    WriteStatus,
}

/// One 25xx-family SPI EEPROM.
pub struct SpiEeprom {
    size: usize,
    addr_bytes: u8,
    wren: bool,
    /// True while a WRITE data phase is in flight; the WEL latch drops when
    /// that cycle completes (chip-select deassert), not when WREN's own
    /// frame ends.
    write_active: bool,
    status: u8,
    state: State,
    addr: usize,
    data: Vec<u8>,
}

impl SpiEeprom {
    /// Capacity in bytes and address width (1 or 2 bytes).
    pub fn new(size: usize, addr_bytes: u8) -> Self {
        let addr_bytes = addr_bytes.clamp(1, 2);
        Self {
            size: size.max(1),
            addr_bytes,
            wren: false,
            write_active: false,
            status: 0,
            state: State::Idle,
            addr: 0,
            data: vec![0xFF; size.max(1)],
        }
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

    /// The assembled status byte (live WEL + writable bits).
    pub fn status(&self) -> u8 {
        self.status | if self.wren { STATUS_WEL } else { 0 }
    }
}

impl SpiTarget for SpiEeprom {
    fn selected(&mut self, _ctx: &mut dyn Ctx, assert: bool) {
        if !assert {
            // Frame boundary: a write cycle in flight is now complete, the
            // WEL latch drops. WREN on its own persists until a write or WRDI.
            if self.write_active {
                self.wren = false;
                self.write_active = false;
            }
            self.state = State::Idle;
        }
    }

    fn exchange_byte(
        &mut self,
        _ctx: &mut dyn Ctx,
        _mode: SpiMode,
        mosi: u8,
    ) -> Result<u8, SpiError> {
        Ok(match self.state {
            State::Idle => {
                match mosi {
                    cmd::WREN => self.wren = true,
                    cmd::WRDI => self.wren = false,
                    cmd::RDSR => self.state = State::ReadStatus,
                    cmd::WRSR => self.state = State::WriteStatus,
                    cmd::READ => {
                        self.addr = 0;
                        self.state = State::Address {
                            seen: 0,
                            write: false,
                        };
                    }
                    cmd::WRITE => {
                        self.addr = 0;
                        self.write_active = true;
                        self.state = State::Address {
                            seen: 0,
                            write: true,
                        };
                    }
                    _ => {} // undefined opcode: ignored
                }
                0x00
            }
            State::ReadStatus => self.status(),
            State::WriteStatus => {
                self.status = mosi & STATUS_WRITABLE;
                self.state = State::Idle;
                0x00
            }
            State::Address { mut seen, write } => {
                self.addr = (self.addr << 8) | mosi as usize;
                seen += 1;
                if seen >= self.addr_bytes {
                    self.addr %= self.size;
                    self.state = if write {
                        State::WriteData
                    } else {
                        State::ReadData
                    };
                } else {
                    self.state = State::Address { seen, write };
                }
                0x00
            }
            State::ReadData => {
                let value = self.data[self.addr];
                self.addr = (self.addr + 1) % self.size;
                value
            }
            State::WriteData => {
                if self.wren {
                    self.data[self.addr] = mosi;
                    // Roll over inside the 16-byte page of the first byte.
                    let page_base = self.addr - (self.addr % PAGE);
                    self.addr = page_base + (self.addr + 1 - page_base) % PAGE;
                }
                0x00
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestCtx;

    fn drive(part: &mut SpiEeprom, bytes: &[u8]) -> Vec<u8> {
        let mut ctx = TestCtx::default();
        part.selected(&mut ctx, true);
        let mut out = Vec::new();
        for &b in bytes {
            out.push(
                part.exchange_byte(&mut ctx, SpiMode::new(false, false), b)
                    .unwrap(),
            );
        }
        part.selected(&mut ctx, false);
        out
    }

    #[test]
    fn write_requires_wren_and_stays_inside_the_page() {
        let mut part = SpiEeprom::new(512, 2);
        // Without WREN the data is silently ignored.
        drive(&mut part, &[0x02, 0x00, 0x08, 0xAA, 0xBB]);
        assert_eq!(part.data()[0x08], 0xFF);
        // With WREN (its own frame) the burst wraps inside the 16-byte page.
        drive(&mut part, &[0x06]);
        drive(&mut part, &[0x02, 0x00, 0x0E, 1, 2, 3, 4, 5]);
        assert_eq!(&part.data()[0x0E..0x10], &[1, 2]);
        assert_eq!(&part.data()[0x00..0x02], &[3, 4]);
        // The latch dropped when the write cycle completed.
        drive(&mut part, &[0x02, 0x00, 0x20, 0x99]);
        assert_eq!(part.data()[0x20], 0xFF);
    }

    #[test]
    fn read_streams_and_wraps_at_the_device_end() {
        let mut part = SpiEeprom::new(256, 1);
        part.load(&[0xA5; 4]);
        let out = drive(&mut part, &[0x03, 0xFE, 0, 0, 0]);
        // Replies: command, address byte, then data from 0xFE with wrap.
        assert_eq!(out, vec![0x00, 0x00, 0xFF, 0xFF, 0xA5]);
    }

    #[test]
    fn status_reports_the_live_latch_and_stores_protect_bits() {
        let mut part = SpiEeprom::new(128, 1);
        let mut ctx = TestCtx::default();
        // RDSR frame: WEL is clear.
        part.selected(&mut ctx, true);
        part.exchange_byte(&mut ctx, SpiMode::new(false, false), 0x05)
            .unwrap();
        let status = part
            .exchange_byte(&mut ctx, SpiMode::new(false, false), 0x00)
            .unwrap();
        part.selected(&mut ctx, false);
        assert_eq!(status & STATUS_WEL, 0);
        // WREN frame: the latch survives the frame boundary.
        drive(&mut part, &[0x06]);
        // RDSR frame again: WEL is live.
        part.selected(&mut ctx, true);
        part.exchange_byte(&mut ctx, SpiMode::new(false, false), 0x05)
            .unwrap();
        let status = part
            .exchange_byte(&mut ctx, SpiMode::new(false, false), 0x00)
            .unwrap();
        part.selected(&mut ctx, false);
        assert_eq!(status & STATUS_WEL, STATUS_WEL);
        // WRSR stores the protect bits; WEL is still live (the latch from the
        // WREN frame survives until a write completes, per the datasheet).
        drive(&mut part, &[0x01, 0xBC]);
        assert_eq!(part.status(), 0xBC | STATUS_WEL);
        drive(&mut part, &[0x06]);
        drive(&mut part, &[0x02, 0x00, 0x00, 0x00]); // WRITE completes
        assert_eq!(part.status() & STATUS_WEL, 0);
    }

    #[test]
    fn undefined_opcodes_are_ignored() {
        let mut part = SpiEeprom::new(64, 1);
        let out = drive(&mut part, &[0x77, 0x00, 0x01]);
        assert_eq!(out, vec![0x00, 0x00, 0x00]);
        assert_eq!(part.data()[0], 0xFF);
    }
}
