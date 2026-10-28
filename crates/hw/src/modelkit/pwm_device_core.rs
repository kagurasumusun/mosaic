//! PWM device core (4-pin fans, RC servos, buzzers) — the shared functional
//! core behind every `part:*/...` PWM-consuming device.
//!
//! One family, one core (ADR 0026 step 2 pattern): the device records the
//! last edge-per-pulse signal per channel and derives the observable behavior
//! — a fan's RPM from the duty cycle and pole pairs, a servo's shaft angle
//! from a 50 Hz pulse width, a buzzer's on/off state from a nonzero duty.

use mosaic_contract::model::Ctx;
use mosaic_contract::pwm::{PwmError, PwmSignal, PwmSink};

/// One channel's captured state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChannelState {
    pub period_ns: u32,
    pub duty_ns: u32,
}

impl ChannelState {
    /// Duty cycle in percent (0-100).
    pub fn duty_percent(&self) -> u8 {
        if self.period_ns == 0 {
            0
        } else {
            ((self.duty_ns as u64 * 100) / self.period_ns as u64).min(100) as u8
        }
    }
}

/// One PWM-consuming device.
pub struct PwmDeviceCore {
    pole_pairs: u32,
    channels: Vec<ChannelState>,
}

impl Default for PwmDeviceCore {
    fn default() -> Self {
        Self::new()
    }
}

impl PwmDeviceCore {
    /// A single-channel device (fan or servo).
    pub fn new() -> Self {
        Self {
            pole_pairs: 4,
            channels: vec![ChannelState::default()],
        }
    }

    /// Motor pole pairs (fans are usually 4; sets the RPM curve).
    pub fn with_pole_pairs(mut self, pole_pairs: u32) -> Self {
        self.pole_pairs = pole_pairs.max(1);
        self
    }

    /// Track N channels.
    pub fn with_channels(mut self, count: usize) -> Self {
        self.channels = vec![ChannelState::default(); count.max(1)];
        self
    }

    /// The captured state of one channel.
    pub fn channel(&self, index: usize) -> Option<ChannelState> {
        self.channels.get(index).copied()
    }

    /// A 4-pin fan's speed at 25 kHz, in RPM: 0% duty = 0 RPM (the fan is
    /// stopped or in its minimum-RPM region), 100% = 3000 RPM for a 4-pole
    /// fan driven at 25 kHz — scaled linearly between.
    pub fn rpm(&self) -> u32 {
        let state = self.channels.first().copied().unwrap_or_default();
        if state.period_ns == 0 {
            return 0;
        }
        (state.duty_percent() as u32 * 3000) / 100
    }

    /// An RC servo's shaft angle at 50 Hz, in tenths of a degree: 1000 µs =
    /// -900 (90° counterclockwise), 1500 µs = center, 2000 µs = +900.
    pub fn servo_angle_tenths(&self) -> i32 {
        let state = self.channels.first().copied().unwrap_or_default();
        if state.period_ns == 0 {
            return 0;
        }
        let pulse_us = state.duty_ns / 1000;
        match pulse_us {
            0 => 0,
            us => ((us as i64 - 1500) * 900 / 500) as i32,
        }
    }

    /// A buzzer's audible state: any nonzero duty drives the diaphragm.
    pub fn buzzing(&self) -> bool {
        self.channels
            .first()
            .map(|c| c.duty_ns > 0)
            .unwrap_or(false)
    }
}

impl PwmSink for PwmDeviceCore {
    fn on_pwm(
        &mut self,
        _ctx: &mut dyn Ctx,
        channel: u8,
        signal: &PwmSignal,
    ) -> Result<(), PwmError> {
        let index = channel as usize;
        let slot = self.channels.get_mut(index).ok_or(PwmError::BadChannel)?;
        *slot = ChannelState {
            period_ns: signal.period_ns,
            duty_ns: signal.duty_ns,
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn signal(period_ns: u32, duty_ns: u32) -> PwmSignal {
        PwmSignal { period_ns, duty_ns }
    }

    #[test]
    fn duty_percent_is_captured() {
        let mut fan = PwmDeviceCore::new();
        let mut ctx = NoopCtx;
        assert!(fan.on_pwm(&mut ctx, 0, &signal(40_000, 20_000)).is_ok());
        assert_eq!(
            fan.channel(0),
            Some(ChannelState {
                period_ns: 40_000,
                duty_ns: 20_000
            })
        );
        assert_eq!(fan.channel(0).unwrap().duty_percent(), 50);
    }

    #[test]
    fn rpm_follows_duty() {
        let mut fan = PwmDeviceCore::new();
        let mut ctx = NoopCtx;
        fan.on_pwm(&mut ctx, 0, &signal(40_000, 40_000)).unwrap();
        assert_eq!(fan.rpm(), 3000);
        fan.on_pwm(&mut ctx, 0, &signal(40_000, 0)).unwrap();
        assert_eq!(fan.rpm(), 0);
    }

    #[test]
    fn servo_angle_from_pulse_width() {
        let mut servo = PwmDeviceCore::new();
        let mut ctx = NoopCtx;
        // 50 Hz: period 20 ms; center = 1500 µs.
        servo
            .on_pwm(&mut ctx, 0, &signal(20_000_000, 1_500_000))
            .unwrap();
        assert_eq!(servo.servo_angle_tenths(), 0);
        servo
            .on_pwm(&mut ctx, 0, &signal(20_000_000, 2_000_000))
            .unwrap();
        assert_eq!(servo.servo_angle_tenths(), 900);
        servo
            .on_pwm(&mut ctx, 0, &signal(20_000_000, 1_000_000))
            .unwrap();
        assert_eq!(servo.servo_angle_tenths(), -900);
    }

    #[test]
    fn unknown_channel_is_rejected() {
        let mut device = PwmDeviceCore::new();
        let mut ctx = NoopCtx;
        assert_eq!(
            device.on_pwm(&mut ctx, 5, &signal(1000, 500)),
            Err(PwmError::BadChannel)
        );
    }

    #[test]
    fn buzzing_tracks_duty() {
        let mut buzzer = PwmDeviceCore::new();
        let mut ctx = NoopCtx;
        assert!(!buzzer.buzzing());
        buzzer
            .on_pwm(&mut ctx, 0, &signal(1_000_000, 500_000))
            .unwrap();
        assert!(buzzer.buzzing());
    }
}
