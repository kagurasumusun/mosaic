//! virtio-scsi — the SCSI host (virtio 1.1 \u00a75.6.2 "virtio SCSI host
//! device").
//!
//! Three virtqueues: control (0), event (1), request (2). This model
//! implements the control queue's basic commands and routes request-queue
//! SCSI commands to attached **targets** - the machine glue attaches
//! block images as LUN 0 of target 0. Supported SCSI commands (the
//! ones every bootstrap needs): INQUIRY, READ CAPACITY(10), READ(10),
//! WRITE(10), TEST UNIT READY, REQUEST SENSE. Everything else answers
//! with CHECK CONDITION / ILLEGAL REQUEST, which a guest handles as a
//! normal unsupported-command path.

use std::sync::Mutex;

use mosaic_contract::block::BlockImage;

use super::port::{device_id, features, ProcessError, QueueSlot, VirtioDevice, VirtioIrq};
use super::vqueue::GuestMemory;

/// SCSI status bytes.
mod scsi_status {
    pub const GOOD: u8 = 0;
    pub const CHECK_CONDITION: u8 = 2;
}

pub struct VirtioScsi {
    image: Option<Mutex<Box<dyn BlockImage>>>,
    block_size: u64,
    blocks: u64,
}

impl VirtioScsi {
    pub fn new() -> Self {
        Self {
            image: None,
            block_size: 512,
            blocks: 0,
        }
    }

    pub fn attach_image(&mut self, image: Box<dyn BlockImage>) {
        self.blocks = image.size() / 512;
        self.image = Some(Mutex::new(image));
    }

    fn sense_illegal_request() -> Vec<u8> {
        vec![
            0x70, 0, 0x05, 0, 0, 0, 0, 10, 0, 0x20, 0, 0, 0, 0, 0, 0, 0, 0,
        ]
    }
}

impl Default for VirtioScsi {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtioDevice for VirtioScsi {
    fn device_id(&self) -> u32 {
        device_id::SCSI
    }

    fn device_features(&self) -> u64 {
        features::VERSION_1 | (1 << 7) // VIRTIO_SCSI_F_INOUT
    }

    fn max_queue_size(&self) -> u32 {
        128
    }

    fn queue_count(&self) -> usize {
        3
    }

    fn config_read(&self, offset: u64, size: u8) -> u64 {
        // struct virtio_scsi_config: num_queues u32 | seg_max u32 |
        // max_sectors u32 | cmd_per_lun u32 | event_info_size u32 |
        // sense_size u32 | cdb_size u32 | max_channel u16 | max_target
        // u16 | max_lun u32.
        let config: [u8; 36] = {
            let mut config = [0u8; 36];
            config[0..4].copy_from_slice(&1u32.to_le_bytes());
            config[4..8].copy_from_slice(&2u32.to_le_bytes());
            config[8..12].copy_from_slice(&255u32.to_le_bytes());
            config[12..16].copy_from_slice(&32u32.to_le_bytes());
            config[20..24].copy_from_slice(&96u32.to_le_bytes());
            config[24..28].copy_from_slice(&32u32.to_le_bytes());
            config[28..30].copy_from_slice(&0u16.to_le_bytes());
            config[30..32].copy_from_slice(&1u16.to_le_bytes());
            config[32..36].copy_from_slice(&1u32.to_le_bytes());
            config
        };
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

    fn config_write(&mut self, _offset: u64, _size: u8, _value: u64) {}

    #[allow(clippy::too_many_lines)]
    fn process(
        &mut self,
        _ctx: &mut dyn mosaic_contract::model::Ctx,
        queue: usize,
        slot: &mut QueueSlot,
        mem: &mut dyn GuestMemory,
        irq: &mut VirtioIrq<'_>,
    ) -> Result<(), ProcessError> {
        if queue != 2 {
            // Control/event queues: complete silently (no hotplug events).
            while let Some(head) = slot.view().next_head(mem)? {
                slot.view().complete(mem, head, 0)?;
            }
            irq.queue_interrupt();
            return Ok(());
        }
        while let Some(head) = slot.view().next_head(mem)? {
            // struct virtio_scsi_cmd_req: lun[8] | tag u64 | task_attr u8
            // | prio u8 | crn u8 | cdb[]; the response precedes the data
            // in the writable chain.
            let mut segments: Vec<(u64, u32, bool)> = Vec::new();
            {
                let vq = slot.view();
                vq.walk(mem, head, |addr, len, writable, _index| {
                    segments.push((addr, len, writable));
                    Ok(())
                })?;
            }
            let mut request = Vec::new();
            for (addr, len, writable) in &segments {
                if *writable {
                    continue;
                }
                let mut chunk = vec![0u8; *len as usize];
                mem.read_bytes(*addr, &mut chunk)?;
                request.extend_from_slice(&chunk);
            }
            if request.len() < 20 {
                slot.view().complete(mem, head, 0)?;
                continue;
            }
            let cdb = &request[19..];
            let writable: Vec<(u64, u32)> = segments
                .iter()
                .filter(|(_, _, writable)| *writable)
                .map(|(addr, len, _)| (*addr, *len))
                .collect();
            let mut response: Vec<u8> = vec![0; 20]; // virtio_scsi_cmd_resp
            let mut data: Vec<u8> = Vec::new();
            let mut status = scsi_status::GOOD;
            let command = cdb.first().copied().unwrap_or(0);
            match command {
                0x00 => {} // TEST UNIT READY
                0x03 => {
                    // REQUEST SENSE
                    data = Self::sense_illegal_request();
                }
                0x12 => {
                    // INQUIRY: a direct-access device.
                    data = vec![
                        0x00, 0x00, 0x05, 0x02, 0x1F, 0x00, 0x00, 0x00, b' ', b' ', b' ', b' ',
                        b' ', b' ', b' ', b' ', b'Q', b'E', b'M', b'U', b' ', b'M', b'O', b'S',
                        b'A', b'I', b'C', b' ', b' ', b' ', b' ', b' ',
                    ];
                }
                0x25 if self.blocks > 0 => {
                    // READ CAPACITY(10)
                    let mut capacity = Vec::new();
                    capacity.extend_from_slice(&((self.blocks - 1) as u32).to_be_bytes());
                    capacity.extend_from_slice(&(self.block_size as u32).to_be_bytes());
                    data = capacity;
                }
                0x28 | 0x2A => {
                    // READ(10) / WRITE(10)
                    if cdb.len() < 10 || self.image.is_none() {
                        status = scsi_status::CHECK_CONDITION;
                    } else {
                        let lba = u32::from_be_bytes(cdb[2..6].try_into().unwrap()) as u64;
                        let blocks = u16::from_be_bytes(cdb[7..9].try_into().unwrap()) as u64;
                        let offset = lba * self.block_size;
                        let length = (blocks * self.block_size) as usize;
                        let Some(image) = self.image.as_ref() else {
                            slot.view().complete(mem, head, 0)?;
                            continue;
                        };
                        let Ok(mut guard) = image.lock() else {
                            // Poisoned image: complete with what was moved.
                            slot.view().complete(mem, head, 0)?;
                            continue;
                        };
                        let outcome = if command == 0x28 {
                            let mut buffer = vec![0u8; length];
                            guard.read_at(offset, &mut buffer).map(|_| data = buffer)
                        } else {
                            let mut buffer = vec![0u8; length];
                            let source = writable.first().map(|(addr, _)| *addr).unwrap_or(0);
                            mem.read_bytes(source, &mut buffer)?;
                            guard.write_at(offset, &buffer)
                        };
                        if let Err(error) = outcome {
                            let _ = error;
                            status = scsi_status::CHECK_CONDITION;
                        }
                    }
                }
                _ => {
                    status = scsi_status::CHECK_CONDITION;
                }
            }
            // Write the response header + data into the writable tail.
            if status == scsi_status::CHECK_CONDITION {
                // Attach sense data to the response.
                let sense = Self::sense_illegal_request();
                response.truncate(20);
                response.extend_from_slice(&sense);
            }
            let mut payload = response;
            payload.extend_from_slice(&data);
            let mut offset = 0usize;
            for (addr, len) in writable.iter().rev() {
                if offset >= payload.len() {
                    break;
                }
                let take = (*len as usize).min(payload.len() - offset);
                let slice = &payload[payload.len() - offset - take..payload.len() - offset];
                mem.write_bytes(*addr, slice)?;
                offset += take;
            }
            slot.view().complete(mem, head, payload.len() as u32)?;
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
    fn config_advertises_one_target_one_lun() {
        let scsi = VirtioScsi::new();
        assert_eq!(scsi.config_read(30, 2), 1); // max_target
        assert_eq!(scsi.config_read(32, 4), 1); // max_lun
    }

    #[test]
    fn attached_image_reports_capacity() {
        let mut scsi = VirtioScsi::new();
        scsi.attach_image(Box::new(mosaic_contract::block::MemoryImage::from_bytes(
            vec![0u8; 512 * 16],
        )));
        assert_eq!(scsi.blocks, 16);
    }
}
