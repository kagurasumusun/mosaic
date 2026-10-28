//! STM32L4 timer
//!
//! Catalog entry `ip:st/tim-l4`. This is a functional register model:
//! the default spawn reads its IRQ output from the catalog output table and
//! the optional console endpoint from the host attachment (ADR 0025).

use mosaic_contract::mmio::MmioTarget;
use mosaic_contract::model::{ContextError, Ctx, EventTarget};
use mosaic_exec::{AccessSize, Attr, BusFault, SignalId};

/// STM32L4 timer
pub struct TimL4 {
    /// Stored until the model raises interrupts (ADR 0025 skeleton).
    _irq: SignalId,
    scratch: [u8; 16],
}

impl TimL4 {
    pub fn new(irq: SignalId) -> Self {
        Self {
            _irq: irq,
            scratch: [0; 16],
        }
    }
}

impl MmioTarget for TimL4 {
    fn read(
        &mut self,
        _ctx: &mut dyn Ctx,
        addr: u64,
        size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        let off = (addr & 0xFF) as usize;
        let n = size.bytes() as usize;
        let mut v = 0u64;
        for i in 0..n {
            let idx = (off + i) % self.scratch.len();
            v |= (self.scratch[idx] as u64) << (i * 8);
        }
        Ok(v)
    }
    fn write(
        &mut self,
        _ctx: &mut dyn Ctx,
        addr: u64,
        size: AccessSize,
        value: u64,
        _attr: &Attr,
    ) -> Result<(), BusFault> {
        let off = (addr & 0xFF) as usize;
        let n = size.bytes() as usize;
        for i in 0..n {
            let idx = (off + i) % self.scratch.len();
            self.scratch[idx] = ((value >> (i * 8)) & 0xFF) as u8;
        }
        Ok(())
    }
}

impl EventTarget for TimL4 {
    fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
        Ok(())
    }
}
