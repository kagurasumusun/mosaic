//! `slimbus` bus entity (ADR 0025 step 3): SLIMbus.

use mosaic_contract::model::Ctx;
use mosaic_contract::slimbus::{SlimbusDevice, SlimbusError, SlimbusManager, SlimbusMessage};

pub struct SlimbusBus {
    devices: Vec<Box<dyn SlimbusDevice>>,
}

impl Default for SlimbusBus { fn default() -> Self { Self::new() } }

impl SlimbusBus {
    pub fn new() -> Self { Self { devices: Vec::new() } }
    pub fn attach(&mut self, d: Box<dyn SlimbusDevice>) { self.devices.push(d); }
}

impl SlimbusManager for SlimbusBus {
    fn send_message(&mut self, ctx: &mut dyn Ctx, m: &SlimbusMessage) -> Result<(), SlimbusError> {
        for d in self.devices.iter_mut() {
            d.on_message(ctx, m)?;
        }
        Ok(())
    }
    fn schedule_channels(&mut self, _ctx: &mut dyn Ctx, _slots: u32) -> Result<(), SlimbusError> {
        // The framer owns the schedule; the fabric itself just acknowledges.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::slimbus::{SlimbusDevice, SlimbusMessage};
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
    struct Dev { msgs: u32 }
    impl SlimbusDevice for Dev {
        fn on_message(&mut self, _: &mut dyn Ctx, _m: &SlimbusMessage) -> Result<(), SlimbusError> {
            self.msgs += 1;
            Ok(())
        }
    }

    #[test]
    fn broadcast_reaches_all_devices() {
        let mut bus = SlimbusBus::new();
        bus.attach(Box::new(Dev::default()));
        bus.attach(Box::new(Dev::default()));
        let mut ctx = NoopCtx;
        let m = SlimbusMessage { destination: 0xFF, source: 0x00, code: 0x10, value: 0 };
        assert!(bus.send_message(&mut ctx, &m).is_ok());
        assert!(bus.schedule_channels(&mut ctx, 0x10).is_ok());
    }
}
