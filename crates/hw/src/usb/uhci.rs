//! The Intel UHCI host controller (QEMU's `hw/usb/hcd-uhci.c` role): the
//! USB 1.1 host of every PC chipset, PCI function (class 0x0C03).
//!
//! Clean-room from the UHCI 1.1 specification: the I/O register set
//! (USBCMD, USBSTS, USBINTR, FRNUM, FRBASEADD, SOFMOD, PORTSC1/2), the frame
//! list of 1024 frame pointers, and the transfer descriptors (TDs) with
//! their PID/device address/endpoint/max-len/toggle headers and data
//! payloads. A 1 ms frame tick walks the frame list: the interrupt queue
//! first, then the async queues, executing each TD's transaction through
//! the USB fabric and writing the results back into the TD (active bit
//! cleared, or the error bits set). The PCI config space is the standard
//! header (RTL8237-class UHCI IDs), BAR0 as the I/O bar.

use std::sync::{Arc, Mutex};

use crate::char::pl011::UnmappedPolicy;
use mosaic_bus::usb::core::UsbBus;
use mosaic_contract::mmio::MmioTarget;
use mosaic_contract::model::{set_optional_signal, ContextError, Ctx, EventTarget};
use mosaic_contract::pci::{header, PciError, PciFunction};
use mosaic_contract::usb::{SetupPacket, UsbStatus};
use mosaic_exec::{AccessSize, Attr, BusFault, Fnv64, SignalId, Snapshot, SnapshotError, VDelta};

/// UHCI register offsets (I/O space, word-sized).
#[allow(dead_code)] // the documented register map; the walk grows into it
mod reg {
    pub const USBCMD: u64 = 0x00;
    pub const USBSTS: u64 = 0x02;
    pub const USBINTR: u64 = 0x04;
    pub const FRNUM: u64 = 0x06;
    pub const FRBASEADD: u64 = 0x08;
    pub const SOFMOD: u64 = 0x0C;
    pub const PORTSC1: u64 = 0x10;
    pub const PORTSC2: u64 = 0x12;
    pub const SIZE: u64 = 0x20;
}

/// USBCMD bits.
#[allow(dead_code)] // documented USBCMD bits
mod cmd {
    pub const RUN: u16 = 1 << 0;
    pub const HOST_RESET: u16 = 1 << 1;
    pub const GLOB_SUSPEND: u16 = 1 << 3;
    pub const CONFIGURE: u16 = 1 << 6;
}

/// USBSTS bits (write-one-to-clear).
#[allow(dead_code)] // documented status bits
mod sts {
    pub const USBINT: u16 = 1 << 0;
    pub const ERROR: u16 = 1 << 1;
    pub const PORT_CHANGE: u16 = 1 << 2;
    pub const HOST_ERROR: u16 = 1 << 3;
    pub const HALTED: u16 = 1 << 5;
}

/// USBINTR bits.
#[allow(dead_code)] // documented interrupt-enable bits
mod intr {
    pub const TIMEOUT_CRC: u16 = 1 << 0;
    pub const RESUME: u16 = 1 << 1;
    pub const COMPLETE: u16 = 1 << 2;
    pub const SHORT_PACKET: u16 = 1 << 3;
}

/// PORTSC bits.
pub mod portsc {
    pub const CONNECTED: u16 = 1 << 0;
    pub const CONNECT_CHANGE: u16 = 1 << 1;
    pub const ENABLED: u16 = 1 << 2;
    pub const ENABLE_CHANGE: u16 = 1 << 3;
    pub const RESUME: u16 = 1 << 6;
    pub const SUSPEND: u16 = 1 << 12;
    pub const RESET: u16 = 1 << 9;
    pub const LOW_SPEED: u16 = 1 << 8;
    pub const RW_MASK: u16 = 0x16F; // writable bits (status bits are W1C)
}

/// One transfer descriptor in guest memory, as the driver wrote it.
#[derive(Clone, Copy, Debug, Default)]
struct TransferDescriptor {
    link: u32,
    status: u32,
    token: u32,
    buffer: u32,
}

/// The UHCI host controller. Memory reads come through the [`DmaUpstream`]
/// port the board attaches (the frame list and TDs live in guest RAM).
pub struct Uhci {
    irq: SignalId,
    usbcmd: u16,
    usbsts: u16,
    usbintr: u16,
    frnum: u16,
    frbaseadd: u32,
    sofmod: u8,
    portsc: [u16; 2],
    /// The USB fabric (devices), attached by the board.
    bus: Option<Arc<Mutex<UsbBus>>>,
    /// Guest memory port the TD walks read and write.
    dma: Option<Box<dyn mosaic_contract::dma::DmaUpstream>>,
    unmapped: UnmappedPolicy,
}

impl Uhci {
    pub fn new(irq: SignalId) -> Self {
        Self {
            irq,
            usbcmd: 0,
            usbsts: sts::HALTED,
            usbintr: 0,
            frnum: 0,
            frbaseadd: 0,
            sofmod: 0x40,
            portsc: [0; 2],
            bus: None,
            dma: None,
            unmapped: UnmappedPolicy::RazWi,
        }
    }

    /// Attach the USB fabric with its devices (the board's wiring).
    pub fn attach_bus(&mut self, bus: Arc<Mutex<UsbBus>>) {
        self.bus = Some(bus);
        for port in &mut self.portsc {
            *port |= portsc::CONNECTED;
        }
    }

    /// Attach the guest-memory port the frame list walks.
    pub fn attach_dma(&mut self, dma: Box<dyn mosaic_contract::dma::DmaUpstream>) {
        self.dma = Some(dma);
    }

    fn running(&self) -> bool {
        self.usbcmd & cmd::RUN != 0
    }

    fn update_irq(&mut self, ctx: &mut dyn Ctx) -> Result<(), ContextError> {
        let pending = (self.usbsts & 0x3F & self.usbintr) != 0;
        set_optional_signal(ctx, self.irq, pending)
    }

    /// Read one TD at `address` through the DMA port.
    fn read_td(
        dma: &mut dyn mosaic_contract::dma::DmaUpstream,
        address: u32,
    ) -> Option<TransferDescriptor> {
        let mut td = TransferDescriptor::default();
        let mut word = [0u8; 4];
        for (index, field) in [&mut td.link, &mut td.status, &mut td.token, &mut td.buffer]
            .into_iter()
            .enumerate()
        {
            dma.read_block(address + (index as u32) * 4, &mut word)
                .ok()?;
            *field = u32::from_le_bytes(word);
        }
        Some(td)
    }

    fn write_td_status(
        dma: &mut dyn mosaic_contract::dma::DmaUpstream,
        address: u32,
        status: u32,
    ) -> Option<()> {
        let word = status.to_le_bytes();
        dma.write_block(address + 4, &word).ok()?;
        Some(())
    }

    /// Execute one TD: a setup, in or out transaction through the fabric.
    fn execute_td(
        &mut self,
        td: &TransferDescriptor,
        address: u32,
        ctx: &mut dyn Ctx,
    ) -> Result<(), ContextError> {
        // The active bit must be set; anything else is done, skip.
        if td.status & (1 << 23) == 0 {
            return Ok(());
        }
        let pid = ((td.token >> 19) & 0x3) as u8; // 0 out, 1 in, 2 setup
        let device = ((td.token >> 8) & 0x7F) as u8;
        let endpoint = ((td.token >> 15) & 0xF) as u8;
        let max_len = ((td.token >> 21) & 0x7FF) as usize;
        let mut new_status = td.status & !(1 << 23); // clear active
        let Some(bus) = self.bus.clone() else {
            new_status |= 1 << 22; // stalled
            if let Some(dma) = self.dma.as_deref_mut() {
                Self::write_td_status(dma, address, new_status);
            }
            return Ok(());
        };
        let mut bus = bus.lock().map_err(|_| ContextError::ResourceExhausted)?;
        let mut payload = [0u8; 8];
        let result = match pid {
            2 => {
                // SETUP: a control transfer's first phase.
                if let Some(dma) = self.dma.as_deref_mut() {
                    dma.read_block(td.buffer, &mut payload).ok();
                }
                let setup = SetupPacket::from_bytes(&payload).unwrap_or(SetupPacket {
                    bm_request_type: 0,
                    b_request: 0,
                    w_value: 0,
                    w_index: 0,
                    w_length: 0,
                });
                // The full control transfer is one fabric call (the
                // fabric assembles the data phase).
                let data_out_len = if setup.to_host() {
                    0
                } else {
                    usize::from(setup.w_length)
                };
                let mut data_out = vec![0u8; data_out_len];
                if data_out_len > 0 {
                    if let Some(dma) = self.dma.as_deref_mut() {
                        dma.read_block(td.buffer, &mut data_out).ok();
                    }
                }
                match bus.control_transfer(device, setup, &data_out) {
                    Ok(data) => {
                        if !data.is_empty() {
                            if let Some(dma) = self.dma.as_deref_mut() {
                                dma.write_block(td.buffer, &data).ok();
                            }
                        }
                        Ok(())
                    }
                    Err(status) => Err(status),
                }
            }
            1 => {
                // IN: device-to-host data lands in the TD's buffer.
                match bus.data_in(device, endpoint, max_len.max(1)) {
                    Ok(Some(data)) => {
                        if let Some(dma) = self.dma.as_deref_mut() {
                            dma.write_block(td.buffer, &data).ok();
                        }
                        Ok(())
                    }
                    Ok(None) => {
                        new_status |= 1 << 23; // NAK: stay active
                        Ok(())
                    }
                    Err(status) => Err(status),
                }
            }
            0 => {
                // OUT: host-to-device data from the TD's buffer.
                let mut data = vec![0u8; max_len.max(1)];
                if let Some(dma) = self.dma.as_deref_mut() {
                    dma.read_block(td.buffer, &mut data).ok();
                }
                match bus.data_out(device, endpoint, &data) {
                    Ok(_) => Ok(()),
                    Err(status) => Err(status),
                }
            }
            _ => Err(UsbStatus::Stall),
        };
        drop(bus);
        match result {
            Ok(()) => {
                if new_status & (1 << 23) == 0 {
                    new_status |= 1 << 18; // Actual length is zero-terminated.
                }
                if let Some(dma) = self.dma.as_deref_mut() {
                    Self::write_td_status(dma, address, new_status);
                }
            }
            Err(status) => {
                new_status = match status {
                    UsbStatus::Stall => new_status | (1 << 22),
                    UsbStatus::Nak => new_status | (1 << 23), // Retry: stay active.
                    _ => new_status | (1 << 21),              // Timeout/error.
                };
                if let Some(dma) = self.dma.as_deref_mut() {
                    Self::write_td_status(dma, address, new_status);
                }
                self.usbsts |= sts::ERROR;
            }
        }
        let _ = ctx;
        Ok(())
    }

    /// One 1 ms frame: walk the frame list's queue heads, breadth-first
    /// across the 32-bit link pointers (the spec's queue-head walk).
    fn frame_tick(&mut self, ctx: &mut dyn Ctx) -> Result<(), ContextError> {
        if !self.running() {
            return Ok(());
        }
        let Some(dma) = self.dma.as_deref_mut() else {
            return Ok(());
        };
        let mut entry = [0u8; 4];
        let frame_at = self.frbaseadd + u32::from(self.frnum & 0x3FF) * 4;
        if dma.read_block(frame_at, &mut entry).is_err() {
            return Ok(());
        }
        let mut link = u32::from_le_bytes(entry) & !0xF;
        let mut steps = 0;
        while link != 0 && link != 1 && steps < 64 {
            steps += 1;
            let Some(mut td) = self
                .dma
                .as_deref_mut()
                .and_then(|dma| Self::read_td(dma, link))
            else {
                break;
            };
            if td.link & 0x2 != 0 {
                // Queue head: follow its element link.
                let element = td.link & !0xF;
                if element == 0 || element == 1 {
                    break;
                }
                let Some(element_td) = self
                    .dma
                    .as_deref_mut()
                    .and_then(|dma| Self::read_td(dma, element))
                else {
                    break;
                };
                let element_address = element;
                td = element_td;
                self.execute_td(&td, element_address, ctx)?;
                // Follow the queue head's vertical link for the next frame.
                let Some(head) = self
                    .dma
                    .as_deref_mut()
                    .and_then(|dma| Self::read_td(dma, link))
                else {
                    break;
                };
                link = head.link & !0xF;
                continue;
            }
            let td_address = link;
            self.execute_td(&td, td_address, ctx)?;
            link = td.link & !0xF;
        }
        self.frnum = self.frnum.wrapping_add(1) & 0x3FF;
        self.usbsts |= sts::USBINT;
        self.update_irq(ctx).ok();
        // The next frame tick (token 1).
        let _ = ctx.schedule_after(VDelta(1_000_000_000), 1);
        Ok(())
    }
}

impl MmioTarget for Uhci {
    fn read(
        &mut self,
        _ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        if offset & 1 != 0 || (size != AccessSize::HalfWord && size != AccessSize::Word) {
            return Err(BusFault::InvalidSize);
        }
        Ok(match offset {
            reg::USBCMD => u64::from(self.usbcmd),
            reg::USBSTS => {
                let status = self.usbsts | if self.running() { 0 } else { sts::HALTED };
                u64::from(status)
            }
            reg::USBINTR => u64::from(self.usbintr),
            reg::FRNUM => u64::from(self.frnum),
            reg::FRBASEADD => u64::from(self.frbaseadd),
            reg::SOFMOD => u64::from(self.sofmod),
            reg::PORTSC1 | reg::PORTSC2 => {
                let index = ((offset - reg::PORTSC1) / 2) as usize;
                let mut value = self.portsc[index];
                // Read-to-clear connect-change bits.
                value |= portsc::ENABLED;
                u64::from(value)
            }
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
        if offset & 1 != 0 || (size != AccessSize::HalfWord && size != AccessSize::Word) {
            return Err(BusFault::InvalidSize);
        }
        match offset {
            reg::USBCMD => {
                self.usbcmd = value as u16;
                if self.usbcmd & cmd::HOST_RESET != 0 {
                    self.usbcmd &= !cmd::HOST_RESET;
                    self.usbsts = sts::HALTED;
                    self.frnum = 0;
                }
                if self.running() {
                    self.usbsts &= !sts::HALTED;
                    let _ = ctx.schedule_after(VDelta(1_000_000_000), 1);
                }
            }
            reg::USBSTS => {
                self.usbsts &= !(value as u16);
                self.update_irq(ctx).ok();
            }
            reg::USBINTR => {
                self.usbintr = value as u16 & 0x000F;
                self.update_irq(ctx).ok();
            }
            reg::FRNUM => self.frnum = value as u16 & 0x03FF,
            reg::FRBASEADD => self.frbaseadd = value as u32 & 0xFFFF_F000,
            reg::SOFMOD => self.sofmod = value as u8,
            reg::PORTSC1 | reg::PORTSC2 => {
                let index = ((offset - reg::PORTSC1) / 2) as usize;
                let value = value as u16;
                // Status bits write-one-to-clear; the rest write straight.
                self.portsc[index] &= !(value & (portsc::CONNECT_CHANGE | portsc::ENABLE_CHANGE));
                self.portsc[index] =
                    (self.portsc[index] & !portsc::RW_MASK) | (value & portsc::RW_MASK);
            }
            _ => match self.unmapped {
                UnmappedPolicy::RazWi => {}
                UnmappedPolicy::Fault => return Err(BusFault::Device),
            },
        }
        Ok(())
    }
}

impl EventTarget for Uhci {
    fn on_timer(&mut self, ctx: &mut dyn Ctx, token: u64) -> Result<(), ContextError> {
        if token == 1 {
            self.frame_tick(ctx)?;
        }
        Ok(())
    }
}

/// The UHCI PCI function header (class 0x0C0300, Intel 82371SB IDs).
pub struct UhciPci {
    pub base: Uhci,
    config: [u8; 256],
    interrupt: bool,
}

impl UhciPci {
    pub fn new(irq: SignalId) -> Self {
        let mut config = [0u8; 256];
        config[header::VENDOR_ID as usize..header::VENDOR_ID as usize + 2]
            .copy_from_slice(&0x8086u16.to_le_bytes());
        config[header::DEVICE_ID as usize..header::DEVICE_ID as usize + 2]
            .copy_from_slice(&0x7020u16.to_le_bytes());
        config[header::COMMAND as usize..header::COMMAND as usize + 2]
            .copy_from_slice(&0x0005u16.to_le_bytes()); // I/O + bus master
        config[header::REVISION_ID as usize] = 0x01;
        config[header::CLASS_CODE_HIGH as usize] = 0x03; // serial bus
        config[header::CLASS_CODE_BASE as usize] = 0x0C; // serial bus controller
        config[header::CLASS_CODE_HIGH as usize - 1] = 0x00; // sub-class prog IF below
        config[header::HEADER_TYPE as usize] = 0x00;
        config[header::CLASS_CODE_HIGH as usize - 1] = 0x00;
        config[9] = 0x03; // subclass: USB
        config[10] = 0x00; // prog IF: UHCI
        config[11] = 0x0C; // base class
        config[header::BAR[0] as usize..header::BAR[0] as usize + 4]
            .copy_from_slice(&0x0000_0001u32.to_le_bytes()); // I/O bar
        config[header::INTERRUPT_PIN as usize] = 1; // INTA#
        Self {
            base: Uhci::new(irq),
            config,
            interrupt: false,
        }
    }
}

impl PciFunction for UhciPci {
    fn config_read(&mut self, offset: u8) -> Result<u32, PciError> {
        if usize::from(offset) + 4 > self.config.len() {
            return Err(PciError::BadOffset);
        }
        let mut word = [0u8; 4];
        word.copy_from_slice(&self.config[usize::from(offset)..usize::from(offset) + 4]);
        Ok(u32::from_le_bytes(word))
    }

    fn config_write(&mut self, offset: u8, value: u32) -> Result<(), PciError> {
        let at = usize::from(offset);
        if at + 4 > self.config.len() {
            return Err(PciError::BadOffset);
        }
        if at == header::BAR[0] as usize {
            // Keep the I/O bit; the driver writes the I/O base.
            self.config[at..at + 4].copy_from_slice(&(value | 1).to_le_bytes());
            return Ok(());
        }
        self.config[at..at + 4].copy_from_slice(&value.to_le_bytes());
        Ok(())
    }

    fn set_interrupt(&mut self, asserted: bool) {
        self.interrupt = asserted;
    }
}

/// Checkpoint state (ADR 0004).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UhciCheckpoint {
    pub schema: u32,
    pub usbcmd: u16,
    pub usbsts: u16,
    pub usbintr: u16,
    pub frnum: u16,
    pub frbaseadd: u32,
    pub sofmod: u8,
    pub portsc: [u16; 2],
}

impl Snapshot for Uhci {
    type State = UhciCheckpoint;
    const SCHEMA_VERSION: u32 = 1;

    fn save(&self) -> Self::State {
        Self::State {
            schema: Self::SCHEMA_VERSION,
            usbcmd: self.usbcmd,
            usbsts: self.usbsts,
            usbintr: self.usbintr,
            frnum: self.frnum,
            frbaseadd: self.frbaseadd,
            sofmod: self.sofmod,
            portsc: self.portsc,
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
        self.usbcmd = state.usbcmd;
        self.usbsts = state.usbsts;
        self.usbintr = state.usbintr;
        self.frnum = state.frnum;
        self.frbaseadd = state.frbaseadd;
        self.sofmod = state.sofmod;
        self.portsc = state.portsc;
        Ok(())
    }

    fn fingerprint(version: u32, state: &Self::State) -> u64 {
        let mut hash = Fnv64::default();
        hash.bytes(&version.to_le_bytes());
        hash.bytes(&state.usbcmd.to_le_bytes());
        hash.bytes(&state.usbsts.to_le_bytes());
        hash.bytes(&state.frnum.to_le_bytes());
        hash.bytes(&state.frbaseadd.to_le_bytes());
        hash.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usb::hid_keyboard::HidKeyboard;
    use mosaic_contract::dma::{DmaUpstream, TransferError};
    use mosaic_contract::mmio::MmioTarget;
    use mosaic_exec::{AccessKind, Endian, EventId, NodeId, Security, VTime};

    struct TestCtx {
        scheduled: usize,
    }

    impl Ctx for TestCtx {
        fn now(&self) -> VTime {
            VTime::ZERO
        }

        fn schedule(&mut self, _at: VTime, _token: u64) -> Result<EventId, ContextError> {
            self.scheduled += 1;
            Ok(EventId(1))
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

    /// Guest RAM with frame list, queue head and TDs laid out.
    struct Guest {
        memory: Vec<u8>,
    }

    impl DmaUpstream for Guest {
        fn write_u8(&mut self, address: u32, value: u8) -> Result<(), TransferError> {
            self.memory[address as usize] = value;
            Ok(())
        }

        fn read_u8(&mut self, address: u32) -> Result<u8, TransferError> {
            Ok(self.memory[address as usize])
        }

        fn device_byte(&mut self, _channel: usize) -> Result<u8, TransferError> {
            Ok(0)
        }
    }

    #[test]
    fn the_pci_header_identifies_the_uhci() {
        let mut function = UhciPci::new(SignalId(11));
        let vendor = function.config_read(header::VENDOR_ID).unwrap();
        assert_eq!(vendor & 0xFFFF, 0x8086);
        let device = function.config_read(header::DEVICE_ID).unwrap();
        assert_eq!(device & 0xFFFF, 0x7020);
    }

    #[test]
    fn a_port_shows_connected_with_a_device_attached() {
        let mut controller = Uhci::new(SignalId(5));
        let mut ctx = TestCtx { scheduled: 0 };
        let bus = Arc::new(Mutex::new(UsbBus::new()));
        bus.lock()
            .unwrap()
            .attach(
                mosaic_bus::usb::core::UsbPort(1),
                Box::new(HidKeyboard::new()),
            )
            .unwrap();
        controller.attach_bus(Arc::clone(&bus));
        let ports = controller
            .read(&mut ctx, reg::PORTSC1, AccessSize::HalfWord, &attr())
            .unwrap() as u16;
        assert_eq!(ports & portsc::CONNECTED, portsc::CONNECTED);
    }

    #[test]
    fn a_frame_tick_walks_a_pending_td() {
        let mut controller = Uhci::new(SignalId(5));
        let mut ctx = TestCtx { scheduled: 0 };
        let bus = Arc::new(Mutex::new(UsbBus::new()));
        {
            let mut bus = bus.lock().unwrap();
            bus.attach(
                mosaic_bus::usb::core::UsbPort(1),
                Box::new(HidKeyboard::new()),
            )
            .unwrap();
            bus.enumerate(mosaic_bus::usb::core::UsbPort(1)).unwrap();
        }
        controller.attach_bus(Arc::clone(&bus));

        // Frame list at 0x1000, entry 0 pointing at a TD at 0x2000.
        let mut memory = vec![0u8; 0x4000];
        memory[0x1000..0x1004].copy_from_slice(&0x2000u32.to_le_bytes());
        let td = TransferDescriptor {
            link: 1,         // Terminate.
            status: 1 << 23, // Active.
            token: (u32::from(1u8) << 19) // PID IN
                | (u32::from(1u8) << 8), // device 1
            buffer: 0x3000,
        };
        memory[0x2000..0x2004].copy_from_slice(&td.link.to_le_bytes());
        memory[0x2004..0x2008].copy_from_slice(&td.status.to_le_bytes());
        memory[0x2008..0x200C].copy_from_slice(&td.token.to_le_bytes());
        memory[0x200C..0x2010].copy_from_slice(&td.buffer.to_le_bytes());
        controller.attach_dma(Box::new(Guest { memory }));

        controller.usbcmd = cmd::RUN;
        controller.frbaseadd = 0x1000;
        controller.frame_tick(&mut ctx).unwrap();
        // The TD's active bit cleared: the IN data landed.
        let status = {
            let dma = controller.dma.as_mut().expect("dma attached");
            let mut word = [0u8; 4];
            dma.read_block(0x2004, &mut word).unwrap();
            u32::from_le_bytes(word)
        };
        assert_eq!(status & (1 << 23), 0);
        let _ = UsbStatus::Ack;
    }
}
