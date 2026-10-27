//! `flexray` contract: FlexRay communication system (TDMA dual-channel).

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum FlexRayError {
    NoNode,
    BadChannel,
    BadSlot,
    Overflow,
    Collision,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrChannel { A, B, Both }

pub trait FlexRayInitiator {
    fn send(&mut self, ctx: &mut dyn Ctx, ch: FrChannel, slot: u16, payload: &[u8]) -> Result<(), FlexRayError>;
    fn recv(&mut self, ctx: &mut dyn Ctx, ch: FrChannel, slot: u16) -> Result<Vec<u8>, FlexRayError>;
}

pub trait FlexRayTarget {
    fn node_id(&self) -> u8;
    fn accepts(&self, ch: FrChannel, slot: u16) -> bool;
    fn send(&mut self, ctx: &mut dyn Ctx, ch: FrChannel, slot: u16, payload: &[u8]) -> Result<(), FlexRayError>;
    fn recv(&mut self, ctx: &mut dyn Ctx, ch: FrChannel, slot: u16) -> Result<Vec<u8>, FlexRayError>;
}
