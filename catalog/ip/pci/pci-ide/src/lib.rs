#![forbid(unsafe_code)]
//! PCI IDE / ATA Storage Controller model supporting disk image and ISO attachment.

use mosaic_contract_mmio::MmioTarget;
use mosaic_contract_model::{ContextError, Ctx, EventTarget};
use mosaic_core_types::{AccessSize, Attr, BusFault, SignalId, Snapshot, SnapshotError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdeDrive {
    pub is_atapi: bool,
    pub image_size: u64,
    pub buffer: Vec<u8>,
}

impl Default for IdeDrive {
    fn default() -> Self {
        Self {
            is_atapi: false,
            image_size: 0,
            buffer: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdeChannelState {
    pub feature: u8,
    pub sector_count: u8,
    pub lba_low: u8,
    pub lba_mid: u8,
    pub lba_high: u8,
    pub drive_head: u8,
    pub status: u8,
    pub error: u8,
    pub data_buffer: Vec<u8>,
    pub data_pos: usize,
}

impl Default for IdeChannelState {
    fn default() -> Self {
        Self {
            feature: 0,
            sector_count: 1,
            lba_low: 0,
            lba_mid: 0,
            lba_high: 0,
            drive_head: 0xA0,
            status: 0x40, // DRIVE READY
            error: 0,
            data_buffer: Vec::new(),
            data_pos: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PciIdeState {
    pub schema: u32,
    pub channels: [IdeChannelState; 2],
}

pub struct PciIde {
    state: PciIdeState,
    drives: [Option<IdeDrive>; 4], // Primary Master, Primary Slave, Secondary Master, Secondary Slave
}

impl PciIde {
    pub fn new() -> Self {
        Self {
            state: PciIdeState {
                schema: 1,
                channels: [IdeChannelState::default(), IdeChannelState::default()],
            },
            drives: [None, None, None, None],
        }
    }

    pub fn attach_drive(&mut self, drive_idx: usize, is_atapi: bool, image_data: Vec<u8>) {
        if drive_idx < 4 {
            let size = image_data.len() as u64;
            self.drives[drive_idx] = Some(IdeDrive {
                is_atapi,
                image_size: size,
                buffer: image_data,
            });
        }
    }

    fn selected_drive_index(&self, ch: usize) -> usize {
        let slave = (self.state.channels[ch].drive_head & 0x10) != 0;
        ch * 2 + if slave { 1 } else { 0 }
    }

    fn write_cmd(&mut self, ctx: &mut dyn Ctx, ch: usize, cmd: u8) {
        let drive_idx = self.selected_drive_index(ch);
        let channel = &mut self.state.channels[ch];
        channel.status = 0x80; // BUSY

        match cmd {
            0xEC => {
                // IDENTIFY DRIVE
                if let Some(ref drive) = self.drives[drive_idx] {
                    if !drive.is_atapi {
                        let mut id = vec![0u8; 512];
                        // Write ATA identify data
                        id[0..2].copy_from_slice(&0x0040u16.to_le_bytes()); // Fixed drive
                        let sectors = drive.image_size / 512;
                        id[120..124].copy_from_slice(&(sectors as u32).to_le_bytes());
                        channel.data_buffer = id;
                        channel.data_pos = 0;
                        channel.status = 0x58; // READY | DATA REQUEST
                    } else {
                        channel.status = 0x01; // ERROR
                        channel.error = 0x04; // ABRT
                    }
                } else {
                    channel.status = 0x01; // ERROR
                    channel.error = 0x04; // ABRT
                }
            }
            0xA1 => {
                // IDENTIFY PACKET DEVICE (ATAPI)
                if let Some(ref drive) = self.drives[drive_idx] {
                    if drive.is_atapi {
                        let mut id = vec![0u8; 512];
                        id[0..2].copy_from_slice(&0x8580u16.to_le_bytes()); // ATAPI CD-ROM
                        channel.data_buffer = id;
                        channel.data_pos = 0;
                        channel.status = 0x58; // READY | DATA REQUEST
                    } else {
                        channel.status = 0x01; // ERROR
                        channel.error = 0x04; // ABRT
                    }
                } else {
                    channel.status = 0x01; // ERROR
                    channel.error = 0x04; // ABRT
                }
            }
            0x20 | 0x24 => {
                // READ SECTOR(S)
                let lba = (u64::from(channel.lba_high) << 16)
                    | (u64::from(channel.lba_mid) << 8)
                    | u64::from(channel.lba_low);
                let count = if channel.sector_count == 0 {
                    256u64
                } else {
                    u64::from(channel.sector_count)
                };

                if let Some(ref drive) = self.drives[drive_idx] {
                    let start = (lba * 512) as usize;
                    let end = std::cmp::min(start + (count * 512) as usize, drive.buffer.len());
                    if start < drive.buffer.len() {
                        channel.data_buffer = drive.buffer[start..end].to_vec();
                    } else {
                        channel.data_buffer = vec![0u8; (count * 512) as usize];
                    }
                    channel.data_pos = 0;
                    channel.status = 0x58; // READY | DATA REQUEST
                } else {
                    channel.status = 0x01; // ERROR
                    channel.error = 0x04; // ABRT
                }
            }
            _ => {
                channel.status = 0x40; // READY
            }
        }

        let irq_sig = SignalId(ch as u32);
        let _ = ctx.set_signal(irq_sig, true);
    }

    fn read_data_word(&mut self, ch: usize) -> u16 {
        let channel = &mut self.state.channels[ch];
        if channel.data_pos + 2 <= channel.data_buffer.len() {
            let b = [
                channel.data_buffer[channel.data_pos],
                channel.data_buffer[channel.data_pos + 1],
            ];
            channel.data_pos += 2;
            if channel.data_pos >= channel.data_buffer.len() {
                channel.status = 0x40; // READY (DRQ cleared)
            }
            u16::from_le_bytes(b)
        } else {
            0
        }
    }
}

impl Default for PciIde {
    fn default() -> Self {
        Self::new()
    }
}

impl MmioTarget for PciIde {
    fn read(
        &mut self,
        _ctx: &mut dyn Ctx,
        offset: u64,
        _size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        let ch = if offset >= 0x10 { 1 } else { 0 };
        let reg = offset & 0x0F;
        let val = match reg {
            0x00 => u64::from(self.read_data_word(ch)),
            0x01 => u64::from(self.state.channels[ch].error),
            0x02 => u64::from(self.state.channels[ch].sector_count),
            0x03 => u64::from(self.state.channels[ch].lba_low),
            0x04 => u64::from(self.state.channels[ch].lba_mid),
            0x05 => u64::from(self.state.channels[ch].lba_high),
            0x06 => u64::from(self.state.channels[ch].drive_head),
            0x07 => u64::from(self.state.channels[ch].status),
            _ => 0xFF,
        };
        Ok(val)
    }

    fn write(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        _size: AccessSize,
        value: u64,
        _attr: &Attr,
    ) -> Result<(), BusFault> {
        let ch = if offset >= 0x10 { 1 } else { 0 };
        let reg = offset & 0x0F;
        let val = value as u8;
        match reg {
            0x01 => self.state.channels[ch].feature = val,
            0x02 => self.state.channels[ch].sector_count = val,
            0x03 => self.state.channels[ch].lba_low = val,
            0x04 => self.state.channels[ch].lba_mid = val,
            0x05 => self.state.channels[ch].lba_high = val,
            0x06 => self.state.channels[ch].drive_head = val,
            0x07 => self.write_cmd(ctx, ch, val),
            _ => {}
        }
        Ok(())
    }
}

impl EventTarget for PciIde {
    fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
        Ok(())
    }
}

impl Snapshot for PciIde {
    type State = PciIdeState;
    const SCHEMA_VERSION: u32 = 1;

    fn save(&self) -> Self::State {
        self.state.clone()
    }

    fn validate(&self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        if version != Self::SCHEMA_VERSION || state.schema != Self::SCHEMA_VERSION {
            return Err(SnapshotError::VersionMismatch);
        }
        Ok(())
    }

    fn load(&mut self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        self.validate(version, state)?;
        self.state = state.clone();
        Ok(())
    }

    fn fingerprint(version: u32, state: &Self::State) -> u64 {
        let mut hash = mosaic_core_types::Fnv64::new();
        hash.u32(version);
        hash.u32(state.schema);
        hash.byte(state.channels[0].status);
        hash.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_core_types::{AccessKind, Endian, NodeId, Security};

    fn test_attr() -> Attr {
        Attr {
            master: NodeId(0),
            security: Security::NonSecure,
            privileged: false,
            kind: AccessKind::Data,
            exclusive: false,
            byte_enable: 0xFF,
            endian: Endian::Little,
        }
    }

    #[test]
    fn ide_identify_drive() {
        let mut ide = PciIde::new();
        ide.attach_drive(0, false, vec![0; 1024]);
        let mut dummy = DummyCtx;
        let attr = test_attr();
        // Select master drive
        ide.write(&mut dummy, 0x06, AccessSize::Byte, 0xA0, &attr)
            .unwrap();
        // IDENTIFY DRIVE
        ide.write(&mut dummy, 0x07, AccessSize::Byte, 0xEC, &attr)
            .unwrap();
        assert_eq!(ide.state.channels[0].status, 0x58);
        let word = ide
            .read(&mut dummy, 0x00, AccessSize::HalfWord, &attr)
            .unwrap();
        assert_eq!(word, 0x0040);
    }

    struct DummyCtx;
    impl Ctx for DummyCtx {
        fn now(&self) -> mosaic_core_types::VTime {
            mosaic_core_types::VTime::ZERO
        }
        fn schedule(
            &mut self,
            _at: mosaic_core_types::VTime,
            _token: u64,
        ) -> Result<mosaic_core_types::EventId, ContextError> {
            Err(ContextError::ResourceExhausted)
        }
        fn cancel(&mut self, _event: mosaic_core_types::EventId) -> bool {
            false
        }
        fn set_signal(&mut self, _signal: SignalId, _high: bool) -> Result<(), ContextError> {
            Ok(())
        }
    }
}
