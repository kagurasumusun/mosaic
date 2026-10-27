//! ARM Generic Interrupt Controller v2 (GICv2) functional model.

use mosaic_contract::mmio::MmioTarget;
use mosaic_contract::model::{ContextError, Ctx, EventTarget};
use mosaic_exec::{AccessSize, Attr, BusFault, SignalId, Snapshot, SnapshotError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GicV2State {
    pub schema: u32,
    pub gicd_ctlr: u32,
    pub gicc_ctlr: u32,
    pub gicc_pmr: u32,
    pub enabled: [u32; 32],
    pub pending: [u32; 32],
    pub active: [u32; 32],
    pub priority: [u8; 1024],
    pub targets: [u8; 1024],
}

impl Default for GicV2State {
    fn default() -> Self {
        let mut state = Self {
            schema: 1,
            gicd_ctlr: 0,
            gicc_ctlr: 0,
            gicc_pmr: 0xFF,
            enabled: [0; 32],
            pending: [0; 32],
            active: [0; 32],
            priority: [0; 1024],
            targets: [0; 1024],
        };
        // Default target CPU 0 (0x01)
        for t in &mut state.targets {
            *t = 0x01;
        }
        state
    }
}

pub struct GicV2 {
    state: GicV2State,
}

impl GicV2 {
    pub fn new() -> Self {
        Self {
            state: GicV2State::default(),
        }
    }

    pub fn set_irq(&mut self, ctx: &mut dyn Ctx, irq: u32, high: bool) {
        if (irq as usize) < 1024 {
            let idx = (irq / 32) as usize;
            let bit = 1u32 << (irq % 32);
            if high {
                self.state.pending[idx] |= bit;
            } else {
                self.state.pending[idx] &= !bit;
            }
            self.update_cpu_irq(ctx);
        }
    }

    fn highest_pending_irq(&self) -> Option<u32> {
        if self.state.gicd_ctlr == 0 || self.state.gicc_ctlr == 0 {
            return None;
        }
        let mut best_irq = None;
        let mut best_prio = self.state.gicc_pmr;

        for i in 0..1024 {
            let idx = i / 32;
            let bit = 1u32 << (i % 32);
            if (self.state.enabled[idx] & bit) != 0 && (self.state.pending[idx] & bit) != 0 {
                let prio = u32::from(self.state.priority[i]);
                if prio < best_prio {
                    best_prio = prio;
                    best_irq = Some(i as u32);
                }
            }
        }
        best_irq
    }

    fn update_cpu_irq(&self, ctx: &mut dyn Ctx) {
        let active = self.highest_pending_irq().is_some();
        let _ = ctx.set_signal(SignalId(0), active);
    }

    fn read_distributor(&mut self, offset: u64) -> u32 {
        match offset {
            0x000 => self.state.gicd_ctlr,
            0x004 => 0x01A0_001F, // ITLinesNumber = 31 (1020 IRQs), 1 CPU
            0x100..=0x17F => {
                let idx = ((offset - 0x100) / 4) as usize;
                if idx < 32 {
                    self.state.enabled[idx]
                } else {
                    0
                }
            }
            0x200..=0x27F => {
                let idx = ((offset - 0x200) / 4) as usize;
                if idx < 32 {
                    self.state.pending[idx]
                } else {
                    0
                }
            }
            0x300..=0x37F => {
                let idx = ((offset - 0x300) / 4) as usize;
                if idx < 32 {
                    self.state.active[idx]
                } else {
                    0
                }
            }
            _ => 0,
        }
    }

    fn write_distributor(&mut self, ctx: &mut dyn Ctx, offset: u64, val: u32) {
        match offset {
            0x000 => self.state.gicd_ctlr = val & 1,
            0x100..=0x17F => {
                let idx = ((offset - 0x100) / 4) as usize;
                if idx < 32 {
                    self.state.enabled[idx] |= val;
                }
            }
            0x180..=0x1FF => {
                let idx = ((offset - 0x180) / 4) as usize;
                if idx < 32 {
                    self.state.enabled[idx] &= !val;
                }
            }
            0x200..=0x27F => {
                let idx = ((offset - 0x200) / 4) as usize;
                if idx < 32 {
                    self.state.pending[idx] |= val;
                }
            }
            0x280..=0x2FF => {
                let idx = ((offset - 0x280) / 4) as usize;
                if idx < 32 {
                    self.state.pending[idx] &= !val;
                }
            }
            _ => {}
        }
        self.update_cpu_irq(ctx);
    }

    fn read_cpu_interface(&mut self, ctx: &mut dyn Ctx, offset: u64) -> u32 {
        match offset {
            0x000 => self.state.gicc_ctlr,
            0x004 => self.state.gicc_pmr,
            0x00C => {
                // IAR: Acknowledge highest pending IRQ
                if let Some(irq) = self.highest_pending_irq() {
                    let idx = (irq / 32) as usize;
                    let bit = 1u32 << (irq % 32);
                    self.state.pending[idx] &= !bit;
                    self.state.active[idx] |= bit;
                    self.update_cpu_irq(ctx);
                    irq
                } else {
                    1023 // Spurious
                }
            }
            _ => 0,
        }
    }

    fn write_cpu_interface(&mut self, ctx: &mut dyn Ctx, offset: u64, val: u32) {
        match offset {
            0x000 => self.state.gicc_ctlr = val & 1,
            0x004 => self.state.gicc_pmr = val & 0xFF,
            0x010 => {
                // EOIR: End of interrupt
                let irq = val & 0x3FF;
                if (irq as usize) < 1024 {
                    let idx = (irq / 32) as usize;
                    let bit = 1u32 << (irq % 32);
                    self.state.active[idx] &= !bit;
                }
            }
            _ => {}
        }
        self.update_cpu_irq(ctx);
    }
}

impl Default for GicV2 {
    fn default() -> Self {
        Self::new()
    }
}

impl MmioTarget for GicV2 {
    fn read(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        _size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        let val = if offset < 0x1000 {
            self.read_distributor(offset)
        } else {
            self.read_cpu_interface(ctx, offset - 0x1000)
        };
        Ok(u64::from(val))
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
        if offset < 0x1000 {
            self.write_distributor(ctx, offset, val);
        } else {
            self.write_cpu_interface(ctx, offset - 0x1000, val);
        }
        Ok(())
    }
}

impl EventTarget for GicV2 {
    fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
        Ok(())
    }

    fn on_signal(
        &mut self,
        ctx: &mut dyn Ctx,
        _source: mosaic_exec::NodeId,
        signal: SignalId,
        high: bool,
    ) -> Result<(), ContextError> {
        self.set_irq(ctx, signal.0, high);
        Ok(())
    }
}

impl Snapshot for GicV2 {
    type State = GicV2State;
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
        hash.u32(state.gicd_ctlr);
        hash.u32(state.gicc_ctlr);
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
    fn gic_v2_enable_and_raise_irq() {
        let mut gic = GicV2::new();
        let mut dummy = DummyCtx;
        let attr = test_attr();
        // Enable GICD and GICC
        gic.write(&mut dummy, 0x000, AccessSize::Word, 1, &attr)
            .unwrap();
        gic.write(&mut dummy, 0x1000, AccessSize::Word, 1, &attr)
            .unwrap();
        // Enable IRQ 32
        gic.write(&mut dummy, 0x104, AccessSize::Word, 1, &attr)
            .unwrap();

        gic.set_irq(&mut dummy, 32, true);
        let iar = gic
            .read(&mut dummy, 0x100C, AccessSize::Word, &attr)
            .unwrap();
        assert_eq!(iar, 32);
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
