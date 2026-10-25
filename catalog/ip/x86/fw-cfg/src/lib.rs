#![forbid(unsafe_code)]
//! Generic Firmware Configuration (fw_cfg) Interface.

use mosaic_contract_mmio::MmioTarget;
use mosaic_contract_model::{ContextError, Ctx, EventTarget};
use mosaic_core_types::{AccessSize, Attr, BusFault, Snapshot, SnapshotError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FwCfgState {
    pub schema: u32,
    pub select: u16,
    pub pos: usize,
    pub signature: [u8; 4], // "QEMU"
}

pub struct FwCfg {
    state: FwCfgState,
    files: std::collections::BTreeMap<u16, Vec<u8>>,
}

impl FwCfg {
    pub fn new() -> Self {
        let mut fw = Self {
            state: FwCfgState {
                schema: 1,
                select: 0,
                pos: 0,
                signature: *b"QEMU",
            },
            files: std::collections::BTreeMap::new(),
        };
        // Add signature file at 0x0000 ("QEMU")
        fw.add_file(0x0000, b"QEMU".to_vec());
        // Add ID file at 0x0001 (Feature bitmap)
        fw.add_file(0x0001, vec![0x03, 0x00, 0x00, 0x00]);
        fw
    }

    pub fn add_file(&mut self, key: u16, data: Vec<u8>) {
        self.files.insert(key, data);
    }

    fn read_data_byte(&mut self) -> u8 {
        if let Some(file) = self.files.get(&self.state.select) {
            if self.state.pos < file.len() {
                let b = file[self.state.pos];
                self.state.pos += 1;
                return b;
            }
        }
        0
    }
}

impl Default for FwCfg {
    fn default() -> Self {
        Self::new()
    }
}

impl MmioTarget for FwCfg {
    fn read(
        &mut self,
        _ctx: &mut dyn Ctx,
        offset: u64,
        _size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        let val = match offset {
            0x00 => u64::from(self.read_data_byte()),
            _ => 0,
        };
        Ok(val)
    }

    fn write(
        &mut self,
        _ctx: &mut dyn Ctx,
        offset: u64,
        _size: AccessSize,
        value: u64,
        _attr: &Attr,
    ) -> Result<(), BusFault> {
        if offset == 0x08 {
            self.state.select = (value as u16).swap_bytes(); // Selector is big-endian
            self.state.pos = 0;
        }
        Ok(())
    }
}

impl EventTarget for FwCfg {
    fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
        Ok(())
    }
}

impl Snapshot for FwCfg {
    type State = FwCfgState;
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
        hash.u16(state.select);
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
    fn fw_cfg_signature_read() {
        let mut fw = FwCfg::new();
        let mut dummy = DummyCtx;
        let attr = test_attr();
        // Select key 0x0000 (Signature)
        fw.write(&mut dummy, 0x08, AccessSize::HalfWord, 0x0000, &attr)
            .unwrap();
        let b0 = fw.read(&mut dummy, 0x00, AccessSize::Byte, &attr).unwrap() as u8;
        let b1 = fw.read(&mut dummy, 0x00, AccessSize::Byte, &attr).unwrap() as u8;
        let b2 = fw.read(&mut dummy, 0x00, AccessSize::Byte, &attr).unwrap() as u8;
        let b3 = fw.read(&mut dummy, 0x00, AccessSize::Byte, &attr).unwrap() as u8;
        assert_eq!(&[b0, b1, b2, b3], b"QEMU");
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
        fn set_signal(
            &mut self,
            _signal: mosaic_core_types::SignalId,
            _high: bool,
        ) -> Result<(), ContextError> {
            Ok(())
        }
    }
}
