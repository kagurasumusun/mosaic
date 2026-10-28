//! Raw NAND flash core (SLC small-page/large-page devices) — the shared
//! functional core behind every `part:*/...` NAND chip.
//!
//! One family, one core (ADR 0026 step 2 pattern): the chip is a *sparse*
//! plane — only programmed pages occupy memory, erased pages read as all
//! `0xFF` — an 8-bit bus, and the ONFi standard command set: READ(0x00),
//! READ ID(0x90), RESET(0xFF), ERASE(0x60+diamond), PAGE PROGRAM(0x80+
//! diamond). The status register is the classic 0xE0 (ready, not
//! write-protected, no fail). Real silicon geometries (64K-128K blocks)
//! instantiate without allocating the full plane: a 256 MB device stays a
//! sparse map until the guest actually programs pages.

use std::collections::BTreeMap;

use mosaic_contract::model::Ctx;
use mosaic_contract::nand::{NandDevice, NandError, NandOp};

/// Standard NAND command bytes.
pub mod cmd {
    pub const READ: u8 = 0x00;
    pub const READ_ID: u8 = 0x90;
    pub const ERASE: u8 = 0x60;
    pub const PAGE_PROGRAM: u8 = 0x80;
    pub const PROGRAM_CONFIRM: u8 = 0x10;
    pub const RESET: u8 = 0xFF;
    pub const READ_STATUS: u8 = 0x70;
}

/// Status: ready, not write-protected, no program/erase failure.
pub const STATUS_READY: u8 = 0xE0;

/// One NAND page: main area + out-of-band spare area.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NandPage {
    pub data: Vec<u8>,
    pub oob: Vec<u8>,
}

impl NandPage {
    /// An erased page: all `0xFF` in both areas.
    pub fn erased(page_size: usize, oob_size: usize) -> Self {
        Self {
            data: vec![0xFF; page_size],
            oob: vec![0xFF; oob_size],
        }
    }
}

/// One raw NAND chip.
pub struct NandFlashCore {
    page_size: usize,
    oob_size: usize,
    blocks: usize,
    pages_per_block: usize,
    manufacturer_id: u8,
    device_id: u8,
    /// Sparse plane: only pages the guest has programmed since erase.
    plane: BTreeMap<usize, NandPage>,
}

impl Default for NandFlashCore {
    fn default() -> Self {
        Self::new()
    }
}

impl NandFlashCore {
    /// A 2048+64 B, 64-block SLC device (Samsung K9F1G08 class).
    pub fn new() -> Self {
        Self::with_geometry(2048, 64, 65536, 64, 0xEC, 0xF1)
    }

    /// Full geometry: page size, OOB size, block count, pages per block and
    /// the READ ID manufacturer/device bytes. The plane stays sparse, so
    /// silicon-scale block counts allocate nothing until pages are written.
    pub fn with_geometry(
        page_size: usize,
        oob_size: usize,
        blocks: usize,
        pages_per_block: usize,
        manufacturer_id: u8,
        device_id: u8,
    ) -> Self {
        Self {
            page_size: page_size.max(1),
            oob_size,
            blocks: blocks.max(1),
            pages_per_block: pages_per_block.max(1),
            manufacturer_id,
            device_id,
            plane: BTreeMap::new(),
        }
    }

    /// Total pages in the device (geometry, not allocation).
    pub fn total_pages(&self) -> usize {
        self.blocks * self.pages_per_block
    }

    /// Pages actually holding programmed data since the last erase.
    pub fn resident_pages(&self) -> usize {
        self.plane.len()
    }

    /// Row address from the address cycles (up to 5 bytes); `None` when the
    /// row is outside the device.
    fn row_of(&self, address: &[u8]) -> Option<usize> {
        if address.len() < 3 {
            return None;
        }
        let column = u16::from_le_bytes([address[0], address[1]]) as usize;
        if column > self.page_size + self.oob_size {
            return None;
        }
        let mut row = 0usize;
        for (shift, byte) in address[2..].iter().take(3).enumerate() {
            row |= (*byte as usize) << (8 * shift);
        }
        if row >= self.total_pages() {
            return None;
        }
        Some(row)
    }
}

impl NandDevice for NandFlashCore {
    fn on_command(&mut self, _ctx: &mut dyn Ctx, op: &NandOp) -> Result<Vec<u8>, NandError> {
        match op.command {
            cmd::READ_ID => Ok(vec![self.manufacturer_id, self.device_id]),
            cmd::RESET | cmd::READ_STATUS | cmd::PROGRAM_CONFIRM => Ok(vec![STATUS_READY]),
            cmd::READ => {
                let row = self.row_of(&op.address).ok_or(NandError::BadBlock)?;
                let column = op.address.first().map(|b| *b as usize).unwrap_or(0);
                let page = self
                    .plane
                    .get(&row)
                    .cloned()
                    .unwrap_or_else(|| NandPage::erased(self.page_size, self.oob_size));
                let mut out = page.data;
                out.extend_from_slice(&page.oob);
                if column <= out.len() {
                    Ok(out[column..].to_vec())
                } else {
                    Ok(Vec::new())
                }
            }
            cmd::PAGE_PROGRAM => {
                let row = self.row_of(&op.address).ok_or(NandError::BadBlock)?;
                let column = op.address.first().map(|b| *b as usize).unwrap_or(0);
                let page = self
                    .plane
                    .entry(row)
                    .or_insert_with(|| NandPage::erased(self.page_size, self.oob_size));
                // NAND can only clear bits: a program ANDs into the cells.
                for (index, byte) in op.data.iter().enumerate() {
                    let target = column + index;
                    if target < page.data.len() {
                        page.data[target] &= *byte;
                    } else if target < page.data.len() + page.oob.len() {
                        page.oob[target - page.data.len()] &= *byte;
                    }
                }
                Ok(vec![STATUS_READY])
            }
            cmd::ERASE => {
                let row = self.row_of(&op.address).ok_or(NandError::BadBlock)?;
                let block = row / self.pages_per_block;
                let base = block * self.pages_per_block;
                for row in base..base + self.pages_per_block {
                    self.plane.remove(&row);
                }
                Ok(vec![STATUS_READY])
            }
            _ => Err(NandError::BadCommand),
        }
    }

    fn ready(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoopCtx;
    impl Ctx for NoopCtx {
        fn now(&self) -> mosaic_exec::VTime {
            mosaic_exec::VTime::ZERO
        }
        fn schedule(
            &mut self,
            _: mosaic_exec::VTime,
            _: u64,
        ) -> Result<mosaic_exec::EventId, mosaic_contract::model::ContextError> {
            Err(mosaic_contract::model::ContextError::ResourceExhausted)
        }
        fn cancel(&mut self, _: mosaic_exec::EventId) -> bool {
            false
        }
        fn set_signal(
            &mut self,
            _: mosaic_exec::SignalId,
            _: bool,
        ) -> Result<(), mosaic_contract::model::ContextError> {
            Ok(())
        }
    }

    fn op(command: u8, address: &[u8], data: &[u8]) -> NandOp {
        NandOp {
            command,
            address: address.to_vec(),
            data: data.to_vec(),
        }
    }

    #[test]
    fn read_id_returns_manufacturer_and_device() {
        let mut flash = NandFlashCore::with_geometry(2048, 64, 8, 64, 0xEC, 0xF1);
        let mut ctx = NoopCtx;
        assert_eq!(
            flash
                .on_command(&mut ctx, &op(cmd::READ_ID, &[], &[]))
                .unwrap(),
            vec![0xEC, 0xF1]
        );
        assert!(flash.ready());
    }

    #[test]
    fn erased_page_reads_all_ones() {
        let mut flash = NandFlashCore::new();
        let mut ctx = NoopCtx;
        let page = flash
            .on_command(&mut ctx, &op(cmd::READ, &[0, 0, 0, 0, 0], &[]))
            .unwrap();
        assert_eq!(page.len(), 2048 + 64);
        assert!(page.iter().all(|b| *b == 0xFF));
        assert_eq!(flash.resident_pages(), 0);
    }

    #[test]
    fn program_clears_bits_and_read_returns_them() {
        let mut flash = NandFlashCore::new();
        let mut ctx = NoopCtx;
        let payload = [0xDE, 0xAD, 0xBE, 0xEF];
        flash
            .on_command(&mut ctx, &op(cmd::PAGE_PROGRAM, &[0, 0, 0, 0, 0], &payload))
            .unwrap();
        assert_eq!(flash.resident_pages(), 1);
        let page = flash
            .on_command(&mut ctx, &op(cmd::READ, &[0, 0, 0, 0, 0], &[]))
            .unwrap();
        assert_eq!(&page[..4], &[0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn erase_restores_ones() {
        let mut flash = NandFlashCore::new();
        let mut ctx = NoopCtx;
        flash
            .on_command(
                &mut ctx,
                &op(cmd::PAGE_PROGRAM, &[0, 0, 0, 0, 0], &[0x00, 0x00]),
            )
            .unwrap();
        flash
            .on_command(&mut ctx, &op(cmd::ERASE, &[0, 0, 0, 0], &[]))
            .unwrap();
        assert_eq!(flash.resident_pages(), 0);
        let page = flash
            .on_command(&mut ctx, &op(cmd::READ, &[0, 0, 0, 0, 0], &[]))
            .unwrap();
        assert_eq!(&page[..2], &[0xFF, 0xFF]);
    }

    #[test]
    fn silicon_scale_geometry_stays_sparse() {
        // A 4 Gbit device: 131072 blocks x 64 pages — must allocate nothing
        // beyond the geometry struct until pages are programmed.
        let flash = NandFlashCore::with_geometry(4096, 128, 131072, 64, 0x2C, 0xDC);
        assert_eq!(flash.total_pages(), 8_388_608);
        assert_eq!(flash.resident_pages(), 0);
    }

    #[test]
    fn unknown_command_is_rejected() {
        let mut flash = NandFlashCore::new();
        let mut ctx = NoopCtx;
        assert_eq!(
            flash.on_command(&mut ctx, &op(0x37, &[], &[])),
            Err(NandError::BadCommand)
        );
    }
}
