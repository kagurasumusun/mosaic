//! ARM PrimeCell UART (PL011) functional register model.
//!
//! Clean-room behavior from ARM DDI 0183: the model covers the data register,
//! FIFO status, baud/line/control storage, the raw/masked interrupt registers
//! and the PrimeCell identification window. The byte stream port and the
//! interrupt pin are contract trait objects injected at construction, so the
//! model never references another model or an interconnect entity.
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::boxed::Box;

use mosaic_contract_mmio::MmioTarget;
use mosaic_contract_model::{ContextError, Ctx, EventTarget};
use mosaic_contract_stream::ByteStream;
use mosaic_core_types::{AccessKind, AccessSize, Attr, BusFault, SignalId};
use mosaic_ip_arm_primecell_id::PrimeCellId;
use mosaic_modelkit_fifo::ByteFifo;

/// PL011 register offsets (word-aligned, DDI 0183 Table 3-1).
mod reg {
    pub const DR: u64 = 0x000;
    pub const RSR_ECR: u64 = 0x004;
    pub const FR: u64 = 0x018;
    pub const ILPR: u64 = 0x020;
    pub const IBRD: u64 = 0x024;
    pub const FBRD: u64 = 0x028;
    pub const LCR_H: u64 = 0x02C;
    pub const CR: u64 = 0x030;
    pub const IMSC: u64 = 0x038;
    pub const RIS: u64 = 0x03C;
    pub const MIS: u64 = 0x040;
    pub const ICR: u64 = 0x044;
    pub const DMACR: u64 = 0x048;
}

/// UARTCR bit positions.
mod cr {
    pub const UARTEN: u32 = 1 << 0;
    pub const TXE: u32 = 1 << 8;
    pub const RXE: u32 = 1 << 9;
}

/// UARTFR bit positions.
mod fr {
    pub const BUSY: u32 = 1 << 3;
    pub const RXFE: u32 = 1 << 4;
    pub const TXFF: u32 = 1 << 5;
}

/// Raw interrupt status bit positions (receive timeout is reserved until an
/// interrupt controller exists to distinguish it from the combined line).
mod ris {
    pub const RX: u32 = 1 << 4;
    pub const TX: u32 = 1 << 5;
    pub const ERROR: u32 = 1 << 7;
}

/// Policy for offsets without a modeled register.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum UnmappedPolicy {
    /// Read As Zero / Write Ignored, the default for reserved space.
    RazWi,
    /// Report a guest-visible device fault.
    Fault,
}

/// Construction parameters. The combined interrupt pin is the only IRQ the
/// P1 board wires; per-source pins can be added alongside an interrupt
/// controller without changing the register behavior.
#[derive(Clone, Copy, Debug)]
pub struct Pl011Params {
    pub combined_irq: SignalId,
    pub unmapped: UnmappedPolicy,
}

impl Pl011Params {
    pub const fn new(combined_irq: SignalId) -> Self {
        Self {
            combined_irq,
            unmapped: UnmappedPolicy::RazWi,
        }
    }
}

/// Snapshot-relevant device state, gathered in one type per design §10.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Pl011State {
    pub schema: u32,
    pub rsr_ecr: u32,
    pub ilpr: u32,
    pub ibrd: u32,
    pub fbrd: u32,
    pub lcr_h: u32,
    pub cr: u32,
    pub imsc: u32,
    pub raw_interrupts: u32,
    pub dmacr: u32,
}

impl Default for Pl011State {
    fn default() -> Self {
        Self {
            schema: 1,
            rsr_ecr: 0,
            ilpr: 0,
            ibrd: 0,
            fbrd: 0,
            lcr_h: 0,
            // UARTEN starts clear, matching the reset state of the hardware.
            cr: 0,
            imsc: 0,
            raw_interrupts: 0,
            dmacr: 0,
        }
    }
}

/// Functional PL011 with a fixed 16-entry FIFO, matching the default FIFO
/// depth declared for the P1 board.
pub struct Pl011 {
    state: Pl011State,
    identity: PrimeCellId,
    params: Pl011Params,
    tx: ByteFifo<16>,
    rx: ByteFifo<16>,
    stream: Option<Box<dyn ByteStream>>,
    irq_asserted: bool,
}

impl Pl011 {
    /// PL011 part number 0x011, first revision, no patch.
    pub const PART_NUMBER: u16 = 0x011;

    pub fn new(params: Pl011Params, stream: Option<Box<dyn ByteStream>>) -> Self {
        Self {
            state: Pl011State::default(),
            identity: PrimeCellId::arm(Self::PART_NUMBER, 0, 0)
                .expect("fixed PL011 identity fields are valid"),
            params,
            tx: ByteFifo::new(),
            rx: ByteFifo::new(),
            stream,
            irq_asserted: false,
        }
    }

    const fn enabled(&self, gate: u32) -> bool {
        self.state.cr & cr::UARTEN != 0 && self.state.cr & gate != 0
    }

    /// Push buffered TX bytes into the stream peer while it accepts data.
    fn drain_tx(&mut self, ctx: &mut dyn Ctx) {
        let Some(port) = self.stream.as_mut() else {
            return;
        };
        while let Some(byte) = self.tx.peek() {
            match port.write_bytes(ctx, &[byte]) {
                Ok(accepted) if accepted > 0 => {
                    self.tx.pop();
                }
                // No progress (zero-length write) or any peer error including
                // Full/Closed/NotConnected applies backpressure: the byte stays
                // in the FIFO so re-attachment cannot lose guest output.
                // StreamError is non_exhaustive, so a wildcard is required.
                _ => break,
            }
        }
        if self.tx.is_empty() {
            self.state.raw_interrupts |= ris::TX;
        }
    }

    /// Pull any bytes the peer offers into the RX FIFO, bounded by capacity.
    fn refill_rx(&mut self, ctx: &mut dyn Ctx) {
        let Some(port) = self.stream.as_mut() else {
            return;
        };
        while !self.rx.is_full() {
            let mut byte = [0u8; 1];
            match port.read_bytes(ctx, &mut byte) {
                Ok(read) if read > 0 => {
                    if self.rx.push(byte[0]).is_err() {
                        self.state.raw_interrupts |= ris::ERROR;
                        break;
                    }
                    self.state.raw_interrupts |= ris::RX;
                }
                _ => break,
            }
        }
    }

    fn masked_interrupts(&self) -> u32 {
        self.state.raw_interrupts & self.state.imsc
    }

    /// Re-evaluate the combined IRQ line. An unwired pin is an optional port:
    /// the context reports it unknown and the device keeps running.
    fn update_irq(&mut self, ctx: &mut dyn Ctx) {
        let asserted = self.masked_interrupts() != 0;
        if asserted != self.irq_asserted {
            self.irq_asserted = asserted;
            let _ = ctx.set_signal(self.params.combined_irq, asserted);
        }
    }

    fn read_word(&mut self, ctx: &mut dyn Ctx, offset: u64, debug: bool) -> Result<u32, BusFault> {
        if offset % 4 != 0 {
            return self.unmapped(offset);
        }
        let value = match offset {
            reg::DR => {
                if debug {
                    // Debug reads never consume data or refill from the peer.
                    return Ok(self.rx.peek().map_or(0, u32::from));
                }
                // Hardware ignores the receive FIFO while RXE is clear.
                if self.enabled(cr::RXE) {
                    self.refill_rx(ctx);
                }
                let byte = self.rx.pop().unwrap_or(0);
                if self.rx.is_empty() {
                    self.state.raw_interrupts &= !ris::RX;
                }
                u32::from(byte)
            }
            reg::RSR_ECR => self.state.rsr_ecr,
            reg::FR => {
                let mut flags = 0u32;
                if self.tx.is_full() {
                    flags |= fr::TXFF;
                }
                if self.rx.is_empty() {
                    flags |= fr::RXFE;
                }
                if !self.tx.is_empty() {
                    flags |= fr::BUSY;
                }
                flags
            }
            reg::ILPR => self.state.ilpr,
            reg::IBRD => self.state.ibrd,
            reg::FBRD => self.state.fbrd,
            reg::LCR_H => self.state.lcr_h,
            reg::CR => self.state.cr,
            reg::IMSC => self.state.imsc,
            reg::RIS => self.state.raw_interrupts,
            reg::MIS => self.masked_interrupts(),
            reg::DMACR => self.state.dmacr,
            _ => {
                if let Some(id) = self.identity.read_word(offset) {
                    id
                } else {
                    return self.unmapped(offset);
                }
            }
        };
        Ok(value)
    }

    fn write_word(&mut self, ctx: &mut dyn Ctx, offset: u64, value: u32) -> Result<(), BusFault> {
        if offset % 4 != 0 {
            return self.unmapped(offset).map(|_| ());
        }
        match offset {
            reg::DR => {
                if !self.enabled(cr::TXE) {
                    return Ok(());
                }
                if self.tx.is_full() {
                    self.state.raw_interrupts |= ris::ERROR;
                    self.state.rsr_ecr |= 1 << 3;
                } else {
                    self.tx.push(value as u8).map_err(|_| BusFault::Device)?;
                    self.state.raw_interrupts &= !ris::TX;
                }
                self.drain_tx(ctx);
            }
            reg::RSR_ECR => {
                // Any write clears the receive status/error clear register.
                self.state.rsr_ecr = 0;
                self.state.raw_interrupts &= !ris::ERROR;
            }
            reg::ILPR => self.state.ilpr = value,
            reg::IBRD => self.state.ibrd = value & 0xFFFF,
            reg::FBRD => self.state.fbrd = value & 0x3F,
            reg::LCR_H => self.state.lcr_h = value,
            reg::CR => self.state.cr = value,
            reg::IMSC => self.state.imsc = value,
            reg::ICR => {
                // Write-1-to-clear raw interrupt status.
                self.state.raw_interrupts &= !value;
            }
            reg::DMACR => self.state.dmacr = value & 0x7,
            reg::FR | reg::RIS | reg::MIS => {}
            _ => {
                if PrimeCellId::covers(offset) {
                    // Identification registers are read-only and ignore writes.
                } else {
                    return self.unmapped(offset).map(|_| ());
                }
            }
        }
        self.update_irq(ctx);
        Ok(())
    }

    fn unmapped(&self, _offset: u64) -> Result<u32, BusFault> {
        match self.params.unmapped {
            UnmappedPolicy::RazWi => Ok(0),
            UnmappedPolicy::Fault => Err(BusFault::Device),
        }
    }
}

impl MmioTarget for Pl011 {
    fn read(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        attr: &Attr,
    ) -> Result<u64, BusFault> {
        let width = u32::from(size.bytes()) * 8;
        let mask = if width >= 32 {
            u32::MAX
        } else {
            (1u32 << width) - 1
        };
        let debug = attr.kind == AccessKind::Debug;
        let word = self.read_word(ctx, offset, debug)?;
        Ok(u64::from(word & mask))
    }

    fn write(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        value: u64,
        _attr: &Attr,
    ) -> Result<(), BusFault> {
        if matches!(size, AccessSize::DoubleWord) {
            // PL011 registers are 32-bit; an 8-byte transfer cannot fit.
            return Err(BusFault::InvalidSize);
        }
        self.write_word(ctx, offset, value as u32)
    }
}

impl EventTarget for Pl011 {
    fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use mosaic_contract_stream::StreamError;
    use std::prelude::v1::*;

    use super::*;
    use mosaic_core_types::{EventId, VTime};
    use std::sync::{Arc, Mutex};
    use std::vec;

    const IRQ: SignalId = SignalId(1);

    #[derive(Default)]
    struct VecStream {
        tx: Vec<u8>,
        rx: std::collections::VecDeque<u8>,
    }

    impl ByteStream for VecStream {
        fn write_bytes(&mut self, _ctx: &mut dyn Ctx, bytes: &[u8]) -> Result<usize, StreamError> {
            self.tx.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn read_bytes(&mut self, _ctx: &mut dyn Ctx, buf: &mut [u8]) -> Result<usize, StreamError> {
            let mut count = 0;
            for slot in buf.iter_mut() {
                let Some(byte) = self.rx.pop_front() else {
                    break;
                };
                *slot = byte;
                count += 1;
            }
            Ok(count)
        }
    }

    struct StubCtx;
    impl Ctx for StubCtx {
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

    fn word_attr() -> Attr {
        Attr {
            master: mosaic_core_types::NodeId(0),
            security: mosaic_core_types::Security::NonSecure,
            privileged: false,
            kind: AccessKind::Data,
            exclusive: false,
            byte_enable: u8::MAX,
            endian: mosaic_core_types::Endian::Little,
        }
    }

    #[test]
    fn enabled_transmit_drains_bytes_to_the_stream_peer() {
        let stream = Arc::new(Mutex::new(VecStream::default()));
        let peer: Box<dyn ByteStream> = Box::new(TestStream {
            inner: Arc::clone(&stream),
        });
        let mut uart = Pl011::new(Pl011Params::new(IRQ), Some(peer));
        let mut ctx = StubCtx;
        uart.state.cr = cr::UARTEN | cr::TXE;
        for byte in *b"mosaic" {
            uart.write(
                &mut ctx,
                reg::DR,
                AccessSize::Byte,
                u64::from(byte),
                &word_attr(),
            )
            .unwrap();
        }
        assert_eq!(stream.lock().unwrap().tx, b"mosaic".to_vec());
        assert!(uart.tx.is_empty());
        // TX raw interrupt is asserted once the FIFO drains.
        assert_eq!(
            uart.read(&mut ctx, reg::RIS, AccessSize::Word, &word_attr())
                .unwrap(),
            u64::from(ris::TX)
        );
    }

    #[test]
    fn disabled_transmit_is_ignored_and_status_reports_the_fifo() {
        let mut uart = Pl011::new(Pl011Params::new(IRQ), None);
        let mut ctx = StubCtx;
        uart.write(
            &mut ctx,
            reg::DR,
            AccessSize::Word,
            b'x'.into(),
            &word_attr(),
        )
        .unwrap();
        let flags = uart
            .read(&mut ctx, reg::FR, AccessSize::Word, &word_attr())
            .unwrap();
        assert_eq!(flags, u64::from(fr::RXFE));
    }

    #[test]
    fn receive_path_refills_and_debug_reads_do_not_consume() {
        let stream = Arc::new(Mutex::new(VecStream::default()));
        stream.lock().unwrap().rx.extend(b"ab");
        let peer: Box<dyn ByteStream> = Box::new(TestStream {
            inner: Arc::clone(&stream),
        });
        let mut uart = Pl011::new(Pl011Params::new(IRQ), Some(peer));
        let mut ctx = StubCtx;
        uart.state.cr = cr::UARTEN | cr::RXE;
        let first = uart
            .read(&mut ctx, reg::DR, AccessSize::Word, &word_attr())
            .unwrap();
        assert_eq!(first, u64::from(b'a'));
        let mut debug = word_attr();
        debug.kind = AccessKind::Debug;
        let peek = uart
            .read(&mut ctx, reg::DR, AccessSize::Word, &debug)
            .unwrap();
        assert_eq!(peek, u64::from(b'b'));
        let consumed = uart
            .read(&mut ctx, reg::DR, AccessSize::Word, &word_attr())
            .unwrap();
        assert_eq!(consumed, u64::from(b'b'));
    }

    #[test]
    fn interrupt_masking_and_clear_drive_one_combined_line() {
        let levels = Arc::new(Mutex::new(Vec::new()));
        let stream = Arc::new(Mutex::new(VecStream::default()));
        let peer: Box<dyn ByteStream> = Box::new(TestStream {
            inner: Arc::clone(&stream),
        });
        let mut uart = Pl011::new(Pl011Params::new(IRQ), Some(peer));
        let mut ctx = RecordingCtx {
            levels: Arc::clone(&levels),
        };
        uart.state.cr = cr::UARTEN | cr::TXE;
        // Mask TX before the first write: the drained FIFO asserts the line.
        uart.write_word(&mut ctx, reg::IMSC, ris::TX | ris::ERROR)
            .unwrap();
        uart.write_word(&mut ctx, reg::DR, u32::from(b'q')).unwrap();
        assert_eq!(*levels.lock().unwrap(), vec![true]);
        // Clearing the raw status deasserts the single combined IRQ.
        uart.write_word(&mut ctx, reg::ICR, ris::TX).unwrap();
        assert_eq!(*levels.lock().unwrap(), vec![true, false]);
    }

    #[test]
    fn primecell_window_and_unmapped_policy_are_guest_safe() {
        let mut uart = Pl011::new(Pl011Params::new(IRQ), None);
        let mut ctx = StubCtx;
        assert_eq!(
            uart.read(&mut ctx, 0xFE0, AccessSize::Word, &word_attr())
                .unwrap(),
            0x11
        );
        assert_eq!(
            uart.read(&mut ctx, 0x900, AccessSize::Word, &word_attr())
                .unwrap(),
            0
        );
        let mut strict = Pl011Params::new(IRQ);
        strict.unmapped = UnmappedPolicy::Fault;
        let mut faulting = Pl011::new(strict, None);
        assert_eq!(
            faulting.read(&mut ctx, 0x900, AccessSize::Word, &word_attr()),
            Err(BusFault::Device)
        );
        assert_eq!(
            uart.write(&mut ctx, reg::DR, AccessSize::DoubleWord, 0, &word_attr()),
            Err(BusFault::InvalidSize)
        );
    }

    /// Test peer that shares the recording stream behind the contract type.
    struct TestStream {
        inner: Arc<Mutex<VecStream>>,
    }
    impl ByteStream for TestStream {
        fn write_bytes(&mut self, ctx: &mut dyn Ctx, bytes: &[u8]) -> Result<usize, StreamError> {
            self.inner.lock().unwrap().write_bytes(ctx, bytes)
        }
        fn read_bytes(&mut self, ctx: &mut dyn Ctx, buf: &mut [u8]) -> Result<usize, StreamError> {
            self.inner.lock().unwrap().read_bytes(ctx, buf)
        }
    }

    struct RecordingCtx {
        levels: Arc<Mutex<Vec<bool>>>,
    }
    impl Ctx for RecordingCtx {
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
            assert_eq!(signal, IRQ);
            self.levels.lock().unwrap().push(high);
            Ok(())
        }
    }
}
