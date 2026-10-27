//! SPI serial bus contract (構成設計書 §11).
//!
//! Chip-select management and clock polarity/phase travel explicitly with
//! every transfer, so one controller can address targets with different mode
//! requirements. The bus entity lives in `mosaic-interconnect-spi-bus`.

use crate::model::Ctx;

/// Clock polarity (CPOL) and clock phase (CPHA).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct SpiMode {
    pub cpol: bool,
    pub cpha: bool,
}

impl SpiMode {
    pub const fn new(cpol: bool, cpha: bool) -> Self {
        Self { cpol, cpha }
    }

    /// Mode number 0..=3 as defined by the CPOL/CPHA table.
    pub const fn from_number(number: u8) -> Result<Self, SpiError> {
        match number {
            0 => Ok(Self::new(false, false)),
            1 => Ok(Self::new(false, true)),
            2 => Ok(Self::new(true, false)),
            3 => Ok(Self::new(true, true)),
            _ => Err(SpiError::BadMode),
        }
    }
}

/// Chip-select line identifier. The interconnect maps it to one target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpiCs(pub u8);

/// SPI failure modes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SpiError {
    /// No target is wired to this chip-select line.
    NoTarget,
    /// A target rejected the requested clock mode.
    BadMode,
    /// A target FIFO could not accept the frame.
    Overflow,
    /// Mode fault, e.g. a second master selected while this one was active.
    ModeFault,
    /// The composition root did not wire a bus.
    NotConnected,
}

/// Controller-side port implemented by the interconnect bus.
pub trait SpiInitiator: Send {
    /// Assert or release one chip-select line independently of a transfer.
    fn select(&mut self, ctx: &mut dyn Ctx, cs: SpiCs, assert: bool) -> Result<(), SpiError>;

    /// Full-duplex frame: `mosi` and `miso` must share a length, the selected
    /// target exchanges one byte pair per clock.
    fn exchange(
        &mut self,
        ctx: &mut dyn Ctx,
        cs: SpiCs,
        mode: SpiMode,
        mosi: &[u8],
        miso: &mut [u8],
    ) -> Result<(), SpiError>;
}

/// Target-side role implemented by parts such as flash or sensor chips.
pub trait SpiTarget: Send {
    /// Chip-select state change. Deselect must reset any frame pointer.
    fn selected(&mut self, ctx: &mut dyn Ctx, assert: bool);

    /// Exchange one byte under the given clock mode.
    fn exchange_byte(&mut self, ctx: &mut dyn Ctx, mode: SpiMode, mosi: u8)
        -> Result<u8, SpiError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_numbers_map_to_cpol_cpha() {
        assert_eq!(SpiMode::from_number(0), Ok(SpiMode::new(false, false)));
        assert_eq!(SpiMode::from_number(1), Ok(SpiMode::new(false, true)));
        assert_eq!(SpiMode::from_number(2), Ok(SpiMode::new(true, false)));
        assert_eq!(SpiMode::from_number(3), Ok(SpiMode::new(true, true)));
        assert_eq!(SpiMode::from_number(4), Err(SpiError::BadMode));
    }
}
