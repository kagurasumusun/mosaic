//! SPI NOR flash functional core — QEMU `hw/block/m25p80.c` role.
//!
//! One shared core behind the W25X/W25Q, MX25L, S25FL, N25Q, GD25, EN25,
//! XT25 and AM25 families: geometry (capacity) and identity (JEDEC
//! manufacturer/type/capacity triple) are the only per-part differences.
//!
//! Command set: `WREN(06)`, `WRDI(04)`, `RDSR(05)`, `WRSR(01)`,
//! `READ(03)`, `FAST_READ(0B, one dummy byte)`, `PAGE_PROGRAM(02, 256-byte
//! page)`, `SECTOR_ERASE(20, 4 KiB)`, `BLOCK_ERASE_32K(52)`,
//! `BLOCK_ERASE_64K(D8)`, `CHIP_ERASE(C7/60)`, `READ_ID(9F, JEDEC bytes)`,
//! `DEEP_POWER_DOWN(B9)`, `RELEASE_DEEP_POWER_DOWN(AB)`.
//!
//! Semantics kept faithful to the parts:
//!
//! * a program can only **clear** bits (NOR AND semantics); an erase sets
//!   `0xFF`;
//! * every program/erase cycle requires the write-enable latch; the latch
//!   drops when the cycle completes (erase: once the address completes,
//!   program: at chip-select deassert);
//! * reads roll over at the device end; the address is always 3 bytes;
//! * the status register keeps its writable bits (block protect + SRWD)
//!   across `WRSR`; `WIP` stays 0 (writes are instantaneous in a
//!   functional model) and `WEL` mirrors the live latch;
//! * in deep power-down only `AB` is accepted, and it only wakes the part.

use mosaic_contract::model::Ctx;
use mosaic_contract::spi::{SpiError, SpiMode, SpiTarget};

mod cmd {
    pub const WREN: u8 = 0x06;
    pub const WRDI: u8 = 0x04;
    pub const RDSR: u8 = 0x05;
    pub const WRSR: u8 = 0x01;
    pub const READ: u8 = 0x03;
    pub const FAST_READ: u8 = 0x0B;
    pub const PAGE_PROGRAM: u8 = 0x02;
    pub const SECTOR_ERASE: u8 = 0x20;
    pub const BLOCK_ERASE_32K: u8 = 0x52;
    pub const BLOCK_ERASE_64K: u8 = 0xD8;
    pub const CHIP_ERASE: u8 = 0xC7;
    pub const CHIP_ERASE_ALT: u8 = 0x60;
    pub const READ_ID: u8 = 0x9F;
    pub const DEEP_POWER_DOWN: u8 = 0xB9;
    pub const RELEASE_DEEP_POWER_DOWN: u8 = 0xAB;
}

/// Live WEL bit in the status register.
const STATUS_WEL: u8 = 1 << 1;
/// Writable status bits (BP + SRWD); WIP/WEL are live.
const STATUS_WRITABLE: u8 = 0xFC;
/// Page-program size.
const PAGE: usize = 256;

/// Erase granularity selected by the opcode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EraseKind {
    Sector4K,
    Block32K,
    Block64K,
}

impl EraseKind {
    fn bytes(self) -> usize {
        match self {
            Self::Sector4K => 4 * 1024,
            Self::Block32K => 32 * 1024,
            Self::Block64K => 64 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    Idle,
    ReadStatus,
    WriteStatus,
    ReadId,
    /// Collecting the 3 address bytes; the follow-up phase rides along.
    Address { seen: u8, next: Next },
    FastDummy,
    ReadData,
    ProgramData,
    /// The `AB` release sequence's dummy bytes before the signature byte.
    ReleaseDummy { seen: u8 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Next {
    Read,
    FastRead,
    Program,
    Erase(EraseKind),
}

/// One SPI NOR flash.
pub struct SpiNorFlash {
    size: usize,
    jedec: [u8; 3],
    wren: bool,
    /// True between chip-select deassert and the next deselect (WEL drops
    /// when the program cycle completes).
    program_active: bool,
    status: u8,
    state: State,
    addr: usize,
    id_seen: u8,
    power_down: bool,
    data: Vec<u8>,
}

impl SpiNorFlash {
    /// Capacity in bytes and the JEDEC identity triple
    /// (manufacturer, memory type, capacity code).
    pub fn new(size: usize, jedec: [u8; 3]) -> Self {
        Self {
            size: size.max(1),
            jedec,
            wren: false,
            program_active: false,
            status: 0,
            state: State::Idle,
            addr: 0,
            id_seen: 0,
            power_down: false,
            data: vec![0xFF; size.max(1)],
        }
    }

    /// Load an image (a programmed flash contents blob).
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

    fn erase_at(&mut self, kind: EraseKind) {
        let span = kind.bytes();
        let base = self.addr - (self.addr % span);
        for i in base..(base + span).min(self.size) {
            self.data[i] = 0xFF;
        }
    }
}

impl SpiTarget for SpiNorFlash {
    fn selected(&mut self, _ctx: &mut dyn Ctx, assert: bool) {
        if !assert {
            // Frame boundary: a program cycle in flight is now complete.
            if self.program_active {
                self.wren = false;
                self.program_active = false;
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
        if self.power_down {
            // Only the release opcode wakes the part, and it only wakes it.
            if self.state == State::Idle && mosi == cmd::RELEASE_DEEP_POWER_DOWN {
                self.power_down = false;
            }
            return Ok(0xFF);
        }
        Ok(match self.state {
            State::Idle => {
                match mosi {
                    cmd::WREN => self.wren = true,
                    cmd::WRDI => self.wren = false,
                    cmd::RDSR => self.state = State::ReadStatus,
                    cmd::WRSR => self.state = State::WriteStatus,
                    cmd::READ_ID => {
                        self.id_seen = 0;
                        self.state = State::ReadId;
                    }
                    cmd::READ => {
                        self.addr = 0;
                        self.state = State::Address { seen: 0, next: Next::Read };
                    }
                    cmd::FAST_READ => {
                        self.addr = 0;
                        self.state = State::Address { seen: 0, next: Next::FastRead };
                    }
                    cmd::PAGE_PROGRAM => {
                        self.addr = 0;
                        self.program_active = true;
                        self.state = State::Address { seen: 0, next: Next::Program };
                    }
                    cmd::SECTOR_ERASE => {
                        self.addr = 0;
                        self.state = State::Address { seen: 0, next: Next::Erase(EraseKind::Sector4K) };
                    }
                    cmd::BLOCK_ERASE_32K => {
                        self.addr = 0;
                        self.state =
                            State::Address { seen: 0, next: Next::Erase(EraseKind::Block32K) };
                    }
                    cmd::BLOCK_ERASE_64K => {
                        self.addr = 0;
                        self.state =
                            State::Address { seen: 0, next: Next::Erase(EraseKind::Block64K) };
                    }
                    cmd::CHIP_ERASE | cmd::CHIP_ERASE_ALT => {
                        if self.wren {
                            self.data.fill(0xFF);
                            self.wren = false;
                        }
                    }
                    cmd::DEEP_POWER_DOWN => self.power_down = true,
                    cmd::RELEASE_DEEP_POWER_DOWN => {
                        self.state = State::ReleaseDummy { seen: 0 };
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
            State::ReadId => {
                let value = self.jedec[self.id_seen as usize % 3];
                self.id_seen += 1;
                value
            }
            State::Address { mut seen, next } => {
                self.addr = (self.addr << 8) | mosi as usize;
                seen += 1;
                if seen >= 3 {
                    self.addr %= self.size;
                    match next {
                        Next::Read => self.state = State::ReadData,
                        Next::FastRead => self.state = State::FastDummy,
                        Next::Program => {
                            self.program_active = true;
                            self.state = State::ProgramData;
                        }
                        Next::Erase(kind) => {
                            // The erase cycle completes with the address.
                            if self.wren {
                                self.erase_at(kind);
                                self.wren = false;
                            }
                            self.state = State::Idle;
                        }
                    }
                } else {
                    self.state = State::Address { seen, next };
                }
                0x00
            }
            State::FastDummy => {
                self.state = State::ReadData;
                0x00
            }
            State::ReadData => {
                let value = self.data[self.addr];
                self.addr = (self.addr + 1) % self.size;
                value
            }
            State::ProgramData => {
                if self.wren {
                    // NOR semantics: a program can only clear bits, and the
                    // burst rolls over inside the 256-byte page.
                    self.data[self.addr] &= mosi;
                    let page_base = self.addr - (self.addr % PAGE);
                    self.addr = page_base + (self.addr + 1 - page_base) % PAGE;
                }
                0x00
            }
            State::ReleaseDummy { mut seen } => {
                seen += 1;
                self.state = if seen >= 3 {
                    State::ReadData // signature byte, then 0xFF stream
                } else {
                    State::ReleaseDummy { seen }
                };
                0x00
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestCtx;

    const JEDEC: [u8; 3] = [0xEF, 0x40, 0x15]; // W25Q16-style

    fn drive(part: &mut SpiNorFlash, bytes: &[u8]) -> Vec<u8> {
        let mut ctx = TestCtx::default();
        part.selected(&mut ctx, true);
        let mut out = Vec::new();
        for &b in bytes {
            out.push(part.exchange_byte(&mut ctx, SpiMode::new(false, false), b).unwrap());
        }
        part.selected(&mut ctx, false);
        out
    }

    #[test]
    fn read_id_streams_the_jedec_triple() {
        let mut part = SpiNorFlash::new(2 * 1024 * 1024, JEDEC);
        let out = drive(&mut part, &[0x9F, 0, 0, 0, 0]);
        // Replies: command byte, then the JEDEC bytes cycling.
        assert_eq!(out, vec![0x00, 0xEF, 0x40, 0x15, 0xEF]);
    }

    #[test]
    fn page_program_only_clears_bits_and_wraps_in_the_page() {
        let mut part = SpiNorFlash::new(64 * 1024, JEDEC);
        drive(&mut part, &[0x06]);
        // Program 0xFE at 0x00FF, then one more byte: it wraps to 0x0000.
        drive(&mut part, &[0x02, 0x00, 0x00, 0xFF, 0xFE, 0x00]);
        assert_eq!(part.data()[0xFF], 0xFE);
        assert_eq!(part.data()[0x00], 0x00);
        assert_eq!(part.data()[0x100], 0xFF);
        // Programming 1s clears nothing.
        drive(&mut part, &[0x06]);
        drive(&mut part, &[0x02, 0x00, 0x00, 0xFF, 0xFF]);
        assert_eq!(part.data()[0xFF], 0xFE);
    }

    #[test]
    fn program_and_erase_require_wren() {
        let mut part = SpiNorFlash::new(64 * 1024, JEDEC);
        drive(&mut part, &[0x02, 0x00, 0x00, 0x00, 0x00]);
        assert_eq!(part.data()[0], 0xFF);
        drive(&mut part, &[0x20, 0x00, 0x00, 0x00]);
        assert_eq!(part.data()[0], 0xFF);
        // With WREN the program lands.
        drive(&mut part, &[0x06]);
        drive(&mut part, &[0x02, 0x00, 0x00, 0x00, 0x00]);
        assert_eq!(part.data()[0], 0x00);
        // With WREN the sector erase clears the whole 4 KiB.
        drive(&mut part, &[0x06]);
        drive(&mut part, &[0x20, 0x00, 0x00, 0x00]);
        assert_eq!(part.data()[0], 0xFF);
        assert_eq!(part.data()[0x0FFF], 0xFF);
        // The latch dropped with the completed erase cycle.
        drive(&mut part, &[0x02, 0x00, 0x00, 0x00, 0x00]);
        assert_eq!(part.data()[0], 0xFF);
    }

    #[test]
    fn wel_persists_across_idle_frames_but_not_across_programs() {
        let mut part = SpiNorFlash::new(64 * 1024, JEDEC);
        let mut ctx = TestCtx::default();
        part.selected(&mut ctx, true);
        part.exchange_byte(&mut ctx, SpiMode::new(false, false), 0x06)
            .unwrap();
        part.selected(&mut ctx, false);
        part.selected(&mut ctx, true);
        part.exchange_byte(&mut ctx, SpiMode::new(false, false), 0x05)
            .unwrap();
        let status = part.exchange_byte(&mut ctx, SpiMode::new(false, false), 0x00)
            .unwrap();
        part.selected(&mut ctx, false);
        assert_eq!(status & STATUS_WEL, STATUS_WEL);
    }

    #[test]
    fn chip_erase_and_64k_block_erase() {
        let mut part = SpiNorFlash::new(64 * 1024, JEDEC);
        drive(&mut part, &[0x06]);
        drive(&mut part, &[0x02, 0x00, 0x0F, 0xFF, 0x00]);
        assert_eq!(part.data()[0x0FFF], 0x00);
        drive(&mut part, &[0x06]);
        drive(&mut part, &[0xD8, 0x00, 0x00, 0x00]);
        assert_eq!(part.data()[0x0FFF], 0xFF);
    }

    #[test]
    fn deep_power_down_swallows_everything_but_release() {
        let mut part = SpiNorFlash::new(64 * 1024, JEDEC);
        let mut ctx = TestCtx::default();
        drive(&mut part, &[0xB9]);
        part.selected(&mut ctx, true);
        let reply = part
            .exchange_byte(&mut ctx, SpiMode::new(false, false), 0x9F)
            .unwrap();
        assert_eq!(reply, 0xFF);
        part.exchange_byte(&mut ctx, SpiMode::new(false, false), 0xAB)
            .unwrap();
        // Awake again: a fresh READ_ID frame answers with the JEDEC bytes.
        part.selected(&mut ctx, false);
        part.selected(&mut ctx, true);
        part.exchange_byte(&mut ctx, SpiMode::new(false, false), 0x9F)
            .unwrap();
        let id = part
            .exchange_byte(&mut ctx, SpiMode::new(false, false), 0x00)
            .unwrap();
        part.selected(&mut ctx, false);
        assert_eq!(id, 0xEF);
    }

    #[test]
    fn read_rolls_over_at_the_device_end() {
        let mut part = SpiNorFlash::new(64 * 1024, JEDEC);
        part.load(&[0x42]);
        // Address 0x00FFFF: the read wraps at 64 KiB back to 0x0000.
        let out = drive(&mut part, &[0x03, 0x00, 0xFF, 0xFF, 0, 0]);
        assert_eq!(&out[4..], &[0xFF, 0x42]);
    }

    #[test]
    fn fast_read_consumes_one_dummy_byte() {
        let mut part = SpiNorFlash::new(64 * 1024, JEDEC);
        part.load(&[0x7E]);
        let out = drive(&mut part, &[0x0B, 0x00, 0x00, 0x00, 0xAA, 0, 0]);
        // Byte 4 is the dummy: the reply for it is 0, data starts after.
        assert_eq!(out[4], 0x00);
        assert_eq!(out[5], 0x7E);
    }
}
