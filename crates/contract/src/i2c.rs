//! I2C serial bus contract (構成設計書 §11).
//!
//! Both representation levels required by the design are covered:
//! transaction-level ports ([`I2cInitiator`] / [`I2cTarget`]) for register
//! models, and the [`I2cPhase`] vocabulary for pin-level bit-bang decoding.
//! The bus entity (address resolution, arbitration, edge decoding) lives in
//! `mosaic-interconnect-i2c-bus`. SMBus extensions will be a separate trait.

use crate::model::Ctx;

/// Bus speed modes. Clock stretching is expressed through [`I2cError`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum I2cSpeed {
    /// 100 kHz.
    Standard,
    /// 400 kHz.
    Fast,
    /// 1 MHz.
    FastPlus,
    /// 3.4 MHz.
    HighSpeed,
}

/// 7-bit or 10-bit target address. Constructors reject reserved ranges.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum I2cAddr {
    Seven(u8),
    Ten(u16),
}

impl I2cAddr {
    /// 7-bit address in 0..=0x7F excluding the 0000xxx/1111xxx reserved pools.
    pub const fn seven(value: u8) -> Result<Self, I2cError> {
        if value > 0x7F || value <= 0x07 || value >= 0x78 {
            return Err(I2cError::BadAddress);
        }
        Ok(Self::Seven(value & 0x7F))
    }

    /// 10-bit address in 0..=0x3FF.
    pub const fn ten(value: u16) -> Result<Self, I2cError> {
        if value > 0x3FF {
            return Err(I2cError::BadAddress);
        }
        Ok(Self::Ten(value))
    }
}

/// Acknowledge level returned by targets and the bus entity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Ack {
    Ack,
    Nack,
}

impl Ack {
    pub const fn is_ack(self) -> bool {
        matches!(self, Self::Ack)
    }
}

/// Transaction failure modes. Every variant is guest-recoverable, never a panic.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum I2cError {
    /// No target answered the address.
    Nack,
    /// The bus is held by another transaction.
    BusBusy,
    /// Arbitration was lost to another initiator.
    ArbitrationLost,
    /// Address outside the legal 7/10-bit ranges.
    BadAddress,
    /// A target requested clock stretching beyond the limit.
    ClockStretch,
    /// A buffer exceeded the model's capacity.
    Overflow,
    /// The composition root did not wire a bus.
    NotConnected,
}

/// Single-wire transmission units, shared by transaction traces and the
/// pin-level bit-bang decoder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum I2cPhase {
    Start,
    RepeatedStart,
    Stop,
    Address { addr: I2cAddr, read: bool },
    Data(u8),
    Acknowledge(Ack),
}

/// Controller-side port. Implemented by the interconnect bus and injected
/// into an initiator model at construction; models never see the bus type.
pub trait I2cInitiator: Send {
    fn set_speed(&mut self, speed: I2cSpeed);

    /// START, address-W, bytes, STOP. A NACK ends the transfer early.
    fn write(&mut self, ctx: &mut dyn Ctx, addr: I2cAddr, bytes: &[u8]) -> Result<(), I2cError>;

    /// START, address-R, fill `buf`, NACK on the final byte, STOP.
    fn read(&mut self, ctx: &mut dyn Ctx, addr: I2cAddr, buf: &mut [u8]) -> Result<(), I2cError>;

    /// WRITE followed by a repeated START and READ without releasing the bus.
    fn write_read(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: I2cAddr,
        out: &[u8],
        input: &mut [u8],
    ) -> Result<(), I2cError>;
}

/// Target-side role implemented by sensor and memory parts.
pub trait I2cTarget: Send {
    fn matches(&self, addr: &I2cAddr) -> bool;

    /// Handle a write frame; the returned level drives SDA during ACK.
    fn target_write(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: &I2cAddr,
        bytes: &[u8],
    ) -> Result<Ack, I2cError>;

    /// Fill a read frame; return NACK only when no byte can be served.
    fn target_read(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: &I2cAddr,
        buf: &mut [u8],
    ) -> Result<Ack, I2cError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seven_bit_addresses_reject_reserved_pools() {
        assert_eq!(I2cAddr::seven(0x76), Ok(I2cAddr::Seven(0x76)));
        assert_eq!(I2cAddr::seven(0x05), Err(I2cError::BadAddress));
        assert_eq!(I2cAddr::seven(0x78), Err(I2cError::BadAddress));
        assert_eq!(I2cAddr::seven(0xFF), Err(I2cError::BadAddress));
    }

    #[test]
    fn ten_bit_addresses_have_a_1024_entry_range() {
        assert_eq!(I2cAddr::ten(0x3FF), Ok(I2cAddr::Ten(0x3FF)));
        assert_eq!(I2cAddr::ten(0x400), Err(I2cError::BadAddress));
    }

    #[test]
    fn ack_carries_its_boolean_state() {
        assert!(Ack::Ack.is_ack());
        assert!(!Ack::Nack.is_ack());
    }
}
