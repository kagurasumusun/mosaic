//! `gameport` contract: legacy IBM PC analog gameport (15-pin DA-15).

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum GamePortError {
    NoStick,
    Timeout,
}

/// Quadrature axis reading: 0..=65535 across the joystick's full travel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GamePortState {
    pub axis_x1: u16,
    pub axis_y1: u16,
    pub axis_x2: u16,
    pub axis_y2: u16,
    pub button1: bool,
    pub button2: bool,
    pub button3: bool,
    pub button4: bool,
}

pub trait GamePortHost {
    fn read_state(&mut self, ctx: &mut dyn Ctx) -> Result<GamePortState, GamePortError>;
    fn trigger_dac(
        &mut self,
        ctx: &mut dyn Ctx,
        channel: u8,
        value: u8,
    ) -> Result<(), GamePortError>;
}

pub trait GamePortStick {
    fn state(&self) -> GamePortState;
    fn on_dac(&mut self, ctx: &mut dyn Ctx, channel: u8, value: u8) -> Result<(), GamePortError>;
}
