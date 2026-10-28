//! virtio-console — the serial port (virtio 1.1 §5.3 "Console Device").
//!
//! Two virtqueues: `receiveq` (0, host → guest) and `transmitq` (1,
//! guest → host). The config space: `cols` u16, `rows` u16,
//! `max_nr_ports` u32, `emerg_wr` u32 — the fields the Linux virtconsole
//! probe reads before touching a queue.
//!
//! The host side is a [`ByteStream`](mosaic_contract::stream::ByteStream)
//! — the same endpoint a PL011 or NS16550A attaches — so a machine wires
//! one console path for every style of serial device. Bytes the guest
//! transmits land in the stream immediately; bytes the host has ready
//! are filled into receiveq buffers on the next kick (the guest re-kicks
//! after consuming, which is the virtconsole flow).

use mosaic_contract::stream::ByteStream;

use super::port::{device_id, features, ProcessError, QueueSlot, VirtioDevice, VirtioIrq};
use super::vqueue::GuestMemory;

pub struct VirtioConsole {
    stream: Option<Box<dyn ByteStream>>,
    cols: u16,
    rows: u16,
    max_nr_ports: u32,
}

impl VirtioConsole {
    pub fn new() -> Self {
        Self {
            stream: None,
            cols: 80,
            rows: 24,
            max_nr_ports: 1,
        }
    }

    pub fn attach_stream(&mut self, stream: Box<dyn ByteStream>) {
        self.stream = Some(stream);
    }
}

impl Default for VirtioConsole {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtioDevice for VirtioConsole {
    fn device_id(&self) -> u32 {
        device_id::CONSOLE
    }

    fn device_features(&self) -> u64 {
        features::VERSION_1 | (1 << 0) // VIRTIO_CONSOLE_F_SIZE
    }

    fn max_queue_size(&self) -> u32 {
        64
    }

    fn queue_count(&self) -> usize {
        2
    }

    fn config_read(&self, offset: u64, size: u8) -> u64 {
        match (offset, size) {
            (0, 2) => u64::from(self.cols),
            (2, 2) => u64::from(self.rows),
            (4, 4) => u64::from(self.max_nr_ports),
            (0..4, _) => {
                let mut value = u64::from(self.cols);
                if size >= 2 {
                    value |= u64::from(self.rows) << 16;
                }
                value
            }
            (4..8, _) => u64::from(self.max_nr_ports),
            _ => 0,
        }
    }

    fn config_write(&mut self, _offset: u64, _size: u8, _value: u64) {}

    fn process(
        &mut self,
        ctx: &mut dyn mosaic_contract::model::Ctx,
        queue: usize,
        slot: &mut QueueSlot,
        mem: &mut dyn GuestMemory,
        irq: &mut VirtioIrq<'_>,
    ) -> Result<(), ProcessError> {
        match queue {
            // Transmit: drain the chain into the host stream.
            1 => {
                while let Some(head) = slot.view().next_head(mem)? {
                    let mut data = Vec::new();
                    {
                        let vq = slot.view();
                        vq.read_chain(mem, head, &mut data)?;
                    }
                    let written = self
                        .stream
                        .as_mut()
                        .map(|stream| stream.write_bytes(ctx, &data).unwrap_or(0))
                        .unwrap_or(0);
                    slot.view().complete(mem, head, written as u32)?;
                }
            }
            // Receive: fill whatever the host stream holds into the
            // posted buffers.
            0 => {
                let mut buffer = [0u8; 512];
                let count = self
                    .stream
                    .as_mut()
                    .map(|stream| stream.read_bytes(ctx, &mut buffer).unwrap_or(0))
                    .unwrap_or(0);
                if count > 0 {
                    'buffers: while let Some(head) = slot.view().next_head(mem)? {
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
                            let take = (len as usize).min(count - written as usize);
                            mem.write_bytes(
                                addr,
                                &buffer[written as usize..written as usize + take],
                            )
                            .map_err(ProcessError::from)?;
                            written += take as u32;
                            if written as usize >= count {
                                break 'buffers;
                            }
                        }
                        slot.view().complete(mem, head, written)?;
                    }
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
    use mosaic_contract::stream::ByteStream;
    use mosaic_exec::SignalId;

    struct LoopStream {
        written: Vec<u8>,
        pending: Vec<u8>,
    }

    impl ByteStream for LoopStream {
        fn write_bytes(
            &mut self,
            _ctx: &mut dyn mosaic_contract::model::Ctx,
            data: &[u8],
        ) -> Result<usize, mosaic_contract::stream::StreamError> {
            self.written.extend_from_slice(data);
            Ok(data.len())
        }
        fn read_bytes(
            &mut self,
            _ctx: &mut dyn mosaic_contract::model::Ctx,
            out: &mut [u8],
        ) -> Result<usize, mosaic_contract::stream::StreamError> {
            let take = out.len().min(self.pending.len());
            out[..take].copy_from_slice(&self.pending[..take]);
            self.pending.drain(..take);
            Ok(take)
        }
    }

    fn queue_rig(ram: &mut TestRam, data_addr: u64, len: u32, writable: bool) -> QueueSlot {
        let base = 0x1000;
        let mut entry = [0u8; 16];
        entry[0..8].copy_from_slice(&data_addr.to_le_bytes());
        entry[8..12].copy_from_slice(&len.to_le_bytes());
        entry[12..14].copy_from_slice(&(if writable { F_WRITE } else { 0 }).to_le_bytes());
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
    fn transmit_drains_into_the_stream() {
        let mut ram = TestRam::new(0x1_0000);
        ram.bytes[(0x4000 - 0x1000) as usize..][..5].copy_from_slice(b"hello");
        let mut slot = queue_rig(&mut ram, 0x4000, 5, false);
        let mut device = VirtioConsole::new();
        let mut stream = LoopStream {
            written: Vec::new(),
            pending: Vec::new(),
        };
        stream.pending.extend_from_slice(b"");
        device.attach_stream(Box::new(stream));
        let mut status = 0u32;
        let mut irq = VirtioIrq::new(SignalId(0), mosaic_exec::NodeId(0), &mut status);
        VirtioDevice::process(
            &mut device,
            &mut RecCtx::new(),
            1,
            &mut slot,
            &mut ram,
            &mut irq,
        )
        .unwrap();
        assert_eq!(status & 1, 1);
        // The used ring carries the byte count.
        let len = u32::from_le_bytes(
            ram.bytes[(0x3008 - 0x1000) as usize..][..4]
                .try_into()
                .unwrap(),
        );
        assert_eq!(len, 5);
    }

    #[test]
    fn receive_fills_posted_buffers() {
        let mut ram = TestRam::new(0x1_0000);
        let mut slot = queue_rig(&mut ram, 0x4000, 16, true);
        let mut device = VirtioConsole::new();
        device.attach_stream(Box::new(LoopStream {
            written: Vec::new(),
            pending: b"ping".to_vec(),
        }));
        let mut status = 0u32;
        let mut irq = VirtioIrq::new(SignalId(0), mosaic_exec::NodeId(0), &mut status);
        VirtioDevice::process(
            &mut device,
            &mut RecCtx::new(),
            0,
            &mut slot,
            &mut ram,
            &mut irq,
        )
        .unwrap();
        assert_eq!(&ram.bytes[(0x4000 - 0x1000) as usize..][..4], b"ping");
    }
}
