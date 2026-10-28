//! virtio-input — the evdev-style input device (virtio 1.1 §5.10
//! "Input Device").
//!
//! Two virtqueues: `eventq` (0, host → guest: input events) and
//! `statusq` (1, guest → host: LED/sound state the guest sets).
//!
//! The config space is the virtio input device tree: `select` u8,
//! `subsel` u8, `size` u8, `reserved` u8, then `data[128]`. The device
//! answers `select/subsel` queries (device name, serial, ID info,
//! supported event codes/bitmaps) the way an evdev probe walks them.
//! Events flow through the shared pending queue the machine glue (or a
//! host input source) pushes into with [`VirtioInput::push_event`].
//!
//! Event types: the standard evdev `input_event` — u16 type, u16 code,
//! u32 value (the 80-byte legacy struct is the guest-visible wire
//! format; this model fills the little-endian stream the driver's
//! `virtinput_recv_events` parses).

use std::sync::Mutex;

use super::port::{device_id, features, ProcessError, QueueSlot, VirtioDevice, VirtioIrq};
use super::vqueue::GuestMemory;

/// evdev event types (uapi/linux/input-event-codes.h).
pub mod ev {
    pub const SYN: u16 = 0x00;
    pub const KEY: u16 = 0x01;
    pub const REL: u16 = 0x02;
    pub const ABS: u16 = 0x03;
    pub const MSC: u16 = 0x04;
}

/// One input event, the host-side push unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputEvent {
    pub kind: u16,
    pub code: u16,
    pub value: u32,
}

impl InputEvent {
    pub fn syn() -> Self {
        Self {
            kind: ev::SYN,
            code: 0,
            value: 0,
        }
    }

    pub fn key(code: u16, down: bool) -> Self {
        Self {
            kind: ev::KEY,
            code,
            value: u32::from(down),
        }
    }
}

/// Config-space selectors (uapi/virtio_input.h).
mod cfg_sel {
    pub const ID_NAME: u8 = 0x01;
    pub const ID_SERIAL: u8 = 0x02;
    pub const ID_DEVIDS: u8 = 0x03;
    pub const PROP_BITS: u8 = 0x04;
    pub const EV_BITS: u8 = 0x05;
    pub const ABS_INFO: u8 = 0x06;
}

/// struct virtio_input_config, the 136-byte register the driver
/// queries with select/subsel.
struct InputConfig {
    select: u8,
    subsel: u8,
    /// The size the last select/subsel query answered with.
    size: u8,
    data: [u8; 128],
}

pub struct VirtioInput {
    config: InputConfig,
    /// Event codes this device claims per event type (select EV_BITS
    /// answers with these bitmaps).
    key_bits: [u8; 16],
    #[allow(dead_code)] // published through the config-space answer
    ev_bits: [u8; 1],
    name: Vec<u8>,
    pending: Mutex<Vec<InputEvent>>,
}

impl VirtioInput {
    pub fn new(name: &str) -> Self {
        let mut key_bits = [0u8; 16];
        // KEY_ESC..KEY_10 (0x01..0x0B) plus arrows and enter.
        for code in 1u16..=0x0B {
            key_bits[(code / 8) as usize] |= 1 << (code % 8);
        }
        for code in [28u16, 103, 105, 106, 108] {
            key_bits[(code / 8) as usize] |= 1 << (code % 8);
        }
        let mut ev_bits = [0u8; 1];
        ev_bits[0] = 0x03; // SYN | KEY
        Self {
            config: InputConfig {
                select: 0,
                subsel: 0,
                size: 0,
                data: [0; 128],
            },
            key_bits,
            ev_bits,
            name: {
                let mut name = name.as_bytes().to_vec();
                name.push(0);
                name
            },
            pending: Mutex::new(Vec::new()),
        }
    }

    /// Push one event from the host side (machine glue or a frontend).
    /// SYN-separated batches are the caller's choice, matching evdev.
    pub fn push_event(&self, event: InputEvent) {
        if let Ok(mut pending) = self.pending.lock() {
            if pending.len() < 256 {
                pending.push(event);
            }
        }
    }

    fn answer_query(&mut self) {
        self.config.data = [0; 128];
        let size = match (self.config.select, self.config.subsel) {
            (cfg_sel::ID_NAME, _) => {
                let len = self.name.len().min(128);
                self.config.data[..len].copy_from_slice(&self.name[..len]);
                len as u8
            }
            (cfg_sel::ID_SERIAL, _) => 0,
            (cfg_sel::ID_DEVIDS, _) => {
                // struct input_id: bustype u16, vendor u16, product u16,
                // version u16.
                self.config.data[0..2].copy_from_slice(&0x06u16.to_le_bytes()); // BUS_VIRTUAL
                self.config.data[2..4].copy_from_slice(&0x5245u16.to_le_bytes());
                self.config.data[4..6].copy_from_slice(&0x0001u16.to_le_bytes());
                self.config.data[6..8].copy_from_slice(&0x0001u16.to_le_bytes());
                8
            }
            (cfg_sel::PROP_BITS, _) => 0,
            (cfg_sel::EV_BITS, ev_type) => {
                // Which event codes does event type `ev_type` support?
                match ev_type {
                    0 => {
                        self.config.data[0] = 0x03; // SYN + KEY
                        1
                    }
                    1 => {
                        let len = self.key_bits.len();
                        self.config.data[..len].copy_from_slice(&self.key_bits);
                        len as u8
                    }
                    _ => 0,
                }
            }
            (cfg_sel::ABS_INFO, _) => 0,
            _ => 0,
        };
        self.config.size = size;
    }
}

impl Default for VirtioInput {
    fn default() -> Self {
        Self::new("mosaic-input")
    }
}

impl VirtioDevice for VirtioInput {
    fn device_id(&self) -> u32 {
        device_id::INPUT
    }

    fn device_features(&self) -> u64 {
        features::VERSION_1
    }

    fn max_queue_size(&self) -> u32 {
        64
    }

    fn queue_count(&self) -> usize {
        2
    }

    fn config_read(&self, offset: u64, size: u8) -> u64 {
        // struct virtio_input_config: select u8 | subsel u8 | size u8 |
        // reserved u8 | data[128].
        match (offset, size) {
            (0, 1) => u64::from(self.config.select),
            (1, 1) => u64::from(self.config.subsel),
            (2, 1) => u64::from(self.config.size),
            (4.., _) => {
                let start = (offset - 4) as usize;
                let mut value = 0u64;
                for (index, byte) in self
                    .config
                    .data
                    .iter()
                    .skip(start)
                    .take(size as usize)
                    .enumerate()
                {
                    value |= u64::from(*byte) << (index * 8);
                }
                value
            }
            _ => 0,
        }
    }

    fn config_write(&mut self, offset: u64, _size: u8, value: u64) {
        match offset {
            0 => {
                self.config.select = value as u8;
                self.answer_query();
            }
            1 => {
                self.config.subsel = value as u8;
                self.answer_query();
            }
            _ => {}
        }
    }

    fn process(
        &mut self,
        _ctx: &mut dyn mosaic_contract::model::Ctx,
        queue: usize,
        slot: &mut QueueSlot,
        mem: &mut dyn GuestMemory,
        irq: &mut VirtioIrq<'_>,
    ) -> Result<(), ProcessError> {
        match queue {
            // Event queue: drain pending events into posted buffers.
            0 => {
                let events: Vec<InputEvent> = self
                    .pending
                    .lock()
                    .map(|mut pending| std::mem::take(&mut *pending))
                    .unwrap_or_default();
                let mut index = 0usize;
                while index < events.len() {
                    let Some(head) = slot.view().next_head(mem)? else {
                        break;
                    };
                    let mut segments: Vec<(u64, u32)> = Vec::new();
                    {
                        let vq = slot.view();
                        vq.walk(mem, head, |addr, len, writable, index| {
                            if !writable {
                                return Err(ProcessError::NotWritable { index });
                            }
                            segments.push((addr, len));
                            Ok(())
                        })?;
                    }
                    let mut written = 0u32;
                    for (addr, len) in segments {
                        let mut offset = 0usize;
                        while offset + 8 <= len as usize && index < events.len() {
                            let event = events[index];
                            let mut bytes = [0u8; 8];
                            bytes[0..2].copy_from_slice(&event.kind.to_le_bytes());
                            bytes[2..4].copy_from_slice(&event.code.to_le_bytes());
                            bytes[4..8].copy_from_slice(&event.value.to_le_bytes());
                            mem.write_bytes(addr + offset as u64, &bytes)
                                .map_err(ProcessError::from)?;
                            offset += 8;
                            index += 1;
                            written += 8;
                        }
                    }
                    slot.view().complete(mem, head, written)?;
                }
            }
            // Status queue: the guest reports LED/repeat state; the
            // device accepts and completes it (no host consumer yet).
            1 => {
                while let Some(head) = slot.view().next_head(mem)? {
                    let mut data = Vec::new();
                    {
                        let vq = slot.view();
                        vq.read_chain(mem, head, &mut data)?;
                    }
                    let _ = data;
                    slot.view().complete(mem, head, 0)?;
                }
            }
            _ => {}
        }
        irq.queue_interrupt();
        Ok(())
    }

    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ip::virtio::test_ctx::support::RecCtx;
    use crate::ip::virtio::vqueue::tests_support::TestRam;
    use crate::ip::virtio::vqueue::{QueueConfig, F_WRITE};

    fn event_rig(ram: &mut TestRam, data: u64, len: u32) -> QueueSlot {
        let base = 0x1000;
        let mut entry = [0u8; 16];
        entry[0..8].copy_from_slice(&data.to_le_bytes());
        entry[8..12].copy_from_slice(&len.to_le_bytes());
        entry[12..14].copy_from_slice(&F_WRITE.to_le_bytes());
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
        slot
    }

    #[test]
    fn config_answers_name_and_bitmaps() {
        let mut device = VirtioInput::new("test-keyboard");
        device.config_write(0, 1, 0x01); // select = ID_NAME
        assert_eq!(device.config_read(2, 1), 14); // len("test-keyboard\0")
        let mut expected = [0u8; 8];
        expected[..4].copy_from_slice(b"test");
        assert_eq!(device.config_read(4, 4), u64::from_le_bytes(expected));
        // EV_BITS for EV_KEY: ESC (code 1) is set.
        device.config_write(0, 1, 0x05);
        device.config_write(1, 1, 0x01);
        let bitmap = device.config_read(4, 2);
        assert_ne!(bitmap & 0b10, 0);
    }

    #[test]
    fn events_flow_into_posted_buffers() {
        let mut ram = TestRam::new(0x1_0000);
        let mut slot = event_rig(&mut ram, 0x4000, 32);
        let mut device = VirtioInput::new("kb");
        device.push_event(InputEvent::key(30, true)); // KEY_A down
        device.push_event(InputEvent::syn());
        let mut status = 0u32;
        let mut irq = VirtioIrq::new(
            mosaic_exec::SignalId(0),
            mosaic_exec::NodeId(0),
            &mut status,
        );
        let mut ctx = RecCtx::new();
        VirtioDevice::process(&mut device, &mut ctx, 0, &mut slot, &mut ram, &mut irq).unwrap();
        // 16 bytes: two 8-byte events.
        let len = u32::from_le_bytes(
            ram.bytes[(0x3008 - 0x1000) as usize..][..4]
                .try_into()
                .unwrap(),
        );
        assert_eq!(len, 16);
        let kind = u16::from_le_bytes(
            ram.bytes[(0x4000 - 0x1000) as usize..][..2]
                .try_into()
                .unwrap(),
        );
        let code = u16::from_le_bytes(
            ram.bytes[(0x4002 - 0x1000) as usize..][..2]
                .try_into()
                .unwrap(),
        );
        assert_eq!((kind, code), (ev::KEY, 30));
    }
}
