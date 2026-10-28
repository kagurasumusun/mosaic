//! `mipi_csi` bus entity (ADR 0025 step 3): MIPI CSI-2.

use mosaic_contract::mipi_csi::{CsiError, CsiReceiver, CsiSource};
use mosaic_contract::model::Ctx;

pub struct CsiBus {
    receivers: Vec<Box<dyn CsiReceiver>>,
    source: Option<Box<dyn CsiSource>>,
}

impl Default for CsiBus {
    fn default() -> Self {
        Self::new()
    }
}

impl CsiBus {
    pub fn new() -> Self {
        Self {
            receivers: Vec::new(),
            source: None,
        }
    }
    pub fn attach_receiver(&mut self, r: Box<dyn CsiReceiver>) {
        self.receivers.push(r);
    }
    pub fn attach_source(&mut self, s: Box<dyn CsiSource>) {
        self.source = Some(s);
    }
    pub fn pump(&mut self, ctx: &mut dyn Ctx) -> Result<(), CsiError> {
        let packets = match self.source.as_deref_mut() {
            Some(s) => s.emit(ctx)?,
            None => return Err(CsiError::NoSource),
        };
        if self.receivers.is_empty() && !packets.is_empty() {
            return Err(CsiError::NoReceiver);
        }
        for p in packets {
            for r in self.receivers.iter_mut() {
                r.on_packet(ctx, &p)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::mipi_csi::{CsiPacket, CsiReceiver, CsiSource};
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

    struct Source {
        count: u32,
    }
    impl CsiSource for Source {
        fn emit(&mut self, _: &mut dyn Ctx) -> Result<Vec<CsiPacket>, CsiError> {
            self.count += 1;
            Ok(vec![CsiPacket {
                vc: 0,
                dt: 0x2A,
                payload: vec![0; 8],
            }])
        }
    }

    #[derive(Default)]
    struct Receiver {
        got: u32,
    }
    impl CsiReceiver for Receiver {
        fn on_packet(&mut self, _: &mut dyn Ctx, _p: &CsiPacket) -> Result<(), CsiError> {
            self.got += 1;
            Ok(())
        }
    }

    #[test]
    fn pump_moves_packets_from_source_to_receivers() {
        let mut bus = CsiBus::new();
        bus.attach_source(Box::new(Source { count: 0 }));
        bus.attach_receiver(Box::new(Receiver::default()));
        bus.attach_receiver(Box::new(Receiver::default()));
        let mut ctx = NoopCtx;
        assert!(bus.pump(&mut ctx).is_ok());
    }

    #[test]
    fn pump_without_source_errors() {
        let mut bus = CsiBus::new();
        bus.attach_receiver(Box::new(Receiver::default()));
        let mut ctx = NoopCtx;
        assert_eq!(bus.pump(&mut ctx), Err(CsiError::NoSource));
    }
}
