//! `slimbus` contract: SLIMbus (Serial Low-power Inter-chip Media Bus).

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SlimbusError {
    NoManager,
    NoFramer,
    BadFrame,
    Overflow,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SlimbusMessage {
    pub destination: u8,
    pub source: u8,
    pub code: u8,
    pub value: u32,
}

pub trait SlimbusManager {
    fn send_message(&mut self, ctx: &mut dyn Ctx, m: &SlimbusMessage) -> Result<(), SlimbusError>;
    fn schedule_channels(&mut self, ctx: &mut dyn Ctx, slots: u32) -> Result<(), SlimbusError>;
}

pub trait SlimbusDevice {
    fn on_message(&mut self, ctx: &mut dyn Ctx, m: &SlimbusMessage) -> Result<(), SlimbusError>;
}
