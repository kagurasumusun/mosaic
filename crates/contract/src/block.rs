//! Block image vocabulary — QEMU's `block-backend` role.
//!
//! Every storage-consuming model (virtio-blk, AHCI, IDE, SD hosts, NVMe)
//! needs the same thing: a random-access, sector-granular image whose
//! provenance the model does not know. Implementations live with the
//! machine harness — an in-memory image for tests, a file-backed image in
//! the host layer — and reach models through [`HostAttachments`]
//! (ADR 0033 §3). A model that grows its own caching layers still sits on
//! this port; the port is the *boundary*, not the storage engine.
//!
//! Determinism rule (ADR 0022's sibling for storage): the image is
//! host-owned state, so it is deliberately **not** part of any model
//! snapshot. A restored machine sees the same image bytes the harness
//! holds, exactly like a real disk surviving a reboot.

/// Why a block request did not complete.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum BlockError {
    /// The request crossed the end of the image.
    OutOfRange,
    /// The image is read-only and the request wrote.
    ReadOnly,
    /// The backing failed (I/O error in the host layer).
    Io,
}

/// A random-access block image. Addresses are byte offsets from the image
/// start; the trait imposes no sector alignment so a model can forward a
/// guest's unaligned access pattern unchanged (virtio-blk requests are
/// always sector-aligned in practice, but the port must not assume it).
///
/// `read_at` takes `&self` so one image can serve several machines
/// read-only; `write_at` needs `&mut self`, which is why the host
/// attachment hands models `Arc<Mutex<dyn BlockImage>>`.
pub trait BlockImage: Send {
    /// Read `buf.len()` bytes at `offset`.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), BlockError>;

    /// Write `data` at `offset`.
    fn write_at(&mut self, offset: u64, data: &[u8]) -> Result<(), BlockError>;

    /// Image size in bytes.
    fn size(&self) -> u64;

    /// Flush buffered writes (a no-op for images without buffering).
    fn flush(&mut self) -> Result<(), BlockError> {
        Ok(())
    }

    /// Whether writes are rejected. A model whose guest attempts a write
    /// sees the error through its own protocol (virtio-blk status byte,
    /// ATA error register), never as a bus fault.
    fn readonly(&self) -> bool {
        false
    }
}

/// An in-memory image: the test backend and the default an operator's
/// zero-filled drive behaves like.
#[derive(Debug)]
pub struct MemoryImage {
    bytes: Vec<u8>,
    readonly: bool,
}

impl MemoryImage {
    pub fn new(size: u64) -> Self {
        Self {
            bytes: vec![0; usize::try_from(size).unwrap_or(usize::MAX)],
            readonly: false,
        }
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            readonly: false,
        }
    }

    /// A read-only view of fixed content (a CD-ROM shaped backend).
    pub fn readonly(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            readonly: true,
        }
    }
}

impl BlockImage for MemoryImage {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        let start = usize::try_from(offset).map_err(|_| BlockError::OutOfRange)?;
        let end = start.checked_add(buf.len()).ok_or(BlockError::OutOfRange)?;
        let slice = self.bytes.get(start..end).ok_or(BlockError::OutOfRange)?;
        buf.copy_from_slice(slice);
        Ok(())
    }

    fn write_at(&mut self, offset: u64, data: &[u8]) -> Result<(), BlockError> {
        if self.readonly {
            return Err(BlockError::ReadOnly);
        }
        let start = usize::try_from(offset).map_err(|_| BlockError::OutOfRange)?;
        let end = start
            .checked_add(data.len())
            .ok_or(BlockError::OutOfRange)?;
        let slice = self
            .bytes
            .get_mut(start..end)
            .ok_or(BlockError::OutOfRange)?;
        slice.copy_from_slice(data);
        Ok(())
    }

    fn size(&self) -> u64 {
        self.bytes.len() as u64
    }

    fn readonly(&self) -> bool {
        self.readonly
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_image_round_trip_and_bounds() {
        let mut image = MemoryImage::from_bytes(vec![1, 2, 3, 4]);
        let mut out = [0u8; 2];
        image.read_at(1, &mut out).unwrap();
        assert_eq!(out, [2, 3]);
        image.write_at(0, &[9, 9]).unwrap();
        image.read_at(0, &mut out).unwrap();
        assert_eq!(out, [9, 9]);
        // Out of range fails without mutation.
        assert_eq!(image.read_at(3, &mut out), Err(BlockError::OutOfRange));
        assert_eq!(image.write_at(4, &[0]), Err(BlockError::OutOfRange));
    }

    #[test]
    fn readonly_image_rejects_writes() {
        let mut image = MemoryImage::readonly(vec![0; 512]);
        assert_eq!(image.write_at(0, &[1]), Err(BlockError::ReadOnly));
        assert!(image.readonly());
    }
}
