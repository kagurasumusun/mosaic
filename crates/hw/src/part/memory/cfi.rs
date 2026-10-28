//! CFI NOR flash (QEMU's `hw/block/pflash_cfi02.c` role, AMD command set).
//!
//! Clean-room from the AMD/Fujitsu CFI specification (command set 0x0002)
//! and the JEDEC CFI query: the autoselect/program/erase command sequences
//! written in 16-bit or 8-bit bus-width steps, the CFI query at `0x55`/`0xAA`
//! unlock cycles answering `QRY` and the device geometry, sector erase with
//! per-sector dirty tracking, and the read-array state every reset sequence
//! (`0xF0`) returns to. The backing bytes are the flash's contents: a board
//! maps the model over a ROM image or a writable image, and dirty sectors
//! report which erase blocks changed.

use crate::modelkit::unmapped_policy::UnmappedPolicy;
use mosaic_contract::mmio::MmioTarget;
use mosaic_contract::model::{ContextError, Ctx, EventTarget};
use mosaic_exec::{AccessSize, Attr, BusFault, Fnv64, Snapshot, SnapshotError};

/// Device states the command sequence decoder walks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    ReadArray,
    Unlock1,
    Autoselect1,
    Autoselect2,
    Program1,
    Erase1,
    Erase2,
    Erase3,
    CfiQuery,
}

/// The AMD/Fujitsu CFI flash model.
#[derive(Debug)]
pub struct CfiFlash {
    /// The flash's backing bytes.
    memory: Vec<u8>,
    /// Uniform sector size (a power of two).
    sector_size: usize,
    /// CFI answer offsets' values are computed; the manufacturer/device IDs
    /// the autoselect sequence reports.
    manufacturer_id: u8,
    device_id: u8,
    /// Command decoder state.
    state: State,
    /// Sectors written since the last erase (dirty tracking).
    dirty: std::collections::BTreeSet<usize>,
    unmapped: UnmappedPolicy,
}

impl CfiFlash {
    /// A flash of `memory.len()` bytes with uniform `sector_size` sectors.
    pub fn new(memory: Vec<u8>, sector_size: usize, manufacturer_id: u8, device_id: u8) -> Self {
        let sector_size = sector_size.max(1).next_power_of_two();
        Self {
            memory,
            sector_size,
            manufacturer_id,
            device_id,
            state: State::ReadArray,
            dirty: std::collections::BTreeSet::new(),
            unmapped: UnmappedPolicy::RazWi,
        }
    }

    pub fn len(&self) -> usize {
        self.memory.len()
    }

    pub fn is_empty(&self) -> bool {
        self.memory.is_empty()
    }

    pub fn memory(&self) -> &[u8] {
        &self.memory
    }

    /// Sectors written since load, in order (an erase clears the set).
    pub fn dirty_sectors(&self) -> Vec<usize> {
        self.dirty.iter().copied().collect()
    }

    fn sector_of(&self, offset: usize) -> usize {
        offset / self.sector_size
    }

    /// The CFI query byte at the offset the specification defines (the
    /// `QRY` string at 0x10..0x13, geometry from 0x27).
    fn cfi_byte(&self, offset: usize) -> u8 {
        match offset {
            0x10 => b'Q',
            0x11 => b'R',
            0x12 => b'Y',
            // Primary command set: 0x0002 (AMD/Fujitsu).
            0x13 => 0x02,
            0x14 => 0x00,
            // System interface: 3.3V, 8-bit only.
            0x27 => 0x17,
            // Max multi-byte program: 1 byte.
            0x2A => 0x00,
            // Erase region count: 1 (uniform).
            0x2C => 0x01,
            0x2D => 0x00,
            // Region 1: sector count - 1.
            0x2E => ((self.memory.len() / self.sector_size).saturating_sub(1) & 0xFF) as u8,
            0x2F => (((self.memory.len() / self.sector_size).saturating_sub(1)) >> 8) as u8,
            // Region 1: log2(sector_size) - 8 in units of 256 bytes.
            0x30 => (self.sector_size.ilog2() - 8) as u8,
            0x31 => 0x00,
            _ => 0,
        }
    }

    fn command(&mut self, offset: usize, value: u8) {
        match (self.state, value) {
            (_, 0xF0) | (_, 0xFF) => {
                // Reset / read array.
                self.state = State::ReadArray;
            }
            (State::ReadArray, 0xAA) if offset & 0xFFF == 0x555 => {
                self.state = State::Unlock1;
            }
            (State::Unlock1, 0x55) if offset & 0xFFF == 0x2AA => {
                self.state = State::Autoselect1;
            }
            (State::Autoselect1, 0x80) => {
                self.state = State::Erase1;
            }
            (State::Autoselect1, 0x90) => {
                self.state = State::Autoselect2;
            }
            (State::Autoselect1, 0xA0) => {
                self.state = State::Program1;
            }
            (State::Autoselect1, 0x98) => {
                self.state = State::CfiQuery;
            }
            (State::Erase1, 0xAA) => {
                self.state = State::Erase2;
            }
            (State::Erase2, 0x55) => {
                self.state = State::Erase3;
            }
            (State::Erase3, 0x30) => {
                // Sector erase: the offset selects the sector.
                let sector = self.sector_of(offset);
                let start = sector * self.sector_size;
                let end = (start + self.sector_size).min(self.memory.len());
                for byte in &mut self.memory[start..end] {
                    *byte = 0xFF;
                }
                self.dirty.remove(&sector);
                self.state = State::ReadArray;
            }
            _ => {
                self.state = State::ReadArray;
            }
        }
    }
}

impl MmioTarget for CfiFlash {
    fn read(
        &mut self,
        _ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        let offset = offset as usize;
        if offset >= self.memory.len() {
            return match self.unmapped {
                UnmappedPolicy::RazWi => Ok(0),
                UnmappedPolicy::Fault => Err(BusFault::Device),
            };
        }
        let value = match self.state {
            State::CfiQuery => self.cfi_byte(offset),
            State::Autoselect2 => match offset {
                0x00 => self.manufacturer_id,
                0x01 => self.device_id,
                _ => 0,
            },
            State::ReadArray
            | State::Program1
            | State::Unlock1
            | State::Autoselect1
            | State::Erase1
            | State::Erase2
            | State::Erase3 => self.memory[offset],
        };
        let mut result = u64::from(value);
        if size == AccessSize::HalfWord {
            let high = u64::from(self.memory.get(offset + 1).copied().unwrap_or(0));
            result |= high << 8;
        }
        Ok(result)
    }

    fn write(
        &mut self,
        _ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        value: u64,
        _attr: &Attr,
    ) -> Result<(), BusFault> {
        let offset = offset as usize;
        let byte = value as u8;
        if self.state == State::Program1 && offset < self.memory.len() {
            // Program: bytes can only clear bits.
            self.memory[offset] &= byte;
            self.dirty.insert(self.sector_of(offset));
            self.state = State::ReadArray;
            return Ok(());
        }
        self.command(offset, byte);
        if size == AccessSize::HalfWord {
            let high = (value >> 8) as u8;
            self.command(offset + 1, high);
        }
        Ok(())
    }
}

impl EventTarget for CfiFlash {
    fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
        Ok(())
    }
}

/// Checkpoint state (ADR 0004): the memory and the decoder state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CfiFlashCheckpoint {
    pub schema: u32,
    pub memory: Vec<u8>,
    pub dirty: Vec<usize>,
}

impl Snapshot for CfiFlash {
    type State = CfiFlashCheckpoint;
    const SCHEMA_VERSION: u32 = 1;

    fn save(&self) -> Self::State {
        Self::State {
            schema: Self::SCHEMA_VERSION,
            memory: self.memory.clone(),
            dirty: self.dirty.iter().copied().collect(),
        }
    }

    fn validate(&self, version: u32, _state: &Self::State) -> Result<(), SnapshotError> {
        if version == Self::SCHEMA_VERSION {
            Ok(())
        } else {
            Err(SnapshotError::VersionMismatch)
        }
    }

    fn load(&mut self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        self.validate(version, state)?;
        self.memory = state.memory.clone();
        self.dirty = state.dirty.iter().copied().collect();
        self.state = State::ReadArray;
        Ok(())
    }

    fn fingerprint(version: u32, state: &Self::State) -> u64 {
        let mut hash = Fnv64::default();
        hash.bytes(&version.to_le_bytes());
        hash.bytes(&state.memory);
        hash.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::mmio::MmioTarget;
    use mosaic_exec::{AccessKind, Endian, NodeId, Security};

    fn attr() -> Attr {
        Attr {
            master: NodeId(0),
            security: Security::Secure,
            privileged: true,
            kind: AccessKind::Data,
            exclusive: false,
            byte_enable: 0,
            endian: Endian::Little,
        }
    }

    struct NoCtx;

    impl Ctx for NoCtx {
        fn now(&self) -> mosaic_exec::VTime {
            mosaic_exec::VTime::ZERO
        }

        fn schedule(
            &mut self,
            _at: mosaic_exec::VTime,
            _token: u64,
        ) -> Result<mosaic_exec::EventId, ContextError> {
            Err(ContextError::ResourceExhausted)
        }

        fn cancel(&mut self, _event: mosaic_exec::EventId) -> bool {
            false
        }

        fn set_signal(
            &mut self,
            _signal: mosaic_exec::SignalId,
            _high: bool,
        ) -> Result<(), ContextError> {
            Ok(())
        }
    }

    fn flash() -> CfiFlash {
        CfiFlash::new(vec![0xFF; 64 * 1024], 4096, 0x01, 0x49)
    }

    fn write(flash: &mut CfiFlash, offset: u64, value: u8) {
        flash
            .write(
                &mut NoCtx,
                offset,
                AccessSize::Byte,
                u64::from(value),
                &attr(),
            )
            .unwrap();
    }

    fn read(flash: &mut CfiFlash, offset: u64) -> u8 {
        flash
            .read(&mut NoCtx, offset, AccessSize::Byte, &attr())
            .unwrap() as u8
    }

    #[test]
    fn programming_clears_bits_and_tracks_the_sector() {
        let mut flash = flash();
        // Autoselect -> program: 0x555 AA, 0x2AA 55, 0x555 A0, then data.
        write(&mut flash, 0x555, 0xAA);
        write(&mut flash, 0x2AA, 0x55);
        write(&mut flash, 0x555, 0xA0);
        write(&mut flash, 0x1000, 0x42);
        assert_eq!(read(&mut flash, 0x1000), 0x42);
        assert_eq!(flash.dirty_sectors(), vec![1]);
        // Programming cannot set bits.
        write(&mut flash, 0x555, 0xAA);
        write(&mut flash, 0x2AA, 0x55);
        write(&mut flash, 0x555, 0xA0);
        write(&mut flash, 0x1000, 0xFF);
        assert_eq!(read(&mut flash, 0x1000), 0x42);
    }

    #[test]
    fn the_cfi_query_answers_qry_and_geometry() {
        let mut flash = flash();
        write(&mut flash, 0x555, 0xAA);
        write(&mut flash, 0x2AA, 0x55);
        write(&mut flash, 0x555, 0x98);
        assert_eq!(read(&mut flash, 0x10), b'Q');
        assert_eq!(read(&mut flash, 0x11), b'R');
        assert_eq!(read(&mut flash, 0x12), b'Y');
        // Sector size log2: 4096 = 2^12 -> 12-8 = 4.
        assert_eq!(read(&mut flash, 0x30), 4);
        // 16 sectors - 1.
        assert_eq!(read(&mut flash, 0x2E), 15);
        // Reset returns to read array.
        write(&mut flash, 0x0, 0xF0);
        assert_eq!(read(&mut flash, 0x10), 0xFF);
    }

    #[test]
    fn autoselect_reports_manufacturer_and_device() {
        let mut flash = flash();
        write(&mut flash, 0x555, 0xAA);
        write(&mut flash, 0x2AA, 0x55);
        write(&mut flash, 0x555, 0x90);
        assert_eq!(read(&mut flash, 0x00), 0x01);
        assert_eq!(read(&mut flash, 0x01), 0x49);
        write(&mut flash, 0x0, 0xF0);
    }

    #[test]
    fn sector_erase_restores_ff_and_clears_dirty() {
        let mut flash = flash();
        write(&mut flash, 0x555, 0xAA);
        write(&mut flash, 0x2AA, 0x55);
        write(&mut flash, 0x555, 0xA0);
        write(&mut flash, 0x1000, 0x00);
        assert_eq!(flash.dirty_sectors(), vec![1]);
        // Erase: 0x555 AA, 0x2AA 55, 0x555 80, then sector offset with 0x30.
        write(&mut flash, 0x555, 0xAA);
        write(&mut flash, 0x2AA, 0x55);
        write(&mut flash, 0x555, 0x80);
        write(&mut flash, 0x555, 0xAA);
        write(&mut flash, 0x2AA, 0x55);
        write(&mut flash, 0x1000, 0x30);
        assert_eq!(read(&mut flash, 0x1000), 0xFF);
        assert!(flash.dirty_sectors().is_empty());
    }
}
