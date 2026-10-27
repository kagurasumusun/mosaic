//! `hdmi` bus entity (ADR 0025 step 3): HDMI digital video/audio link.

use mosaic_contract::hdmi::{AudioSample, HdmiError, HdmiSink, HdmiSource, VideoFrame};
use mosaic_contract::model::Ctx;

pub struct HdmiBus {
    sink: Option<Box<dyn HdmiSink>>,
}

impl Default for HdmiBus { fn default() -> Self { Self::new() } }

impl HdmiBus {
    pub fn new() -> Self { Self { sink: None } }
    pub fn attach_sink(&mut self, s: Box<dyn HdmiSink>) { self.sink = Some(s); }
}

impl HdmiSource for HdmiBus {
    fn push_video(&mut self, ctx: &mut dyn Ctx, frame: &VideoFrame) -> Result<(), HdmiError> {
        match self.sink.as_deref_mut() {
            Some(s) => {
                if !s.hpd() { return Err(HdmiError::HpdDeasserted); }
                s.on_video(ctx, frame)
            }
            None => Err(HdmiError::NoSink),
        }
    }
    fn push_audio(&mut self, ctx: &mut dyn Ctx, sample: &AudioSample) -> Result<(), HdmiError> {
        match self.sink.as_deref_mut() {
            Some(s) => {
                if !s.hpd() { return Err(HdmiError::HpdDeasserted); }
                s.on_audio(ctx, sample)
            }
            None => Err(HdmiError::NoSink),
        }
    }
    fn read_edid(&mut self, _ctx: &mut dyn Ctx) -> Result<Vec<u8>, HdmiError> {
        match self.sink.as_deref() {
            Some(s) => Ok(s.edid().to_vec()),
            None => Err(HdmiError::NoSink),
        }
    }
    fn cec_send(
        &mut self,
        ctx: &mut dyn Ctx,
        initiator: u8,
        destination: u8,
        payload: &[u8],
    ) -> Result<(), HdmiError> {
        match self.sink.as_deref_mut() {
            Some(s) => s.cec_receive(ctx, initiator, destination, payload),
            None => Err(HdmiError::NoSink),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::hdmi::{AudioSample, HdmiSink, VideoFrame};
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

    struct Sink { hpd: bool, edid: Vec<u8>, frames: u32 }
    impl HdmiSink for Sink {
        fn on_video(&mut self, _ctx: &mut dyn Ctx, _f: &VideoFrame) -> Result<(), HdmiError> { self.frames += 1; Ok(()) }
        fn on_audio(&mut self, _ctx: &mut dyn Ctx, _s: &AudioSample) -> Result<(), HdmiError> { Ok(()) }
        fn edid(&self) -> &[u8] { &self.edid }
        fn hpd(&self) -> bool { self.hpd }
        fn cec_receive(&mut self, _: &mut dyn Ctx, _: u8, _: u8, _: &[u8]) -> Result<(), HdmiError> { Ok(()) }
    }

    #[test]
    fn video_reaches_sink_when_hpd_asserted() {
        let mut bus = HdmiBus::new();
        bus.attach_sink(Box::new(Sink { hpd: true, edid: vec![0x80], frames: 0 }));
        let mut ctx = NoopCtx;
        let frame = VideoFrame { width: 640, height: 480, pixels: vec![0; 32] };
        assert!(bus.push_video(&mut ctx, &frame).is_ok());
    }

    #[test]
    fn no_sink_yields_no_sink_error() {
        let mut bus = HdmiBus::new();
        let mut ctx = NoopCtx;
        let frame = VideoFrame::default();
        assert_eq!(bus.push_video(&mut ctx, &frame), Err(HdmiError::NoSink));
    }

    #[test]
    fn hpd_low_blocks_video() {
        let mut bus = HdmiBus::new();
        bus.attach_sink(Box::new(Sink { hpd: false, edid: vec![], frames: 0 }));
        let mut ctx = NoopCtx;
        let frame = VideoFrame::default();
        assert_eq!(bus.push_video(&mut ctx, &frame), Err(HdmiError::HpdDeasserted));
    }
}
