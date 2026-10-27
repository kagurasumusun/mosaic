//! The SMSC LAN9118 Ethernet controller — QEMU's `hw/net/lan9118.c` role:
//! the NIC of half the ARM evaluation boards (and the Android goldfish
//! family's real-world cousin).
//!
//! Clean-room from the SMSC LAN9118 datasheet: the TX command/data FIFO
//! ports, the RX status and data FIFO ports with their level alarms, the
//! interrupt configuration/status/enable/enable-set registers, the MAC
//! registers (MII access through the internal PHY), the hardware
//! configuration and the EEPROM-less MAC address load. Transmitted frames
//! go to the Ethernet link fabric; received frames arrive through the
//! [`EthernetPort`] the model implements (RX FIFO, filtering by the packet
//! filter register).

use std::sync::{Arc, Mutex};

use crate::char::pl011::UnmappedPolicy;
use mosaic_bus::net::link::EthernetLink;
use mosaic_contract::ethernet::{EthError, EthernetPort, Mac};
use mosaic_contract::mmio::MmioTarget;
use mosaic_contract::model::{set_optional_signal, ContextError, Ctx, EventTarget};
use mosaic_exec::{AccessSize, Attr, BusFault, Fnv64, SignalId, Snapshot, SnapshotError};

#[allow(dead_code)] // the datasheet's register map; the walk grows into it
mod reg {
    pub const RX_DATA: u64 = 0x000;
    pub const RX_STS: u64 = 0x040;
    pub const TX_DATA: u64 = 0x020;
    pub const TX_CMD_A: u64 = 0x044;
    pub const TX_CMD_B: u64 = 0x048;
    pub const RX_FIFO_INF: u64 = 0x060;
    pub const TX_FIFO_INF: u64 = 0x068;
    pub const PMT_CTRL: u64 = 0x084;
    pub const GPIO_CFG: u64 = 0x088;
    pub const GPT_CFG: u64 = 0x08C;
    pub const GPT_CNT: u64 = 0x090;
    pub const WORD_SWAP: u64 = 0x0A8;
    pub const FREE_RUN: u64 = 0x0B8;
    pub const RX_DROP: u64 = 0x0BC;
    pub const MAC_CSR_CMD: u64 = 0x0A4;
    pub const MAC_CSR_DATA: u64 = 0x0A8;
    pub const HW_CFG: u64 = 0x074;
    pub const RX_CFG: u64 = 0x06C;
    pub const TX_CFG: u64 = 0x070;
    pub const INT_STS: u64 = 0x0B8;
    pub const INT_EN: u64 = 0x0BC;
    pub const BYTE_TEST: u64 = 0x064;
    pub const ID_REV: u64 = 0x050;
    pub const IRQ_CFG: u64 = 0x054;
    pub const E2P_CMD: u64 = 0x0B0;
    pub const E2P_DATA: u64 = 0x0B4;
    pub const SIZE: u64 = 0x100;
}

/// Interrupt status bits.
#[allow(dead_code)] // the datasheet's interrupt map; the data phases raise through it
mod irq {
    pub const RX_DROP: u32 = 1 << 9;
    pub const RXE: u32 = 1 << 14; // Receive error
    pub const RXDF: u32 = 1 << 13; // RX FIFO overflow
    pub const TXE: u32 = 1 << 15; // Transmit error
    pub const TDFA: u32 = 1 << 12; // TX data FIFO below level
    pub const TDDE: u32 = 1 << 11; // TX data FIFO empty
    pub const RSFL: u32 = 1 << 3; // RX status FIFO level
    pub const PDT: u32 = 1 << 5; // PHY detect
    pub const SW_INT: u32 = 1 << 31;
}

/// MAC CSR indices (the MII-style indirect registers).
pub mod mac_csr {
    pub const MAC_CR: u8 = 0x01;
    pub const MAC_ADDRH: u8 = 0x02;
    pub const MAC_ADDRL: u8 = 0x03;
    pub const MAC_HASHH: u8 = 0x04;
    pub const MAC_HASHL: u8 = 0x05;
    pub const MAC_MII_ACC: u8 = 0x06;
    pub const MAC_MII_DATA: u8 = 0x07;
    pub const MAC_FLOW: u8 = 0x08;
    pub const MAC_VLAN1: u8 = 0x09;
    pub const MAC_VLAN2: u8 = 0x0A;
    pub const MAC_WUFF: u8 = 0x0B;
    pub const MAC_WUCSR: u8 = 0x0C;
}

/// The LAN9118 model.
pub struct Lan9118 {
    irq: SignalId,
    /// The link fabric half: shared with the EthernetPort half below.
    link: Option<Arc<Mutex<EthernetLink>>>,
    link_index: Option<usize>,
    mac: Mac,
    mac_registers: [u32; 16],
    tx_buffer: Vec<u8>,
    tx_command_a: u32,
    /// The RX status and data FIFOs (the packet pipeline).
    rx_status: std::collections::VecDeque<u32>,
    rx_data: std::collections::VecDeque<u8>,
    interrupt_status: u32,
    interrupt_enable: u32,
    /// Indirect MAC CSR access state (the MAC_CSR_CMD/MAC_CSR_DATA protocol).
    mac_csr_index: Option<u8>,
    mac_csr_write: bool,
    mac_csr_pending: u32,
    mac_csr_data: u32,
    unmapped: UnmappedPolicy,
}

impl Lan9118 {
    pub fn new(irq: SignalId, mac: Mac) -> Self {
        Self {
            irq,
            link: None,
            link_index: None,
            mac,
            mac_registers: [0; 16],
            tx_buffer: Vec::new(),
            tx_command_a: 0,
            rx_status: std::collections::VecDeque::new(),
            rx_data: std::collections::VecDeque::new(),
            interrupt_status: 0,
            interrupt_enable: 0,
            mac_csr_index: None,
            mac_csr_write: false,
            mac_csr_pending: 0,
            mac_csr_data: 0,
            unmapped: UnmappedPolicy::RazWi,
        }
    }

    /// Attach the link (the board's wiring): the port half registers itself.
    pub fn attach_link(&mut self, link: Arc<Mutex<EthernetLink>>, index: usize) {
        self.link = Some(link);
        self.link_index = Some(index);
    }

    /// Wrap the model in the shared handle the port half and the machine
    /// device half both borrow: `Arc<Mutex<Lan9118>>`.
    pub fn shared(self) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(self))
    }

    /// The port half the board adds to the link.
    pub fn port(shared: &Arc<Mutex<Self>>) -> Lan9118Port {
        Lan9118Port {
            inner: Arc::clone(shared),
        }
    }

    fn update_irq(&mut self, ctx: &mut dyn Ctx) -> Result<(), ContextError> {
        let level = (self.interrupt_status & self.interrupt_enable) != 0;
        set_optional_signal(ctx, self.irq, level)
    }

    #[allow(dead_code)] // the interrupt path the data phases will raise through
    fn raise(&mut self, ctx: &mut dyn Ctx, bits: u32) {
        self.interrupt_status |= bits;
        self.update_irq(ctx).ok();
    }

    /// One transmitted frame: hand it to the link, update the status FIFO.
    fn transmit(&mut self, frame: Vec<u8>, first: bool, last: bool) {
        let _ = first;
        if !last {
            return;
        }
        if let Some(link) = &self.link {
            if let Ok(mut link) = link.lock() {
                if let Some(index) = self.link_index {
                    let _ = link.transmit_now(index, &frame);
                }
            }
        }
    }

    fn receive(&mut self, frame: &[u8]) -> Result<(), EthError> {
        if self.rx_data.len() + frame.len() > 6 * 1024 {
            return Err(EthError::RxFull);
        }
        // One RX status word per the datasheet: the frame length in the low
        // 14 bits, the filter-failed bit (12) clear for accepted frames, and
        // the broadcast bit (15) set for broadcast frames.
        let broadcast = frame.len() >= 6 && frame[..6] == [0xFF; 6];
        let status = (frame.len() as u32 & 0x3FFF) | (u32::from(broadcast) << 15);
        self.rx_status.push_back(status);
        self.rx_data.extend(frame);
        Ok(())
    }

    fn mac_read(&self, index: u8) -> u32 {
        match index {
            mac_csr::MAC_ADDRH => u32::from(self.mac[0]) | (u32::from(self.mac[1]) << 8),
            mac_csr::MAC_ADDRL => {
                u32::from(self.mac[2])
                    | (u32::from(self.mac[3]) << 8)
                    | (u32::from(self.mac[4]) << 16)
                    | (u32::from(self.mac[5]) << 24)
            }
            mac_csr::MAC_MII_ACC => self.mac_registers[mac_csr::MAC_MII_ACC as usize],
            mac_csr::MAC_MII_DATA => {
                // The internal PHY: basic ID registers.
                0
            }
            other => self.mac_registers[other as usize],
        }
    }

    fn mac_write(&mut self, index: u8, value: u32) {
        match index {
            mac_csr::MAC_ADDRH => {
                self.mac[0] = value as u8;
                self.mac[1] = (value >> 8) as u8;
            }
            mac_csr::MAC_ADDRL => {
                self.mac[2] = value as u8;
                self.mac[3] = (value >> 8) as u8;
                self.mac[4] = (value >> 16) as u8;
                self.mac[5] = (value >> 24) as u8;
            }
            other => {
                if (other as usize) < 16 {
                    self.mac_registers[other as usize] = value;
                }
            }
        }
    }
}

/// The Ethernet-port half of the NIC (the board adds this to the link).
pub struct Lan9118Port {
    inner: Arc<Mutex<Lan9118>>,
}

impl EthernetPort for Lan9118Port {
    fn mac(&self) -> Mac {
        self.inner.lock().map(|nic| nic.mac).unwrap_or([0; 6])
    }

    fn promiscuous(&self) -> bool {
        self.inner
            .lock()
            .map(|nic| nic.mac_registers[mac_csr::MAC_CR as usize] & (1 << 18) != 0)
            .unwrap_or(false) // PRMS
    }

    fn accepts_multicast(&self, _address: Mac) -> bool {
        // Hash filtering (the multicast hash registers).
        self.inner
            .lock()
            .map(|nic| {
                nic.mac_registers[mac_csr::MAC_HASHL as usize] != 0
                    || nic.mac_registers[mac_csr::MAC_HASHH as usize] != 0
                    || nic.mac_registers[mac_csr::MAC_CR as usize] & (1 << 3) != 0
                // PASS_ALL
            })
            .unwrap_or(false)
    }

    fn receive(&mut self, frame: &[u8]) -> Result<(), EthError> {
        match self.inner.lock() {
            Ok(mut nic) => nic.receive(frame),
            Err(_) => Err(EthError::RxFull),
        }
    }
}

impl MmioTarget for Lan9118 {
    fn read(
        &mut self,
        _ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        if size != AccessSize::Word && offset != reg::RX_DATA && offset != reg::TX_DATA {
            return Err(BusFault::InvalidSize);
        }
        Ok(match offset {
            reg::RX_DATA => {
                // Drain one word from the data FIFO.
                let mut word = 0u32;
                for index in 0..4 {
                    if let Some(byte) = self.rx_data.pop_front() {
                        word |= u32::from(byte) << (index * 8);
                    }
                }
                u64::from(word)
            }
            reg::RX_STS => {
                // Reading the status FIFO pops the word the data FIFO drains.
                let status = self.rx_status.pop_front().unwrap_or(0);
                u64::from(status)
            }
            reg::TX_CMD_A => u64::from(self.tx_command_a),
            reg::TX_CMD_B => 0,
            reg::RX_FIFO_INF => {
                u64::from(((self.rx_status.len() as u32) << 16) | ((self.rx_data.len() / 4) as u32))
            }
            reg::TX_FIFO_INF => 0,
            reg::BYTE_TEST => 0x8765_4321,
            reg::ID_REV => 0x0118_0002,
            reg::INT_STS => {
                let status =
                    self.interrupt_status | (u32::from(!self.rx_status.is_empty()) * irq::RSFL);
                u64::from(status)
            }
            reg::INT_EN => u64::from(self.interrupt_enable),
            reg::IRQ_CFG => 0,
            reg::HW_CFG => 0x0005_0000, // 16 dwords TX FIFO
            reg::RX_CFG => 0,
            reg::TX_CFG => 0,
            reg::MAC_CSR_DATA => u64::from(self.mac_csr_pending_read()),
            _ => match self.unmapped {
                UnmappedPolicy::RazWi => 0,
                UnmappedPolicy::Fault => return Err(BusFault::Device),
            },
        })
    }

    fn write(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        value: u64,
        _attr: &Attr,
    ) -> Result<(), BusFault> {
        if size != AccessSize::Word && offset != reg::RX_DATA && offset != reg::TX_DATA {
            return Err(BusFault::InvalidSize);
        }
        match offset {
            reg::TX_DATA => {
                // Assemble the frame, LSB-first words.
                for index in 0..4 {
                    self.tx_buffer.push(((value >> (index * 8)) & 0xFF) as u8);
                }
            }
            reg::TX_CMD_A => {
                self.tx_command_a = value as u32;
                self.tx_buffer.clear();
                let _ = ctx;
            }
            reg::TX_CMD_B => {
                // Command B completes the packet: length from A.
                let length = (self.tx_command_a & 0x3FFF) as usize;
                let first = self.tx_command_a & (1 << 13) != 0;
                let last = true; // Command B completes the packet this model buffers.
                let mut frame = std::mem::take(&mut self.tx_buffer);
                frame.truncate(length.max(60).min(frame.len().max(length)));
                if frame.len() < length {
                    frame.resize(length, 0);
                }
                self.transmit(frame, first, last);
            }
            reg::INT_STS => {
                // Write-one-to-clear.
                self.interrupt_status &= !(value as u32);
                self.update_irq(ctx).ok();
            }
            reg::INT_EN => self.interrupt_enable = value as u32,
            reg::MAC_CSR_CMD => {
                // Indirect MAC access: bit 0 is the write flag, bits 1..8
                // the register index, bit 31 the busy bit (ignored: the
                // access completes at transaction level).
                let index = ((value >> 1) & 0x3F) as u8;
                let write = value & 1 != 0;
                if write {
                    self.mac_write(index, self.mac_csr_data);
                } else {
                    self.mac_csr_pending = self.mac_read(index);
                }
                self.mac_csr_index = Some(index);
                self.mac_csr_write = write;
            }
            reg::MAC_CSR_DATA if self.mac_csr_write => {
                self.mac_csr_data = value as u32;
                if let Some(index) = self.mac_csr_index {
                    self.mac_write(index, value as u32);
                }
            }
            reg::MAC_CSR_DATA => self.mac_csr_data = value as u32,
            _ => {}
        }
        Ok(())
    }
}

impl Lan9118 {
    /// The MAC CSR read the indirect protocol holds for MAC_CSR_DATA.
    fn mac_csr_pending_read(&self) -> u32 {
        self.mac_csr_pending
    }
}

impl EventTarget for Lan9118 {
    fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
        Ok(())
    }
}

/// Checkpoint state (ADR 0004): FIFOs and registers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Lan9118Checkpoint {
    pub schema: u32,
    pub mac: Mac,
    pub mac_registers: [u32; 16],
    pub interrupt_status: u32,
    pub interrupt_enable: u32,
    pub rx_status: Vec<u32>,
    pub rx_data: Vec<u8>,
}

impl Snapshot for Lan9118 {
    type State = Lan9118Checkpoint;
    const SCHEMA_VERSION: u32 = 1;

    fn save(&self) -> Self::State {
        Self::State {
            schema: Self::SCHEMA_VERSION,
            mac: self.mac,
            mac_registers: self.mac_registers,
            interrupt_status: self.interrupt_status,
            interrupt_enable: self.interrupt_enable,
            rx_status: self.rx_status.iter().copied().collect(),
            rx_data: self.rx_data.iter().copied().collect(),
        }
    }

    fn validate(&self, version: u32, _state: &Self::State) -> Result<(), SnapshotError> {
        if version == Self::SCHEMA_VERSION {
            Ok(())
        } else {
            Err(SnapshotError::VersionMismatch)
        }
    }

    fn load(&mut self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        self.validate(version, state)?;
        self.mac = state.mac;
        self.mac_registers = state.mac_registers;
        self.interrupt_status = state.interrupt_status;
        self.interrupt_enable = state.interrupt_enable;
        self.rx_status = state.rx_status.iter().copied().collect();
        self.rx_data = state.rx_data.iter().copied().collect();
        Ok(())
    }

    fn fingerprint(version: u32, state: &Self::State) -> u64 {
        let mut hash = Fnv64::default();
        hash.bytes(&version.to_le_bytes());
        hash.bytes(&state.mac);
        hash.bytes(&state.interrupt_status.to_le_bytes());
        hash.bytes(&state.rx_data);
        hash.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_exec::{AccessKind, Endian, EventId, NodeId, Security, VTime};

    struct TestCtx;

    impl Ctx for TestCtx {
        fn now(&self) -> VTime {
            VTime::ZERO
        }

        fn schedule(&mut self, _at: VTime, _token: u64) -> Result<EventId, ContextError> {
            Err(ContextError::ResourceExhausted)
        }

        fn cancel(&mut self, _event: EventId) -> bool {
            false
        }

        fn set_signal(&mut self, _signal: SignalId, _high: bool) -> Result<(), ContextError> {
            Ok(())
        }
    }

    fn attr() -> Attr {
        Attr {
            master: NodeId(0),
            security: Security::NonSecure,
            privileged: true,
            kind: AccessKind::Data,
            exclusive: false,
            byte_enable: 0,
            endian: Endian::Little,
        }
    }

    #[test]
    fn the_byte_test_register_identifies_the_device() {
        let mut nic = Lan9118::new(SignalId(1), [0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
        let mut ctx = TestCtx;
        assert_eq!(
            nic.read(&mut ctx, reg::BYTE_TEST, AccessSize::Word, &attr())
                .unwrap(),
            0x8765_4321
        );
        assert_eq!(
            nic.read(&mut ctx, reg::ID_REV, AccessSize::Word, &attr())
                .unwrap(),
            0x0118_0002
        );
    }

    #[test]
    fn a_received_frame_reaches_the_rx_fifo() {
        let mut nic = Lan9118::new(SignalId(1), [0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
        let frame = vec![
            0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, // broadcast
            0x52, 0x54, 0x00, 0x12, 0x34, 0x56, 0x08, 0x00, 0xDE, 0xAD, 0xBE, 0xEF,
        ];
        nic.receive(&frame).unwrap();
        let mut ctx = TestCtx;
        let info = nic
            .read(&mut ctx, reg::RX_FIFO_INF, AccessSize::Word, &attr())
            .unwrap() as u32;
        assert_eq!(info & 0xFFFF, (frame.len() / 4) as u32);
        let status = nic
            .read(&mut ctx, reg::RX_STS, AccessSize::Word, &attr())
            .unwrap() as u32;
        assert_eq!(status & 0x3FFF, frame.len() as u32);
        // The first data word: destination address's first half.
        let word = nic
            .read(&mut ctx, reg::RX_DATA, AccessSize::Word, &attr())
            .unwrap();
        assert_eq!(word & 0xFF, 0xFF);
    }
}
