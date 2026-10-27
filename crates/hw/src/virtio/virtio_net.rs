//! VirtIO Network Device model.

use mosaic_contract::mmio::MmioTarget;
use mosaic_contract::model::{ContextError, Ctx, EventTarget};
use mosaic_exec::{AccessSize, Attr, BusFault, SignalId, Snapshot, SnapshotError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VirtioNetState {
    pub schema: u32,
    pub device_features: u32,
    pub driver_features: u32,
    pub queue_sel: u32,
    pub queue_num: u32,
    pub queue_ready: u32,
    pub interrupt_status: u32,
    pub status: u32,
    pub mac_address: [u8; 6],
}

pub struct VirtioNet {
    state: VirtioNetState,
}

impl VirtioNet {
    pub fn new() -> Self {
        Self {
            state: VirtioNetState {
                schema: 1,
                device_features: 1 << 5, // VIRTIO_NET_F_MAC
                driver_features: 0,
                queue_sel: 0,
                queue_num: 256,
                queue_ready: 0,
                interrupt_status: 0,
                status: 0,
                mac_address: [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
            },
        }
    }
}

impl Default for VirtioNet {
    fn default() -> Self {
        Self::new()
    }
}

impl MmioTarget for VirtioNet {
    fn read(
        &mut self,
        _ctx: &mut dyn Ctx,
        offset: u64,
        _size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        let val = match offset {
            0x000 => 0x74726976, // Magic "virt"
            0x004 => 2,          // Version 2
            0x008 => 1,          // Device ID 1 = Network
            0x00C => 0x554D4551, // Vendor ID
            0x010 => u64::from(self.state.device_features),
            0x034 => 256,
            0x044 => u64::from(self.state.queue_ready),
            0x060 => u64::from(self.state.interrupt_status),
            0x070 => u64::from(self.state.status),
            0x100..=0x105 => {
                let idx = (offset - 0x100) as usize;
                u64::from(self.state.mac_address[idx])
            }
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
                self.state.interrupt_status |= 1;
                let _ = ctx.set_signal(SignalId(0), true);
            }
            0x064 => {
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

impl EventTarget for VirtioNet {
    fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
        Ok(())
    }
}

impl Snapshot for VirtioNet {
    type State = VirtioNetState;
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
        let mut hash = mosaic_exec::Fnv64::new();
        hash.u32(version);
        hash.u32(state.schema);
        hash.byte(state.mac_address[0]);
        hash.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_exec::{AccessKind, Endian, NodeId, Security};

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
    fn virtio_net_mac_read() {
        let mut net = VirtioNet::new();
        let mut dummy = DummyCtx;
        let attr = test_attr();
        let m0 = net
            .read(&mut dummy, 0x100, AccessSize::Byte, &attr)
            .unwrap();
        assert_eq!(m0, 0x52);
    }

    struct DummyCtx;
    impl Ctx for DummyCtx {
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
        fn set_signal(&mut self, _signal: SignalId, _high: bool) -> Result<(), ContextError> {
            Ok(())
        }
    }
}
