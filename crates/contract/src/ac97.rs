//! `ac97` contract: AC'97 audio codec bus (Intel): AC-Link 20-bit frames.

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Ac97Error {
    NoCodec,
    BadSlot,
    Overflow,
    Underrun,
}

pub trait Ac97Initiator {
    fn read_register(&mut self, ctx: &mut dyn Ctx, codec_id: u8, reg: u8)
        -> Result<u16, Ac97Error>;
    fn write_register(
        &mut self,
        ctx: &mut dyn Ctx,
        codec_id: u8,
        reg: u8,
        value: u16,
    ) -> Result<(), Ac97Error>;
    fn stream(&mut self, ctx: &mut dyn Ctx, pcm_out: &[u16]) -> Result<Vec<u16>, Ac97Error>;
}

pub trait Ac97Target {
    fn codec_id(&self) -> u8;
    fn read_register(&mut self, ctx: &mut dyn Ctx, reg: u8) -> Result<u16, Ac97Error>;
    fn write_register(&mut self, ctx: &mut dyn Ctx, reg: u8, value: u16) -> Result<(), Ac97Error>;
    fn stream(&mut self, ctx: &mut dyn Ctx, pcm_in: &[u16]) -> Result<Vec<u16>, Ac97Error>;
}
