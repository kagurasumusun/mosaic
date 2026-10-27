//! `pwm` contract: pulse-width-modulated channel outputs.

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PwmError {
    NoSink,
    BadChannel,
    BadDuty,
    BadPeriod,
}

/// One PWM channel period: period and duty in nanoseconds.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PwmSignal {
    pub period_ns: u32,
    pub duty_ns: u32,
}

pub trait PwmSource {
    fn set_period(
        &mut self,
        ctx: &mut dyn Ctx,
        channel: u8,
        period_ns: u32,
    ) -> Result<(), PwmError>;
    fn set_duty(&mut self, ctx: &mut dyn Ctx, channel: u8, duty_ns: u32) -> Result<(), PwmError>;
    fn enable(&mut self, ctx: &mut dyn Ctx, channel: u8) -> Result<(), PwmError>;
    fn disable(&mut self, ctx: &mut dyn Ctx, channel: u8) -> Result<(), PwmError>;
}

pub trait PwmSink {
    fn on_pwm(
        &mut self,
        ctx: &mut dyn Ctx,
        channel: u8,
        signal: &PwmSignal,
    ) -> Result<(), PwmError>;
}
