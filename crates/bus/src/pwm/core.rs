//! `pwm` bus entity (ADR 0029): pulse-width-modulated channel outputs.

use mosaic_contract::model::Ctx;
use mosaic_contract::pwm::{PwmError, PwmSignal, PwmSink, PwmSource};

/// One channel: the signal state plus the attached sink (fan, servo, ...).
struct Channel {
    signal: PwmSignal,
    sink: Option<Box<dyn PwmSink>>,
}

pub struct PwmBus {
    channels: Vec<Channel>,
}

impl Default for PwmBus {
    fn default() -> Self {
        Self::new()
    }
}

impl PwmBus {
    pub fn new() -> Self {
        Self {
            channels: Vec::new(),
        }
    }
    pub fn attach_sink(&mut self, channel: u8, sink: Box<dyn PwmSink>) {
        let index = channel as usize;
        while self.channels.len() <= index {
            self.channels.push(Channel {
                signal: PwmSignal::default(),
                sink: None,
            });
        }
        self.channels[index].sink = Some(sink);
    }
    fn channel(&mut self, channel: u8) -> Result<&mut Channel, PwmError> {
        let index = channel as usize;
        if index >= self.channels.len() {
            return Err(PwmError::BadChannel);
        }
        Ok(&mut self.channels[index])
    }
}

impl PwmSource for PwmBus {
    fn set_period(
        &mut self,
        _ctx: &mut dyn Ctx,
        channel: u8,
        period_ns: u32,
    ) -> Result<(), PwmError> {
        if period_ns == 0 {
            return Err(PwmError::BadPeriod);
        }
        let ch = self.channel(channel)?;
        if ch.signal.duty_ns > period_ns {
            return Err(PwmError::BadDuty);
        }
        ch.signal.period_ns = period_ns;
        Ok(())
    }
    fn set_duty(&mut self, _ctx: &mut dyn Ctx, channel: u8, duty_ns: u32) -> Result<(), PwmError> {
        let ch = self.channel(channel)?;
        if duty_ns > ch.signal.period_ns {
            return Err(PwmError::BadDuty);
        }
        ch.signal.duty_ns = duty_ns;
        Ok(())
    }
    fn enable(&mut self, ctx: &mut dyn Ctx, channel: u8) -> Result<(), PwmError> {
        let ch = self.channel(channel)?;
        match ch.sink.as_deref_mut() {
            Some(sink) => sink.on_pwm(ctx, channel, &ch.signal),
            None => Err(PwmError::NoSink),
        }
    }
    fn disable(&mut self, _ctx: &mut dyn Ctx, channel: u8) -> Result<(), PwmError> {
        let ch = self.channel(channel)?;
        ch.signal.duty_ns = 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::pwm::PwmSink;

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

    struct Fan {
        pulses: u32,
    }
    impl PwmSink for Fan {
        fn on_pwm(
            &mut self,
            _ctx: &mut dyn Ctx,
            _channel: u8,
            signal: &PwmSignal,
        ) -> Result<(), PwmError> {
            if signal.period_ns > 0 {
                self.pulses += 1;
            }
            Ok(())
        }
    }

    #[test]
    fn enable_drives_the_sink() {
        let mut bus = PwmBus::new();
        bus.attach_sink(0, Box::new(Fan { pulses: 0 }));
        let mut ctx = NoopCtx;
        bus.set_period(&mut ctx, 0, 40_000).unwrap();
        bus.set_duty(&mut ctx, 0, 20_000).unwrap();
        assert!(bus.enable(&mut ctx, 0).is_ok());
    }

    #[test]
    fn duty_above_period_is_rejected() {
        let mut bus = PwmBus::new();
        bus.attach_sink(0, Box::new(Fan { pulses: 0 }));
        let mut ctx = NoopCtx;
        bus.set_period(&mut ctx, 0, 40_000).unwrap();
        assert_eq!(bus.set_duty(&mut ctx, 0, 40_001), Err(PwmError::BadDuty));
    }

    #[test]
    fn unknown_channel_is_rejected() {
        let mut bus = PwmBus::new();
        let mut ctx = NoopCtx;
        assert_eq!(bus.set_duty(&mut ctx, 7, 1), Err(PwmError::BadChannel));
    }
}
