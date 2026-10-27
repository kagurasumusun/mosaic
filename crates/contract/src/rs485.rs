//! `rs485` contract: RS-485 multi-drop serial: half-duplex with direction control.

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Rs485Error {
    NoResponder,
    BadAddress,
    Collision,
    Timeout,
    Overflow,
}

pub trait Rs485Initiator {
    fn send_to(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: u8,
        payload: &[u8],
    ) -> Result<Vec<u8>, Rs485Error>;
    fn broadcast(&mut self, ctx: &mut dyn Ctx, payload: &[u8]) -> Result<(), Rs485Error>;
}

pub trait Rs485Target {
    fn address(&self) -> u8;
    fn receive(&mut self, ctx: &mut dyn Ctx, payload: &[u8])
        -> Result<Option<Vec<u8>>, Rs485Error>;
}
