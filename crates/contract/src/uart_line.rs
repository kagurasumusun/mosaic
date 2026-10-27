//! UART line-level contract (構成設計書 §11).
//!
//! UART links are point-to-point pairs: the interconnect entity is a pair,
//! not an addressed bus. This crate fixes the line configuration vocabulary
//! and the endpoint trait; the paired implementation lives in
//! `mosaic-interconnect-uart-pair`.

use crate::model::Ctx;

/// Parity scheme applied to the data bits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Parity {
    None,
    Even,
    Odd,
}

/// Full line configuration. Construct through [`UartConfig::new`] so that
/// illegal frame parameters are rejected up front.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct UartConfig {
    pub baud: u32,
    pub data_bits: u8,
    pub parity: Parity,
    pub stop_bits: u8,
}

impl UartConfig {
    pub const fn new(
        baud: u32,
        data_bits: u8,
        parity: Parity,
        stop_bits: u8,
    ) -> Result<Self, UartError> {
        if baud == 0 {
            return Err(UartError::BadConfig);
        }
        if data_bits < 5 || data_bits > 9 {
            return Err(UartError::BadConfig);
        }
        if stop_bits < 1 || stop_bits > 2 {
            return Err(UartError::BadConfig);
        }
        Ok(Self {
            baud,
            data_bits,
            parity,
            stop_bits,
        })
    }

    /// Conventional 115200 8N1 line.
    pub const fn default_115200_8n1() -> Self {
        Self {
            baud: 115_200,
            data_bits: 8,
            parity: Parity::None,
            stop_bits: 1,
        }
    }
}

/// Line and buffer failure modes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum UartError {
    /// Baud/data/stop combination outside the supported range.
    BadConfig,
    /// A received frame violated the configured parity or stop bits.
    Framing,
    /// Incoming data overran the endpoint buffer.
    Overrun,
    /// No peer endpoint is attached.
    NotConnected,
}

/// One end of a UART pair. Bytes written to `tx` become readable at the peer.
pub trait UartLine: Send {
    fn configure(&mut self, config: UartConfig) -> Result<(), UartError>;

    /// Transmit bytes; returns the number accepted before the buffer filled.
    fn tx(&mut self, ctx: &mut dyn Ctx, bytes: &[u8]) -> Result<usize, UartError>;

    /// Receive bytes previously written by the peer.
    fn rx(&mut self, ctx: &mut dyn Ctx, buf: &mut [u8]) -> Result<usize, UartError>;

    /// Assert or release the break condition on the physical line.
    fn set_break(&mut self, ctx: &mut dyn Ctx, asserted: bool);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_rejects_illegal_frames() {
        assert!(UartConfig::new(0, 8, Parity::None, 1).is_err());
        assert!(UartConfig::new(9600, 4, Parity::None, 1).is_err());
        assert!(UartConfig::new(9600, 8, Parity::None, 3).is_err());
        assert!(UartConfig::new(9600, 7, Parity::Even, 1).is_ok());
    }

    #[test]
    fn default_line_is_115200_8n1() {
        let config = UartConfig::default_115200_8n1();
        assert_eq!(config.baud, 115_200);
        assert_eq!(config.data_bits, 8);
        assert_eq!(config.parity, Parity::None);
        assert_eq!(config.stop_bits, 1);
    }
}
