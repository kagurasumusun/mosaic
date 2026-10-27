//! `pmbus` contract: PMBus (Power Management Bus): SMBus-derived, power-supply commands.

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PmbusError {
    Nack,
    BadCommand,
    BadFormat,
    Fault,
    Overflow,
}

/// PMBus standard command codes (subset).
pub mod cmd {
    pub const PAGE: u8 = 0x00;
    pub const CLEAR_FAULTS: u8 = 0x03;
    pub const CAPABILITY: u8 = 0x19;
    pub const VOUT_MODE: u8 = 0x20;
    pub const VOUT_COMMAND: u8 = 0x21;
    pub const VOUT_OV_FAULT_LIMIT: u8 = 0x40;
    pub const STATUS_BYTE: u8 = 0x78;
    pub const STATUS_WORD: u8 = 0x79;
    pub const STATUS_VOUT: u8 = 0x7A;
    pub const STATUS_IOUT: u8 = 0x7B;
    pub const STATUS_TEMPERATURE: u8 = 0x7D;
    pub const READ_VIN: u8 = 0x88;
    pub const READ_VOUT: u8 = 0x8B;
    pub const READ_IOUT: u8 = 0x8C;
    pub const READ_TEMPERATURE_1: u8 = 0x8D;
    pub const READ_TEMPERATURE_2: u8 = 0x8E;
    pub const READ_FAN_SPEED_1: u8 = 0x90;
    pub const PMBUS_REVISION: u8 = 0x98;
}

pub trait PmbusInitiator {
    fn read_byte(&mut self, ctx: &mut dyn Ctx, addr: u8, cmd: u8) -> Result<u8, PmbusError>;
    fn read_word(&mut self, ctx: &mut dyn Ctx, addr: u8, cmd: u8) -> Result<u16, PmbusError>;
    fn read_block(&mut self, ctx: &mut dyn Ctx, addr: u8, cmd: u8) -> Result<Vec<u8>, PmbusError>;
    fn write_byte(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: u8,
        cmd: u8,
        value: u8,
    ) -> Result<(), PmbusError>;
    fn write_word(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: u8,
        cmd: u8,
        value: u16,
    ) -> Result<(), PmbusError>;
    fn write_block(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: u8,
        cmd: u8,
        data: &[u8],
    ) -> Result<(), PmbusError>;
}

pub trait PmbusTarget {
    fn address(&self) -> u8;
    fn command(
        &mut self,
        ctx: &mut dyn Ctx,
        cmd: u8,
        write: bool,
        data: &mut Vec<u8>,
    ) -> Result<(), PmbusError>;
}
