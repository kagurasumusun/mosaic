//! DMA upstream port: where a DMA engine's transfers land.
//!
//! A DMA controller never owns the bus (ADR 0025): the board attaches this
//! port — an adapter over the machine's memory view or an engine's DMA
//! path — and the controller writes and reads through it. The device-side
//! byte source (`device_byte`) is the single-byte handshake a channel's
//! device presents; block-wise DMA (SDHCI's SDMA) reads and writes whole
//! blocks through the same port.

/// Why a DMA transfer did not complete.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TransferError {
    /// The address is not backed on this machine.
    Unmapped { address: u32 },
    /// The peer device has no byte ready.
    NoData,
}

/// The upstream side of a DMA engine.
pub trait DmaUpstream: Send {
    /// Write one byte into memory.
    fn write_u8(&mut self, address: u32, value: u8) -> Result<(), TransferError>;

    /// Read one byte from memory.
    fn read_u8(&mut self, address: u32) -> Result<u8, TransferError>;

    /// The byte a device presents on `channel` for one single transfer.
    fn device_byte(&mut self, channel: usize) -> Result<u8, TransferError>;

    /// Write a whole block (SDHCI SDMA and block transfers).
    fn write_block(&mut self, address: u32, data: &[u8]) -> Result<(), TransferError> {
        for (index, byte) in data.iter().enumerate() {
            let at = address as u64 + index as u64;
            if at > u32::MAX as u64 {
                return Err(TransferError::Unmapped { address });
            }
            self.write_u8(at as u32, *byte)?;
        }
        Ok(())
    }

    /// Read a whole block.
    fn read_block(&mut self, address: u32, data: &mut [u8]) -> Result<(), TransferError> {
        for (index, byte) in data.iter_mut().enumerate() {
            let at = address as u64 + index as u64;
            if at > u32::MAX as u64 {
                return Err(TransferError::Unmapped { address });
            }
            *byte = self.read_u8(at as u32)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Ram {
        memory: Vec<u8>,
    }

    impl DmaUpstream for Ram {
        fn write_u8(&mut self, address: u32, value: u8) -> Result<(), TransferError> {
            self.memory[address as usize] = value;
            Ok(())
        }

        fn read_u8(&mut self, address: u32) -> Result<u8, TransferError> {
            Ok(self.memory[address as usize])
        }

        fn device_byte(&mut self, _channel: usize) -> Result<u8, TransferError> {
            Ok(0x5A)
        }
    }

    #[test]
    fn block_transfers_round_trip() {
        let mut ram = Ram {
            memory: vec![0; 4096],
        };
        ram.write_block(0x100, &[1, 2, 3, 4]).unwrap();
        let mut out = [0u8; 4];
        ram.read_block(0x100, &mut out).unwrap();
        assert_eq!(out, [1, 2, 3, 4]);
    }
}
