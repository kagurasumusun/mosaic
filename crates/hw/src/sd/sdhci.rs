//! The SD Host Controller Interface (SDHCI) — QEMU's `hw/sd/sdhci.c` role:
//! the standard register set every SDHCI driver on every OS programs
//! (`/dev/mmcblk` on Linux, `sdhc` on Windows, the SD stack in U-Boot).
//!
//! Clean-room from the SD Host Controller Simplified Specification 3.0: the
//! capabilities register, present state, host control (bus width, high
//! speed), power and clock control, the normal/error interrupt
//! status/enable/signal sets, block size/count, argument, transfer mode
//! (single/multi-block, read/write, block-count enable, auto-CMD12), the
//! command register with its response types, the response registers, the
//! buffer data port (PIO), and the software reset. Data moves PIO through
//! the buffer port: a block lands in the read buffer at command issue and
//! drains through the port, write data assembles in the port order.
//! Interrupts assert through one signal output.

use std::sync::Arc;

use crate::char::pl011::UnmappedPolicy;
use mosaic_bus::sd::{SdCardHandle, SdError, SdResponse, SdResponseType};
use mosaic_contract::mmio::MmioTarget;
use mosaic_contract::model::{set_optional_signal, ContextError, Ctx, EventTarget};
use mosaic_exec::{AccessSize, Attr, BusFault, Fnv64, SignalId, Snapshot, SnapshotError};

#[allow(dead_code)] // the documented register map; the walk grows into it
mod reg {
    pub const SDMA_SYSTEM_ADDRESS: u64 = 0x000;
    pub const BLOCK_SIZE: u64 = 0x004;
    pub const BLOCK_COUNT: u64 = 0x006;
    pub const ARGUMENT: u64 = 0x008;
    pub const TRANSFER_MODE: u64 = 0x00C;
    pub const COMMAND: u64 = 0x00E;
    pub const RESPONSE: u64 = 0x010; // ..= 0x01F (4 x 32-bit)
    pub const BUFFER_DATA_PORT: u64 = 0x020;
    pub const PRESENT_STATE: u64 = 0x024;
    pub const HOST_CONTROL: u64 = 0x028;
    pub const POWER_CONTROL: u64 = 0x029;
    pub const CLOCK_CONTROL: u64 = 0x02C;
    pub const TIMEOUT_CONTROL: u64 = 0x02E;
    pub const SOFTWARE_RESET: u64 = 0x02F;
    pub const INTERRUPT_STATUS: u64 = 0x030;
    pub const INTERRUPT_ENABLE: u64 = 0x034;
    pub const SIGNAL_ENABLE: u64 = 0x038;
    pub const AUTO_CMD12_ERROR: u64 = 0x03C;
    pub const CAPABILITIES: u64 = 0x040;
    pub const MAX_CURRENT: u64 = 0x044;
    pub const FORCE_EVENT: u64 = 0x052;
    pub const ADMA_ERROR: u64 = 0x054;
    pub const PRESET_VALUES: u64 = 0x060;
    pub const SLOT_INTERRUPT: u64 = 0x0FC;
    pub const HOST_VERSION: u64 = 0x0FE;
    pub const SIZE: u64 = 0x100;
}

/// Present-state bits.
#[allow(dead_code)] // documented present-state bits
mod state {
    pub const COMMAND_INHIBIT_CMD: u32 = 1 << 0;
    pub const COMMAND_INHIBIT_DAT: u32 = 1 << 1;
    pub const BUFFER_READ_ENABLE: u32 = 1 << 11;
    pub const BUFFER_WRITE_ENABLE: u32 = 1 << 10;
    pub const READ_TRANSFER_ACTIVE: u32 = 1 << 9;
    pub const WRITE_TRANSFER_ACTIVE: u32 = 1 << 8;
    pub const CARD_INSERTED: u32 = 1 << 16;
    pub const CARD_STATE_STABLE: u32 = 1 << 17;
    pub const CARD_DETECT: u32 = 1 << 18;
}

/// Interrupt bits (normal and error halves).
#[allow(dead_code)] // documented interrupt bits
mod irq {
    pub const COMMAND_COMPLETE: u32 = 1 << 0;
    pub const TRANSFER_COMPLETE: u32 = 1 << 1;
    pub const BUFFER_READ_READY: u32 = 1 << 5;
    pub const BUFFER_WRITE_READY: u32 = 1 << 4;
    pub const CARD_INTERRUPT: u32 = 1 << 8;
    pub const ERROR: u32 = 1 << 15;
}

/// Transfer-mode bits.
#[allow(dead_code)] // documented transfer-mode bits
mod xfer {
    pub const READ: u16 = 0;
    pub const WRITE: u16 = 1 << 4;
    pub const MULTI_BLOCK: u16 = 1 << 5;
    pub const BLOCK_COUNT_ENABLE: u16 = 1 << 1;
    pub const AUTO_CMD12: u16 = 1 << 2;
}

/// One SDHCI host.
pub struct Sdhci {
    irq: SignalId,
    card: Option<Arc<SdCardHandle>>,
    block_size: u16,
    block_count: u16,
    argument: u32,
    transfer_mode: u16,
    command: u16,
    response: [u32; 4],
    host_control: u8,
    clock_control: u16,
    interrupt_status: u32,
    interrupt_enable: u32,
    signal_enable: u32,
    /// The PIO read buffer (a block fetched from the card, draining through
    /// the buffer port) and its cursor.
    read_buffer: Vec<u8>,
    read_at: usize,
    /// The PIO write buffer (assembling through the buffer port).
    write_buffer: Vec<u8>,
    /// Remaining blocks of a multi-block transfer.
    blocks_left: u16,
    /// Whether the current data phase writes to the card.
    writing: bool,
    unmapped: UnmappedPolicy,
}

impl Sdhci {
    pub fn new(irq: SignalId) -> Self {
        Self {
            irq,
            card: None,
            block_size: 0,
            block_count: 0,
            argument: 0,
            transfer_mode: 0,
            command: 0,
            response: [0; 4],
            host_control: 0,
            clock_control: 0,
            interrupt_status: 0,
            interrupt_enable: 0,
            signal_enable: 0,
            read_buffer: Vec::new(),
            read_at: 0,
            write_buffer: Vec::new(),
            blocks_left: 0,
            writing: false,
            unmapped: UnmappedPolicy::RazWi,
        }
    }

    /// Attach the card (the board's wiring, through the host attachment).
    pub fn attach_card(&mut self, card: Arc<SdCardHandle>) {
        self.card = Some(card);
    }

    fn present_state(&self) -> u32 {
        let mut value = 0;
        if self.card.is_some() {
            value |= state::CARD_INSERTED | state::CARD_STATE_STABLE | state::CARD_DETECT;
        }
        if self.command_inhibited() {
            value |= state::COMMAND_INHIBIT_CMD;
        }
        if !self.read_buffer.is_empty() && self.read_at < self.read_buffer.len() {
            value |= state::BUFFER_READ_ENABLE | state::READ_TRANSFER_ACTIVE;
        }
        if !self.writing && self.blocks_left > 0 {
            value |= state::BUFFER_READ_ENABLE;
        }
        if self.writing {
            value |= state::BUFFER_WRITE_ENABLE | state::WRITE_TRANSFER_ACTIVE;
        }
        value
    }

    fn command_inhibited(&self) -> bool {
        false // Commands complete synchronously at transaction level.
    }

    fn update_irq(&mut self, ctx: &mut dyn Ctx) -> Result<(), ContextError> {
        let level = (self.interrupt_status & self.interrupt_enable & self.signal_enable) != 0;
        set_optional_signal(ctx, self.irq, level)
    }

    fn raise(&mut self, ctx: &mut dyn Ctx, bits: u32) -> Result<(), ContextError> {
        self.interrupt_status |= bits;
        self.update_irq(ctx)
    }

    /// Issue the command in the command register: the card answers, the
    /// response registers fill, and a data phase sets up the buffers.
    fn issue_command(&mut self, ctx: &mut dyn Ctx) -> Result<(), ContextError> {
        let index = ((self.command >> 8) & 0x3F) as u8; // CMD index
        let response_type = self.command & 0x3;
        let with_data = self.transfer_mode != 0 || self.has_data_phase(index);
        let _ = with_data;
        let Some(card) = self.card.clone() else {
            self.raise(ctx, irq::ERROR).ok();
            return Ok(());
        };
        let bus_cmd = 0x40 + index;
        let sd_response_type = match response_type {
            0 | 1 => SdResponseType::R1,
            2 | 3 => SdResponseType::R2,
            _ => SdResponseType::R1,
        };
        let result = card.command(bus_cmd, self.argument, sd_response_type);
        match result {
            Ok(response) => {
                self.response = match response {
                    Some(SdResponse::Status(word)) => [word, 0, 0, 0],
                    Some(SdResponse::Rca(word)) => [word, 0, 0, 0],
                    Some(SdResponse::Ocr(word)) => [word, 0, 0, 0],
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
                // Data phases: CMD17/18 read, CMD24/25 write.
                let data_read = index == 17 || index == 18;
                let data_write = index == 24 || index == 25;
                if data_read {
                    let block_len = self.block_size.max(1) as usize;
                    let lba = u64::from(self.argument);
                    match card.read_block(lba, block_len) {
                        Ok(block) => {
                            self.read_buffer = block;
                            self.read_at = 0;
                            self.blocks_left = if self.transfer_mode & xfer::MULTI_BLOCK != 0
                                && self.transfer_mode & xfer::BLOCK_COUNT_ENABLE != 0
                            {
                                self.block_count.saturating_sub(1)
                            } else {
                                0
                            };
                            self.writing = false;
                            self.raise(ctx, irq::BUFFER_READ_READY).ok();
                        }
                        Err(_) => self.raise(ctx, irq::ERROR)?,
                    }
                } else if data_write {
                    self.write_buffer = Vec::new();
                    self.writing = true;
                    self.blocks_left = if self.transfer_mode & xfer::MULTI_BLOCK != 0
                        && self.transfer_mode & xfer::BLOCK_COUNT_ENABLE != 0
                    {
                        self.block_count
                    } else {
                        1
                    };
                }
                self.raise(ctx, irq::COMMAND_COMPLETE).ok();
            }
            Err(SdError::NoCard) | Err(SdError::IllegalCommand) | Err(_) => {
                self.response = [0; 4];
                self.raise(ctx, irq::ERROR).ok();
            }
        }
        Ok(())
    }

    fn has_data_phase(&self, index: u8) -> bool {
        matches!(index, 17 | 18 | 24 | 25)
    }

    /// Move one completed write buffer into the card.
    fn flush_write(&mut self, ctx: &mut dyn Ctx) -> Result<(), ContextError> {
        let block_len = self.block_size.max(1) as usize;
        if !self.writing || self.write_buffer.len() < block_len {
            return Ok(());
        }
        let lba = u64::from(self.argument);
        let Some(card) = self.card.clone() else {
            return Ok(());
        };
        let result = card.write_block(
            lba + (self.blocks_left.max(1) as u64).saturating_sub(1),
            &self.write_buffer,
        );
        self.write_buffer.clear();
        self.blocks_left = self.blocks_left.saturating_sub(1);
        match result {
            Ok(()) => {
                if self.blocks_left == 0 {
                    self.writing = false;
                    self.raise(ctx, irq::TRANSFER_COMPLETE).ok();
                } else {
                    self.raise(ctx, irq::BUFFER_WRITE_READY).ok();
                }
            }
            Err(_) => self.raise(ctx, irq::ERROR)?,
        }
        Ok(())
    }
}

impl MmioTarget for Sdhci {
    fn read(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        let value = match offset {
            reg::SDMA_SYSTEM_ADDRESS => 0,
            reg::BLOCK_SIZE if size == AccessSize::HalfWord => u64::from(self.block_size),
            reg::BLOCK_COUNT if size == AccessSize::HalfWord => u64::from(self.block_count),
            reg::ARGUMENT => u64::from(self.argument),
            reg::TRANSFER_MODE if size == AccessSize::HalfWord => u64::from(self.transfer_mode),
            reg::COMMAND if size == AccessSize::HalfWord => u64::from(self.command),
            offset if (reg::RESPONSE..reg::RESPONSE + 16).contains(&offset) => {
                let index = ((offset - reg::RESPONSE) / 4) as usize;
                u64::from(self.response[index])
            }
            reg::BUFFER_DATA_PORT => {
                // PIO read: drain the read buffer one dword at a time.
                let mut value = 0u64;
                let width = 4usize;
                for index in 0..width {
                    let at = self.read_at + index;
                    if at < self.read_buffer.len() {
                        value |= u64::from(self.read_buffer[at]) << (index * 8);
                    }
                }
                self.read_at = (self.read_at + width).min(self.read_buffer.len());
                if !self.read_buffer.is_empty() && self.read_at >= self.read_buffer.len() {
                    // Block drained: fetch the next block of a multi-block
                    // read or complete the transfer.
                    if self.blocks_left > 0 {
                        if let Some(card) = self.card.clone() {
                            let block_len = self.block_size.max(1) as usize;
                            let next = u64::from(self.argument)
                                + 1
                                + (self.block_count.saturating_sub(self.blocks_left) as u64);
                            if let Ok(block) = card.read_block(next, block_len) {
                                self.read_buffer = block;
                                self.read_at = 0;
                                self.blocks_left -= 1;
                            }
                        }
                    } else {
                        self.read_buffer.clear();
                        self.raise(ctx, irq::TRANSFER_COMPLETE).ok();
                    }
                }
                value
            }
            reg::PRESENT_STATE => u64::from(self.present_state()),
            reg::HOST_CONTROL => u64::from(self.host_control),
            reg::CLOCK_CONTROL if size == AccessSize::HalfWord => u64::from(self.clock_control),
            reg::INTERRUPT_STATUS => u64::from(self.interrupt_status),
            reg::INTERRUPT_ENABLE => u64::from(self.interrupt_enable),
            reg::SIGNAL_ENABLE => u64::from(self.signal_enable),
            reg::CAPABILITIES => {
                // 3.3V, 48 MHz base clock, high speed, SDMA, no suspend, 64
                // blocks? spec bits: timeout clock unit 1 MHz, 48 MHz, base
                // 48 MHz, 3.3V, high speed, 8-bit bus.
                0x057E_CD32_0000_0001 | 1
            }
            reg::SLOT_INTERRUPT => 0,
            reg::HOST_VERSION if size == AccessSize::HalfWord => 0x0002, // Version 3.0
            _ => match self.unmapped {
                UnmappedPolicy::RazWi => 0,
                UnmappedPolicy::Fault => return Err(BusFault::Device),
            },
        };
        Ok(value)
    }

    fn write(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        value: u64,
        _attr: &Attr,
    ) -> Result<(), BusFault> {
        match offset {
            reg::BLOCK_SIZE => self.block_size = value as u16 & 0x0FFF,
            reg::BLOCK_COUNT => self.block_count = value as u16,
            reg::ARGUMENT => self.argument = value as u32,
            reg::TRANSFER_MODE => self.transfer_mode = value as u16,
            reg::COMMAND => {
                self.command = value as u16;
                self.issue_command(ctx).ok();
            }
            reg::BUFFER_DATA_PORT => {
                if self.writing {
                    let block_len = self.block_size.max(1) as usize;
                    for index in 0..4 {
                        if size == AccessSize::Word {
                            self.write_buffer
                                .push(((value >> (index * 8)) & 0xFF) as u8);
                        }
                    }
                    if self.write_buffer.len() >= block_len {
                        self.write_buffer.truncate(block_len);
                        self.flush_write(ctx).ok();
                    }
                }
            }
            reg::HOST_CONTROL => self.host_control = value as u8,
            reg::CLOCK_CONTROL => self.clock_control = value as u16,
            reg::SOFTWARE_RESET => {
                // Reset all: clear the data engine.
                if value & 1 != 0 || value & 4 != 0 {
                    self.read_buffer.clear();
                    self.read_at = 0;
                    self.write_buffer.clear();
                    self.blocks_left = 0;
                    self.writing = false;
                    self.interrupt_status = 0;
                }
                if value & 2 != 0 {
                    self.command = 0;
                    self.argument = 0;
                }
                self.update_irq(ctx).ok();
            }
            reg::INTERRUPT_STATUS => {
                // Write-one-to-clear.
                self.interrupt_status &= !(value as u32);
                self.update_irq(ctx).ok();
            }
            reg::INTERRUPT_ENABLE => self.interrupt_enable = value as u32,
            reg::SIGNAL_ENABLE => {
                self.signal_enable = value as u32;
                self.update_irq(ctx).ok();
            }
            reg::POWER_CONTROL
            | reg::TIMEOUT_CONTROL
            | reg::FORCE_EVENT
            | reg::AUTO_CMD12_ERROR => {}
            _ => match self.unmapped {
                UnmappedPolicy::RazWi => {}
                UnmappedPolicy::Fault => return Err(BusFault::Device),
            },
        }
        Ok(())
    }
}

impl EventTarget for Sdhci {
    fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
        Ok(())
    }
}

/// Checkpoint state (ADR 0004).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SdhciCheckpoint {
    pub schema: u32,
    pub block_size: u16,
    pub block_count: u16,
    pub argument: u32,
    pub transfer_mode: u16,
    pub command: u16,
    pub response: [u32; 4],
    pub host_control: u8,
    pub clock_control: u16,
    pub interrupt_status: u32,
    pub interrupt_enable: u32,
    pub signal_enable: u32,
    pub blocks_left: u16,
    pub writing: bool,
}

impl Snapshot for Sdhci {
    type State = SdhciCheckpoint;
    const SCHEMA_VERSION: u32 = 1;

    fn save(&self) -> Self::State {
        Self::State {
            schema: Self::SCHEMA_VERSION,
            block_size: self.block_size,
            block_count: self.block_count,
            argument: self.argument,
            transfer_mode: self.transfer_mode,
            command: self.command,
            response: self.response,
            host_control: self.host_control,
            clock_control: self.clock_control,
            interrupt_status: self.interrupt_status,
            interrupt_enable: self.interrupt_enable,
            signal_enable: self.signal_enable,
            blocks_left: self.blocks_left,
            writing: self.writing,
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
        self.block_size = state.block_size;
        self.block_count = state.block_count;
        self.argument = state.argument;
        self.transfer_mode = state.transfer_mode;
        self.command = state.command;
        self.response = state.response;
        self.host_control = state.host_control;
        self.clock_control = state.clock_control;
        self.interrupt_status = state.interrupt_status;
        self.interrupt_enable = state.interrupt_enable;
        self.signal_enable = state.signal_enable;
        self.blocks_left = state.blocks_left;
        self.writing = state.writing;
        Ok(())
    }

    fn fingerprint(version: u32, state: &Self::State) -> u64 {
        let mut hash = Fnv64::default();
        hash.bytes(&version.to_le_bytes());
        hash.bytes(&state.block_size.to_le_bytes());
        hash.bytes(&state.argument.to_le_bytes());
        hash.bytes(&state.interrupt_status.to_le_bytes());
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

    struct TestCtx {
        signals: Vec<(SignalId, bool)>,
    }

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

        fn set_signal(&mut self, signal: SignalId, high: bool) -> Result<(), ContextError> {
            self.signals.push((signal, high));
            Ok(())
        }
    }

    fn attr() -> Attr {
        Attr {
            master: NodeId(0),
            security: Security::NonSecure,
            privileged: true,
            kind: AccessKind::Data,
            exclusive: false,
            byte_enable: 0,
            endian: Endian::Little,
        }
    }

    fn host_with_card() -> Sdhci {
        let mut host = Sdhci::new(SignalId(9));
        let identity = CardIdentity {
            cid: [0; 16],
            csd: [0; 16],
        };
        host.attach_card(card_handle(Box::new(SdCard::sd(
            vec![0xAB; 512 * 8],
            identity,
        ))));
        host
    }

    fn issue(host: &mut Sdhci, ctx: &mut dyn Ctx, index: u16, argument: u32) {
        host.write(
            ctx,
            reg::ARGUMENT,
            AccessSize::Word,
            u64::from(argument),
            &attr(),
        )
        .unwrap();
        host.write(
            ctx,
            reg::COMMAND,
            AccessSize::HalfWord,
            u64::from((index << 8) | 0x1A),
            &attr(),
        )
        .unwrap();
    }

    #[test]
    fn a_card_is_present_in_the_state_register() {
        let mut host = host_with_card();
        let mut ctx = TestCtx {
            signals: Vec::new(),
        };
        let state = host
            .read(&mut ctx, reg::PRESENT_STATE, AccessSize::Word, &attr())
            .unwrap() as u32;
        assert_eq!(state & state::CARD_INSERTED, state::CARD_INSERTED);
    }

    #[test]
    fn cmd17_fills_the_buffer_and_pio_drains_it() {
        let mut host = host_with_card();
        let mut ctx = TestCtx {
            signals: Vec::new(),
        };
        host.write(
            &mut ctx,
            reg::BLOCK_SIZE,
            AccessSize::HalfWord,
            512,
            &attr(),
        )
        .unwrap();
        host.write(
            &mut ctx,
            reg::INTERRUPT_ENABLE,
            AccessSize::Word,
            0xFFFF,
            &attr(),
        )
        .unwrap();
        host.write(
            &mut ctx,
            reg::SIGNAL_ENABLE,
            AccessSize::Word,
            0xFFFF,
            &attr(),
        )
        .unwrap();
        issue(&mut host, &mut ctx, 17, 0);
        assert!(host.interrupt_status & irq::COMMAND_COMPLETE != 0);
        assert!(host.interrupt_status & irq::BUFFER_READ_READY != 0);
        let state = host
            .read(&mut ctx, reg::PRESENT_STATE, AccessSize::Word, &attr())
            .unwrap() as u32;
        assert_eq!(state & state::BUFFER_READ_ENABLE, state::BUFFER_READ_ENABLE);
        // Drain 128 dwords = 512 bytes.
        let mut bytes = 0;
        while bytes < 512 {
            host.read(&mut ctx, reg::BUFFER_DATA_PORT, AccessSize::Word, &attr())
                .unwrap();
            bytes += 4;
        }
        assert!(host.interrupt_status & irq::TRANSFER_COMPLETE != 0);
    }

    #[test]
    fn a_reset_clears_the_data_engine() {
        let mut host = host_with_card();
        let mut ctx = TestCtx {
            signals: Vec::new(),
        };
        host.write(
            &mut ctx,
            reg::BLOCK_SIZE,
            AccessSize::HalfWord,
            512,
            &attr(),
        )
        .unwrap();
        issue(&mut host, &mut ctx, 17, 0);
        host.write(
            &mut ctx,
            reg::SOFTWARE_RESET,
            AccessSize::Byte,
            0x4,
            &attr(),
        )
        .unwrap();
        assert!(
            host.read(&mut ctx, reg::BUFFER_DATA_PORT, AccessSize::Word, &attr())
                .unwrap()
                == 0
        );
        assert!(host.interrupt_status == 0);
    }
}
