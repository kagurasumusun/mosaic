//! `smbus` contract: SMBus (System Management Bus): I2C-derived, with ARP/PEC/alert.

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SmbusError {
    Nack,
    BadAddress,
    PecMismatch,
    Alert,
    Timeout,
    Overflow,
}

/// SMBus Address Resolution Protocol (ARP) address.
pub const SMBUS_ARP_ADDRESS: u8 = 0x61;

pub trait SmbusInitiator {
    fn quick_command(&mut self, ctx: &mut dyn Ctx, addr: u8, read: bool) -> Result<(), SmbusError>;
    fn send_byte(&mut self, ctx: &mut dyn Ctx, addr: u8, byte: u8) -> Result<(), SmbusError>;
    fn receive_byte(&mut self, ctx: &mut dyn Ctx, addr: u8) -> Result<u8, SmbusError>;
    fn read_block(&mut self, ctx: &mut dyn Ctx, addr: u8, cmd: u8) -> Result<Vec<u8>, SmbusError>;
    fn process_call(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: u8,
        cmd: u8,
        value: u16,
    ) -> Result<u16, SmbusError>;
}

pub trait SmbusTarget {
    fn address(&self) -> u8;
    fn receive(&mut self, ctx: &mut dyn Ctx, bytes: &[u8]) -> Result<(), SmbusError>;
    fn respond(&mut self, ctx: &mut dyn Ctx, len: usize) -> Result<Vec<u8>, SmbusError>;
}

/// PEC (Packet Error Code) CRC-8, polynomial 0x07.
pub fn pec(data: &[u8]) -> u8 {
    let mut crc: u8 = 0;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            if crc & 0x80 != 0 {
                crc = (crc << 1) ^ 0x07;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}
