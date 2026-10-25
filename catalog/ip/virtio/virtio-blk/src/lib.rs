#![forbid(unsafe_code)]
//! VirtIO MMIO Block Device model.

use mosaic_contract_mmio::MmioTarget;
use mosaic_contract_model::{ContextError, Ctx, EventTarget};
use mosaic_core_types::{AccessSize, Attr, BusFault, SignalId, Snapshot, SnapshotError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VirtioBlkState {
    pub schema: u32,
    pub device_features: u32,
    pub driver_features: u32,
    pub queue_sel: u32,
    pub queue_num: u32,
    pub queue_ready: u32,
    pub interrupt_status: u32,
    pub status: u32,
    pub capacity_sectors: u64,
}

pub struct VirtioBlk {
    state: VirtioBlkState,
    disk_data: Vec<u8>,
}

impl VirtioBlk {
    pub fn new() -> Self {
        Self {
            state: VirtioBlkState {
                schema: 1,
                device_features: 1, // VIRTIO_BLK_F_SIZE_MAX
                driver_features: 0,
                queue_sel: 0,
                queue_num: 128,
                queue_ready: 0,
                interrupt_status: 0,
                status: 0,
                capacity_sectors: 0,
            },
            disk_data: Vec::new(),
        }
    }

    pub fn attach_disk(&mut self, data: Vec<u8>) {
        let size = data.len() as u64;
        self.state.capacity_sectors = size / 512;
        self.disk_data = data;
    }
}

impl Default for VirtioBlk {
    fn default() -> Self {
        Self::new()
    }
}

impl MmioTarget for VirtioBlk {
    fn read(
        &mut self,
        _ctx: &mut dyn Ctx,
        offset: u64,
        _size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        let val = match offset {
            0x000 => 0x74726976, // Magic "virt"
            0x004 => 2,          // Version 2 (v1.1)
            0x008 => 2,          // Device ID 2 = Block
            0x00C => 0x554D4551, // Vendor ID
            0x010 => u64::from(self.state.device_features),
            0x034 => 128, // Max queue size
            0x044 => u64::from(self.state.queue_ready),
            0x060 => u64::from(self.state.interrupt_status),
            0x070 => u64::from(self.state.status),
            0x100 => self.state.capacity_sectors & 0xFFFFFFFF,
            0x104 => (self.state.capacity_sectors >> 32) & 0xFFFFFFFF,
            _ => 0,
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
        let val = value as u32;
        match offset {
            0x020 => self.state.driver_features = val,
            0x030 => self.state.queue_sel = val,
            0x038 => self.state.queue_num = val,
            0x044 => self.state.queue_ready = val,
            0x050 => {
                // Queue notify
                self.state.interrupt_status |= 1;
                let _ = ctx.set_signal(SignalId(0), true);
            }
            0x064 => {
                // Interrupt ACK
                self.state.interrupt_status &= !val;
                if self.state.interrupt_status == 0 {
                    let _ = ctx.set_signal(SignalId(0), false);
                }
            }
            0x070 => self.state.status = val,
            _ => {}
        }
        Ok(())
    }
}

impl EventTarget for VirtioBlk {
    fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
        Ok(())
    }
}

impl Snapshot for VirtioBlk {
    type State = VirtioBlkState;
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
        hash.u64(state.capacity_sectors);
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
    fn virtio_blk_magic_and_capacity() {
        let mut virtio = VirtioBlk::new();
        virtio.attach_disk(vec![0; 1024 * 1024]); // 1MB = 2048 sectors
        let mut dummy = DummyCtx;
        let attr = test_attr();
        let magic = virtio
            .read(&mut dummy, 0x000, AccessSize::Word, &attr)
            .unwrap();
        assert_eq!(magic, 0x74726976);
        let cap = virtio
            .read(&mut dummy, 0x100, AccessSize::Word, &attr)
            .unwrap();
        assert_eq!(cap, 2048);
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
