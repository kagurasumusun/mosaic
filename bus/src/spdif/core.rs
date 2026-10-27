//! `spdif` bus entity (ADR 0025 step 3): S/PDIF / IEC 60958.

use mosaic_contract::model::Ctx;
use mosaic_contract::spdif::{SpdifError, SpdifFrame, SpdifReceiver, SpdifTransmitter};

pub struct SpdifBus {
    receivers: Vec<Box<dyn SpdifReceiver>>,
}

impl Default for SpdifBus { fn default() -> Self { Self::new() } }

impl SpdifBus {
    pub fn new() -> Self { Self { receivers: Vec::new() } }
    pub fn attach(&mut self, r: Box<dyn SpdifReceiver>) { self.receivers.push(r); }
}

impl SpdifTransmitter for SpdifBus {
    fn transmit(&mut self, ctx: &mut dyn Ctx, frames: &[SpdifFrame]) -> Result<(), SpdifError> {
        if self.receivers.is_empty() { return Err(SpdifError::NoReceiver); }
        for r in self.receivers.iter_mut() {
            r.on_frames(ctx, frames)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::spdif::{SpdifFrame, SpdifReceiver};
    use mosaic_contract::model::Ctx;

    struct NoopCtx;
    impl Ctx for NoopCtx {
        fn now(&self) -> mosaic_exec::VTime { mosaic_exec::VTime::ZERO }
        fn schedule(&mut self, _: mosaic_exec::VTime, _: u64)
            -> Result<mosaic_exec::EventId, mosaic_contract::model::ContextError> {
            Err(mosaic_contract::model::ContextError::ResourceExhausted)
        }
        fn cancel(&mut self, _: mosaic_exec::EventId) -> bool { false }
        fn set_signal(&mut self, _: mosaic_exec::SignalId, _: bool)
            -> Result<(), mosaic_contract::model::ContextError> { Ok(()) }
    }

    #[derive(Default)]
    struct Rx { frames: u32 }
    impl SpdifReceiver for Rx {
        fn on_frames(&mut self, _: &mut dyn Ctx, _: &[SpdifFrame]) -> Result<(), SpdifError> {
            self.frames += 1;
            Ok(())
        }
    }

    #[test]
    fn transmit_reaches_receivers() {
        let mut bus = SpdifBus::new();
        bus.attach(Box::new(Rx::default()));
        bus.attach(Box::new(Rx::default()));
        let mut ctx = NoopCtx;
        let frames = vec![SpdifFrame::default()];
        assert!(bus.transmit(&mut ctx, &frames).is_ok());
    }

    #[test]
    fn no_receivers_yields_error() {
        let mut bus = SpdifBus::new();
        let mut ctx = NoopCtx;
        assert_eq!(bus.transmit(&mut ctx, &[]), Err(SpdifError::NoReceiver));
    }
}
