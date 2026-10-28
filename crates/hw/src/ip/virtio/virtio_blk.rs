//! virtio-blk — the block device (virtio 1.1 §5.2 "Block Device").
//!
//! One virtqueue (index 0, the request queue). A request is a chain:
//!
//! ```text
//! header   16 bytes, read-only:  type u32 (0 IN / 1 OUT) | reserved u32 |
//!                                 sector u64
//! data     sector bytes, IN: device-writable, OUT: read-only
//! status   1 byte, device-writable: 0 OK | 1 IOERR | 2 UNSUPPORTED
//! ```
//!
//! The device walks the chain without assuming where each part lives,
//! honors the writability the driver marked, and completes the whole
//! chain with the byte count it moved. The image is a
//! [`BlockImage`](mosaic_contract::block::BlockImage) the machine glue
//! attaches — an in-memory image in tests, a file in the host layer —
//! reached through a mutex so several drives can share one read-only
//! image.
//!
//! Config space (struct virtio_blk_config, the fields a Linux probe
//! reads): `capacity` u64 (in 512-byte sectors), `size_max` u32,
//! `seg_max` u32, `geometry` {cylinders u16, heads u8, sectors u8},
//! `blk_size` u32. Features: VERSION_1, FLUSH, BLK_SIZE — the minimum a
//! modern driver negotiates plus the flush it expects to find.

use std::sync::Mutex;

use mosaic_contract::block::{BlockError, BlockImage};

use super::port::{device_id, features, ProcessError, QueueSlot, VirtioDevice, VirtioIrq};
use super::vqueue::GuestMemory;

/// Status byte values the device writes into the trailing descriptor.
mod status {
    pub const OK: u8 = 0;
    pub const IOERR: u8 = 1;
    pub const UNSUPPORTED: u8 = 2;
}

/// VIRTIO_BLK_F_* bits worth advertising.
#[allow(dead_code)] // the documented feature set; negotiation grows into it
mod blk_features {
    pub const FLUSH: u64 = 1 << 9;
    pub const BLK_SIZE: u64 = 1 << 6;
    pub const SEG_MAX: u64 = 1 << 2;
    pub const SIZE_MAX: u64 = 1 << 1;
    pub const RO: u64 = 1 << 5;
}

pub const SECTOR_SIZE: u64 = 512;

pub struct VirtioBlk {
    image: Option<Mutex<Box<dyn BlockImage>>>,
    capacity_sectors: u64,
    negotiated: u64,
    readonly: bool,
}

impl VirtioBlk {
    /// An unattached device: no image, zero capacity. The machine glue
    /// attaches the image before the guest probes.
    pub fn new() -> Self {
        Self {
            image: None,
            capacity_sectors: 0,
            negotiated: 0,
            readonly: false,
        }
    }

    pub fn attach_image(&mut self, image: Box<dyn BlockImage>) {
        self.capacity_sectors = image.size() / SECTOR_SIZE;
        self.readonly = image.readonly();
        self.image = Some(Mutex::new(image));
    }

    pub fn capacity_sectors(&self) -> u64 {
        self.capacity_sectors
    }

    fn config_bytes(&self) -> [u8; 28] {
        let mut config = [0u8; 28];
        config[0..8].copy_from_slice(&self.capacity_sectors.to_le_bytes());
        config[8..12].copy_from_slice(&u32::MAX.to_le_bytes()); // size_max
        config[12..16].copy_from_slice(&2u32.to_le_bytes()); // seg_max
                                                             // geometry: cylinders 2, heads 16, sectors 63 (the classic lie)
        config[16..18].copy_from_slice(&2u16.to_le_bytes());
        config[18] = 16;
        config[19] = 63;
        config[20..24].copy_from_slice(&(SECTOR_SIZE as u32).to_le_bytes());
        config
    }

    fn status_for(&self, error: BlockError) -> u8 {
        match error {
            BlockError::ReadOnly | BlockError::OutOfRange => status::IOERR,
            BlockError::Io => status::IOERR,
            _ => status::IOERR,
        }
    }
}

impl Default for VirtioBlk {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtioDevice for VirtioBlk {
    fn device_id(&self) -> u32 {
        device_id::BLOCK
    }

    fn device_features(&self) -> u64 {
        let mut offered = features::VERSION_1 | blk_features::FLUSH | blk_features::BLK_SIZE;
        if self.readonly {
            offered |= blk_features::RO;
        }
        offered
    }

    fn set_driver_features(&mut self, features: u64) {
        self.negotiated = features;
    }

    fn max_queue_size(&self) -> u32 {
        128
    }

    fn queue_count(&self) -> usize {
        1
    }

    fn config_read(&self, offset: u64, size: u8) -> u64 {
        let config = self.config_bytes();
        let start = offset as usize;
        if start + size as usize > config.len() {
            return 0;
        }
        let mut value = 0u64;
        for (index, byte) in config[start..start + size as usize].iter().enumerate() {
            value |= u64::from(*byte) << (index * 8);
        }
        value
    }

    fn config_write(&mut self, _offset: u64, _size: u8, _value: u64) {
        // The block config space is read-only by specification.
    }

    #[allow(clippy::too_many_lines)]
    fn process(
        &mut self,
        _ctx: &mut dyn mosaic_contract::model::Ctx,
        _queue: usize,
        slot: &mut QueueSlot,
        mem: &mut dyn GuestMemory,
        irq: &mut VirtioIrq<'_>,
    ) -> Result<(), ProcessError> {
        let Some(image) = self.image.as_ref() else {
            // No image attached: every request completes UNSUPPORTED.
            while let Some(head) = slot.view().next_head(mem)? {
                slot.view().complete(mem, head, 1)?;
            }
            irq.queue_interrupt();
            return Ok(());
        };
        while let Some(head) = slot.view().next_head(mem)? {
            // Pass 1: collect the chain as (address, length, writable)
            // segments — the spec's request shape (header RO | data |
            // status as the chain's final byte) decoded without assuming
            // which descriptor carries which part.
            let mut segments: Vec<(u64, u32, bool)> = Vec::new();
            {
                let vq = slot.view();
                vq.walk(mem, head, |addr, len, writable, _index| {
                    segments.push((addr, len, writable));
                    Ok(())
                })?;
            }
            let total: u32 = segments.iter().map(|(_, len, _)| *len).sum();
            if total < 17 {
                // Malformed: no room for header + status.
                slot.view().complete(mem, head, 1)?;
                continue;
            }
            let read_only: u32 = segments
                .iter()
                .filter(|(_, _, writable)| !writable)
                .map(|(_, len, _)| *len)
                .sum();
            if read_only < 16 {
                slot.view().complete(mem, head, 1)?;
                continue;
            }
            let mut header = [0u8; 16];
            {
                let mut filled = 0usize;
                for (addr, len, writable) in &segments {
                    if *writable {
                        continue;
                    }
                    let take = (*len as usize).min(header.len() - filled);
                    mem.read_bytes(*addr, &mut header[filled..filled + take])?;
                    filled += take;
                    if filled == header.len() {
                        break;
                    }
                }
            }
            let request_type = u32::from_le_bytes(header[0..4].try_into().unwrap());
            let sector = u64::from_le_bytes(header[8..16].try_into().unwrap());
            let flush = request_type == 4; // VIRTIO_BLK_T_FLUSH
            let read = request_type == 0;
            let write = request_type == 1;

            let mut outcome = status::OK;
            let mut moved = 0u32;
            if flush {
                if let Ok(mut guard) = image.lock() {
                    let _ = guard.flush();
                }
            } else if read {
                // Writable bytes except the chain's final byte.
                let writable_span = total - read_only - 1;
                let mut offset = sector * SECTOR_SIZE;
                let mut remaining = writable_span;
                for (addr, len, writable) in &segments {
                    if remaining == 0 {
                        break;
                    }
                    if !writable {
                        continue;
                    }
                    let take = (*len).min(remaining);
                    let mut buffer = vec![0u8; take as usize];
                    match image
                        .lock()
                        .map_err(|_| ProcessError::Unmapped { address: *addr })?
                        .read_at(offset, &mut buffer)
                    {
                        Ok(()) => {
                            mem.write_bytes(*addr, &buffer)?;
                            moved += take;
                            offset += u64::from(take);
                            remaining -= take;
                        }
                        Err(error) => {
                            outcome = self.status_for(error);
                            break;
                        }
                    }
                }
                if outcome == status::OK && moved != writable_span && writable_span > 0 {
                    outcome = status::IOERR;
                }
            } else if write {
                // Read-only bytes after the 16-byte header.
                let data_span = read_only - 16;
                let mut offset = sector * SECTOR_SIZE;
                let mut remaining = data_span;
                let mut skipped_header = 0u32;
                for (addr, len, writable) in &segments {
                    if *writable || remaining == 0 {
                        continue;
                    }
                    let mut take = *len;
                    if skipped_header < 16 {
                        let header_rest = 16 - skipped_header;
                        if take <= header_rest {
                            skipped_header += take;
                            continue;
                        }
                        take -= header_rest;
                        skipped_header = 16;
                    }
                    take = take.min(remaining);
                    let mut buffer = vec![0u8; take as usize];
                    let source = addr + u64::from(*len - take);
                    mem.read_bytes(source, &mut buffer)?;
                    match image
                        .lock()
                        .map_err(|_| ProcessError::Unmapped { address: *addr })?
                        .write_at(offset, &buffer)
                    {
                        Ok(()) => {
                            moved += take;
                            offset += u64::from(take);
                            remaining -= take;
                        }
                        Err(error) => {
                            outcome = self.status_for(error);
                            break;
                        }
                    }
                }
            } else {
                outcome = status::UNSUPPORTED;
            }

            // The status byte: the chain's final byte (the last writable
            // descriptor's last byte).
            for (addr, len, writable) in segments.iter().rev() {
                if *writable && *len >= 1 {
                    mem.write_bytes(addr + u64::from(*len - 1), &[outcome])?;
                    break;
                }
            }
            slot.view().complete(mem, head, moved)?;
        }
        irq.queue_interrupt();
        Ok(())
    }

    fn reset(&mut self) {
        self.negotiated = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ip::virtio::test_ctx::support::RecCtx;
    use crate::ip::virtio::vqueue::tests_support::TestRam;
    use crate::ip::virtio::vqueue::{QueueConfig, F_WRITE};
    use mosaic_contract::block::MemoryImage;

    struct Rig {
        ram: TestRam,
        slot: QueueSlot,
        status: u32,
    }

    /// One IN/OUT request chain: header(16, RO) + data(512, per-request) +
    /// status(1, W). Addresses: desc 0x1000, avail 0x2000, used 0x3000,
    /// header 0x4000, data 0x5000, status 0x6000.
    fn rig(request_type: u32, sector: u64, image_bytes: Vec<u8>) -> (VirtioBlk, Rig) {
        let mut ram = TestRam::new(0x1_0000);
        let base = 0x1000;
        let mut desc = |slot: usize, addr: u64, len: u32, flags: u16, next: u16| {
            let mut entry = [0u8; 16];
            entry[0..8].copy_from_slice(&addr.to_le_bytes());
            entry[8..12].copy_from_slice(&len.to_le_bytes());
            entry[12..14].copy_from_slice(&flags.to_le_bytes());
            entry[14..16].copy_from_slice(&next.to_le_bytes());
            ram.bytes[slot * 16..slot * 16 + 16].copy_from_slice(&entry);
        };
        let f_next = crate::ip::virtio::vqueue::F_NEXT;
        desc(0, 0x4000, 16, f_next, 1);
        let data_flags = if request_type == 0 {
            F_WRITE | f_next
        } else {
            f_next
        };
        desc(1, 0x5000, 512, data_flags, 2);
        desc(2, 0x6000, 1, F_WRITE, 0);
        let mut header = [0u8; 16];
        header[0..4].copy_from_slice(&request_type.to_le_bytes());
        header[8..16].copy_from_slice(&sector.to_le_bytes());
        ram.bytes[(0x4000 - base) as usize..][..16].copy_from_slice(&header);
        ram.bytes[(0x2000 + 2 - base) as usize..][..2].copy_from_slice(&1u16.to_le_bytes());
        ram.bytes[(0x2000 + 4 - base) as usize..][..2].copy_from_slice(&0u16.to_le_bytes());
        let mut slot = QueueSlot::new();
        slot.config = QueueConfig {
            size: 8,
            desc_addr: 0x1000,
            avail_addr: 0x2000,
            used_addr: 0x3000,
            ready: true,
        };
        let mut device = VirtioBlk::new();
        device.attach_image(Box::new(MemoryImage::from_bytes(image_bytes)));
        (
            device,
            Rig {
                ram,
                slot,
                status: 0,
            },
        )
    }

    fn run(device: &mut VirtioBlk, rig: &mut Rig) {
        let mut ctx = RecCtx::new();
        let mut irq = VirtioIrq::new(
            mosaic_exec::SignalId(0),
            mosaic_exec::NodeId(0),
            &mut rig.status,
        );
        VirtioDevice::process(device, &mut ctx, 0, &mut rig.slot, &mut rig.ram, &mut irq).unwrap();
    }

    fn used(rig: &Rig) -> (u32, u32) {
        let base = 0x1000;
        let entry = 0x3004;
        let id = u32::from_le_bytes(
            rig.ram.bytes[(entry - base) as usize..][..4]
                .try_into()
                .unwrap(),
        );
        let len = u32::from_le_bytes(
            rig.ram.bytes[(entry + 4 - base) as usize..][..4]
                .try_into()
                .unwrap(),
        );
        (id, len)
    }

    #[test]
    fn read_request_returns_the_sector() {
        // A 3-sector image with the pattern in sector 2.
        let mut image = vec![0u8; 512 * 3];
        image[1024..1032].copy_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD, 1, 2, 3, 4]);
        let (mut device, mut rig) = rig(0, 2, image);
        run(&mut device, &mut rig);
        assert_eq!(rig.status & 1, 1);
        let (id, len) = used(&rig);
        assert_eq!((id, len), (0, 512));
        assert_eq!(
            &rig.ram.bytes[(0x5000 - 0x1000) as usize..][..8],
            &[0xAA, 0xBB, 0xCC, 0xDD, 1, 2, 3, 4]
        );
        // Status byte: OK.
        assert_eq!(rig.ram.bytes[(0x6000 - 0x1000) as usize], 0);
    }

    #[test]
    fn write_request_persists() {
        let (mut device, mut rig) = rig(1, 1, vec![0u8; 512 * 2]);
        rig.ram.bytes[(0x5000 - 0x1000) as usize..][..4].copy_from_slice(&[9, 8, 7, 6]);
        run(&mut device, &mut rig);
        assert_eq!(rig.ram.bytes[(0x6000 - 0x1000) as usize], 0);
        // The image carries the bytes the guest wrote.
        let image = device.image.as_ref().unwrap().lock().unwrap();
        let mut probe = [0u8; 4];
        image.read_at(512, &mut probe).unwrap();
        assert_eq!(probe, [9, 8, 7, 6]);
    }

    #[test]
    fn capacity_comes_from_the_image() {
        let mut device = VirtioBlk::new();
        device.attach_image(Box::new(MemoryImage::from_bytes(vec![0u8; 4096])));
        assert_eq!(device.capacity_sectors(), 8);
        assert_eq!(device.config_read(0, 8), 8);
    }

    #[test]
    fn unsupported_request_type_completes_unsupported() {
        let (mut device, mut rig) = rig(7, 0, vec![0u8; 512]);
        run(&mut device, &mut rig);
        assert_eq!(rig.ram.bytes[(0x6000 - 0x1000) as usize], 2);
    }
}
