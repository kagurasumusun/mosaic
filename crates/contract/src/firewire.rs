//! `firewire` contract: IEEE 1394 (FireWire / i.LINK) serial bus.

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum FireWireError {
    NoNode,
    Timeout,
    Crc,
    AckError,
    OutOfOrder,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FwTcode {
    QuadletRead = 0,
    QuadletWrite = 1,
    BlockRead = 2,
    BlockWrite = 3,
}

#[derive(Clone, Debug, Default)]
pub struct FireWirePacket {
    pub source_id: u16,
    pub destination_id: u16,
    pub tlabel: u8,
    pub tcode: u8,
    pub address: u64,
    pub data: Vec<u8>,
}

pub trait FireWireInitiator {
    fn send(&mut self, ctx: &mut dyn Ctx, p: &FireWirePacket) -> Result<Vec<u8>, FireWireError>;
}

pub trait FireWireNode {
    fn on_packet(
        &mut self,
        ctx: &mut dyn Ctx,
        p: &FireWirePacket,
    ) -> Result<Vec<u8>, FireWireError>;
}
