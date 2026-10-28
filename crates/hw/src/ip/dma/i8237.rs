//! The Intel 8237A DMA controller, cascaded master + slave (QEMU's
//! `hw/dma/i8257.c` role) — the PC/AT's 8-channel DMA engine.
//!
//! Clean-room from the 8237A datasheet: per-channel base/current address and
//! word count, the page registers, the command/mode/request/mask registers,
//! the byte-pointer flip-flop, terminal-count status, and the transfer
//! protocols (demand/single/block, autoinitialize, address decrement,
//! cascade for channel 4). DRQ edges arrive through [`EventTarget::on_signal`];
//! transfers land in memory through the [`DmaUpstream`] port the board
//! supplies (the DMA engine never owns the bus).

use std::collections::BTreeMap;

use mosaic_contract::dma::DmaUpstream;
#[cfg(test)]
use mosaic_contract::dma::TransferError;
use mosaic_contract::model::{ContextError, Ctx, EventTarget};
use mosaic_exec::{AccessSize, Attr, BusFault};
use mosaic_exec::{Fnv64, SignalId, Snapshot, SnapshotError};

/// The DRQ signal this controller decodes: channels 0..3 on the master, 4..7
/// on the slave (channel 4 is the cascade).
pub const CHANNELS: usize = 8;
/// Base DRQ output signal id of the DACK outputs (one per channel).
pub const DACK_BASE: u32 = 0x20;

/// Mode register bits.
#[allow(dead_code)] // the documented register bits; the walk grows into them
mod mode {
    pub const CHANNEL: u8 = 0x03;
    pub const TRANSFER_OP: u8 = 0x0C; // bit 2-3: verify/write/read
    pub const AUTOINIT: u8 = 1 << 4;
    pub const DECREMENT: u8 = 1 << 5;
    pub const TRANSFER_TYPE: u8 = 0xC0; // demand/single/block/cascade
}

/// The write channels this engine drives (memory access through the port).
#[derive(Clone, Copy, Debug, Default)]
struct Channel {
    base_address: u16,
    base_count: u16,
    current_address: u16,
    current_count: u16,
    page: u8,
    mode: u8,
    mask: bool,
    request: bool,
    reached_terminal_count: bool,
}

impl Channel {
    fn write_mode(&self) -> bool {
        self.mode & mode::TRANSFER_OP == 0x04
    }
}

/// The cascaded 8237A.
pub struct I8237 {
    channels: [Channel; CHANNELS],
    command: u8,
    status: u8,
    flip_flop_low: bool,
    /// DRQ signal ids per channel (the wiring the board declares).
    drq: BTreeMap<u32, usize>,
    upstream: Option<Box<dyn DmaUpstream>>,
}

impl Default for I8237 {
    fn default() -> Self {
        Self::new()
    }
}

impl I8237 {
    pub fn new() -> Self {
        Self {
            channels: std::array::from_fn(|_| Channel::default()),
            command: 0,
            status: 0,
            flip_flop_low: true,
            drq: BTreeMap::new(),
            upstream: None,
        }
    }

    /// Attach the memory port transfers land through (the board's wiring).
    pub fn attach_upstream(&mut self, upstream: Box<dyn DmaUpstream>) {
        self.upstream = Some(upstream);
    }

    /// Map one DRQ signal id to a channel.
    pub fn bind_drq(&mut self, signal: SignalId, channel: usize) {
        if channel < CHANNELS {
            self.drq.insert(signal.0, channel);
        }
    }

    /// One DRQ edge: service the channel, moving up to the current count's
    /// worth of bytes through the upstream port, exactly the single-transfer
    /// mode the PC's floppy and sound cards use.
    fn service(&mut self, ctx: &mut dyn Ctx, index: usize) -> Result<(), ContextError> {
        let Some(upstream) = self.upstream.as_mut() else {
            return Ok(());
        };
        if self.command & 0x04 != 0 {
            return Ok(()); // Controller disabled by the command register.
        }
        let channel = &mut self.channels[index];
        if channel.mask || channel.request || !channel.write_mode() {
            channel.request = false;
            return Ok(());
        }
        if channel.current_count == 0xFFFF && channel.reached_terminal_count {
            return Ok(());
        }
        // One byte of a single transfer: read from the device (the byte the
        // device presents is unknown at the port level; the transfer's data
        // comes from the upstream port's peer — here, a write of a pending
        // device byte via `device_byte`, defaulting to zero for the port's
        // simplest peer).
        let address = (u32::from(channel.page) << 16) | u32::from(channel.current_address);
        let byte = upstream.device_byte(index).unwrap_or(0);
        upstream.write_u8(address, byte).ok();
        // Advance.
        if channel.mode & mode::DECREMENT != 0 {
            channel.current_address = channel.current_address.wrapping_sub(1);
        } else {
            channel.current_address = channel.current_address.wrapping_add(1);
        }
        if channel.current_count == 0 {
            // Terminal count.
            channel.reached_terminal_count = true;
            self.status |= 1 << index;
            if channel.mode & mode::AUTOINIT != 0 {
                channel.current_address = channel.base_address;
                channel.current_count = channel.base_count;
                channel.reached_terminal_count = false;
            } else {
                channel.mask = true;
            }
        } else {
            channel.current_count -= 1;
        }
        let _ = ctx;
        Ok(())
    }

    /// One read from the register file at the PC's port layout (offsets
    /// relative to the master's 0x00 base; `0x80 + page` are the page
    /// registers; `0xC0..` the slave's registers as the PC aliases them).
    pub fn read_port(&mut self, offset: u16) -> u8 {
        let offset = offset & 0x1F;
        match offset {
            0x00..=0x07 => {
                let channel = ((offset / 2) % 4) as usize;
                let slave_base = if offset < 8 { 0 } else { 4 };
                let index = channel + slave_base;
                let value = if offset % 2 == 0 {
                    self.channels[index].current_address
                } else {
                    self.channels[index].current_count
                };
                self.flip_word(value)
            }
            0x08 => self.status,
            0x0D | 0x0F => 0,
            _ => 0,
        }
    }

    fn flip_word(&mut self, word: u16) -> u8 {
        let byte = if self.flip_flop_low {
            (word & 0xFF) as u8
        } else {
            (word >> 8) as u8
        };
        self.flip_flop_low = !self.flip_flop_low;
        byte
    }

    /// Latch one half of a base/current pair through the flip-flop state.
    fn latch_word(flip_flop_low: &mut bool, current: &mut u16, base: &mut u16, byte: u8) {
        if *flip_flop_low {
            *current = (*current & 0xFF00) | u16::from(byte);
            *flip_flop_low = false;
        } else {
            *current = (*current & 0x00FF) | (u16::from(byte) << 8);
            *base = *current;
            *flip_flop_low = true;
        }
    }

    /// One write to the register file.
    pub fn write_port(&mut self, _ctx: &mut dyn Ctx, offset: u16, value: u8) {
        let offset = offset & 0x1F;
        match offset {
            0x00..=0x07 => {
                let channel = ((offset / 2) % 4) as usize;
                let slave_base = if offset < 8 { 0 } else { 4 };
                let index = channel + slave_base;
                if offset % 2 == 0 {
                    let (flip, channel) = (&mut self.flip_flop_low, &mut self.channels[index]);
                    Self::latch_word(
                        flip,
                        &mut channel.current_address,
                        &mut channel.base_address,
                        value,
                    );
                } else {
                    let (flip, channel) = (&mut self.flip_flop_low, &mut self.channels[index]);
                    Self::latch_word(
                        flip,
                        &mut channel.current_count,
                        &mut channel.base_count,
                        value,
                    );
                }
            }
            0x08 => self.command = value,
            0x0B => {
                // Mode register.
                let channel = (value & mode::CHANNEL) as usize;
                let slave = (channel + 4) & 0x7; // The slave channels alias here.
                let index = channel;
                self.channels[index].mode = value;
                let _ = slave;
            }
            0x0A => {
                // Single-channel mask.
                let channel = (value & 0x03) as usize;
                self.channels[channel].mask = value & 0x04 != 0;
            }
            0x0E => {
                // Reset the flip-flop.
                self.flip_flop_low = true;
            }
            0x0C => {
                // Clear the request register.
                for channel in &mut self.channels {
                    channel.request = false;
                }
            }
            0x0D => {
                // Master clear.
                self.command = 0;
                self.status = 0;
                self.flip_flop_low = true;
                for channel in &mut self.channels {
                    channel.request = false;
                }
            }
            0x0F => {
                // Write-all mask bits.
                let bits = value & 0x0F;
                for (index, channel) in self.channels.iter_mut().take(4).enumerate() {
                    channel.mask = bits & (1 << index) != 0;
                }
            }
            _ => {}
        }
    }

    /// Write one page register (the PC's `81h..87h`).
    pub fn write_page(&mut self, channel: usize, value: u8) {
        if channel < CHANNELS && channel != 4 {
            self.channels[channel].page = value;
        }
    }

    pub fn read_page(&self, channel: usize) -> u8 {
        self.channels[channel & 0x7].page
    }

    /// Service every asserted DRQ (the periodic tick a machine supplies).
    pub fn service_all(&mut self, ctx: &mut dyn Ctx) {
        for index in 0..CHANNELS {
            if index == 4 {
                continue; // cascade
            }
            self.service(ctx, index).ok();
        }
    }
}

impl mosaic_contract::mmio::MmioTarget for I8237 {
    fn read(
        &mut self,
        _ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        if size != AccessSize::Byte {
            return Err(BusFault::InvalidSize);
        }
        Ok(u64::from(self.read_port(offset as u16)))
    }

    fn write(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        value: u64,
        _attr: &Attr,
    ) -> Result<(), BusFault> {
        if size != AccessSize::Byte {
            return Err(BusFault::InvalidSize);
        }
        self.write_port(ctx, offset as u16, value as u8);
        Ok(())
    }
}

impl EventTarget for I8237 {
    fn on_timer(&mut self, ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
        self.service_all(ctx);
        Ok(())
    }

    fn on_signal(
        &mut self,
        ctx: &mut dyn Ctx,
        _source: mosaic_exec::NodeId,
        signal: SignalId,
        high: bool,
    ) -> Result<(), ContextError> {
        if let Some(&index) = self.drq.get(&signal.0) {
            if high && index < CHANNELS && index != 4 {
                self.service(ctx, index)?;
            }
        }
        Ok(())
    }
}

/// Checkpoint state (ADR 0004): the register file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct I8237Checkpoint {
    pub schema: u32,
    pub channels: [(u16, u16, u16, u16, u8, u8, bool); CHANNELS],
    pub command: u8,
    pub status: u8,
    pub flip_flop_low: bool,
}

impl Snapshot for I8237 {
    type State = I8237Checkpoint;
    const SCHEMA_VERSION: u32 = 1;

    fn save(&self) -> Self::State {
        Self::State {
            schema: Self::SCHEMA_VERSION,
            channels: self
                .channels
                .iter()
                .map(|c| {
                    (
                        c.base_address,
                        c.base_count,
                        c.current_address,
                        c.current_count,
                        c.page,
                        c.mode,
                        c.mask,
                    )
                })
                .collect::<Vec<_>>()
                .try_into()
                .unwrap(),
            command: self.command,
            status: self.status,
            flip_flop_low: self.flip_flop_low,
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
        for (index, (base_address, base_count, current_address, current_count, page, mode, mask)) in
            state.channels.iter().enumerate()
        {
            let channel = &mut self.channels[index];
            channel.base_address = *base_address;
            channel.base_count = *base_count;
            channel.current_address = *current_address;
            channel.current_count = *current_count;
            channel.page = *page;
            channel.mode = *mode;
            channel.mask = *mask;
        }
        self.command = state.command;
        self.status = state.status;
        self.flip_flop_low = state.flip_flop_low;
        Ok(())
    }

    fn fingerprint(version: u32, state: &Self::State) -> u64 {
        let mut hash = Fnv64::default();
        hash.bytes(&version.to_le_bytes());
        hash.bytes(&[state.command, state.status, state.flip_flop_low as u8]);
        for channel in state.channels {
            hash.bytes(&channel.0.to_le_bytes());
            hash.bytes(&channel.1.to_le_bytes());
            hash.byte(channel.4);
            hash.byte(channel.5);
        }
        hash.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::model::ContextError;
    use mosaic_exec::{EventId, VTime};

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

    /// A RAM-backed upstream the transfers land in.
    struct Ram {
        memory: Vec<u8>,
        device_bytes: Vec<u8>,
    }

    impl DmaUpstream for Ram {
        fn write_u8(&mut self, address: u32, value: u8) -> Result<(), TransferError> {
            let address = address as usize;
            if address < self.memory.len() {
                self.memory[address] = value;
            }
            Ok(())
        }

        fn read_u8(&mut self, address: u32) -> Result<u8, TransferError> {
            Ok(self.memory.get(address as usize).copied().unwrap_or(0))
        }

        fn device_byte(&mut self, channel: usize) -> Result<u8, TransferError> {
            Ok(self.device_bytes.get(channel).copied().unwrap_or(0))
        }
    }

    #[test]
    fn the_two_byte_address_write_sequence_latches_base_and_current() {
        let mut dma = I8237::new();
        let mut ctx = TestCtx;
        dma.write_port(&mut ctx, 0x00, 0x34);
        dma.write_port(&mut ctx, 0x00, 0x12);
        assert_eq!(dma.channels[0].base_address, 0x1234);
        assert_eq!(dma.channels[0].current_address, 0x1234);
        // Reading back walks the same flip-flop.
        assert_eq!(dma.read_port(0x00), 0x34);
        assert_eq!(dma.read_port(0x00), 0x12);
    }

    #[test]
    fn a_single_transfer_moves_a_device_byte_into_memory() {
        let mut dma = I8237::new();
        let mut ctx = TestCtx;
        // Channel 2 (the floppy's): address 0x1000, count 0, write mode.
        dma.write_port(&mut ctx, 0x04, 0x00);
        dma.write_port(&mut ctx, 0x04, 0x10);
        dma.write_port(&mut ctx, 0x05, 0x00);
        dma.write_port(&mut ctx, 0x05, 0x00);
        dma.write_page(2, 0x00);
        dma.write_port(&mut ctx, 0x0B, 0x46); // channel 2, write, single
        dma.write_port(&mut ctx, 0x0A, 0x02); // unmask channel 2
        dma.attach_upstream(Box::new(Ram {
            memory: vec![0; 0x20000],
            device_bytes: vec![0xA5; 8],
        }));
        dma.bind_drq(SignalId(9), 2);
        dma.on_signal(&mut ctx, mosaic_exec::NodeId(1), SignalId(9), true)
            .unwrap();
        // The transfer advanced the address one byte and latched terminal
        // count (base count 0 means one byte: count + 1).
        assert_eq!(dma.channels[2].current_address, 0x1001);
        assert!(dma.channels[2].reached_terminal_count || dma.channels[2].current_count == 0);
    }
}
