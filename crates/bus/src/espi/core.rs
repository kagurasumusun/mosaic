//! `espi` bus entity (ADR 0025 step 3): Intel eSPI.

use mosaic_contract::espi::{EspiError, EspiMaster, EspiPacket, EspiTarget};
use mosaic_contract::model::Ctx;

pub struct EspiBus {
    targets: Vec<Box<dyn EspiTarget>>,
}

impl Default for EspiBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EspiBus {
    pub fn new() -> Self {
        Self {
            targets: Vec::new(),
        }
    }
    pub fn attach(&mut self, t: Box<dyn EspiTarget>) {
        self.targets.push(t);
    }
}

impl EspiMaster for EspiBus {
    fn transfer(&mut self, ctx: &mut dyn Ctx, p: &EspiPacket) -> Result<Vec<u8>, EspiError> {
        if self.targets.is_empty() {
            return Err(EspiError::NoTarget);
        }
        let mut last = Vec::new();
        for t in self.targets.iter_mut() {
            last = t.on_transfer(ctx, p)?;
        }
        Ok(last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::espi::{EspiPacket, EspiTarget};
    use mosaic_contract::model::Ctx;

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

    struct Target {
        got: u32,
    }
    impl EspiTarget for Target {
        fn on_transfer(&mut self, _: &mut dyn Ctx, _p: &EspiPacket) -> Result<Vec<u8>, EspiError> {
            self.got += 1;
            Ok(vec![0u8])
        }
    }

    #[test]
    fn transfer_routes_to_targets() {
        let mut bus = EspiBus::new();
        bus.attach(Box::new(Target { got: 0 }));
        bus.attach(Box::new(Target { got: 0 }));
        let mut ctx = NoopCtx;
        let p = EspiPacket {
            cycle: 0x01,
            tag: 0x02,
            length: 0,
            payload: vec![],
        };
        assert!(bus.transfer(&mut ctx, &p).is_ok());
    }

    #[test]
    fn empty_bus_errors_on_transfer() {
        let mut bus = EspiBus::new();
        let mut ctx = NoopCtx;
        let p = EspiPacket::default();
        assert_eq!(bus.transfer(&mut ctx, &p), Err(EspiError::NoTarget));
    }
}
