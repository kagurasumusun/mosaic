//! `swd` bus entity (ADR 0025): ARM Serial Wire Debug (SW-DP): DP/AP register access.

use mosaic_contract::model::Ctx;
use mosaic_contract::swd::{SwdAp, SwdError, SwdInitiator, SwdTarget};

/// SWD bus entity: a single SW-DP target (multi-drop extensions are future work).
pub struct SwdBus { target: Option<Box<dyn SwdTarget>> }

impl Default for SwdBus { fn default() -> Self { Self::new() } }

impl SwdBus {
    pub fn new() -> Self { Self { target: None } }
    pub fn attach(&mut self, t: Box<dyn SwdTarget>) { self.target = Some(t); }
}

impl SwdInitiator for SwdBus {
    fn read(&mut self, ctx: &mut dyn Ctx, ap: SwdAp, addr: u8) -> Result<u32, SwdError> {
        self.target.as_mut().ok_or(SwdError::NoAck)?.read(ctx, ap, addr)
    }
    fn write(&mut self, ctx: &mut dyn Ctx, ap: SwdAp, addr: u8, value: u32) -> Result<(), SwdError> {
        self.target.as_mut().ok_or(SwdError::NoAck)?.write(ctx, ap, addr, value)
    }
}
