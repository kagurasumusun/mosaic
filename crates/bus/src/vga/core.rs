//! `vga` bus entity (ADR 0029): analog RGB video link (VGA + DDC).

use mosaic_contract::model::Ctx;
use mosaic_contract::vga::{VgaError, VgaFrame, VgaMode, VgaSink, VgaSource};

pub struct VgaBus {
    sink: Option<Box<dyn VgaSink>>,
}

impl Default for VgaBus {
    fn default() -> Self {
        Self::new()
    }
}

impl VgaBus {
    pub fn new() -> Self {
        Self { sink: None }
    }
    pub fn attach_sink(&mut self, s: Box<dyn VgaSink>) {
        self.sink = Some(s);
    }
}

impl VgaSource for VgaBus {
    fn set_mode(&mut self, ctx: &mut dyn Ctx, mode: &VgaMode) -> Result<(), VgaError> {
        match self.sink.as_deref_mut() {
            Some(s) => {
                if mode.width == 0 || mode.height == 0 || mode.hz == 0 {
                    return Err(VgaError::BadMode);
                }
                s.on_mode(ctx, mode)
            }
            None => Err(VgaError::NoSink),
        }
    }
    fn push_frame(&mut self, ctx: &mut dyn Ctx, frame: &VgaFrame) -> Result<(), VgaError> {
        match self.sink.as_deref_mut() {
            Some(s) => s.on_frame(ctx, frame),
            None => Err(VgaError::NoSink),
        }
    }
    fn read_edid(&mut self, _ctx: &mut dyn Ctx) -> Result<Vec<u8>, VgaError> {
        match self.sink.as_deref() {
            Some(s) if !s.edid().is_empty() => Ok(s.edid().to_vec()),
            Some(_) => Err(VgaError::DdcNack),
            None => Err(VgaError::NoSink),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::vga::{VgaFrame, VgaMode, VgaSink};

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

    struct Monitor {
        edid: Vec<u8>,
        frames: u32,
    }
    impl VgaSink for Monitor {
        fn on_mode(&mut self, _ctx: &mut dyn Ctx, _m: &VgaMode) -> Result<(), VgaError> {
            Ok(())
        }
        fn on_frame(&mut self, _ctx: &mut dyn Ctx, _f: &VgaFrame) -> Result<(), VgaError> {
            self.frames += 1;
            Ok(())
        }
        fn edid(&self) -> &[u8] {
            &self.edid
        }
    }

    #[test]
    fn frame_reaches_monitor() {
        let mut bus = VgaBus::new();
        bus.attach_sink(Box::new(Monitor {
            edid: vec![0x80],
            frames: 0,
        }));
        let mut ctx = NoopCtx;
        let frame = VgaFrame {
            width: 640,
            height: 480,
            pixels: vec![0; 16],
        };
        assert!(bus.push_frame(&mut ctx, &frame).is_ok());
    }

    #[test]
    fn no_sink_yields_no_sink_error() {
        let mut bus = VgaBus::new();
        let mut ctx = NoopCtx;
        assert_eq!(
            bus.push_frame(&mut ctx, &VgaFrame::default()),
            Err(VgaError::NoSink)
        );
    }

    #[test]
    fn empty_edid_is_a_ddc_nack() {
        let mut bus = VgaBus::new();
        bus.attach_sink(Box::new(Monitor {
            edid: vec![],
            frames: 0,
        }));
        let mut ctx = NoopCtx;
        assert_eq!(bus.read_edid(&mut ctx), Err(VgaError::DdcNack));
    }
}
