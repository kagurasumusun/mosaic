//! `pdm` bus entity (ADR 0029): PDM digital microphone clock/data pair.

use mosaic_contract::model::Ctx;
use mosaic_contract::pdm::{PdmError, PdmMic, PdmSamples, PdmSource};

pub struct PdmBus {
    mic: Option<Box<dyn PdmMic>>,
    rate_hz: u32,
}

impl Default for PdmBus {
    fn default() -> Self {
        Self::new()
    }
}

impl PdmBus {
    pub fn new() -> Self {
        Self {
            mic: None,
            rate_hz: 0,
        }
    }
    pub fn attach_mic(&mut self, m: Box<dyn PdmMic>) {
        self.mic = Some(m);
    }
}

impl PdmSource for PdmBus {
    fn set_rate(&mut self, ctx: &mut dyn Ctx, hz: u32) -> Result<(), PdmError> {
        match self.mic.as_deref_mut() {
            Some(m) => {
                if hz == 0 {
                    return Err(PdmError::BadRate);
                }
                self.rate_hz = hz;
                m.on_rate(ctx, hz)
            }
            None => Err(PdmError::NoMic),
        }
    }
    fn capture(&mut self, ctx: &mut dyn Ctx, count: usize) -> Result<PdmSamples, PdmError> {
        match self.mic.as_deref_mut() {
            Some(m) => {
                if self.rate_hz == 0 {
                    return Err(PdmError::ClockStopped);
                }
                m.sample(ctx, count)
            }
            None => Err(PdmError::NoMic),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::pdm::PdmMic;

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

    struct Mic;
    impl PdmMic for Mic {
        fn on_rate(&mut self, _ctx: &mut dyn Ctx, _hz: u32) -> Result<(), PdmError> {
            Ok(())
        }
        fn sample(&mut self, _ctx: &mut dyn Ctx, count: usize) -> Result<PdmSamples, PdmError> {
            Ok(PdmSamples {
                bits: vec![0xAA; count / 8 + 1],
            })
        }
    }

    #[test]
    fn capture_after_rate_set() {
        let mut bus = PdmBus::new();
        bus.attach_mic(Box::new(Mic));
        let mut ctx = NoopCtx;
        assert!(bus.set_rate(&mut ctx, 2_048_000).is_ok());
        let samples = bus.capture(&mut ctx, 16).unwrap();
        assert_eq!(samples.bits.len(), 3);
    }

    #[test]
    fn no_mic_yields_error() {
        let mut bus = PdmBus::new();
        let mut ctx = NoopCtx;
        assert_eq!(bus.set_rate(&mut ctx, 1_024_000), Err(PdmError::NoMic));
    }

    #[test]
    fn capture_without_rate_is_clock_stopped() {
        let mut bus = PdmBus::new();
        bus.attach_mic(Box::new(Mic));
        let mut ctx = NoopCtx;
        assert_eq!(bus.capture(&mut ctx, 8), Err(PdmError::ClockStopped));
    }
}
