//! Generic PCI Host Bridge and Configuration Space Controller model.

use mosaic_contract::mmio::MmioTarget;
use mosaic_contract::model::{ContextError, Ctx, EventTarget};
use mosaic_exec::{AccessSize, Attr, BusFault, Snapshot, SnapshotError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PciDeviceHeader {
    pub bus: u8,
    pub dev: u8,
    pub func: u8,
    pub config: [u8; 256],
}

impl PciDeviceHeader {
    pub fn new(
        bus: u8,
        dev: u8,
        func: u8,
        vendor_id: u16,
        device_id: u16,
        class_code: u32,
    ) -> Self {
        let mut config = [0u8; 256];
        config[0..2].copy_from_slice(&vendor_id.to_le_bytes());
        config[2..4].copy_from_slice(&device_id.to_le_bytes());
        // Status & Command
        config[4..6].copy_from_slice(&0x0007u16.to_le_bytes()); // IO + Memory + BusMaster
        config[6..8].copy_from_slice(&0x0210u16.to_le_bytes()); // Fast back-to-back + Capabilities
                                                                // Class code
        config[8..11].copy_from_slice(&class_code.to_le_bytes()[0..3]);
        config[14] = 0x00; // Type 0 header
        Self {
            bus,
            dev,
            func,
            config,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PciHostState {
    pub schema: u32,
    pub config_addr: u32,
    pub devices: Vec<PciDeviceHeader>,
}

pub struct PciHost {
    state: PciHostState,
}

impl PciHost {
    pub fn new() -> Self {
        let mut host = Self {
            state: PciHostState {
                schema: 1,
                config_addr: 0,
                devices: Vec::new(),
            },
        };
        // Register host bridge itself at Bus 0, Dev 0, Func 0 (e.g. Intel i440FX Host)
        host.add_device(PciDeviceHeader::new(0, 0, 0, 0x8086, 0x1237, 0x060000));
        host
    }

    pub fn add_device(&mut self, dev: PciDeviceHeader) {
        self.state
            .devices
            .retain(|d| !(d.bus == dev.bus && d.dev == dev.dev && d.func == dev.func));
        self.state.devices.push(dev);
    }

    fn read_config(&self, bus: u8, dev: u8, func: u8, offset: u8, size: AccessSize) -> u32 {
        if let Some(target) = self
            .state
            .devices
            .iter()
            .find(|d| d.bus == bus && d.dev == dev && d.func == func)
        {
            let idx = offset as usize;
            match size {
                AccessSize::Byte => u32::from(target.config[idx]),
                AccessSize::HalfWord => {
                    let mut b = [0u8; 2];
                    b.copy_from_slice(&target.config[idx..idx + 2]);
                    u32::from(u16::from_le_bytes(b))
                }
                AccessSize::Word | AccessSize::DoubleWord => {
                    let mut b = [0u8; 4];
                    b.copy_from_slice(&target.config[idx..idx + 4]);
                    u32::from_le_bytes(b)
                }
            }
        } else {
            0xFFFFFFFF
        }
    }

    fn write_config(&mut self, bus: u8, dev: u8, func: u8, offset: u8, size: AccessSize, val: u32) {
        if let Some(target) = self
            .state
            .devices
            .iter_mut()
            .find(|d| d.bus == bus && d.dev == dev && d.func == func)
        {
            let idx = offset as usize;
            match size {
                AccessSize::Byte => {
                    target.config[idx] = val as u8;
                }
                AccessSize::HalfWord => {
                    let bytes = (val as u16).to_le_bytes();
                    target.config[idx..idx + 2].copy_from_slice(&bytes);
                }
                AccessSize::Word | AccessSize::DoubleWord => {
                    let bytes = val.to_le_bytes();
                    target.config[idx..idx + 4].copy_from_slice(&bytes);
                }
            }
        }
    }
}

impl Default for PciHost {
    fn default() -> Self {
        Self::new()
    }
}

impl MmioTarget for PciHost {
    fn read(
        &mut self,
        _ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        let val = match offset {
            0x00 => u64::from(self.state.config_addr),
            0x04..=0x07 => {
                if self.state.config_addr & 0x8000_0000 != 0 {
                    let bus = ((self.state.config_addr >> 16) & 0xFF) as u8;
                    let dev = ((self.state.config_addr >> 11) & 0x1F) as u8;
                    let func = ((self.state.config_addr >> 8) & 0x07) as u8;
                    let reg_off =
                        ((self.state.config_addr & 0xFC) + ((offset - 0x04) as u32)) as u8;
                    u64::from(self.read_config(bus, dev, func, reg_off, size))
                } else {
                    0xFFFFFFFF
                }
            }
            _ => 0xFFFFFFFF,
        };
        Ok(val)
    }

    fn write(
        &mut self,
        _ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        value: u64,
        _attr: &Attr,
    ) -> Result<(), BusFault> {
        let val = value as u32;
        match offset {
            0x00 => {
                self.state.config_addr = val;
            }
            0x04..=0x07 => {
                if self.state.config_addr & 0x8000_0000 != 0 {
                    let bus = ((self.state.config_addr >> 16) & 0xFF) as u8;
                    let dev = ((self.state.config_addr >> 11) & 0x1F) as u8;
                    let func = ((self.state.config_addr >> 8) & 0x07) as u8;
                    let reg_off =
                        ((self.state.config_addr & 0xFC) + ((offset - 0x04) as u32)) as u8;
                    self.write_config(bus, dev, func, reg_off, size, val);
                }
            }
            _ => {}
        }
        Ok(())
    }
}

impl EventTarget for PciHost {
    fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
        Ok(())
    }
}

impl Snapshot for PciHost {
    type State = PciHostState;
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
        hash.u32(state.config_addr);
        hash.usize(state.devices.len());
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
    fn pci_host_read_vendor_id() {
        let mut pci = PciHost::new();
        let mut dummy = DummyCtx;
        let attr = test_attr();
        // Select Bus 0, Dev 0, Func 0, Reg 0
        pci.write(&mut dummy, 0x00, AccessSize::Word, 0x8000_0000, &attr)
            .unwrap();
        let val = pci.read(&mut dummy, 0x04, AccessSize::Word, &attr).unwrap();
        // Host bridge: Vendor 0x8086, Device 0x1237 -> 0x12378086
        assert_eq!(val, 0x12378086);
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
        fn set_signal(
            &mut self,
            _signal: mosaic_exec::SignalId,
            _high: bool,
        ) -> Result<(), ContextError> {
            Ok(())
        }
    }
}
