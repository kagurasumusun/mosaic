//! `dvi` bus entity (ADR 0029): DVI digital video link (TMDS + DDC).

use mosaic_contract::dvi::{DviError, DviFrame, DviSink, DviSource};
use mosaic_contract::model::Ctx;

pub struct DviBus {
    sink: Option<Box<dyn DviSink>>,
}

impl Default for DviBus {
    fn default() -> Self {
        Self::new()
    }
}

impl DviBus {
    pub fn new() -> Self {
        Self { sink: None }
    }
    pub fn attach_sink(&mut self, s: Box<dyn DviSink>) {
        self.sink = Some(s);
    }
}

impl DviSource for DviBus {
    fn push_video(&mut self, ctx: &mut dyn Ctx, frame: &DviFrame) -> Result<(), DviError> {
        match self.sink.as_deref_mut() {
            Some(s) => {
                if !s.hpd() {
                    return Err(DviError::HpdDeasserted);
                }
                s.on_video(ctx, frame)
            }
            None => Err(DviError::NoSink),
        }
    }
    fn read_edid(&mut self, _ctx: &mut dyn Ctx) -> Result<Vec<u8>, DviError> {
        match self.sink.as_deref() {
            Some(s) => Ok(s.edid().to_vec()),
            None => Err(DviError::NoSink),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::dvi::{DviFrame, DviSink};

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

    struct Panel {
        hpd: bool,
        edid: Vec<u8>,
        frames: u32,
    }
    impl DviSink for Panel {
        fn on_video(&mut self, _ctx: &mut dyn Ctx, _f: &DviFrame) -> Result<(), DviError> {
            self.frames += 1;
            Ok(())
        }
        fn edid(&self) -> &[u8] {
            &self.edid
        }
        fn hpd(&self) -> bool {
            self.hpd
        }
    }

    #[test]
    fn video_reaches_panel_when_hpd_asserted() {
        let mut bus = DviBus::new();
        bus.attach_sink(Box::new(Panel {
            hpd: true,
            edid: vec![0x80],
            frames: 0,
        }));
        let mut ctx = NoopCtx;
        let frame = DviFrame {
            width: 1280,
            height: 1024,
            pixels: vec![0; 32],
        };
        assert!(bus.push_video(&mut ctx, &frame).is_ok());
    }

    #[test]
    fn no_sink_yields_no_sink_error() {
        let mut bus = DviBus::new();
        let mut ctx = NoopCtx;
        assert_eq!(
            bus.push_video(&mut ctx, &DviFrame::default()),
            Err(DviError::NoSink)
        );
    }

    #[test]
    fn hpd_low_blocks_video() {
        let mut bus = DviBus::new();
        bus.attach_sink(Box::new(Panel {
            hpd: false,
            edid: vec![],
            frames: 0,
        }));
        let mut ctx = NoopCtx;
        assert_eq!(
            bus.push_video(&mut ctx, &DviFrame::default()),
            Err(DviError::HpdDeasserted)
        );
    }
}
