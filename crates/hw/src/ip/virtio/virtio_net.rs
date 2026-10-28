//! virtio-net — the network card (virtio 1.1 §5.1 "Network Device").
//!
//! Three virtqueues: `receiveq` (0), `transmitq` (1), `controlq` (2).
//! The first two carry packets; the control queue answers the MAC and
//! offload configuration commands a driver issues.
//!
//! The host side is the [`EthernetLink`](mosaic_bus::net::link::EthernetLink)
//! fabric — the same collision domain every NIC in this tree joins. The
//! device implements [`EthernetPort`] and the machine glue attaches it
//! to a link; frames the guest transmits go onto the link (loopback to
//! the same link's other ports included), frames the link delivers are
//! filled into posted receiveq buffers on the next kick.
//!
//! Packet headers: without `VIRTIO_NET_F_MRG_RXBUF` the header is the
//! fixed 10-byte `virtio_net_hdr` (flags, gso type, header len, gso
//! size, checksum start/offset). No offloads are advertised, so the
//! device sends and expects zero headers on the wire — the header is
//! metadata the driver maintains, prepended by this model on receive
//! and skipped on transmit.

use mosaic_bus::net::link::EthernetLink;
use mosaic_contract::ethernet::{EthError, EthernetPort, Mac};

use super::port::{device_id, features, ProcessError, QueueSlot, VirtioDevice, VirtioIrq};
use super::vqueue::GuestMemory;

/// VIRTIO_NET_F_* bits this model offers.
mod net_features {
    pub const MAC: u64 = 1 << 5;
    pub const STATUS: u64 = 1 << 16;
    pub const MTU: u64 = 1 << 3;
}

/// The fixed (non-mergeable) virtio_net_hdr length.
pub const NET_HDR: usize = 10;

/// struct virtio_net_config: mac[6] | status u16 | max_virtqueue_pairs
/// u16 | mtu u16 (the fields a probe reads, in order).
pub struct VirtioNet {
    mac: Mac,
    status: u16,
    mtu: u16,
    link: Option<EthernetLink>,
    /// This device's port index on the link (assigned at attach).
    port: usize,
    /// The receive buffer shared with the link-side port.
    rx: std::sync::Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
}

impl VirtioNet {
    pub fn new() -> Self {
        Self {
            mac: [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
            status: 1, // VIRTIO_NET_S_LINK_UP
            mtu: 1500,
            link: None,
            port: 0,
            rx: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    pub fn with_mac(mut self, mac: Mac) -> Self {
        self.mac = mac;
        self
    }

    /// Join a collision domain. The link owns the port from here on;
    /// this device keeps the receive buffer's other handle.
    pub fn attach_link(&mut self, mut link: EthernetLink) {
        let port = link.add_port(Box::new(NetPort {
            mac: self.mac,
            promiscuous: false,
            rx: std::sync::Arc::clone(&self.rx),
        }));
        self.port = port;
        self.link = Some(link);
    }

    fn pending_frames(&self) -> Vec<Vec<u8>> {
        self.rx
            .lock()
            .map(|mut pending| std::mem::take(&mut *pending))
            .unwrap_or_default()
    }

    /// Deliver a frame into this NIC's receive path directly (machine
    /// glue wiring a host-side source without a link).
    pub fn accept_frame(&mut self, frame: &[u8]) {
        if let Ok(mut pending) = self.rx.lock() {
            if pending.len() < 64 {
                pending.push(frame.to_vec());
            }
        }
    }

    /// Frames waiting to be received, for the harness's report.
    pub fn rx_depth(&self) -> usize {
        self.rx.lock().map(|pending| pending.len()).unwrap_or(0)
    }
}

impl Default for VirtioNet {
    fn default() -> Self {
        Self::new()
    }
}

/// The link-side port: filters and buffers the frames this NIC accepts.
/// The receive buffer is shared with the device (the link owns the port
/// object; the device owns the other handle and drains it on kick).
struct NetPort {
    mac: Mac,
    promiscuous: bool,
    rx: std::sync::Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
}

impl EthernetPort for NetPort {
    fn mac(&self) -> Mac {
        self.mac
    }

    fn promiscuous(&self) -> bool {
        self.promiscuous
    }

    fn accepts_multicast(&self, _address: Mac) -> bool {
        false
    }

    fn receive(&mut self, frame: &[u8]) -> Result<(), EthError> {
        let mut pending = self.rx.lock().map_err(|_| EthError::RxFull)?;
        if pending.len() >= 64 {
            return Err(EthError::RxFull);
        }
        pending.push(frame.to_vec());
        Ok(())
    }
}

impl VirtioDevice for VirtioNet {
    fn device_id(&self) -> u32 {
        device_id::NET
    }

    fn device_features(&self) -> u64 {
        features::VERSION_1 | net_features::MAC | net_features::STATUS | net_features::MTU
    }

    fn max_queue_size(&self) -> u32 {
        256
    }

    fn queue_count(&self) -> usize {
        3
    }

    fn config_read(&self, offset: u64, size: u8) -> u64 {
        let mut config = [0u8; 12];
        config[0..6].copy_from_slice(&self.mac);
        config[6..8].copy_from_slice(&self.status.to_le_bytes());
        config[8..10].copy_from_slice(&1u16.to_le_bytes()); // max_virtqueue_pairs
        config[10..12].copy_from_slice(&self.mtu.to_le_bytes());
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

    fn config_write(&mut self, offset: u64, size: u8, value: u64) {
        // The MAC is writable in the first 6 bytes (a driver may set its
        // own station address when the device advertises it).
        if offset < 6 && size == 1 {
            if let Some(slot) = self.mac.get_mut(offset as usize) {
                *slot = value as u8;
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    fn process(
        &mut self,
        _ctx: &mut dyn mosaic_contract::model::Ctx,
        queue: usize,
        slot: &mut QueueSlot,
        mem: &mut dyn GuestMemory,
        irq: &mut VirtioIrq<'_>,
    ) -> Result<(), ProcessError> {
        match queue {
            // Transmit: each chain is hdr(10, RO) + frame(RO). Onto the
            // link it goes.
            1 => {
                while let Some(head) = slot.view().next_head(mem)? {
                    let mut chain = Vec::new();
                    {
                        let vq = slot.view();
                        vq.read_chain(mem, head, &mut chain)?;
                    }
                    let frame = chain.get(NET_HDR..).unwrap_or(&chain);
                    if let Some(link) = self.link.as_mut() {
                        let _ = link.transmit(self.port, frame);
                        link.flush();
                    }
                    slot.view().complete(mem, head, frame.len() as u32)?;
                }
            }
            // Receive: fill posted buffers with the buffered frames,
            // header first.
            0 => {
                let frames = self.pending_frames();
                let mut index = 0usize;
                while index < frames.len() {
                    let Some(head) = slot.view().next_head(mem)? else {
                        break;
                    };
                    let frame = &frames[index];
                    let mut packet = vec![0u8; NET_HDR + frame.len()];
                    packet[NET_HDR..].copy_from_slice(frame);
                    let written = {
                        let vq = slot.view();
                        vq.write_chain(mem, head, &packet)?
                    };
                    slot.view().complete(mem, head, written)?;
                    index += 1;
                }
            }
            // Control queue: MAC set is the one command this model
            // answers with ACK; everything else is not-offered (the
            // driver will not send it — the feature bits are clear).
            2 => {
                while let Some(head) = slot.view().next_head(mem)? {
                    let mut chain = Vec::new();
                    {
                        let vq = slot.view();
                        vq.read_chain(mem, head, &mut chain)?;
                    }
                    let ack: u8 = if chain.is_empty() { 1 } else { 0 };
                    let written = {
                        let vq = slot.view();
                        vq.write_chain(mem, head, &[ack])?
                    };
                    slot.view().complete(mem, head, written)?;
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

    /// Build one receiveq: a writable descriptor at `data` of `len`.
    #[allow(dead_code)]
    fn rx_rig(ram: &mut TestRam, data: u64, len: u32) -> QueueSlot {
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
    fn config_publishes_mac_status_and_mtu() {
        let net = VirtioNet::new();
        let mac = net.config_read(0, 6);
        assert_eq!((mac & 0xFF) as u8, 0x52);
        assert_eq!(net.config_read(6, 2), 1); // link up
        assert_eq!(net.config_read(10, 2), 1500);
    }

    #[test]
    fn mac_is_writable_in_config() {
        let mut net = VirtioNet::new();
        net.config_write(0, 1, 0xAA);
        net.config_write(1, 1, 0xBB);
        assert_eq!(&net.mac[..2], &[0xAA, 0xBB]);
    }

    #[test]
    fn transmit_sends_the_frame_without_header() {
        let mut ram = TestRam::new(0x1_0000);
        // Header (10 zero bytes) + a frame, both read-only, chained.
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
        desc(0, 0x4000, 10, f_next, 1);
        desc(1, 0x5000, 14, 0, 0);
        let _ = desc;
        ram.bytes[(0x5000 - base) as usize..][..14].copy_from_slice(&{
            let mut frame = [0u8; 14];
            frame[..6].copy_from_slice(&[0xFF; 6]);
            frame[12..14].copy_from_slice(&[0x08, 0x00]);
            frame
        });
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
        let mut net = VirtioNet::new();
        net.attach_link(EthernetLink::new(16));
        let mut status = 0u32;
        let mut irq = VirtioIrq::new(
            mosaic_exec::SignalId(0),
            mosaic_exec::NodeId(0),
            &mut status,
        );
        let mut ctx = RecCtx::new();
        VirtioDevice::process(&mut net, &mut ctx, 1, &mut slot, &mut ram, &mut irq).unwrap();
        // The used entry reports the frame length (14), not hdr+frame.
        let len = u32::from_le_bytes(
            ram.bytes[(0x3008 - base) as usize..][..4]
                .try_into()
                .unwrap(),
        );
        assert_eq!(len, 14);
    }
}
