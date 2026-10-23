#![no_std]
#![forbid(unsafe_code)]
//! CAN serial bus contract (構成設計書 §11).
//!
//! Frames carry either a standard 11-bit or extended 29-bit identifier and at
//! most eight data bytes. The bus entity in `mosaic-interconnect-can-bus`
//! performs deterministic identifier arbitration; this crate only fixes the
//! frame shape and the controller/receiver roles.

use mosaic_contract_model::Ctx;

/// CAN identifier with the two frame formats.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
#[non_exhaustive]
pub enum CanId {
    /// Standard 11-bit identifier.
    Base(u16),
    /// Extended 29-bit identifier.
    Extended(u32),
}

impl CanId {
    pub const fn base(value: u16) -> Result<Self, CanError> {
        if value > 0x7FF {
            return Err(CanError::BadIdentifier);
        }
        Ok(Self::Base(value))
    }

    pub const fn extended(value: u32) -> Result<Self, CanError> {
        if value > 0x1FFF_FFFF {
            return Err(CanError::BadIdentifier);
        }
        Ok(Self::Extended(value))
    }

    /// Numeric priority: lower identifiers win arbitration first.
    pub const fn priority(self) -> u32 {
        match self {
            Self::Base(value) => u32::from(value),
            Self::Extended(value) => value,
        }
    }
}

/// One CAN data or remote frame. Construction validates the DLC so that no
/// invalid frame can reach a receiver.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CanFrame {
    id: CanId,
    dlc: u8,
    data: [u8; 8],
    remote: bool,
}

impl CanFrame {
    pub fn new(id: CanId, data: &[u8], remote: bool) -> Result<Self, CanError> {
        if data.len() > 8 {
            return Err(CanError::BadFrame);
        }
        let mut frame = Self {
            id,
            dlc: data.len() as u8,
            data: [0; 8],
            remote,
        };
        frame.data[..data.len()].copy_from_slice(data);
        Ok(frame)
    }

    pub const fn id(&self) -> CanId {
        self.id
    }

    pub const fn dlc(&self) -> u8 {
        self.dlc
    }

    pub const fn is_remote(&self) -> bool {
        self.remote
    }

    pub fn data(&self) -> &[u8] {
        &self.data[..usize::from(self.dlc)]
    }
}

/// CAN failure modes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CanError {
    /// Identifier outside its frame format's width.
    BadIdentifier,
    /// DLC above 8 or another malformed frame.
    BadFrame,
    /// The controller lost non-destructive arbitration.
    ArbitrationLost,
    /// The controller entered bus-off and may not transmit yet.
    BusOff,
    /// The bus entity had no capacity for the frame.
    Full,
    /// The composition root did not wire a bus.
    NotConnected,
}

/// Transmit-side role implemented by a CAN controller model.
pub trait CanController: Send {
    fn transmit(
        &mut self,
        ctx: &mut dyn Ctx,
        frame: CanFrame,
    ) -> Result<(), CanError>;
}

/// Receive-side role implemented by ECU models and transceivers.
pub trait CanReceiver: Send {
    /// Hardware acceptance filter; the bus only delivers accepted frames.
    fn accepts(&self, id: &CanId) -> bool;

    fn receive(&mut self, ctx: &mut dyn Ctx, frame: &CanFrame);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_stay_inside_their_widths() {
        assert!(CanId::base(0x7FF).is_ok());
        assert_eq!(CanId::base(0x800), Err(CanError::BadIdentifier));
        assert!(CanId::extended(0x1FFF_FFFF).is_ok());
        assert_eq!(CanId::extended(0x2000_0000), Err(CanError::BadIdentifier));
    }

    #[test]
    fn frames_validate_dlc_and_expose_only_valid_bytes() {
        let frame = CanFrame::new(CanId::Base(0x100), &[1, 2, 3], false).unwrap();
        assert_eq!(frame.dlc(), 3);
        assert_eq!(frame.data(), &[1, 2, 3]);
        assert!(CanFrame::new(CanId::Base(0), &[0; 9], false).is_err());
    }

    #[test]
    fn arbitration_priority_follows_the_identifier_number() {
        assert!(CanId::Base(0x10).priority() < CanId::Base(0x20).priority());
    }
}
