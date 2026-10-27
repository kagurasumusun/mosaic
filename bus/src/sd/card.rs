//! The SD/eMMC card-side port: commands in, responses and data out.
//!
//! Clean-room from the SD Physical Layer Simplified Specification 9.0 (the
//! command/response protocol) and the JEDEC eMMC 5.1 (the MMC variants:
//! `CMD1` OCR negotiation, `CMD8` SEND_EXT_CSD). The host-side port is the
//! trait; the card models in `hw/sd/` implement it. Timing (Ncr/Nrc delays)
//! is not modelled — transactions are instantaneous, deterministic events.

use std::sync::{Arc, Mutex};

/// Why a command was not answered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SdError {
    /// The card is in a state where this command is illegal.
    IllegalCommand,
    /// The card was never given a card to hold.
    NoCard,
    /// A data block beyond the card's extents.
    OutOfRange,
    /// A CRC check failed (functional models report, never invent).
    CrcError,
}

/// One card-side response, in the response-type shape the spec gives it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SdResponse {
    /// R1: 32-bit status word.
    Status(u32),
    /// R2 (CID or CSD): the 128-bit register contents, as 16 bytes.
    Long([u8; 16]),
    /// R3 (OCR) / R7 (CMD8 reply): 32 bits.
    Ocr(u32),
    /// R6 (published RCA): 32 bits.
    Rca(u32),
    /// Data phase: the card has `bytes` ready for the host to read.
    Data,
}

/// One response type code, as host drivers request them (SD spec §4.9.2).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SdResponseType {
    None,
    R1,
    R1b,
    R2,
    R3,
    R6,
    R7,
}

/// The card-side port: every command a host controller can issue, at
/// transaction granularity.
pub trait SdCardPort: Send {
    /// Issue `cmd` with `argument` (`cmd` includes the 0x40 start bit base,
    /// e.g. `CMD0` = 0x40). The response shape tells the caller how to read
    /// the answer; `Ok(None)` = no response phase.
    fn command(
        &mut self,
        cmd: u8,
        argument: u32,
        response_type: SdResponseType,
    ) -> Result<Option<SdResponse>, SdError>;

    /// Read one data block of `block_len` bytes at `lba`.
    fn read_block(&mut self, lba: u64, block_len: usize) -> Result<Vec<u8>, SdError>;

    /// Write one data block at `lba`.
    fn write_block(&mut self, lba: u64, data: &[u8]) -> Result<(), SdError>;

    /// Stop a multi-block transfer (CMD12 has no data phase).
    fn stop_transmission(&mut self) -> Result<(), SdError>;
}

/// A shareable handle to a card, the way boards hand cards to host
/// controllers: the model instance stays with the card model.
pub struct SdCardHandle {
    card: Mutex<Box<dyn SdCardPort>>,
}

impl SdCardHandle {
    pub fn new(card: Box<dyn SdCardPort>) -> Self {
        Self {
            card: Mutex::new(card),
        }
    }

    pub fn command(
        &self,
        cmd: u8,
        argument: u32,
        response_type: SdResponseType,
    ) -> Result<Option<SdResponse>, SdError> {
        match self.card.lock() {
            Ok(mut card) => card.command(cmd, argument, response_type),
            Err(_) => Err(SdError::NoCard),
        }
    }

    pub fn read_block(&self, lba: u64, block_len: usize) -> Result<Vec<u8>, SdError> {
        match self.card.lock() {
            Ok(mut card) => card.read_block(lba, block_len),
            Err(_) => Err(SdError::NoCard),
        }
    }

    pub fn write_block(&self, lba: u64, data: &[u8]) -> Result<(), SdError> {
        match self.card.lock() {
            Ok(mut card) => card.write_block(lba, data),
            Err(_) => Err(SdError::NoCard),
        }
    }

    pub fn stop_transmission(&self) -> Result<(), SdError> {
        match self.card.lock() {
            Ok(mut card) => card.stop_transmission(),
            Err(_) => Err(SdError::NoCard),
        }
    }
}

/// Wrap a card model into the shareable handle boards pass to host
/// controllers.
pub fn card_handle(card: Box<dyn SdCardPort>) -> Arc<SdCardHandle> {
    Arc::new(SdCardHandle::new(card))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal card that answers CMD0 and reads zeros.
    struct TestCard;

    impl SdCardPort for TestCard {
        fn command(
            &mut self,
            cmd: u8,
            _argument: u32,
            response_type: SdResponseType,
        ) -> Result<Option<SdResponse>, SdError> {
            match cmd {
                0x40 => Ok(None),
                0x4D => Ok(Some(SdResponse::Status(0x0000_0100))),
                _ => {
                    let _ = response_type;
                    Err(SdError::IllegalCommand)
                }
            }
        }

        fn read_block(&mut self, _lba: u64, block_len: usize) -> Result<Vec<u8>, SdError> {
            Ok(vec![0; block_len])
        }

        fn write_block(&mut self, _lba: u64, _data: &[u8]) -> Result<(), SdError> {
            Ok(())
        }

        fn stop_transmission(&mut self) -> Result<(), SdError> {
            Ok(())
        }
    }

    #[test]
    fn commands_reach_the_card_through_the_handle() {
        let handle = card_handle(Box::new(TestCard));
        assert_eq!(handle.command(0x40, 0, SdResponseType::None), Ok(None));
        assert_eq!(
            handle.command(0x4D, 0, SdResponseType::R1),
            Ok(Some(SdResponse::Status(0x100)))
        );
        assert_eq!(
            handle.command(0x99, 0, SdResponseType::None),
            Err(SdError::IllegalCommand)
        );
        assert_eq!(handle.read_block(0, 512).unwrap().len(), 512);
    }
}
