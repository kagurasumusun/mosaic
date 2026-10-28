//! Guest-memory port — the bus-master vocabulary (QEMU's
//! `address_space_*` role, one trait).
//!
//! Any device that masters the bus itself — virtio rings, NIC
//! descriptors, USB frame lists, AHCI command lists — reads and writes
//! guest RAM at guest-physical addresses. The machine harness implements
//! this port over the machine's RAM authority (the engine-shared block
//! on Unicorn machines, the backing store elsewhere) and attaches it to
//! the devices that need it. The port is deliberately byte-granular
//! with 64-bit addresses: a device converts its own field widths
//! explicitly, so endianness on the wire stays visible in the model.
//!
//! This is not the CPU's path: instruction fetch and load/store go
//! through the accelerator. This is the *other* bus master's path, and
//! the one place DMA may enter the machine.
//!
//! Determinism note: an implementation must be snapshot-transparent —
//! the same guest-physical bytes before and after a save/load — which
//! the shared-RAM authority guarantees by construction.

/// Why a guest-memory access failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum GuestMemError {
    /// The address range is not backed by RAM on this machine.
    Unmapped { address: u64 },
}

/// Read/write access to guest RAM at guest-physical addresses.
pub trait GuestMemory: Send {
    /// Read `out.len()` bytes at `address`.
    fn read_bytes(&self, address: u64, out: &mut [u8]) -> Result<(), GuestMemError>;

    /// Write `data` at `address`.
    fn write_bytes(&mut self, address: u64, data: &[u8]) -> Result<(), GuestMemError>;
}

/// A contiguous RAM window implementation for tests and simple harnesses:
/// bytes at `base .. base + len`.
pub struct RamWindow {
    pub base: u64,
    pub bytes: std::sync::Mutex<Vec<u8>>,
}

impl RamWindow {
    pub fn new(base: u64, size: usize) -> Self {
        Self {
            base,
            bytes: std::sync::Mutex::new(vec![0; size]),
        }
    }
}

impl GuestMemory for RamWindow {
    fn read_bytes(&self, address: u64, out: &mut [u8]) -> Result<(), GuestMemError> {
        let start = (address - self.base) as usize;
        let guard = self
            .bytes
            .lock()
            .map_err(|_| GuestMemError::Unmapped { address })?;
        let slice = guard
            .get(start..start + out.len())
            .ok_or(GuestMemError::Unmapped { address })?;
        out.copy_from_slice(slice);
        Ok(())
    }

    fn write_bytes(&mut self, address: u64, data: &[u8]) -> Result<(), GuestMemError> {
        let start = (address - self.base) as usize;
        let mut guard = self
            .bytes
            .lock()
            .map_err(|_| GuestMemError::Unmapped { address })?;
        let slice = guard
            .get_mut(start..start + data.len())
            .ok_or(GuestMemError::Unmapped { address })?;
        slice.copy_from_slice(data);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_round_trip_and_bounds() {
        let mut window = RamWindow::new(0x4000_0000, 256);
        window.write_bytes(0x4000_0010, &[1, 2, 3]).unwrap();
        let mut out = [0u8; 3];
        window.read_bytes(0x4000_0010, &mut out).unwrap();
        assert_eq!(out, [1, 2, 3]);
        assert_eq!(
            window.read_bytes(0x4000_0100, &mut out),
            Err(GuestMemError::Unmapped {
                address: 0x4000_0100
            })
        );
        assert_eq!(
            window.write_bytes(0x3FFF_FFFF, &[0]),
            Err(GuestMemError::Unmapped {
                address: 0x3FFF_FFFF
            })
        );
    }
}
