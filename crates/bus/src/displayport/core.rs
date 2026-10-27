//! `displayport` bus entity (ADR 0025 step 3): VESA DisplayPort.

use mosaic_contract::displayport::{
    DisplayPortError, DisplayPortSink, DisplayPortSource, DpVideoFrame,
};
use mosaic_contract::model::Ctx;

pub struct DisplayPortBus {
    sink: Option<Box<dyn DisplayPortSink>>,
}

impl Default for DisplayPortBus {
    fn default() -> Self {
        Self::new()
    }
}

impl DisplayPortBus {
    pub fn new() -> Self {
        Self { sink: None }
    }
    pub fn attach_sink(&mut self, s: Box<dyn DisplayPortSink>) {
        self.sink = Some(s);
    }
}

impl DisplayPortSource for DisplayPortBus {
    fn push_video(
        &mut self,
        ctx: &mut dyn Ctx,
        frame: &DpVideoFrame,
    ) -> Result<(), DisplayPortError> {
        match self.sink.as_deref_mut() {
            Some(s) => {
                if !s.hpd() {
                    return Err(DisplayPortError::HpdDeasserted);
                }
                s.on_video(ctx, frame)
            }
            None => Err(DisplayPortError::NoSink),
        }
    }
    fn aux_read(&mut self, ctx: &mut dyn Ctx, address: u32) -> Result<Vec<u8>, DisplayPortError> {
        match self.sink.as_deref_mut() {
            Some(s) => {
                let mut out = Vec::new();
                s.aux_handle(ctx, false, address, &mut out)?;
                Ok(out)
            }
            None => Err(DisplayPortError::NoSink),
        }
    }
    fn aux_write(
        &mut self,
        ctx: &mut dyn Ctx,
        address: u32,
        data: &[u8],
    ) -> Result<(), DisplayPortError> {
        match self.sink.as_deref_mut() {
            Some(s) => {
                let mut buf = data.to_vec();
                s.aux_handle(ctx, true, address, &mut buf)
            }
            None => Err(DisplayPortError::NoSink),
        }
    }
    fn read_dpcd(&mut self, ctx: &mut dyn Ctx, address: u32) -> Result<u32, DisplayPortError> {
        let bytes = self.aux_read(ctx, address)?;
        let v = bytes
            .iter()
            .take(4)
            .fold(0u32, |acc, &b| (acc << 8) | b as u32);
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::displayport::{DisplayPortSink, DpVideoFrame};
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

    struct Sink {
        hpd: bool,
        aux_log: Vec<u32>,
    }
    impl DisplayPortSink for Sink {
        fn on_video(&mut self, _: &mut dyn Ctx, _: &DpVideoFrame) -> Result<(), DisplayPortError> {
            Ok(())
        }
        fn hpd(&self) -> bool {
            self.hpd
        }
        fn aux_handle(
            &mut self,
            _: &mut dyn Ctx,
            write: bool,
            address: u32,
            _: &mut Vec<u8>,
        ) -> Result<(), DisplayPortError> {
            if !write {
                self.aux_log.push(address);
            }
            Ok(())
        }
    }

    #[test]
    fn aux_read_routes_to_sink() {
        let mut bus = DisplayPortBus::new();
        bus.attach_sink(Box::new(Sink {
            hpd: true,
            aux_log: vec![],
        }));
        let mut ctx = NoopCtx;
        let _ = bus.aux_read(&mut ctx, 0x100);
        // hpd true so reads succeed
        let v = bus.read_dpcd(&mut ctx, 0x200);
        assert!(v.is_ok());
    }

    #[test]
    fn no_sink_aux_read_fails() {
        let mut bus = DisplayPortBus::new();
        let mut ctx = NoopCtx;
        assert_eq!(bus.aux_read(&mut ctx, 0x100), Err(DisplayPortError::NoSink));
    }
}
