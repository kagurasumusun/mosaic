//! `gameport` bus entity (ADR 0025 step 3): IBM PC analog gameport.

use mosaic_contract::gameport::{GamePortError, GamePortHost, GamePortState, GamePortStick};
use mosaic_contract::model::Ctx;

pub struct GamePortBus {
    stick: Option<Box<dyn GamePortStick>>,
}

impl Default for GamePortBus {
    fn default() -> Self {
        Self::new()
    }
}

impl GamePortBus {
    pub fn new() -> Self {
        Self { stick: None }
    }
    pub fn attach(&mut self, s: Box<dyn GamePortStick>) {
        self.stick = Some(s);
    }
}

impl GamePortHost for GamePortBus {
    fn read_state(&mut self, _ctx: &mut dyn Ctx) -> Result<GamePortState, GamePortError> {
        match self.stick.as_deref() {
            Some(s) => Ok(s.state()),
            None => Err(GamePortError::NoStick),
        }
    }
    fn trigger_dac(
        &mut self,
        ctx: &mut dyn Ctx,
        channel: u8,
        value: u8,
    ) -> Result<(), GamePortError> {
        match self.stick.as_deref_mut() {
            Some(s) => s.on_dac(ctx, channel, value),
            None => Err(GamePortError::NoStick),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::gameport::{GamePortState, GamePortStick};
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

    struct Stick {
        state: GamePortState,
    }
    impl GamePortStick for Stick {
        fn state(&self) -> GamePortState {
            self.state
        }
        fn on_dac(&mut self, _: &mut dyn Ctx, _: u8, _: u8) -> Result<(), GamePortError> {
            Ok(())
        }
    }

    #[test]
    fn read_state_returns_attached_stick_state() {
        let mut bus = GamePortBus::new();
        let state = GamePortState {
            axis_x1: 0x1000,
            axis_y1: 0x2000,
            button1: true,
            ..Default::default()
        };
        bus.attach(Box::new(Stick { state }));
        let mut ctx = NoopCtx;
        let s = bus.read_state(&mut ctx).unwrap();
        assert_eq!(s.axis_x1, 0x1000);
        assert!(s.button1);
    }

    #[test]
    fn no_stick_errors() {
        let mut bus = GamePortBus::new();
        let mut ctx = NoopCtx;
        assert_eq!(bus.read_state(&mut ctx), Err(GamePortError::NoStick));
    }
}
