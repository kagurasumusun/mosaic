#![forbid(unsafe_code)]
//! Dual Intel 8259 Programmable Interrupt Controller (PIC) model for x86.

use mosaic_contract_mmio::MmioTarget;
use mosaic_contract_model::{ContextError, Ctx, EventTarget};
use mosaic_core_types::{AccessSize, Attr, BusFault, SignalId, Snapshot, SnapshotError};

#[derive(Clone, Debug, PartialEq, Eq)]
struct SinglePicState {
    icw_step: u8,
    icw1: u8,
    icw2: u8,
    icw3: u8,
    icw4: u8,
    irr: u8,
    imr: u8,
    isr: u8,
    priority_add: u8,
    read_reg_select: u8,
    special_mask: bool,
    auto_eoi: bool,
    rotate_on_auto_eoi: bool,
}

impl Default for SinglePicState {
    fn default() -> Self {
        Self {
            icw_step: 0,
            icw1: 0,
            icw2: 0,
            icw3: 0,
            icw4: 0,
            irr: 0,
            imr: 0,
            isr: 0,
            priority_add: 0,
            read_reg_select: 0,
            special_mask: false,
            auto_eoi: false,
            rotate_on_auto_eoi: false,
        }
    }
}

impl SinglePicState {
    #[allow(dead_code)]
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn write_cmd(&mut self, val: u8) {
        if val & 0x10 != 0 {
            // ICW1
            self.icw1 = val;
            self.imr = 0;
            self.isr = 0;
            self.irr = 0;
            self.icw_step = 1;
            self.auto_eoi = false;
        } else if val & 0x08 == 0 {
            // OCW2
            let cmd = val >> 5;
            match cmd {
                1 | 5 => {
                    // Non-specific EOI / Rotate on non-specific EOI
                    let top = self.get_highest_priority(self.isr);
                    if top < 8 {
                        self.isr &= !(1 << top);
                        if cmd == 5 {
                            self.priority_add = (top + 1) & 7;
                        }
                    }
                }
                3 => {
                    // Specific EOI
                    let line = val & 7;
                    self.isr &= !(1 << line);
                }
                0 | 4 => {
                    // Rotate on auto EOI mode set/clear
                    self.rotate_on_auto_eoi = cmd == 4;
                }
                _ => {}
            }
        } else {
            // OCW3
            if val & 0x02 != 0 {
                self.read_reg_select = val & 0x01; // 0=IRR, 1=ISR
            }
            if val & 0x40 != 0 {
                self.special_mask = (val & 0x20) != 0;
            }
        }
    }

    fn write_data(&mut self, val: u8) {
        match self.icw_step {
            1 => {
                self.icw2 = val;
                if self.icw1 & 0x02 != 0 {
                    // Single mode
                    self.icw_step = if self.icw1 & 0x01 != 0 { 3 } else { 0 };
                } else {
                    self.icw_step = 2;
                }
            }
            2 => {
                self.icw3 = val;
                self.icw_step = if self.icw1 & 0x01 != 0 { 3 } else { 0 };
            }
            3 => {
                self.icw4 = val;
                self.auto_eoi = (val & 0x02) != 0;
                self.icw_step = 0;
            }
            _ => {
                // OCW1: IMR
                self.imr = val;
            }
        }
    }

    fn read_cmd(&self) -> u8 {
        if self.read_reg_select == 1 {
            self.isr
        } else {
            self.irr
        }
    }

    fn read_data(&self) -> u8 {
        self.imr
    }

    fn get_highest_priority(&self, mask: u8) -> u8 {
        if mask == 0 {
            return 8;
        }
        for i in 0..8 {
            let line = (self.priority_add + i) & 7;
            if mask & (1 << line) != 0 {
                return line;
            }
        }
        8
    }

    fn pending_irq(&self) -> Option<u8> {
        let mask = self.irr & !self.imr;
        let highest_irr = self.get_highest_priority(mask);
        if highest_irr >= 8 {
            return None;
        }
        let highest_isr = self.get_highest_priority(self.isr);
        if highest_isr < 8 && highest_irr >= highest_isr {
            return None;
        }
        Some(highest_irr)
    }

    fn acknowledge_irq(&mut self) -> u8 {
        if let Some(irq) = self.pending_irq() {
            self.irr &= !(1 << irq);
            if !self.auto_eoi {
                self.isr |= 1 << irq;
            } else if self.rotate_on_auto_eoi {
                self.priority_add = (irq + 1) & 7;
            }
            self.icw2 + irq
        } else {
            self.icw2 + 7 // Spurious IRQ 7
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pic8259State {
    pub schema: u32,
    master: SinglePicState,
    slave: SinglePicState,
}

pub struct Pic8259 {
    state: Pic8259State,
}

impl Pic8259 {
    pub fn new() -> Self {
        Self {
            state: Pic8259State {
                schema: 1,
                master: SinglePicState::default(),
                slave: SinglePicState::default(),
            },
        }
    }

    pub fn set_irq(&mut self, ctx: &mut dyn Ctx, irq: u8, level: bool) {
        if irq < 8 {
            if level {
                self.state.master.irr |= 1 << irq;
            } else {
                self.state.master.irr &= !(1 << irq);
            }
        } else if irq < 16 {
            let slave_irq = irq - 8;
            if level {
                self.state.slave.irr |= 1 << slave_irq;
            } else {
                self.state.slave.irr &= !(1 << slave_irq);
            }
            // Cascade slave to master IRQ2
            if self.state.slave.pending_irq().is_some() {
                self.state.master.irr |= 1 << 2;
            } else {
                self.state.master.irr &= !(1 << 2);
            }
        }
        self.update_output(ctx);
    }

    pub fn acknowledge(&mut self, ctx: &mut dyn Ctx) -> u8 {
        let vec = if let Some(m_irq) = self.state.master.pending_irq() {
            if m_irq == 2 {
                self.state.slave.acknowledge_irq()
            } else {
                self.state.master.acknowledge_irq()
            }
        } else {
            self.state.master.icw2 + 7
        };
        self.update_output(ctx);
        vec
    }

    fn update_output(&mut self, ctx: &mut dyn Ctx) {
        let active = self.state.master.pending_irq().is_some();
        let _ = ctx.set_signal(SignalId(0), active);
    }
}

impl Default for Pic8259 {
    fn default() -> Self {
        Self::new()
    }
}

impl MmioTarget for Pic8259 {
    fn read(
        &mut self,
        _ctx: &mut dyn Ctx,
        offset: u64,
        _size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        let val = match offset {
            0x00 => self.state.master.read_cmd(),
            0x01 => self.state.master.read_data(),
            0x02 | 0x80 | 0xA0 => self.state.slave.read_cmd(),
            0x03 | 0x81 | 0xA1 => self.state.slave.read_data(),
            _ => 0xFF,
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
        let val = value as u8;
        match offset {
            0x00 => self.state.master.write_cmd(val),
            0x01 => self.state.master.write_data(val),
            0x02 | 0x80 | 0xA0 => self.state.slave.write_cmd(val),
            0x03 | 0x81 | 0xA1 => self.state.slave.write_data(val),
            _ => {}
        }
        self.update_output(ctx);
        Ok(())
    }
}

impl EventTarget for Pic8259 {
    fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
        Ok(())
    }

    fn on_signal(
        &mut self,
        ctx: &mut dyn Ctx,
        _source: mosaic_core_types::NodeId,
        signal: SignalId,
        high: bool,
    ) -> Result<(), ContextError> {
        if signal.0 < 16 {
            self.set_irq(ctx, signal.0 as u8, high);
        }
        Ok(())
    }
}

impl Snapshot for Pic8259 {
    type State = Pic8259State;
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
        hash.byte(state.master.imr);
        hash.byte(state.slave.imr);
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
    fn pic8259_initialization_and_irq_raise() {
        let mut pic = Pic8259::new();
        let mut dummy = DummyCtx;
        let attr = test_attr();
        // Init Master
        pic.write(&mut dummy, 0x00, AccessSize::Byte, 0x11, &attr)
            .unwrap();
        pic.write(&mut dummy, 0x01, AccessSize::Byte, 0x20, &attr)
            .unwrap(); // Vec base 0x20
        pic.write(&mut dummy, 0x01, AccessSize::Byte, 0x04, &attr)
            .unwrap();
        pic.write(&mut dummy, 0x01, AccessSize::Byte, 0x01, &attr)
            .unwrap();

        pic.set_irq(&mut dummy, 0, true);
        assert_eq!(pic.acknowledge(&mut dummy), 0x20);
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
