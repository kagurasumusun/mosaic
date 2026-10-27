//! `soundwire` bus entity (ADR 0025 step 3): MIPI SoundWire.

use mosaic_contract::model::Ctx;
use mosaic_contract::soundwire::{
    SoundWireError, SoundWireFrame, SoundWireMaster, SoundWirePeripheral,
};

pub struct SoundWireBus {
    peripherals: Vec<Box<dyn SoundWirePeripheral>>,
}

impl Default for SoundWireBus {
    fn default() -> Self {
        Self::new()
    }
}

impl SoundWireBus {
    pub fn new() -> Self {
        Self {
            peripherals: Vec::new(),
        }
    }
    pub fn attach(&mut self, p: Box<dyn SoundWirePeripheral>) {
        self.peripherals.push(p);
    }
}

impl SoundWireMaster for SoundWireBus {
    fn transmit(
        &mut self,
        ctx: &mut dyn Ctx,
        frame: &SoundWireFrame,
    ) -> Result<(), SoundWireError> {
        if self.peripherals.is_empty() {
            return Err(SoundWireError::NoPeripheral);
        }
        for p in self.peripherals.iter_mut() {
            p.on_frame(ctx, frame)?;
        }
        Ok(())
    }
    fn receive(&mut self, ctx: &mut dyn Ctx) -> Result<SoundWireFrame, SoundWireError> {
        if self.peripherals.is_empty() {
            return Err(SoundWireError::NoPeripheral);
        }
        let mut frame = SoundWireFrame::default();
        for p in self.peripherals.iter_mut() {
            p.reply(ctx, &mut frame)?;
        }
        Ok(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::model::Ctx;
    use mosaic_contract::soundwire::{SoundWireFrame, SoundWirePeripheral};

    struct NoopCtx;
    impl Ctx for NoopCtx {
        fn now(&self) -> mosaic_exec::VTime {
            mosaic_exec::VTime::ZERO
        }
        fn schedule(
            &mut self,
            _: mosaic_exec::VTime,
            _: u64,
        ) -> Result<mosaic_exec::EventId, mosaic_contract::model::ContextError> {
            Err(mosaic_contract::model::ContextError::ResourceExhausted)
        }
        fn cancel(&mut self, _: mosaic_exec::EventId) -> bool {
            false
        }
        fn set_signal(
            &mut self,
            _: mosaic_exec::SignalId,
            _: bool,
        ) -> Result<(), mosaic_contract::model::ContextError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct Periph {
        frames: u32,
    }
    impl SoundWirePeripheral for Periph {
        fn on_frame(&mut self, _: &mut dyn Ctx, _: &SoundWireFrame) -> Result<(), SoundWireError> {
            self.frames += 1;
            Ok(())
        }
        fn reply(
            &mut self,
            _: &mut dyn Ctx,
            _f: &mut SoundWireFrame,
        ) -> Result<(), SoundWireError> {
            Ok(())
        }
    }

    #[test]
    fn transmit_reaches_peripherals() {
        let mut bus = SoundWireBus::new();
        bus.attach(Box::new(Periph::default()));
        let mut ctx = NoopCtx;
        let frame = SoundWireFrame::default();
        assert!(bus.transmit(&mut ctx, &frame).is_ok());
    }

    #[test]
    fn empty_bus_errors_on_transmit() {
        let mut bus = SoundWireBus::new();
        let mut ctx = NoopCtx;
        let frame = SoundWireFrame::default();
        assert_eq!(
            bus.transmit(&mut ctx, &frame),
            Err(SoundWireError::NoPeripheral)
        );
    }
}
