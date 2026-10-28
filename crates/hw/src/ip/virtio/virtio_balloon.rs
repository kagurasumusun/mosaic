//! virtio-balloon — the memory balloon (virtio 1.1 §5.5).
//!
//! Three virtqueues: inflate (0), deflate (1), stats (2). The guest
//! posts pages; the device accepts them and answers nothing (the
//! balloon protocol is one-way: completion is the used-ring entry).
//!
//! Scope: this model tracks the balloon's page count and the deflate
//! path symmetrically, but does not reclaim host memory — the machine's
//! RAM block is the engine's shared page, so ballooning a fixed-size
//! window is a later host-layer feature. A driver that inflates and
//! deflates sees consistent state and completions, which is what the
//! protocol's control plane observes.

use super::port::{device_id, features, ProcessError, QueueSlot, VirtioDevice, VirtioIrq};
use super::vqueue::GuestMemory;

pub struct VirtioBalloon {
    /// Pages currently in the balloon (inflate minus deflate).
    pub inflated_pages: u64,
}

impl VirtioBalloon {
    pub fn new() -> Self {
        Self { inflated_pages: 0 }
    }
}

impl Default for VirtioBalloon {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtioDevice for VirtioBalloon {
    fn device_id(&self) -> u32 {
        device_id::BALLOON
    }

    fn device_features(&self) -> u64 {
        features::VERSION_1 | (1 << 5) // VIRTIO_BALLOON_F_STATS_VQ
    }

    fn max_queue_size(&self) -> u32 {
        128
    }

    fn queue_count(&self) -> usize {
        3
    }

    fn config_read(&self, _offset: u64, _size: u8) -> u64 {
        // num_pages is a driver write; the device reports actual only
        // when it can, which a fixed-window model cannot yet.
        0
    }

    fn config_write(&mut self, _offset: u64, _size: u8, _value: u64) {}

    fn process(
        &mut self,
        _ctx: &mut dyn mosaic_contract::model::Ctx,
        queue: usize,
        slot: &mut QueueSlot,
        mem: &mut dyn GuestMemory,
        irq: &mut VirtioIrq<'_>,
    ) -> Result<(), ProcessError> {
        // Each chain is an array of u32 page numbers.
        while let Some(head) = slot.view().next_head(mem)? {
            let mut data = Vec::new();
            {
                let vq = slot.view();
                vq.read_chain(mem, head, &mut data)?;
            }
            let pages = data.len() / 4;
            match queue {
                0 => self.inflated_pages += pages as u64,
                1 => self.inflated_pages = self.inflated_pages.saturating_sub(pages as u64),
                _ => {}
            }
            slot.view().complete(mem, head, 0)?;
        }
        irq.queue_interrupt();
        Ok(())
    }

    fn reset(&mut self) {
        self.inflated_pages = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ip::virtio::test_ctx::support::RecCtx;
    use crate::ip::virtio::vqueue::tests_support::TestRam;
    use crate::ip::virtio::vqueue::{QueueConfig, F_WRITE};

    #[test]
    fn inflate_and_deflate_count_pages() {
        let mut ram = TestRam::new(0x1_0000);
        let base = 0x1000;
        // One read-only descriptor: four page numbers at 0x4000.
        for (index, page) in [0x100u32, 0x200, 0x300, 0x400].iter().enumerate() {
            ram.bytes[(0x4000 - base) as usize + index * 4..][..4]
                .copy_from_slice(&page.to_le_bytes());
        }
        let mut entry = [0u8; 16];
        entry[0..8].copy_from_slice(&0x4000u64.to_le_bytes());
        entry[8..12].copy_from_slice(&16u32.to_le_bytes());
        ram.bytes[0..16].copy_from_slice(&entry);
        ram.bytes[(0x2002 - base) as usize..][..2].copy_from_slice(&1u16.to_le_bytes());
        ram.bytes[(0x2004 - base) as usize..][..2].copy_from_slice(&0u16.to_le_bytes());
        let mut slot = QueueSlot::new();
        slot.config = QueueConfig {
            size: 8,
            desc_addr: 0x1000,
            avail_addr: 0x2000,
            used_addr: 0x3000,
            ready: true,
        };
        let mut device = VirtioBalloon::new();
        let mut status = 0u32;
        let mut irq = VirtioIrq::new(
            mosaic_exec::SignalId(0),
            mosaic_exec::NodeId(0),
            &mut status,
        );
        let mut ctx = RecCtx::new();
        VirtioDevice::process(&mut device, &mut ctx, 0, &mut slot, &mut ram, &mut irq).unwrap();
        assert_eq!(device.inflated_pages, 4);
        // Post the chain again (avail idx 2) and deflate it.
        ram.bytes[(0x2002 - base) as usize..][..2].copy_from_slice(&2u16.to_le_bytes());
        VirtioDevice::process(&mut device, &mut ctx, 1, &mut slot, &mut ram, &mut irq).unwrap();
        assert_eq!(device.inflated_pages, 0);
        let _ = F_WRITE;
    }
}
