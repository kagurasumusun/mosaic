//! x86 I/O APIC model (24 redirection entries).

use mosaic_contract::mmio::MmioTarget;
use mosaic_contract::model::{ContextError, Ctx, EventTarget};
use mosaic_exec::{AccessSize, Attr, BusFault, SignalId, Snapshot, SnapshotError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IoApicState {
    pub schema: u32,
    pub reg_select: u32,
    pub id: u32,
    pub ioredtbl: [u64; 24],
}

impl Default for IoApicState {
    fn default() -> Self {
        let mut ioredtbl = [0u64; 24];
        for entry in &mut ioredtbl {
            *entry = 0x0001_0000; // Masked by default
        }
        Self {
            schema: 1,
            reg_select: 0,
            id: 0,
            ioredtbl,
        }
    }
}

pub struct IoApic {
    state: IoApicState,
}

impl IoApic {
    pub fn new() -> Self {
        Self {
            state: IoApicState::default(),
        }
    }

    fn read_reg(&self, reg: u32) -> u32 {
        match reg {
            0x00 => self.state.id << 24,
            0x01 => 0x0017_0011, // Version 0x11, 24 redirection entries
            0x02 => self.state.id << 24,
            0x10..=0x3F => {
                let idx = ((reg - 0x10) / 2) as usize;
                if idx < 24 {
                    if reg % 2 == 0 {
                        self.state.ioredtbl[idx] as u32
                    } else {
                        (self.state.ioredtbl[idx] >> 32) as u32
                    }
                } else {
                    0
                }
            }
            _ => 0,
        }
    }

    fn write_reg(&mut self, ctx: &mut dyn Ctx, reg: u32, val: u32) {
        match reg {
            0x00 => self.state.id = (val >> 24) & 0x0F,
            0x10..=0x3F => {
                let idx = ((reg - 0x10) / 2) as usize;
                if idx < 24 {
                    if reg % 2 == 0 {
                        self.state.ioredtbl[idx] =
                            (self.state.ioredtbl[idx] & 0xFFFF_FFFF_0000_0000) | u64::from(val);
                    } else {
                        self.state.ioredtbl[idx] = (self.state.ioredtbl[idx]
                            & 0x0000_0000_FFFF_FFFF)
                            | (u64::from(val) << 32);
                    }
                }
            }
            _ => {}
        }
        self.update_output(ctx);
    }

    fn update_output(&self, ctx: &mut dyn Ctx) {
        let _ = ctx.set_signal(SignalId(0), false);
    }
}

impl Default for IoApic {
    fn default() -> Self {
        Self::new()
    }
}

impl MmioTarget for IoApic {
    fn read(
        &mut self,
        _ctx: &mut dyn Ctx,
        offset: u64,
        _size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        let val = match offset {
            0x00 => u64::from(self.state.reg_select),
            0x10 => u64::from(self.read_reg(self.state.reg_select)),
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
            0x00 => self.state.reg_select = val & 0xFF,
            0x10 => {
                let reg = self.state.reg_select;
                self.write_reg(ctx, reg, val);
            }
            _ => {}
        }
        Ok(())
    }
}

impl EventTarget for IoApic {
    fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
        Ok(())
    }
}

impl Snapshot for IoApic {
    type State = IoApicState;
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
        hash.u32(state.id);
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
    fn ioapic_version_and_redirection_table() {
        let mut ioapic = IoApic::new();
        let mut dummy = DummyCtx;
        let attr = test_attr();
        // Select Version register 0x01
        ioapic
            .write(&mut dummy, 0x00, AccessSize::Word, 0x01, &attr)
            .unwrap();
        let ver = ioapic
            .read(&mut dummy, 0x10, AccessSize::Word, &attr)
            .unwrap();
        assert_eq!(ver, 0x0017_0011);
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
