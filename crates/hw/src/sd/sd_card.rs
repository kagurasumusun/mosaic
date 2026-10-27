//! The SD/eMMC card model (QEMU's `hw/sd/sd.c` role): one removable card
//! the host controllers (SDHCI, PL181) drive through the bus-side
//! [`SdCardPort`].
//!
//! Clean-room from the SD Physical Layer Simplified Specification 9.0 (the
//! SD path: CMD0 idle, CMD8 interface condition, ACMD41 operating
//! condition, CMD2 ALL_SEND_CID, CMD3 SEND_RELATIVE_ADDR, CMD9 SEND_CSD,
//! CMD7 select/deselect, CMD17/18 read, CMD24/25 write, CMD12 stop) and the
//! JEDEC eMMC 5.1 (the MMC path: CMD1 OCR negotiation, CMD8 SEND_EXT_CSD
//! in place of SD's ACMD41 path). The card is the *media*: a backing image
//! the board provides, behind the response state machine.

use mosaic_bus::sd::{SdCardPort, SdError, SdResponse, SdResponseType};

/// Card identification, as ALL_SEND_CID (CMD2) answers it (R2).
pub struct CardIdentity {
    /// 128-bit CID: manufacturer, OEM, product name, revision, serial.
    pub cid: [u8; 16],
    /// 128-bit CSD: the register that carries capacity and timing.
    pub csd: [u8; 16],
}

/// Card status bits (the R1 word) the model sets.
#[allow(dead_code)] // documented status/status bits
mod status {
    pub const READY_FOR_DATA: u32 = 1 << 8;
    pub const CURRENT_STATE: u32 = 0xF << 9;
    pub const STATE_IDLE: u32 = 0 << 9;
    pub const STATE_READY: u32 = 1 << 9;
    pub const STATE_IDENT: u32 = 2 << 9;
    pub const STATE_STBY: u32 = 3 << 9;
    pub const STATE_TRAN: u32 = 4 << 9;
    pub const STATE_DATA: u32 = 5 << 9;
    pub const STATE_RCV: u32 = 6 << 9;
    pub const STATE_PRG: u32 = 7 << 9;
    pub const ILLEGAL_COMMAND: u32 = 1 << 22;
    pub const ERROR: u32 = 1 << 19;
}

/// The card media and its command state machine.
pub struct SdCard {
    /// The card kind: SD (ACMD41 path) or eMMC (CMD1 path).
    kind: CardKind,
    /// The backing image (block device contents).
    image: Vec<u8>,
    /// Logical block length (SD: 512 after SET_BLOCKLEN; eMMC: 512).
    block_len: usize,
    capacity_blocks: u64,
    identity: CardIdentity,
    /// The RCA CMD3 assigned (SD); eMMC uses a fixed one.
    rca: u16,
    /// Current status word.
    status: u32,
    /// The last CMD55: the next ACMD is expected.
    app_command: bool,
    /// OCR: voltage window + busy bit.
    ocr: u32,
    /// EXT_CSD (eMMC): 512 bytes of configuration.
    ext_csd: [u8; 512],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CardKind {
    Sd,
    Emmc,
}

/// The OCR window every modern card answers (3.2-3.3V... 2.7-3.6V bits).
const OCR_VOLTAGE_WINDOW: u32 = 0x00FF_8000;
/// OCR busy bit (power-up complete).
const OCR_BUSY: u32 = 1 << 31;
/// The OCR's card capacity status bit (SD: 1 = SDHC/SDXC, block-addressed).
const OCR_CCS: u32 = 1 << 30;

impl SdCard {
    /// An SD card over a backing `image` (multiple of 512).
    pub fn sd(image: Vec<u8>, identity: CardIdentity) -> Self {
        let capacity_blocks = image.len() as u64 / 512;
        Self {
            kind: CardKind::Sd,
            image,
            block_len: 512,
            capacity_blocks,
            identity,
            rca: 0,
            status: status::STATE_IDLE,
            app_command: false,
            ocr: 0,
            ext_csd: [0; 512],
        }
    }

    /// An eMMC over a backing `image`.
    pub fn emmc(image: Vec<u8>, identity: CardIdentity) -> Self {
        let mut card = Self::sd(image, identity);
        card.kind = CardKind::Emmc;
        // EXT_CSD: revision 7 (5.1), secured write etc. left zero.
        card.ext_csd[192] = 7; // EXT_CSD_REV
        card
    }

    pub fn kind(&self) -> CardKind {
        self.kind
    }

    pub fn capacity_blocks(&self) -> u64 {
        self.capacity_blocks
    }

    /// The RCA the CMD3 exchange assigned (diagnostics and routing).
    pub fn rca(&self) -> u16 {
        self.rca
    }

    fn status_word(&self) -> u32 {
        self.status | status::READY_FOR_DATA
    }

    fn set_state(&mut self, state: u32) {
        self.status = (self.status & !status::CURRENT_STATE) | state;
    }

    fn block_at(&self, lba: u64, len: usize) -> Result<usize, SdError> {
        if lba >= self.capacity_blocks {
            return Err(SdError::OutOfRange);
        }
        let at = lba as usize * self.block_len;
        if at + len > self.image.len() {
            return Err(SdError::OutOfRange);
        }
        Ok(at)
    }
}

impl SdCardPort for SdCard {
    fn command(
        &mut self,
        cmd: u8,
        argument: u32,
        response_type: SdResponseType,
    ) -> Result<Option<SdResponse>, SdError> {
        let _ = response_type;
        // ACMD handling: CMD55 prefixes the next command as app-specific.
        if self.app_command && (0x40..=0x7F).contains(&cmd) {
            self.app_command = false;
            return match cmd {
                0x69 => {
                    // ACMD41 (CMD55 + 41)
                    // ACMD41 SD_SEND_OP_COND.
                    self.ocr = OCR_BUSY | OCR_VOLTAGE_WINDOW | OCR_CCS;
                    self.set_state(status::STATE_READY);
                    Ok(Some(SdResponse::Ocr(self.ocr)))
                }
                _ => {
                    self.status |= status::ILLEGAL_COMMAND;
                    Err(SdError::IllegalCommand)
                }
            };
        }
        match cmd {
            0x40 | 0x00 => {
                // CMD0 GO_IDLE_STATE.
                self.set_state(status::STATE_IDLE);
                self.rca = 0;
                self.ocr = 0;
                Ok(None)
            }
            0x41 => {
                // eMMC CMD1 SEND_OP_COND.
                if self.kind != CardKind::Emmc {
                    self.status |= status::ILLEGAL_COMMAND;
                    return Err(SdError::IllegalCommand);
                }
                self.ocr = OCR_BUSY | OCR_VOLTAGE_WINDOW;
                self.set_state(status::STATE_READY);
                Ok(Some(SdResponse::Ocr(self.ocr)))
            }
            0x48 => {
                // CMD8: SD SEND_IF_COND vs eMMC SEND_EXT_CSD.
                if self.kind == CardKind::Sd {
                    // Echo the check pattern.
                    Ok(Some(SdResponse::Ocr(argument & 0x0000_01FF | 0x0000_0100)))
                } else {
                    // EXT_CSD as a 512-byte data block (the host reads it
                    // through the data phase; the response is R1).
                    self.set_state(status::STATE_DATA);
                    Ok(Some(SdResponse::Status(self.status_word())))
                }
            }
            0x4A => {
                // CMD13 SEND_STATUS.
                Ok(Some(SdResponse::Status(self.status_word())))
            }
            0x42 => {
                // CMD2 ALL_SEND_CID.
                self.set_state(status::STATE_IDENT);
                Ok(Some(SdResponse::Long(self.identity.cid)))
            }
            0x43 => {
                // CMD3 SEND_RELATIVE_ADDR (SD) / SET_RELATIVE_ADDR (eMMC).
                if self.kind == CardKind::Sd {
                    self.rca = 0x0001;
                } else {
                    self.rca = (argument >> 16) as u16;
                }
                self.set_state(status::STATE_STBY);
                Ok(Some(SdResponse::Rca(u32::from(self.rca) << 16)))
            }
            0x49 => {
                // CMD9 SEND_CSD.
                Ok(Some(SdResponse::Long(self.identity.csd)))
            }
            0x47 => {
                // CMD7 SELECT/DESELECT.
                self.set_state(status::STATE_TRAN);
                Ok(Some(SdResponse::Status(self.status_word())))
            }
            0x50 => {
                // CMD16 SET_BLOCKLEN.
                if (argument as usize).is_power_of_two() && argument as usize <= 4096 {
                    self.block_len = argument as usize;
                    Ok(Some(SdResponse::Status(self.status_word())))
                } else {
                    Err(SdError::IllegalCommand)
                }
            }
            0x51 => {
                // CMD17 READ_SINGLE_BLOCK.
                self.set_state(status::STATE_DATA);
                Ok(Some(SdResponse::Data))
            }
            0x52 => {
                // CMD18 READ_MULTIPLE_BLOCK.
                self.set_state(status::STATE_DATA);
                Ok(Some(SdResponse::Data))
            }
            0x58 => {
                // CMD24 WRITE_BLOCK.
                self.set_state(status::STATE_RCV);
                Ok(Some(SdResponse::Data))
            }
            0x59 => {
                // CMD25 WRITE_MULTIPLE_BLOCK.
                self.set_state(status::STATE_RCV);
                Ok(Some(SdResponse::Data))
            }
            0x4C => {
                // CMD12 STOP_TRANSMISSION.
                self.set_state(status::STATE_TRAN);
                Ok(Some(SdResponse::Status(self.status_word())))
            }
            0x77 => {
                // CMD55 APP_CMD.
                self.app_command = true;
                Ok(Some(SdResponse::Status(
                    self.status_word() | (1 << 5), // APP_CMD bit
                )))
            }
            0x4F => {
                // CMD15 GO_INACTIVE_STATE.
                self.set_state(status::STATE_IDLE);
                Ok(None)
            }
            _ => {
                self.status |= status::ILLEGAL_COMMAND | status::ERROR;
                Err(SdError::IllegalCommand)
            }
        }
    }

    fn read_block(&mut self, lba: u64, block_len: usize) -> Result<Vec<u8>, SdError> {
        let at = self.block_at(lba, block_len)?;
        self.set_state(status::STATE_DATA);
        Ok(self.image[at..at + block_len].to_vec())
    }

    fn write_block(&mut self, lba: u64, data: &[u8]) -> Result<(), SdError> {
        let at = self.block_at(lba, data.len())?;
        self.image[at..at + data.len()].copy_from_slice(data);
        self.set_state(status::STATE_PRG);
        self.set_state(status::STATE_TRAN);
        Ok(())
    }

    fn stop_transmission(&mut self) -> Result<(), SdError> {
        self.set_state(status::STATE_TRAN);
        Ok(())
    }
}

impl SdCard {
    /// The EXT_CSD block (eMMC): read through CMD8's data phase.
    pub fn ext_csd(&self) -> [u8; 512] {
        self.ext_csd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> CardIdentity {
        CardIdentity {
            cid: [
                0x01, 0x44, 0x41, b'S', b'D', b'0', b'1', 0x10, 0, 0, 0, 0x12, 0x34, 0x56, 0x78,
                0x9A,
            ],
            csd: [
                0x40, 0x0E, 0x00, 0x32, 0x5B, 0x59, 0x00, 0x00, 0x1D, 0x6F, 0x7F, 0x80, 0x0A, 0x40,
                0x00, 0x23,
            ],
        }
    }

    fn card() -> SdCard {
        SdCard::sd(vec![0xA5; 512 * 16], identity())
    }

    #[test]
    fn the_sd_initialization_sequence_walks_the_states() {
        let mut card = card();
        assert_eq!(card.command(0x40, 0, SdResponseType::None), Ok(None));
        let check = card
            .command(0x48, 0x1AA, SdResponseType::R7)
            .unwrap()
            .unwrap();
        assert!(matches!(check, SdResponse::Ocr(value) if value & 0x1FF == 0x1AA));
        card.command(0x77, 0, SdResponseType::R1).unwrap();
        let ocr = card
            .command(0x40 + 41, OCR_VOLTAGE_WINDOW, SdResponseType::R3)
            .unwrap()
            .unwrap();
        assert!(matches!(ocr, SdResponse::Ocr(value) if value & OCR_BUSY != 0));
        let cid = card.command(0x42, 0, SdResponseType::R2).unwrap().unwrap();
        assert!(matches!(cid, SdResponse::Long(bytes) if bytes == identity().cid));
        let rca = card.command(0x43, 0, SdResponseType::R6).unwrap().unwrap();
        assert!(matches!(rca, SdResponse::Rca(value) if value >> 16 == 1));
        card.command(0x47, u32::from(card.rca()) << 16, SdResponseType::R1b)
            .unwrap();
        // A block reads back.
        let block = card.read_block(2, 512).unwrap();
        assert_eq!(block.len(), 512);
        assert!(block.iter().all(|byte| *byte == 0xA5));
    }

    #[test]
    fn blocks_read_and_write_through_the_media() {
        let mut card = card();
        card.command(0x40, 0, SdResponseType::None).unwrap();
        card.command(0x47, 0x0001_0000, SdResponseType::R1b)
            .unwrap();
        card.write_block(3, &[0x33; 512]).unwrap();
        let block = card.read_block(3, 512).unwrap();
        assert!(block.iter().all(|byte| *byte == 0x33));
        assert_eq!(card.read_block(999, 512), Err(SdError::OutOfRange));
    }

    #[test]
    fn an_emmc_uses_cmd1_and_keeps_a_fixed_rca_path() {
        let mut card = SdCard::emmc(vec![0; 512 * 8], identity());
        assert_eq!(card.kind(), CardKind::Emmc);
        let ocr = card.command(0x41, 0, SdResponseType::R3).unwrap().unwrap();
        assert!(matches!(ocr, SdResponse::Ocr(value) if value & OCR_BUSY != 0));
        let rca = card
            .command(0x43, 0x0002_0000, SdResponseType::R6)
            .unwrap()
            .unwrap();
        assert!(matches!(rca, SdResponse::Rca(0x0002_0000)));
        assert_eq!(card.ext_csd()[192], 7);
    }

    #[test]
    fn an_illegal_command_is_rejected_with_status() {
        let mut card = card();
        assert_eq!(
            card.command(0x42 + 0x30, 0, SdResponseType::None),
            Err(SdError::IllegalCommand)
        );
        assert!(card.status & status::ILLEGAL_COMMAND != 0);
    }
}
