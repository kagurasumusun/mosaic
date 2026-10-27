//! `ps2` contract: PS/2 keyboard/mouse serial (bidirectional, 16-bit frames).

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Ps2Error {
    NoTarget,
    Overflow,
    Reset,
    BadParity,
}

pub trait Ps2Host {
    /// Send a byte from host to device (e.g. set LEDs, reset, disable stream).
    fn send(&mut self, ctx: &mut dyn Ctx, byte: u8) -> Result<(), Ps2Error>;
    /// Pull the next scancode/movement byte emitted by the device.
    fn recv(&mut self, ctx: &mut dyn Ctx) -> Result<u8, Ps2Error>;
}

pub trait Ps2Device {
    /// Byte received from host.
    fn on_recv(&mut self, ctx: &mut dyn Ctx, byte: u8) -> Result<(), Ps2Error>;
    /// Next byte the device wants to push to host.
    fn poll(&mut self, ctx: &mut dyn Ctx) -> Option<u8>;
}
