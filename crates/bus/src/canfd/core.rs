//! `canfd` bus entity (ADR 0025): CAN FD (Flexible Data-rate): extended from CAN with up to 64-byte frames.

use mosaic_contract::canfd::{CanFdError, CanFdFrame, CanFdInitiator, CanFdTarget};
use mosaic_contract::model::Ctx;

pub struct CanFdBus {
    targets: Vec<Box<dyn CanFdTarget>>,
    queue: Vec<CanFdFrame>,
}

impl Default for CanFdBus {
    fn default() -> Self {
        Self::new()
    }
}

impl CanFdBus {
    pub fn new() -> Self {
        Self {
            targets: Vec::new(),
            queue: Vec::new(),
        }
    }
    pub fn add_target(&mut self, t: Box<dyn CanFdTarget>) {
        self.targets.push(t);
    }
    pub fn drain(&mut self) -> Vec<CanFdFrame> {
        std::mem::take(&mut self.queue)
    }
}

impl CanFdInitiator for CanFdBus {
    fn transmit(&mut self, ctx: &mut dyn Ctx, frame: &CanFdFrame) -> Result<(), CanFdError> {
        // Simple arbitration: lower ID wins, no bit-level simulation.
        for t in self.targets.iter_mut() {
            if t.accepts(frame) {
                t.receive(ctx, frame)?;
            }
        }
        self.queue.push(frame.clone());
        Ok(())
    }
    fn receive(&mut self, ctx: &mut dyn Ctx) -> Result<CanFdFrame, CanFdError> {
        // Poll each target for a pending frame; deterministic order.
        for t in self.targets.iter_mut() {
            if let Some(frame) = t.transmit(ctx)? {
                self.queue.push(frame.clone());
                return Ok(frame);
            }
        }
        self.queue.pop().ok_or(CanFdError::NoAck)
    }
}
