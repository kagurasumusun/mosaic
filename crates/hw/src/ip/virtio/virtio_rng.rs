//! virtio-rng — the entropy source (virtio 1.1 §5.4 "Entropy Device").
//!
//! One virtqueue: the driver posts writable buffers, the device fills
//! them with entropy and completes them. The config space is empty.
//!
//! Entropy source: a deterministic xorshift stream seeded from the
//! machine's spawn parameter — virtual hardware must be reproducible
//! (design P8: no host randomness inside a model), and the host layer
//! can later inject a stronger stream through the same seed port. The
//! stream is **not** cryptographic; a guest that cares must run its own
//! CSPRNG over the bytes, which is exactly what Linux's hwrng path does
//! upstream of the entropy pool.

use super::port::{device_id, features, ProcessError, QueueSlot, VirtioDevice, VirtioIrq};
use super::vqueue::GuestMemory;

pub struct VirtioRng {
    /// xorshift64* state; zero is remapped (a zero state locks up).
    state: u64,
    served: u64,
}

impl VirtioRng {
    pub fn new() -> Self {
        Self {
            state: 0x9E37_79B9_7F4A_7C15,
            served: 0,
        }
    }

    pub fn with_seed(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0x9E37_79B9_7F4A_7C15
            } else {
                seed
            },
            served: 0,
        }
    }

    fn refill(&mut self, buffer: &mut [u8]) {
        for chunk in buffer.chunks_mut(8) {
            let mut x = self.state;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.state = x;
            let value = x.wrapping_mul(0x2545_F491_4F6C_DD1D);
            for (index, byte) in chunk.iter_mut().enumerate() {
                *byte = (value >> (index * 8)) as u8;
            }
        }
    }
}

impl Default for VirtioRng {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtioDevice for VirtioRng {
    fn device_id(&self) -> u32 {
        device_id::RNG
    }

    fn device_features(&self) -> u64 {
        features::VERSION_1
    }

    fn max_queue_size(&self) -> u32 {
        128
    }

    fn queue_count(&self) -> usize {
        1
    }

    fn config_read(&self, _offset: u64, _size: u8) -> u64 {
        0
    }

    fn config_write(&mut self, _offset: u64, _size: u8, _value: u64) {}

    fn process(
        &mut self,
        _ctx: &mut dyn mosaic_contract::model::Ctx,
        _queue: usize,
        slot: &mut QueueSlot,
        mem: &mut dyn GuestMemory,
        irq: &mut VirtioIrq<'_>,
    ) -> Result<(), ProcessError> {
        while let Some(head) = slot.view().next_head(mem)? {
            let mut segments: Vec<(u64, u32)> = Vec::new();
            {
                let vq = slot.view();
                vq.walk(mem, head, |addr, len, _writable, _index| {
                    segments.push((addr, len));
                    Ok(())
                })?;
            }
            let mut filled = 0usize;
            for (addr, len) in segments {
                let take = (len as usize).min(256);
                let mut chunk = vec![0u8; take];
                self.refill(&mut chunk);
                mem.write_bytes(addr, &chunk).map_err(ProcessError::from)?;
                filled += take;
            }
            slot.view().complete(mem, head, filled as u32)?;
        }
        irq.queue_interrupt();
        Ok(())
    }

    fn reset(&mut self) {
        // The stream keeps its state: entropy survives a guest reboot the
        // way a real device's does, and determinism only needs the seed.
    }
}

/// Bytes served, for the harness's report.
impl VirtioRng {
    pub fn served_bytes(&self) -> u64 {
        self.served
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ip::virtio::port::QueueSlot;
    use crate::ip::virtio::test_ctx::support::RecCtx;
    use crate::ip::virtio::vqueue::tests_support::TestRam;
    use crate::ip::virtio::vqueue::{QueueConfig, QueueCursor, SplitVirtqueue, F_WRITE};

    fn setup(size: u32) -> (VirtioRng, QueueSlot, TestRam) {
        let mut ram = TestRam::new(0x1_0000);
        // One writable descriptor at 0x4000, 32 bytes.
        let base = 0x1000;
        let mut entry = [0u8; 16];
        entry[0..8].copy_from_slice(&0x4000u64.to_le_bytes());
        entry[8..12].copy_from_slice(&32u32.to_le_bytes());
        entry[12..14].copy_from_slice(&F_WRITE.to_le_bytes());
        ram.bytes[0..16].copy_from_slice(&entry);
        // Avail: idx 1, ring[0] = 0.
        ram.bytes[(0x2000 + 2 - base) as usize..][..2].copy_from_slice(&1u16.to_le_bytes());
        ram.bytes[(0x2000 + 4 - base) as usize..][..2].copy_from_slice(&0u16.to_le_bytes());
        let mut slot = QueueSlot::new();
        slot.config = QueueConfig {
            size,
            desc_addr: 0x1000,
            avail_addr: 0x2000,
            used_addr: 0x3000,
            ready: true,
        };
        (VirtioRng::with_seed(42), slot, ram)
    }

    #[test]
    fn fills_buffers_and_completes() {
        let (mut rng, mut slot, mut ram) = setup(8);
        let mut status = 0u32;
        let mut ctx = RecCtx::new();
        {
            let mut irq = VirtioIrq::new(
                mosaic_exec::SignalId(0),
                mosaic_exec::NodeId(0),
                &mut status,
            );
            VirtioDevice::process(&mut rng, &mut ctx, 0, &mut slot, &mut ram, &mut irq).unwrap();
        }
        assert_eq!(status & 1, 1);
        // The buffer holds non-zero entropy.
        let buf = &ram.bytes[(0x4000 - 0x1000) as usize..][..32];
        assert!(buf.iter().any(|byte| *byte != 0));
        // The used ring carries one 32-byte completion.
        let mut cursor = QueueCursor::default();
        let vq = SplitVirtqueue::new(&slot.config, &mut cursor);
        assert_eq!(vq.chain_len(&ram, 0).unwrap(), 32);
    }

    #[test]
    fn stream_is_deterministic_for_a_seed() {
        let mut a = [0u8; 16];
        let mut b = [0u8; 16];
        VirtioRng::with_seed(7).refill(&mut a);
        VirtioRng::with_seed(7).refill(&mut b);
        assert_eq!(a, b);
        let mut c = [0u8; 16];
        VirtioRng::with_seed(8).refill(&mut c);
        assert_ne!(a, c);
    }
}
