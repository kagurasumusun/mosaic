//! virtio-9p — the 9P filesystem share (virtio 1.1 \u00a75.10, "9P
//! transport").
//!
//! **Not implemented yet, and this file says so.** A 9P device needs a
//! full 9P2000.L server (Tversion/Rversion, Tattach, Twalk, Topen,
//! Tread...) backed by a host filesystem boundary the `host` layer does
//! not expose yet (ADR 0028 reserves it). The model answers every
//! request with an RLerror so a guest's mount fails loudly instead of
//! hanging - the honest failure, per the testing policy (ADR 0022): no
//! synthetic success.

use super::port::{device_id, features, ProcessError, QueueSlot, VirtioDevice, VirtioIrq};
use super::vqueue::GuestMemory;

pub struct Virtio9p {
    tag: Vec<u8>,
}

impl Virtio9p {
    pub fn new() -> Self {
        Self {
            tag: b"share".to_vec(),
        }
    }
}

impl Default for Virtio9p {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtioDevice for Virtio9p {
    fn device_id(&self) -> u32 {
        device_id::_9P
    }

    fn device_features(&self) -> u64 {
        features::VERSION_1 | (1 << 0) // VIRTIO_9P_MOUNT_TAG
    }

    fn max_queue_size(&self) -> u32 {
        128
    }

    fn queue_count(&self) -> usize {
        1
    }

    fn config_read(&self, offset: u64, size: u8) -> u64 {
        let mut value = 0u64;
        for (index, byte) in self
            .tag
            .iter()
            .skip(offset as usize)
            .take(size as usize)
            .enumerate()
        {
            value |= u64::from(*byte) << (index * 8);
        }
        value
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
        // 9P2000.L RLerror: size[4] type[1] == 107, then error string.
        let response: [u8; 9] = [0, 0, 0, 9, 107, 0, 0, 0, 0];
        while let Some(head) = slot.view().next_head(mem)? {
            {
                let vq = slot.view();
                vq.write_chain(mem, head, &response)?;
            }
            slot.view().complete(mem, head, response.len() as u32)?;
        }
        irq.queue_interrupt();
        Ok(())
    }

    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tag_is_readable_from_config() {
        let p9 = Virtio9p::new();
        let mut expected = [0u8; 8];
        expected[..5].copy_from_slice(b"share");
        assert_eq!(p9.config_read(0, 5), u64::from_le_bytes(expected));
    }
}
