//! The ARM PL181 MultiMedia Card Interface (PrimeCell MCI) — QEMU's
//! `hw/sd/pl181.c` role: the MMC host of the ARM Versatile/PB platform.
//!
//! Clean-room from ARM DDI 0232: the command register (48-bit commands with
//! response types), the data control register (block size, direction,
//! enable), the data timer and length, the FIFO (16 deep, 32-bit wide) the
//! data phase moves through, the status and mask registers (FIFO levels,
//! command send/end, data end, block transfer), the clear register, and the
//! PrimeCell IDs. Data moves PIO through the FIFO: a read block fills it and
//! the driver drains words; a write block assembles words then lands in the
//! card.

use std::sync::Arc;

use crate::char::pl011::UnmappedPolicy;
use mosaic_bus::sd::{SdCardHandle, SdResponse, SdResponseType};
use mosaic_contract::mmio::MmioTarget;
use mosaic_contract::model::{set_optional_signal, ContextError, Ctx, EventTarget};
use mosaic_exec::{AccessSize, Attr, BusFault, Fnv64, SignalId, Snapshot, SnapshotError};

#[allow(dead_code)] // the documented register map; the walk grows into it
mod reg {
    pub const POWER: u64 = 0x000;
    pub const CLOCK: u64 = 0x004;
    pub const ARGUMENT: u64 = 0x008;
    pub const COMMAND: u64 = 0x00C;
    pub const RESPCMD: u64 = 0x010;
    pub const RESPONSE0: u64 = 0x014;
    pub const RESPONSE1: u64 = 0x018;
    pub const RESPONSE2: u64 = 0x01C;
    pub const RESPONSE3: u64 = 0x020;
    pub const DATATIMER: u64 = 0x024;
    pub const DATALENGTH: u64 = 0x028;
    pub const DATACTRL: u64 = 0x02C;
    pub const DATACNT: u64 = 0x030;
    pub const STATUS: u64 = 0x034;
    pub const CLEAR: u64 = 0x038;
    pub const MASK0: u64 = 0x03C;
    pub const MASK1: u64 = 0x040;
    pub const SELECT: u64 = 0x048;
    pub const FIFO_CNT: u64 = 0x048; // FIFO counter alias
    pub const FIFO: u64 = 0x080;
    pub const PERIPH_ID: u64 = 0xFE0;
    pub const PCELL_ID: u64 = 0xFF0;
    pub const SIZE: u64 = 0x1000;
}

/// Status bits (DDI 0232 §4.9).
#[allow(dead_code)] // documented status/status bits
mod status {
    pub const CMD_TX: u32 = 1 << 3;
    pub const CMD_SEND: u32 = 1 << 4;
    pub const DATA_END: u32 = 1 << 8;
    pub const START_BIT_ERR: u32 = 1 << 9;
    pub const TX_FIFO_FULL: u32 = 1 << 16;
    pub const RX_FIFO_FULL: u32 = 1 << 17;
    pub const TX_FIFO_HALF_EMPTY: u32 = 1 << 18;
    pub const RX_FIFO_HALF_FULL: u32 = 1 << 19;
    pub const TX_FIFO_EMPTY: u32 = 1 << 20;
    pub const RX_FIFO_LAST: u32 = 1 << 21;
    pub const CMD_CRC_FAIL: u32 = 1 << 22;
    pub const DATA_CRC_FAIL: u32 = 1 << 23;
    pub const FIFO_ACTIVE: u32 = 1 << 31;
}

/// The PL181.
pub struct Pl181 {
    irq0: SignalId,
    irq1: SignalId,
    card: Option<Arc<SdCardHandle>>,
    argument: u32,
    response: [u32; 4],
    resp_cmd: u32,
    data_ctrl: u32,
    data_length: u32,
    status: u32,
    mask0: u32,
    /// The FIFO (words) the data phase moves through.
    fifo: std::collections::VecDeque<u32>,
    /// The read block being drained, byte-wise.
    read_pending: Vec<u8>,
    read_at: usize,
    writing: bool,
    write_block: Vec<u8>,
    lba: u64,
    unmapped: UnmappedPolicy,
}

impl Pl181 {
    pub fn new(irq0: SignalId, irq1: SignalId) -> Self {
        Self {
            irq0,
            irq1,
            card: None,
            argument: 0,
            response: [0; 4],
            resp_cmd: 0,
            data_ctrl: 0,
            data_length: 0,
            status: status::TX_FIFO_EMPTY,
            mask0: 0,
            fifo: std::collections::VecDeque::new(),
            read_pending: Vec::new(),
            read_at: 0,
            writing: false,
            write_block: Vec::new(),
            lba: 0,
            unmapped: UnmappedPolicy::RazWi,
        }
    }

    pub fn attach_card(&mut self, card: Arc<SdCardHandle>) {
        self.card = Some(card);
    }

    fn update_irq(&mut self, ctx: &mut dyn Ctx) -> Result<(), ContextError> {
        let level = (self.status & self.mask0) != 0;
        set_optional_signal(ctx, self.irq0, level)?;
        set_optional_signal(ctx, self.irq1, false)
    }

    fn issue_command(&mut self, ctx: &mut dyn Ctx, command: u32) {
        let index = (command & 0x3F) as u8;
        let response_type = command & 0x3; // 0 = no response, 1 = long, 2 = short
        let with_data = command & (1 << 5) != 0 && command & (1 << 6) != 0;
        self.status |= status::CMD_TX;
        self.status &= !status::CMD_SEND;
        let Some(card) = self.card.clone() else {
            self.status |= status::CMD_CRC_FAIL;
            self.update_irq(ctx).ok();
            return;
        };
        let sd_type = match response_type {
            0 => SdResponseType::None,
            1 => SdResponseType::R2,
            _ => SdResponseType::R1,
        };
        match card.command(0x40 + index, self.argument, sd_type) {
            Ok(response) => {
                self.response = match response {
                    Some(SdResponse::Status(word))
                    | Some(SdResponse::Ocr(word))
                    | Some(SdResponse::Rca(word)) => [word, 0, 0, 0],
                    Some(SdResponse::Long(bytes)) => {
                        let word = |at: usize| {
                            u32::from_be_bytes([
                                bytes[at],
                                bytes[at + 1],
                                bytes[at + 2],
                                bytes[at + 3],
                            ])
                        };
                        [word(0), word(4), word(8), word(12)]
                    }
                    _ => [0; 4],
                };
                self.resp_cmd = u32::from(index);
                self.status |= status::CMD_SEND;
                // Data phases are commanded through DATACTRL on PL181, not
                // through the command register; a command with data still
                // records the LBA from ARGUMENT.
                let _ = with_data;
                self.lba = u64::from(self.argument);
            }
            Err(_) => {
                self.response = [0; 4];
                self.status |= status::CMD_CRC_FAIL;
            }
        }
        self.status &= !status::CMD_TX;
        self.update_irq(ctx).ok();
    }

    fn start_data(&mut self, ctx: &mut dyn Ctx) {
        let block_len = (self.data_ctrl >> 4) & 0xF;
        let block_len = 1usize << block_len.min(11);
        let read = self.data_ctrl & 0x1 != 0;
        let enabled = self.data_ctrl & 0x8 != 0;
        if !enabled {
            return;
        }
        let Some(card) = self.card.clone() else {
            return;
        };
        if read {
            match card.read_block(self.lba, block_len) {
                Ok(block) => {
                    self.read_pending = block;
                    self.read_at = 0;
                    self.writing = false;
                    // Fill the FIFO with the first words.
                    while self.fifo.len() < 16 && self.read_at + 4 <= self.read_pending.len() {
                        let word = u32::from_le_bytes([
                            self.read_pending[self.read_at],
                            self.read_pending[self.read_at + 1],
                            self.read_pending[self.read_at + 2],
                            self.read_pending[self.read_at + 3],
                        ]);
                        self.fifo.push_back(word);
                        self.read_at += 4;
                    }
                    self.status |= status::FIFO_ACTIVE | status::RX_FIFO_HALF_FULL;
                }
                Err(_) => self.status |= status::DATA_CRC_FAIL,
            }
        } else {
            self.writing = true;
            self.write_block = Vec::new();
            self.status |= status::FIFO_ACTIVE | status::TX_FIFO_HALF_EMPTY;
            let _ = ctx;
        }
    }

    fn drain_write(&mut self, ctx: &mut dyn Ctx, word: u32) {
        if !self.writing {
            return;
        }
        let block_len = (self.data_ctrl >> 4) & 0xF;
        let block_len = 1usize << block_len.min(11);
        self.write_block.extend_from_slice(&word.to_le_bytes());
        if self.write_block.len() >= block_len {
            if let Some(card) = self.card.clone() {
                let result = card.write_block(self.lba, &self.write_block);
                self.write_block.clear();
                match result {
                    Ok(()) => {
                        self.status |= status::DATA_END;
                        self.status &= !status::FIFO_ACTIVE;
                        self.writing = false;
                    }
                    Err(_) => self.status |= status::DATA_CRC_FAIL,
                }
                self.update_irq(ctx).ok();
            }
        }
    }
}

impl MmioTarget for Pl181 {
    fn read(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        if size != AccessSize::Word {
            return Err(BusFault::InvalidSize);
        }
        Ok(match offset {
            reg::POWER => 0xBF, // Power on (the driver polls it).
            reg::CLOCK => 0,
            reg::ARGUMENT => u64::from(self.argument),
            reg::RESPCMD => u64::from(self.resp_cmd),
            reg::RESPONSE0 => u64::from(self.response[0]),
            reg::RESPONSE1 => u64::from(self.response[1]),
            reg::RESPONSE2 => u64::from(self.response[2]),
            reg::RESPONSE3 => u64::from(self.response[3]),
            reg::DATATIMER => 0,
            reg::DATALENGTH => u64::from(self.data_length),
            reg::DATACTRL => u64::from(self.data_ctrl),
            reg::DATACNT => 0,
            reg::STATUS => {
                let mut value = self.status;
                if self.fifo.len() >= 16 {
                    value |= status::RX_FIFO_FULL | status::TX_FIFO_FULL;
                }
                if !self.fifo.is_empty() {
                    value |= status::RX_FIFO_HALF_FULL;
                }
                u64::from(value)
            }
            reg::MASK0 => u64::from(self.mask0),
            reg::MASK1 => 0,
            reg::FIFO | reg::FIFO_CNT => {
                // Draining the FIFO: the read block fills it back up.
                if let Some(word) = self.fifo.pop_front() {
                    while self.fifo.len() < 16 && self.read_at + 4 <= self.read_pending.len() {
                        let next = u32::from_le_bytes([
                            self.read_pending[self.read_at],
                            self.read_pending[self.read_at + 1],
                            self.read_pending[self.read_at + 2],
                            self.read_pending[self.read_at + 3],
                        ]);
                        self.fifo.push_back(next);
                        self.read_at += 4;
                    }
                    if self.read_at >= self.read_pending.len() && self.fifo.is_empty() {
                        self.status |= status::DATA_END;
                        self.status &= !status::FIFO_ACTIVE;
                        self.update_irq(ctx).ok();
                    }
                    u64::from(word)
                } else {
                    0
                }
            }
            offset if (reg::PERIPH_ID..reg::PERIPH_ID + 16).contains(&offset) => {
                let index = (offset - reg::PERIPH_ID) / 4;
                [0x81, 0x11, 0x18, 0x00][index as usize % 4] as u64
            }
            offset if (reg::PCELL_ID..reg::PCELL_ID + 16).contains(&offset) => {
                let index = (offset - reg::PCELL_ID) / 4;
                [0x0D, 0xF0, 0x05, 0xB1][index as usize % 4] as u64
            }
            _ => match self.unmapped {
                UnmappedPolicy::RazWi => 0,
                UnmappedPolicy::Fault => return Err(BusFault::Device),
            },
        })
    }

    fn write(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        value: u64,
        _attr: &Attr,
    ) -> Result<(), BusFault> {
        if size != AccessSize::Word {
            return Err(BusFault::InvalidSize);
        }
        match offset {
            reg::POWER | reg::CLOCK | reg::SELECT => {}
            reg::ARGUMENT => self.argument = value as u32,
            reg::COMMAND => self.issue_command(ctx, value as u32),
            reg::DATATIMER => {}
            reg::DATALENGTH => self.data_length = value as u32,
            reg::DATACTRL => {
                self.data_ctrl = value as u32;
                self.start_data(ctx);
            }
            reg::CLEAR => {
                // Write-one-to-clear on the clear register.
                self.status &= !(value as u32 & 0x7FF_FFFF);
                self.update_irq(ctx).ok();
            }
            reg::MASK0 => {
                self.mask0 = value as u32 & 0x7FFF_FFFF;
                self.update_irq(ctx).ok();
            }
            reg::FIFO => {
                self.drain_write(ctx, value as u32);
            }
            _ => match self.unmapped {
                UnmappedPolicy::RazWi => {}
                UnmappedPolicy::Fault => return Err(BusFault::Device),
            },
        }
        Ok(())
    }
}

impl EventTarget for Pl181 {
    fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
        Ok(())
    }
}

/// Checkpoint state (ADR 0004).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pl181Checkpoint {
    pub schema: u32,
    pub argument: u32,
    pub response: [u32; 4],
    pub resp_cmd: u32,
    pub data_ctrl: u32,
    pub data_length: u32,
    pub status: u32,
    pub mask0: u32,
    pub fifo: Vec<u32>,
    pub read_pending: usize,
    pub writing: bool,
    pub lba: u64,
}

impl Snapshot for Pl181 {
    type State = Pl181Checkpoint;
    const SCHEMA_VERSION: u32 = 1;

    fn save(&self) -> Self::State {
        Self::State {
            schema: Self::SCHEMA_VERSION,
            argument: self.argument,
            response: self.response,
            resp_cmd: self.resp_cmd,
            data_ctrl: self.data_ctrl,
            data_length: self.data_length,
            status: self.status,
            mask0: self.mask0,
            fifo: self.fifo.iter().copied().collect(),
            read_pending: self.read_pending.len(),
            writing: self.writing,
            lba: self.lba,
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
        self.argument = state.argument;
        self.response = state.response;
        self.resp_cmd = state.resp_cmd;
        self.data_ctrl = state.data_ctrl;
        self.data_length = state.data_length;
        self.status = state.status;
        self.mask0 = state.mask0;
        self.fifo = state.fifo.iter().copied().collect();
        self.writing = state.writing;
        self.lba = state.lba;
        Ok(())
    }

    fn fingerprint(version: u32, state: &Self::State) -> u64 {
        let mut hash = Fnv64::default();
        hash.bytes(&version.to_le_bytes());
        hash.bytes(&state.argument.to_le_bytes());
        hash.bytes(&state.status.to_le_bytes());
        hash.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sd::sd_card::{CardIdentity, SdCard};
    use mosaic_bus::sd::card_handle;
    use mosaic_contract::mmio::MmioTarget;
    use mosaic_exec::{AccessKind, Endian, EventId, NodeId, Security, VTime};

    struct TestCtx;

    impl Ctx for TestCtx {
        fn now(&self) -> VTime {
            VTime::ZERO
        }

        fn schedule(&mut self, _at: VTime, _token: u64) -> Result<EventId, ContextError> {
            Err(ContextError::ResourceExhausted)
        }

        fn cancel(&mut self, _event: EventId) -> bool {
            false
        }

        fn set_signal(&mut self, _signal: SignalId, _high: bool) -> Result<(), ContextError> {
            Ok(())
        }
    }

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

    fn host() -> Pl181 {
        let mut host = Pl181::new(SignalId(1), SignalId(2));
        host.attach_card(card_handle(Box::new(SdCard::sd(
            vec![0xCD; 512 * 4],
            CardIdentity {
                cid: [0; 16],
                csd: [0; 16],
            },
        ))));
        host
    }

    fn command(host: &mut Pl181, index: u8, argument: u32) {
        host.write(
            &mut TestCtx,
            reg::ARGUMENT,
            AccessSize::Word,
            u64::from(argument),
            &attr(),
        )
        .unwrap();
        // Response type 2 (short), no data: the PL181's encoding.
        host.write(
            &mut TestCtx,
            reg::COMMAND,
            AccessSize::Word,
            u64::from(index as u32 | 0x40),
            &attr(),
        )
        .unwrap();
    }

    #[test]
    fn cmd3_answers_the_rca_in_response0() {
        let mut host = host();
        command(&mut host, 3, 0);
        assert!(host.status & status::CMD_SEND != 0);
        assert_ne!(
            host.read(&mut TestCtx, reg::RESPONSE0, AccessSize::Word, &attr())
                .unwrap(),
            0
        );
    }

    #[test]
    fn a_read_block_fills_the_fifo_and_drains() {
        let mut host = host();
        // CMD17 with block size 512 (2^9), read, enable.
        command(&mut host, 17, 0);
        host.write(
            &mut TestCtx,
            reg::DATACTRL,
            AccessSize::Word,
            0x1 | 0x8 | (9 << 4),
            &attr(),
        )
        .unwrap();
        assert!(host.status & status::FIFO_ACTIVE != 0);
        let mut words = 0;
        while host
            .read(&mut TestCtx, reg::STATUS, AccessSize::Word, &attr())
            .unwrap() as u32
            & status::FIFO_ACTIVE
            != 0
            || words < 128
        {
            let _ = host
                .read(&mut TestCtx, reg::FIFO, AccessSize::Word, &attr())
                .unwrap();
            words += 1;
            if words > 200 {
                break;
            }
        }
        assert!(host.status & status::DATA_END != 0);
        assert_eq!(words, 128); // 512 bytes = 128 words.
    }

    #[test]
    fn the_primecell_ids_read_back() {
        let mut host = host();
        assert_eq!(
            host.read(&mut TestCtx, reg::PERIPH_ID, AccessSize::Word, &attr())
                .unwrap(),
            0x81
        );
        assert_eq!(
            host.read(&mut TestCtx, reg::PCELL_ID, AccessSize::Word, &attr())
                .unwrap(),
            0x0D
        );
    }
}
