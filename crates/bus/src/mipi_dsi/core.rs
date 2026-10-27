//! `mipi_dsi` bus entity (ADR 0025 step 3): MIPI DSI.

use mosaic_contract::mipi_dsi::{DsiError, DsiHost, DsiPacket, DsiPeripheral};
use mosaic_contract::model::Ctx;

pub struct DsiBus {
    peripherals: Vec<Box<dyn DsiPeripheral>>,
    mode: mosaic_contract::mipi_dsi::DsiMode,
}

impl Default for DsiBus {
    fn default() -> Self {
        Self::new(mosaic_contract::mipi_dsi::DsiMode::Command)
    }
}

impl DsiBus {
    pub fn new(mode: mosaic_contract::mipi_dsi::DsiMode) -> Self {
        Self {
            peripherals: Vec::new(),
            mode,
        }
    }
    pub fn attach(&mut self, p: Box<dyn DsiPeripheral>) {
        self.peripherals.push(p);
    }
}

impl DsiHost for DsiBus {
    fn send_packet(&mut self, ctx: &mut dyn Ctx, p: &DsiPacket) -> Result<(), DsiError> {
        if self.peripherals.is_empty() {
            return Err(DsiError::NoPeripheral);
        }
        for per in self.peripherals.iter_mut() {
            per.on_packet(ctx, p)?;
        }
        Ok(())
    }
    fn read_response(&mut self, ctx: &mut dyn Ctx) -> Result<Vec<u8>, DsiError> {
        if self.peripherals.is_empty() {
            return Err(DsiError::NoPeripheral);
        }
        self.peripherals[0].read_request(ctx)
    }
    fn mode(&self) -> mosaic_contract::mipi_dsi::DsiMode {
        self.mode
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::mipi_dsi::{DsiMode, DsiPacket, DsiPeripheral};
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

    struct Periph {
        received: u32,
    }
    impl DsiPeripheral for Periph {
        fn on_packet(&mut self, _: &mut dyn Ctx, _p: &DsiPacket) -> Result<(), DsiError> {
            self.received += 1;
            Ok(())
        }
        fn read_request(&mut self, _: &mut dyn Ctx) -> Result<Vec<u8>, DsiError> {
            Ok(vec![0u8])
        }
    }

    #[test]
    fn packet_reaches_all_peripherals() {
        let mut bus = DsiBus::new(DsiMode::Command);
        bus.attach(Box::new(Periph { received: 0 }));
        bus.attach(Box::new(Periph { received: 0 }));
        let mut ctx = NoopCtx;
        let p = DsiPacket {
            vc: 0,
            data_type: 0x39,
            payload: vec![],
        };
        assert!(bus.send_packet(&mut ctx, &p).is_ok());
    }

    #[test]
    fn empty_bus_errors_on_send() {
        let mut bus = DsiBus::new(DsiMode::Video);
        let mut ctx = NoopCtx;
        let p = DsiPacket::default();
        assert_eq!(bus.send_packet(&mut ctx, &p), Err(DsiError::NoPeripheral));
    }
}
