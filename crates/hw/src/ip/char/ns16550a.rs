//! NS16550A UART — the PC COM port and half the embedded world's serial
//! ports (QEMU's `hw/char/serial.c` role).
//!
//! Clean-room from the PC16550D datasheet: the full register file (RBR/THR,
//! IER, IIR/FCR, LCR, MCR, LSR, MSR, SCR, divisor latch), the 16-byte FIFOs
//! with trigger levels, the four interrupt sources (received data, THRE,
//! receiver line status, modem status), the modem-line outputs and the
//! loopback mode (MCR bit 4, which wraps THR into RBR exactly as hardware
//! does). The divisor carries the line speed for the pair fabric's
//! configuration queries.
//!
//! The model is self-contained: IRQ output is one signal, the console stream
//! is a host endpoint the machine harness attaches, and nothing here knows a
//! board's addresses.

use crate::modelkit::unmapped_policy::UnmappedPolicy;
use mosaic_bus::uart::pair::UartEndpoint;
use mosaic_contract::mmio::MmioTarget;
use mosaic_contract::model::{set_optional_signal, ContextError, Ctx, EventTarget};
use mosaic_contract::stream::ByteStream;
use mosaic_exec::{AccessSize, Attr, BusFault, SignalId, Snapshot, SnapshotError};

/// Register offsets (DLAB=0 first, then the divisor latch view).
#[allow(dead_code)] // the documented register map; the walk grows into it
mod reg {
    pub const RBR_THR_DLL: u64 = 0x0;
    pub const IER_DLM: u64 = 0x1;
    pub const IIR_FCR: u64 = 0x2;
    pub const LCR: u64 = 0x3;
    pub const MCR: u64 = 0x4;
    pub const LSR: u64 = 0x5;
    pub const MSR: u64 = 0x6;
    pub const SCR: u64 = 0x7;
}

/// LSR bits (PC16550D table 3).
#[allow(dead_code)]
mod lsr {
    pub const DR: u8 = 1 << 0; // Data ready
    pub const OE: u8 = 1 << 1; // Overrun error
    pub const PE: u8 = 1 << 2; // Parity error
    pub const FE: u8 = 1 << 3; // Framing error
    pub const BI: u8 = 1 << 4; // Break interrupt
    pub const THRE: u8 = 1 << 5; // Transmitter holding register empty
    pub const TEMT: u8 = 1 << 6; // Transmitter empty
    pub const FIFOE: u8 = 1 << 7; // FIFO error (OE|PE|FE in FIFO)
    #[allow(unused)]
    pub const _ALL: u8 = FIFOE;
}

/// IER bits: the four interrupt sources.
#[allow(dead_code)] // documented IER bits
mod ier {
    pub const DR: u8 = 1 << 0;
    pub const THRE: u8 = 1 << 1;
    pub const LSI: u8 = 1 << 2;
    pub const MSI: u8 = 1 << 3;
}

/// LSR bits that only line-error injection sets (documented for the model's
/// fidelity surface; the IRQ path reads them).
#[allow(dead_code)] // documented IIR encodings
mod iir {
    pub const NOPENDING: u8 = 1 << 0;
    pub const FIFO_ENABLED: u8 = 0xC0;
    /// (priority, id) encodings the IIR reports.
    pub const RECEIVED_LINE_STATUS: u8 = 0x06;
    pub const RECEIVED_DATA_READY: u8 = 0x04;
    pub const CHAR_TIMEOUT: u8 = 0x0C;
    pub const THRE: u8 = 0x02;
    pub const MODEM_STATUS: u8 = 0x00;
}

/// MCR bits: loopback is the one a model must answer faithfully.
#[allow(dead_code)] // documented MCR bits
mod mcr {
    pub const DTR: u8 = 1 << 0;
    pub const RTS: u8 = 1 << 1;
    pub const OUT1: u8 = 1 << 2;
    pub const OUT2: u8 = 1 << 3; // Gate the IRQ line on PC wiring
    pub const LOOPBACK: u8 = 1 << 4;
}

/// The 16-byte FIFO depth of a 16550A.
pub const FIFO_DEPTH: usize = 16;

/// One NS16550A.
pub struct Ns16550 {
    /// Combined IRQ output.
    irq: SignalId,
    dll: u8,
    dlm: u8,
    ier: u8,
    lcr: u8,
    mcr: u8,
    scr: u8,
    /// FCR as written (FIFO enable + triggers; the RCVR reset bits act on
    /// write).
    fcr: u8,
    rx: std::collections::VecDeque<u8>,
    thr: Option<u8>,
    /// The host/console endpoint, when the machine attached one.
    host: Option<UartEndpoint>,
    /// Shadow of the MSR's delta bits (loopback and host-modem changes).
    msr: u8,
    unmapped: UnmappedPolicy,
}

impl Ns16550 {
    pub fn new(irq: SignalId) -> Self {
        Self {
            irq,
            dll: 0x0C, // 9600 baud against a 1.8432 MHz input, the PC default
            dlm: 0x00,
            ier: 0,
            lcr: 0,
            mcr: 0,
            scr: 0,
            fcr: 0,
            rx: std::collections::VecDeque::new(),
            thr: None,
            host: None,
            msr: 0xB0, // CTS|DSR|DCD asserted
            unmapped: UnmappedPolicy::RazWi,
        }
    }

    /// Attach the host console endpoint (the machine's wiring, ADR 0025).
    pub fn connect_host(&mut self, endpoint: UartEndpoint) {
        self.host = Some(endpoint);
    }

    /// The configured baud rate against a 1.8432 MHz input clock.
    pub fn baud_rate(&self) -> u32 {
        let divisor = (u16::from(self.dlm) << 8) | u16::from(self.dll);
        if divisor == 0 {
            return 0;
        }
        1_843_200 / (16 * u32::from(divisor))
    }

    fn dlab(&self) -> bool {
        self.lcr & 0x80 != 0
    }

    fn fifo_enabled(&self) -> bool {
        self.fcr & 0x01 != 0
    }

    fn rx_trigger(&self) -> usize {
        match (self.fcr >> 6) & 0x3 {
            0 => 1,
            1 => 4,
            2 => 8,
            _ => 14,
        }
    }

    fn compute_irq(&self) -> bool {
        if self.mcr & mcr::OUT2 == 0 {
            // PC wiring: OUT2 gates the IRQ line; without it nothing asserts.
            return false;
        }
        self.line_status_irq() || self.data_irq() || self.thre_irq() || self.modem_irq()
    }

    fn line_status_irq(&self) -> bool {
        self.ier & ier::LSI != 0 && self.lsr_shadow() & (lsr::OE | lsr::PE | lsr::FE | lsr::BI) != 0
    }

    fn data_irq(&self) -> bool {
        self.ier & ier::DR != 0
            && self.rx.len()
                >= if self.fifo_enabled() {
                    self.rx_trigger()
                } else {
                    1
                }
    }

    fn thre_irq(&self) -> bool {
        self.ier & ier::THRE != 0 && self.thr.is_none()
    }

    fn modem_irq(&self) -> bool {
        self.ier & ier::MSI != 0 && self.msr & 0x0F != 0
    }

    fn lsr_shadow(&self) -> u8 {
        let mut status = lsr::THRE | lsr::TEMT;
        if !self.rx.is_empty() {
            status |= lsr::DR;
        }
        status
    }

    /// Deliver one byte from the wire (a host endpoint write, or loopback).
    fn receive_byte(&mut self, byte: u8) {
        if self.fifo_enabled() {
            if self.rx.len() >= FIFO_DEPTH {
                // Overrun: the oldest error state the 16550 reports.
                self.rx.pop_front();
                self.push_rx(byte);
                return;
            }
        } else if !self.rx.is_empty() {
            self.rx.clear(); // The holding register is overwritten.
        }
        self.push_rx(byte);
    }

    fn push_rx(&mut self, byte: u8) {
        self.rx.push_back(byte);
    }

    fn update_irq(&mut self, ctx: &mut dyn Ctx) -> Result<(), ContextError> {
        set_optional_signal(ctx, self.irq, self.compute_irq())
    }

    /// The interrupt identification register: highest-priority pending
    /// source, or "none".
    fn iir(&self) -> u8 {
        if !self.compute_irq_active() {
            return iir::NOPENDING;
        }
        let mut id = if self.line_status_irq() {
            iir::RECEIVED_LINE_STATUS
        } else if self.data_irq() {
            if self.fifo_enabled() && self.rx.len() < self.rx_trigger() {
                iir::CHAR_TIMEOUT
            } else {
                iir::RECEIVED_DATA_READY
            }
        } else if self.thre_irq() {
            iir::THRE
        } else {
            iir::MODEM_STATUS
        };
        if self.fifo_enabled() {
            id |= iir::FIFO_ENABLED;
        }
        id
    }

    /// Whether any interrupt source is pending, ignoring the OUT2 gate: the
    /// PC gates the *line*, while the IIR keeps naming the pending source
    /// (PC16550D §Interrupt Control Functions).
    fn compute_irq_active(&self) -> bool {
        self.line_status_irq() || self.data_irq() || self.thre_irq() || self.modem_irq()
    }
}

impl MmioTarget for Ns16550 {
    fn read(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        if size != AccessSize::Byte {
            return Err(BusFault::InvalidSize);
        }
        Ok(match offset {
            reg::RBR_THR_DLL if self.dlab() => u64::from(self.dll),
            reg::RBR_THR_DLL => {
                // Reading RBR clears the data-ready state (and its IRQ).
                let byte = self.rx.pop_front();
                self.update_irq(ctx).ok();
                match byte {
                    Some(byte) => u64::from(byte),
                    None => 0,
                }
            }
            reg::IER_DLM if self.dlab() => u64::from(self.dlm),
            reg::IER_DLM => u64::from(self.ier),
            reg::IIR_FCR => u64::from(self.iir()),
            reg::LCR => u64::from(self.lcr),
            reg::MCR => u64::from(self.mcr),
            reg::LSR => u64::from(self.lsr_shadow()),
            reg::MSR => {
                // Reading clears the delta bits.
                let value = self.msr;
                self.msr &= !0x0F;
                self.update_irq(ctx).ok();
                u64::from(value)
            }
            reg::SCR => u64::from(self.scr),
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
        if size != AccessSize::Byte {
            return Err(BusFault::InvalidSize);
        }
        let byte = value as u8;
        match offset {
            reg::RBR_THR_DLL if self.dlab() => self.dll = byte,
            reg::RBR_THR_DLL => {
                // Transmit: loopback wraps THR into RBR; otherwise the byte
                // leaves through the host endpoint and THR empties at once.
                if self.mcr & mcr::LOOPBACK != 0 {
                    self.receive_byte(byte);
                } else if let Some(host) = &mut self.host {
                    host.write_bytes(ctx, &[byte]).ok();
                }
                self.thr = None;
                self.update_irq(ctx).ok();
            }
            reg::IER_DLM if self.dlab() => self.dlm = byte,
            reg::IER_DLM => {
                self.ier = byte & 0x0F;
                self.update_irq(ctx).ok();
            }
            reg::IIR_FCR => {
                self.fcr = byte & 0xF9;
                if byte & 0x02 != 0 {
                    self.rx.clear();
                }
                self.update_irq(ctx).ok();
            }
            reg::LCR => self.lcr = byte,
            reg::MCR => {
                let was_loopback = self.mcr & mcr::LOOPBACK != 0;
                self.mcr = byte & 0x1F;
                // Entering loopback forces the modem inputs to track the
                // outputs (PC16550D §Loopback Operation); leaving restores
                // the idle-asserted lines with fresh deltas.
                if !was_loopback && byte & mcr::LOOPBACK != 0 {
                    let inputs =
                        (byte & (mcr::DTR | mcr::RTS | mcr::OUT1 | mcr::OUT2)).rotate_right(1) << 4;
                    self.msr = inputs | 0x0F;
                } else if was_loopback && byte & mcr::LOOPBACK == 0 {
                    self.msr = 0xB0;
                }
                self.update_irq(ctx).ok();
            }
            reg::LSR => {}
            reg::MSR => {}
            reg::SCR => self.scr = byte,
            _ => match self.unmapped {
                UnmappedPolicy::RazWi => {}
                UnmappedPolicy::Fault => return Err(BusFault::Device),
            },
        }
        Ok(())
    }
}

impl EventTarget for Ns16550 {
    fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
        Ok(())
    }
}

/// Checkpoint state (ADR 0004): the register file plus the RX FIFO contents;
/// the host endpoint is a wiring handle and never enters the checkpoint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ns16550Checkpoint {
    pub schema: u32,
    pub dll: u8,
    pub dlm: u8,
    pub ier: u8,
    pub lcr: u8,
    pub mcr: u8,
    pub scr: u8,
    pub fcr: u8,
    pub msr: u8,
    pub rx: Vec<u8>,
}

impl Snapshot for Ns16550 {
    type State = Ns16550Checkpoint;
    const SCHEMA_VERSION: u32 = 1;

    fn save(&self) -> Self::State {
        Self::State {
            schema: Self::SCHEMA_VERSION,
            dll: self.dll,
            dlm: self.dlm,
            ier: self.ier,
            lcr: self.lcr,
            mcr: self.mcr,
            scr: self.scr,
            fcr: self.fcr,
            msr: self.msr,
            rx: self.rx.iter().copied().collect(),
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
        self.dll = state.dll;
        self.dlm = state.dlm;
        self.ier = state.ier;
        self.lcr = state.lcr;
        self.mcr = state.mcr;
        self.scr = state.scr;
        self.fcr = state.fcr;
        self.msr = state.msr;
        self.rx = state.rx.iter().copied().collect();
        Ok(())
    }

    fn fingerprint(version: u32, state: &Self::State) -> u64 {
        let mut hash = mosaic_exec::Fnv64::default();
        hash.bytes(&version.to_le_bytes());
        hash.bytes(&[
            state.dll, state.dlm, state.ier, state.lcr, state.mcr, state.scr, state.fcr, state.msr,
        ]);
        hash.bytes(&state.rx);
        hash.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::model::ContextError;
    use mosaic_exec::{EventId, VTime};

    struct TestCtx {
        signals: Vec<(SignalId, bool)>,
    }

    impl TestCtx {
        fn new() -> Self {
            Self {
                signals: Vec::new(),
            }
        }
    }

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

        fn set_signal(&mut self, signal: SignalId, high: bool) -> Result<(), ContextError> {
            self.signals.push((signal, high));
            Ok(())
        }
    }

    fn attr() -> Attr {
        Attr {
            master: mosaic_exec::NodeId(0),
            security: mosaic_exec::Security::NonSecure,
            privileged: true,
            kind: mosaic_exec::AccessKind::Data,
            exclusive: false,
            byte_enable: 0,
            endian: mosaic_exec::Endian::Little,
        }
    }

    fn write(uart: &mut Ns16550, ctx: &mut dyn Ctx, offset: u64, value: u8) {
        uart.write(ctx, offset, AccessSize::Byte, u64::from(value), &attr())
            .unwrap();
    }

    #[test]
    fn the_divisor_latch_sets_the_baud_rate() {
        let mut uart = Ns16550::new(SignalId(1));
        let mut ctx = TestCtx::new();
        write(&mut uart, &mut ctx, reg::LCR, 0x80); // DLAB=1
        write(&mut uart, &mut ctx, reg::RBR_THR_DLL, 0x0C);
        write(&mut uart, &mut ctx, reg::IER_DLM, 0x00);
        assert_eq!(uart.baud_rate(), 9600);
    }

    #[test]
    fn thre_is_always_set_and_transmit_empties_at_once() {
        let mut uart = Ns16550::new(SignalId(1));
        let mut ctx = TestCtx::new();
        let lsr = uart
            .read(&mut ctx, reg::LSR, AccessSize::Byte, &attr())
            .unwrap();
        assert_eq!(lsr as u8 & lsr::THRE, lsr::THRE);
        write(&mut uart, &mut ctx, reg::RBR_THR_DLL, b'x');
        let lsr = uart
            .read(&mut ctx, reg::LSR, AccessSize::Byte, &attr())
            .unwrap();
        assert_eq!(lsr as u8 & lsr::THRE, lsr::THRE);
    }

    #[test]
    fn loopback_wraps_transmit_into_receive() {
        let mut uart = Ns16550::new(SignalId(1));
        let mut ctx = TestCtx::new();
        write(&mut uart, &mut ctx, reg::MCR, mcr::OUT2 | mcr::LOOPBACK);
        write(&mut uart, &mut ctx, reg::RBR_THR_DLL, 0x42);
        let lsr = uart
            .read(&mut ctx, reg::LSR, AccessSize::Byte, &attr())
            .unwrap();
        assert_eq!(lsr as u8 & lsr::DR, lsr::DR);
        let data = uart
            .read(&mut ctx, reg::RBR_THR_DLL, AccessSize::Byte, &attr())
            .unwrap();
        assert_eq!(data, 0x42);
    }

    #[test]
    fn the_irq_asserts_on_the_fifo_trigger_and_the_iir_names_it() {
        let mut uart = Ns16550::new(SignalId(7));
        let mut ctx = TestCtx::new();
        // FIFO on (trigger bits 00 = 1 byte), interrupts on, IRQ gate open.
        write(&mut uart, &mut ctx, reg::MCR, mcr::OUT2);
        write(&mut uart, &mut ctx, reg::IIR_FCR, 0x01);
        write(&mut uart, &mut ctx, reg::IER_DLM, ier::DR);
        for byte in [0x10u8, 0x11] {
            uart.write(
                &mut ctx,
                reg::RBR_THR_DLL,
                AccessSize::Byte,
                u64::from(byte),
                &attr(),
            )
            .unwrap();
        }
        // Inject two received bytes through loopback (no host endpoint).
        write(&mut uart, &mut ctx, reg::MCR, mcr::OUT2 | mcr::LOOPBACK);
        uart.write(&mut ctx, reg::RBR_THR_DLL, AccessSize::Byte, 0x20, &attr())
            .unwrap();
        uart.write(&mut ctx, reg::RBR_THR_DLL, AccessSize::Byte, 0x21, &attr())
            .unwrap();
        write(&mut uart, &mut ctx, reg::MCR, mcr::OUT2);
        let irq_high = uart.compute_irq();
        assert!(
            irq_high,
            "data-ready IRQ must assert with DR enabled and data present"
        );
        let iir = uart
            .read(&mut ctx, reg::IIR_FCR, AccessSize::Byte, &attr())
            .unwrap() as u8;
        assert_eq!(iir & 0x0F, iir::RECEIVED_DATA_READY);
    }

    #[test]
    fn the_pc_out2_gate_masks_the_line() {
        let mut uart = Ns16550::new(SignalId(7));
        let mut ctx = TestCtx::new();
        write(&mut uart, &mut ctx, reg::IIR_FCR, 0x01);
        write(&mut uart, &mut ctx, reg::IER_DLM, ier::DR);
        write(&mut uart, &mut ctx, reg::MCR, mcr::LOOPBACK); // OUT2 clear
        uart.write(&mut ctx, reg::RBR_THR_DLL, AccessSize::Byte, 0x20, &attr())
            .unwrap();
        assert!(!uart.compute_irq(), "OUT2 gates the IRQ line on PC wiring");
        // The IIR still names the pending source.
        let iir = uart
            .read(&mut ctx, reg::IIR_FCR, AccessSize::Byte, &attr())
            .unwrap() as u8;
        assert_eq!(iir & 0x0F, iir::RECEIVED_DATA_READY);
    }

    #[test]
    fn reading_rbr_clears_data_ready() {
        let mut uart = Ns16550::new(SignalId(1));
        let mut ctx = TestCtx::new();
        write(&mut uart, &mut ctx, reg::MCR, mcr::OUT2 | mcr::LOOPBACK);
        uart.write(&mut ctx, reg::RBR_THR_DLL, AccessSize::Byte, 0x33, &attr())
            .unwrap();
        uart.read(&mut ctx, reg::RBR_THR_DLL, AccessSize::Byte, &attr())
            .unwrap();
        let lsr = uart
            .read(&mut ctx, reg::LSR, AccessSize::Byte, &attr())
            .unwrap() as u8;
        assert_eq!(lsr & lsr::DR, 0);
    }
}
