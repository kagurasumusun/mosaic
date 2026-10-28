//! I2C bus entity: address resolution, transaction dispatch and pin-level
//! bit-bang decoding (構成設計書 §11).
//!
//! One [`I2cBus`] owns the target list and serves both access paths:
//! register models drive the transaction-level [`I2cInitiator`] port, while
//! GPIO bit-bang masters feed SDA/SCL edges into [`I2cBitBang`], which
//! reconstructs the same frames and dispatches through the identical target
//! trait. A target therefore cannot tell which path reached it.

use std::sync::{Arc, Mutex, MutexGuard};

use mosaic_contract::gpio::{GpioError, GpioPort, GpioSink, PinConfig, PinLevel};
use mosaic_contract::i2c::{Ack, I2cAddr, I2cError, I2cInitiator, I2cPhase, I2cSpeed, I2cTarget};
use mosaic_contract::model::Ctx;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Addressed I2C bus. Targets are matched purely by address; mux devices
/// implement [`I2cTarget`] themselves and extend the reachable addresses.
pub struct I2cBus {
    targets: Vec<Box<dyn I2cTarget>>,
    speed: I2cSpeed,
    phases: Vec<I2cPhase>,
}

impl Default for I2cBus {
    fn default() -> Self {
        Self::new()
    }
}

impl I2cBus {
    pub fn new() -> Self {
        Self {
            targets: Vec::new(),
            speed: I2cSpeed::Standard,
            phases: Vec::new(),
        }
    }

    pub fn add_target(&mut self, target: Box<dyn I2cTarget>) {
        self.targets.push(target);
    }

    /// Share the bus behind the pin-level decoder.
    pub fn shared(self) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(self))
    }

    pub fn set_speed(&mut self, speed: I2cSpeed) {
        self.speed = speed;
    }

    pub fn speed(&self) -> I2cSpeed {
        self.speed
    }

    /// Transaction trace for conformance tests.
    pub fn phases(&self) -> &[I2cPhase] {
        &self.phases
    }

    /// Whether any attached target would answer this address. Mux parts
    /// probe their downstream segments with this to extend the reachable
    /// address set (their own `matches` unions the enabled segments).
    pub fn resolves(&self, addr: &I2cAddr) -> bool {
        self.target_index(addr).is_some()
    }

    fn target_index(&self, addr: &I2cAddr) -> Option<usize> {
        self.targets.iter().position(|target| target.matches(addr))
    }

    fn dispatch_write(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: I2cAddr,
        bytes: &[u8],
    ) -> Result<(), I2cError> {
        let Some(index) = self.target_index(&addr) else {
            self.phases.push(I2cPhase::Acknowledge(Ack::Nack));
            return Err(I2cError::Nack);
        };
        let ack = self.targets[index].target_write(ctx, &addr, bytes)?;
        self.phases.push(I2cPhase::Acknowledge(ack));
        if ack == Ack::Nack {
            return Err(I2cError::Nack);
        }
        Ok(())
    }

    fn dispatch_read(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: I2cAddr,
        buf: &mut [u8],
    ) -> Result<(), I2cError> {
        let Some(index) = self.target_index(&addr) else {
            self.phases.push(I2cPhase::Acknowledge(Ack::Nack));
            return Err(I2cError::Nack);
        };
        let ack = self.targets[index].target_read(ctx, &addr, buf)?;
        self.phases.push(I2cPhase::Acknowledge(ack));
        if ack == Ack::Nack {
            return Err(I2cError::Nack);
        }
        Ok(())
    }
}

impl I2cInitiator for I2cBus {
    fn set_speed(&mut self, speed: I2cSpeed) {
        self.set_speed(speed);
    }

    fn write(&mut self, ctx: &mut dyn Ctx, addr: I2cAddr, bytes: &[u8]) -> Result<(), I2cError> {
        self.phases.push(I2cPhase::Start);
        self.phases.push(I2cPhase::Address { addr, read: false });
        for byte in bytes {
            self.phases.push(I2cPhase::Data(*byte));
        }
        let result = self.dispatch_write(ctx, addr, bytes);
        self.phases.push(I2cPhase::Stop);
        result
    }

    fn read(&mut self, ctx: &mut dyn Ctx, addr: I2cAddr, buf: &mut [u8]) -> Result<(), I2cError> {
        self.phases.push(I2cPhase::Start);
        self.phases.push(I2cPhase::Address { addr, read: true });
        let result = self.dispatch_read(ctx, addr, buf);
        for byte in buf.iter().take_while(|_| result.is_ok()) {
            self.phases.push(I2cPhase::Data(*byte));
        }
        self.phases.push(I2cPhase::Stop);
        result
    }

    fn write_read(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: I2cAddr,
        out: &[u8],
        input: &mut [u8],
    ) -> Result<(), I2cError> {
        self.phases.push(I2cPhase::Start);
        self.phases.push(I2cPhase::Address { addr, read: false });
        for byte in out {
            self.phases.push(I2cPhase::Data(*byte));
        }
        self.dispatch_write(ctx, addr, out)?;
        self.phases.push(I2cPhase::RepeatedStart);
        self.phases.push(I2cPhase::Address { addr, read: true });
        let result = self.dispatch_read(ctx, addr, input);
        for byte in input.iter().take_while(|_| result.is_ok()) {
            self.phases.push(I2cPhase::Data(*byte));
        }
        self.phases.push(I2cPhase::Stop);
        result
    }
}

/// Which physical line a [`BitBangLine`] feeds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Line {
    Sda,
    Scl,
}

/// GPIO sink adapter that forwards pin edges to the bit-bang decoder.
pub struct BitBangLine {
    decoder: Arc<Mutex<I2cBitBang>>,
    line: Line,
}

impl GpioSink for BitBangLine {
    fn level_changed(&mut self, ctx: &mut dyn Ctx, level: PinLevel) {
        let mut decoder = lock(&self.decoder);
        match self.line {
            Line::Sda => decoder.on_sda(ctx, level),
            Line::Scl => decoder.on_scl(ctx, level),
        }
    }
}

/// Master-side [`GpioPort`] adapter for one bit-bang line.
///
/// Drives and configuration are delegated to the inner port (normally a
/// net-backed open-drain port built by the composition root). `current_level`
/// additionally models the target end of the wire: while the decoder is in a
/// slot the addressed target drives (write-frame ACK or read-frame data bits),
/// the SDA port reports the decoder's simulated target level instead of the
/// master's own net level, exactly like a real open-drain bus. This lets a
/// register-level GPIO model bit-bang I2C through the contract vocabulary
/// alone, without ever seeing the decoder type (ADR 0008 port injection).
pub struct BitBangPort {
    decoder: Arc<Mutex<I2cBitBang>>,
    line: Line,
    inner: Box<dyn GpioPort>,
}

impl GpioPort for BitBangPort {
    fn drive(&mut self, ctx: &mut dyn Ctx, level: PinLevel) -> Result<(), GpioError> {
        // The net notification reaches the BitBangLine sinks, so the decoder
        // observes this edge without being called here a second time.
        self.inner.drive(ctx, level)
    }

    fn release(&mut self, ctx: &mut dyn Ctx) -> Result<(), GpioError> {
        self.inner.release(ctx)
    }

    fn configure(&mut self, config: PinConfig) -> Result<(), GpioError> {
        self.inner.configure(config)
    }

    fn current_level(&self) -> PinLevel {
        if self.line == Line::Sda {
            let decoder = lock(&self.decoder);
            if decoder.target_driven_slot() {
                return decoder.sda_readback();
            }
        }
        self.inner.current_level()
    }
}

/// Pin-level I2C decoder. It reconstructs frames from SDA/SCL edges and runs
/// them through the same [`I2cBus`] as transaction-level initiators.
pub struct I2cBitBang {
    bus: Arc<Mutex<I2cBus>>,
    prev_sda: PinLevel,
    prev_scl: PinLevel,
    in_frame: bool,
    addr: Option<I2cAddr>,
    read: bool,
    target_present: bool,
    /// Set while the address byte's ACK slot is on the wire: the slot is
    /// target-driven and completes on the ninth SCL pulse.
    addr_ack_pending: bool,
    bit: u8,
    shift: u8,
    write_bytes: Vec<u8>,
    read_byte: u8,
    read_bits: u8,
    phases: Vec<I2cPhase>,
    /// Outcome of the last completed frame, surfaced to the bit-bang master.
    pub last_result: Result<(), I2cError>,
}

impl I2cBitBang {
    pub fn new(bus: Arc<Mutex<I2cBus>>) -> Self {
        Self {
            bus,
            prev_sda: PinLevel::High,
            prev_scl: PinLevel::High,
            in_frame: false,
            addr: None,
            read: false,
            target_present: false,
            addr_ack_pending: false,
            bit: 0,
            shift: 0,
            write_bytes: Vec::new(),
            read_byte: 0,
            read_bits: 0,
            phases: Vec::new(),
            last_result: Ok(()),
        }
    }

    /// Build the SDA and SCL sink pair wired to one decoder.
    pub fn lines(decoder: Arc<Mutex<Self>>) -> (BitBangLine, BitBangLine) {
        let sda = BitBangLine {
            decoder: Arc::clone(&decoder),
            line: Line::Sda,
        };
        let scl = BitBangLine {
            decoder,
            line: Line::Scl,
        };
        (sda, scl)
    }

    /// Master-side SDA port over an inner (normally net-backed) port. While the
    /// addressed target drives SDA, `current_level` reports the decoder's
    /// simulated target level; otherwise the inner net level shows through.
    pub fn sda_port(decoder: Arc<Mutex<Self>>, inner: Box<dyn GpioPort>) -> BitBangPort {
        BitBangPort {
            decoder,
            line: Line::Sda,
            inner,
        }
    }

    /// Master-side SCL port; SCL is master-driven only, so reads pass through.
    pub fn scl_port(decoder: Arc<Mutex<Self>>, inner: Box<dyn GpioPort>) -> BitBangPort {
        BitBangPort {
            decoder,
            line: Line::Scl,
            inner,
        }
    }

    pub fn phases(&self) -> &[I2cPhase] {
        &self.phases
    }

    /// True while the addressed target drives SDA: any ACK slot of the
    /// address byte, the ACK slot of a write-frame data byte, or any data bit
    /// of a read frame. During a read frame's data-byte ACK slots the master
    /// drives, so this is false there.
    pub fn target_driven_slot(&self) -> bool {
        if !self.in_frame || self.addr.is_none() {
            return false;
        }
        if self.bit == 8 {
            return self.addr_ack_pending || !self.read;
        }
        self.read
    }

    /// Simulated SDA level for masters that sample read data and ACK slots.
    pub fn sda_readback(&self) -> PinLevel {
        if !self.in_frame || self.addr.is_none() {
            return PinLevel::High;
        }
        if self.bit == 8 {
            // ACK slot: the addressed target holds SDA low, otherwise it stays high.
            return if self.target_present {
                PinLevel::Low
            } else {
                PinLevel::High
            };
        }
        if self.read {
            let mask = 1u8 << (7 - self.read_bits.min(7));
            return if self.read_byte & mask != 0 {
                PinLevel::High
            } else {
                PinLevel::Low
            };
        }
        PinLevel::High
    }

    fn on_sda(&mut self, ctx: &mut dyn Ctx, level: PinLevel) {
        if level == self.prev_sda {
            return;
        }
        let edge_high = level == PinLevel::High;
        if self.prev_scl == PinLevel::High {
            // High while SCL is high: START on falling edge, STOP on rising.
            if edge_high {
                self.on_stop(ctx);
            } else {
                self.on_start();
            }
        }
        self.prev_sda = level;
    }

    fn on_scl(&mut self, ctx: &mut dyn Ctx, level: PinLevel) {
        if level == self.prev_scl {
            return;
        }
        if level == PinLevel::High && self.in_frame {
            self.on_scl_rising(ctx);
        }
        self.prev_scl = level;
    }

    fn on_start(&mut self) {
        self.in_frame = true;
        self.addr = None;
        self.read = false;
        self.target_present = false;
        self.addr_ack_pending = false;
        self.bit = 0;
        self.shift = 0;
        self.write_bytes.clear();
        self.read_bits = 0;
        self.last_result = Ok(());
        self.phases.push(I2cPhase::Start);
    }

    fn on_scl_rising(&mut self, ctx: &mut dyn Ctx) {
        if self.bit < 8 {
            if self.read && self.addr.is_some() {
                // Target-driven bit; the master samples through sda_readback.
                self.read_bits += 1;
            } else if self.prev_sda == PinLevel::High {
                self.shift = (self.shift << 1) | 1;
            } else {
                self.shift <<= 1;
            }
            self.bit += 1;
            if self.bit == 8 && self.addr.is_none() {
                // Decode the address as soon as its eight bits are clocked in
                // so the master can sample the target-driven ACK during the
                // ninth pulse (target_driven_slot / sda_readback).
                self.decode_address();
            }
            return;
        }
        self.complete_slot(ctx);
        self.bit = 0;
        self.shift = 0;
    }

    /// Resolve the clocked-in address byte and record its ACK phase.
    fn decode_address(&mut self) {
        match I2cAddr::seven(self.shift >> 1) {
            Ok(addr) => {
                self.read = self.shift & 1 == 1;
                self.target_present = lock(&self.bus).target_index(&addr).is_some();
                self.addr = Some(addr);
                self.addr_ack_pending = true;
                self.phases.push(I2cPhase::Address {
                    addr,
                    read: self.read,
                });
                self.phases.push(I2cPhase::Acknowledge(self.ack_level()));
            }
            Err(error) => {
                self.last_result = Err(error);
            }
        }
    }

    /// Handle the ninth SCL pulse: address ACK completion, data ACK and the
    /// read prefetch for the next target byte.
    fn complete_slot(&mut self, ctx: &mut dyn Ctx) {
        if self.addr_ack_pending {
            // The Address/Acknowledge phases were recorded at decode time; a
            // read transfer prefetches its first target byte on this pulse.
            self.addr_ack_pending = false;
            if self.read && self.target_present {
                self.prefetch_read(ctx);
            }
            return;
        }
        let Some(addr) = self.addr else {
            return;
        };
        if self.read {
            // Master's SDA during the ACK slot: low means continue, high means NACK.
            let master_ack = self.prev_sda == PinLevel::Low;
            self.phases.push(I2cPhase::Acknowledge(self.ack_level()));
            if master_ack && self.target_present {
                self.prefetch_read(ctx);
            }
        } else {
            self.write_bytes.push(self.shift);
            self.phases.push(I2cPhase::Data(self.shift));
            self.phases.push(I2cPhase::Acknowledge(self.ack_level()));
        }
        let _ = addr;
    }

    fn prefetch_read(&mut self, ctx: &mut dyn Ctx) {
        let Some(addr) = self.addr else {
            return;
        };
        let mut byte = [0u8; 1];
        match lock(&self.bus).dispatch_read_decoded(ctx, addr, &mut byte) {
            Ok(()) => {
                self.read_byte = byte[0];
                self.read_bits = 0;
            }
            Err(error) => self.last_result = Err(error),
        }
    }

    fn on_stop(&mut self, ctx: &mut dyn Ctx) {
        if !self.in_frame {
            return;
        }
        if let Some(addr) = self.addr {
            if !self.read && self.target_present {
                let bytes = core::mem::take(&mut self.write_bytes);
                self.last_result = lock(&self.bus)
                    .dispatch_write_decoded(ctx, addr, &bytes)
                    .map(|_| ());
            }
            self.phases.push(I2cPhase::Stop);
        }
        self.in_frame = false;
        self.addr = None;
        self.bit = 0;
        self.shift = 0;
        self.read_bits = 0;
    }

    fn ack_level(&self) -> Ack {
        if self.target_present {
            Ack::Ack
        } else {
            Ack::Nack
        }
    }
}

impl I2cBus {
    /// Decoder entry point: behaves like [`I2cInitiator::write`] but is driven
    /// by reconstructed pin edges.
    fn dispatch_write_decoded(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: I2cAddr,
        bytes: &[u8],
    ) -> Result<Ack, I2cError> {
        let Some(index) = self.target_index(&addr) else {
            return Ok(Ack::Nack);
        };
        self.targets[index].target_write(ctx, &addr, bytes)
    }

    fn dispatch_read_decoded(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: I2cAddr,
        buf: &mut [u8],
    ) -> Result<(), I2cError> {
        let Some(index) = self.target_index(&addr) else {
            return Err(I2cError::Nack);
        };
        match self.targets[index].target_read(ctx, &addr, buf)? {
            Ack::Ack => Ok(()),
            // Ack is non_exhaustive: any future acknowledgement variant is
            // conservatively treated as a failed transfer.
            Ack::Nack | _ => Err(I2cError::Nack),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::model::ContextError;

    /// Register-pointer target: first written byte selects the register,
    /// reads return a deterministic byte sequence from that pointer.
    struct PointerTarget {
        addr: I2cAddr,
        pointer: u8,
        writes: Vec<u8>,
    }

    impl I2cTarget for PointerTarget {
        fn matches(&self, addr: &I2cAddr) -> bool {
            &self.addr == addr
        }

        fn target_write(
            &mut self,
            _ctx: &mut dyn Ctx,
            _addr: &I2cAddr,
            bytes: &[u8],
        ) -> Result<Ack, I2cError> {
            self.writes.extend_from_slice(bytes);
            if let Some((pointer, _)) = bytes.split_first() {
                self.pointer = *pointer;
            }
            Ok(Ack::Ack)
        }

        fn target_read(
            &mut self,
            _ctx: &mut dyn Ctx,
            _addr: &I2cAddr,
            buf: &mut [u8],
        ) -> Result<Ack, I2cError> {
            for slot in buf.iter_mut() {
                *slot = self.pointer.wrapping_add(0x20);
                self.pointer = self.pointer.wrapping_add(1);
            }
            Ok(Ack::Ack)
        }
    }

    struct DummyCtx;
    impl Ctx for DummyCtx {
        fn now(&self) -> mosaic_exec::VTime {
            mosaic_exec::VTime::ZERO
        }
        fn schedule(
            &mut self,
            _at: mosaic_exec::VTime,
            _token: u64,
        ) -> Result<mosaic_exec::EventId, ContextError> {
            Err(ContextError::ResourceExhausted)
        }
        fn cancel(&mut self, _event: mosaic_exec::EventId) -> bool {
            false
        }
        fn set_signal(
            &mut self,
            _signal: mosaic_exec::SignalId,
            _high: bool,
        ) -> Result<(), ContextError> {
            Ok(())
        }
    }

    /// Mock net-backed pin: records the resolved level like a pull-up net and
    /// forwards every drive to the decoder sink, mirroring how a real net
    /// entity notifies attached [`BitBangLine`]s on level changes.
    struct TestLinePort {
        line: BitBangLine,
        level: PinLevel,
    }

    impl TestLinePort {
        fn new(line: BitBangLine) -> Self {
            Self {
                line,
                level: PinLevel::High,
            }
        }
    }

    impl GpioPort for TestLinePort {
        fn drive(&mut self, ctx: &mut dyn Ctx, level: PinLevel) -> Result<(), GpioError> {
            self.level = level;
            self.line.level_changed(ctx, level);
            Ok(())
        }
        fn release(&mut self, ctx: &mut dyn Ctx) -> Result<(), GpioError> {
            self.level = PinLevel::High;
            self.line.level_changed(ctx, self.level);
            Ok(())
        }
        fn configure(&mut self, _config: PinConfig) -> Result<(), GpioError> {
            Ok(())
        }
        fn current_level(&self) -> PinLevel {
            self.level
        }
    }

    /// Build the SDA/SCL master ports over one decoder, as a composition root
    /// would: net-facing sinks feed the decoder, master ports wrap them.
    fn master_ports(decoder: &Arc<Mutex<I2cBitBang>>) -> (BitBangPort, BitBangPort) {
        let (sda_line, scl_line) = I2cBitBang::lines(Arc::clone(decoder));
        let sda = I2cBitBang::sda_port(Arc::clone(decoder), Box::new(TestLinePort::new(sda_line)));
        let scl = I2cBitBang::scl_port(Arc::clone(decoder), Box::new(TestLinePort::new(scl_line)));
        (sda, scl)
    }

    /// START (or repeated START) from released lines; ends with SCL low so
    /// data bits can be driven legally.
    fn start(sda: &mut BitBangPort, scl: &mut BitBangPort, ctx: &mut dyn Ctx) {
        sda.drive(ctx, PinLevel::High).unwrap();
        scl.drive(ctx, PinLevel::High).unwrap();
        sda.drive(ctx, PinLevel::Low).unwrap();
        scl.drive(ctx, PinLevel::Low).unwrap();
    }

    /// STOP: SDA rises while SCL is high.
    fn stop(sda: &mut BitBangPort, scl: &mut BitBangPort, ctx: &mut dyn Ctx) {
        sda.drive(ctx, PinLevel::Low).unwrap();
        scl.drive(ctx, PinLevel::High).unwrap();
        sda.drive(ctx, PinLevel::High).unwrap();
    }

    /// Clock one byte out, MSB first, without the ninth (ACK) pulse.
    fn pulse_byte(sda: &mut BitBangPort, scl: &mut BitBangPort, ctx: &mut dyn Ctx, byte: u8) {
        for bit in (0..8).rev() {
            let level = if byte & (1 << bit) != 0 {
                PinLevel::High
            } else {
                PinLevel::Low
            };
            sda.drive(ctx, level).unwrap();
            scl.drive(ctx, PinLevel::High).unwrap();
            scl.drive(ctx, PinLevel::Low).unwrap();
        }
    }

    /// Run the ninth pulse. `drive` is the master's SDA level for the slot, or
    /// `None` to release the line. Returns the level observed before the pulse.
    fn ack_pulse(
        sda: &mut BitBangPort,
        scl: &mut BitBangPort,
        ctx: &mut dyn Ctx,
        drive: Option<PinLevel>,
    ) -> PinLevel {
        sda.drive(ctx, drive.unwrap_or(PinLevel::High)).unwrap();
        let observed = sda.current_level();
        scl.drive(ctx, PinLevel::High).unwrap();
        scl.drive(ctx, PinLevel::Low).unwrap();
        observed
    }

    /// Sample one target-driven read bit: release SDA, observe, then clock.
    fn read_bit(sda: &mut BitBangPort, scl: &mut BitBangPort, ctx: &mut dyn Ctx) -> PinLevel {
        sda.drive(ctx, PinLevel::High).unwrap();
        let bit = sda.current_level();
        scl.drive(ctx, PinLevel::High).unwrap();
        scl.drive(ctx, PinLevel::Low).unwrap();
        bit
    }

    #[test]
    fn transaction_path_writes_and_reads_through_the_target() {
        let mut bus = I2cBus::new();
        bus.add_target(Box::new(PointerTarget {
            addr: I2cAddr::seven(0x40).unwrap(),
            pointer: 0,
            writes: Vec::new(),
        }));
        let mut ctx = DummyCtx;
        bus.write(&mut ctx, I2cAddr::seven(0x40).unwrap(), &[0x0D, 0x01])
            .unwrap();
        let mut data = [0u8; 2];
        bus.read(&mut ctx, I2cAddr::seven(0x40).unwrap(), &mut data)
            .unwrap();
        // Pointer 0x0D selects the deterministic pattern 0x0D + 0x20, 0x0E + 0x20.
        assert_eq!(data, [0x2D, 0x2E]);
        assert_eq!(
            bus.write(&mut ctx, I2cAddr::seven(0x41).unwrap(), &[1]),
            Err(I2cError::Nack)
        );
    }

    #[test]
    fn bit_bang_path_reaches_the_same_target_with_identical_effect() {
        let bus = I2cBus::new();
        let bus = {
            let mut bus = bus;
            bus.add_target(Box::new(PointerTarget {
                addr: I2cAddr::seven(0x40).unwrap(),
                pointer: 0,
                writes: Vec::new(),
            }));
            bus.shared()
        };
        let decoder = Arc::new(Mutex::new(I2cBitBang::new(Arc::clone(&bus))));
        let (mut sda, mut scl) = master_ports(&decoder);
        let mut ctx = DummyCtx;

        // START + address write 0x40 (0x80) + register pointer 0x0D + STOP.
        start(&mut sda, &mut scl, &mut ctx);
        pulse_byte(&mut sda, &mut scl, &mut ctx, 0x80);
        // The address is decoded on the eighth pulse, so the target-driven ACK
        // is observable during the ninth slot.
        assert_eq!(ack_pulse(&mut sda, &mut scl, &mut ctx, None), PinLevel::Low);
        pulse_byte(&mut sda, &mut scl, &mut ctx, 0x0D);
        assert_eq!(ack_pulse(&mut sda, &mut scl, &mut ctx, None), PinLevel::Low);
        stop(&mut sda, &mut scl, &mut ctx);
        assert!(lock(&decoder).last_result.is_ok());

        // START + address read 0x40 (0x81), two sampled bytes, NACK, STOP.
        start(&mut sda, &mut scl, &mut ctx);
        pulse_byte(&mut sda, &mut scl, &mut ctx, 0x81);
        assert_eq!(ack_pulse(&mut sda, &mut scl, &mut ctx, None), PinLevel::Low);
        let mut read = [0u8; 2];
        let total = read.len();
        for (index, byte) in read.iter_mut().enumerate() {
            for _ in 0..8 {
                *byte <<= 1;
                if read_bit(&mut sda, &mut scl, &mut ctx) == PinLevel::High {
                    *byte |= 1;
                }
            }
            // The master drives the ACK slot of a read frame: low continues,
            // high NACKs the final byte. The port shows the master's own level.
            let final_byte = index + 1 == total;
            let drive = if final_byte {
                PinLevel::High
            } else {
                PinLevel::Low
            };
            assert_eq!(ack_pulse(&mut sda, &mut scl, &mut ctx, Some(drive)), drive);
        }
        stop(&mut sda, &mut scl, &mut ctx);
        assert_eq!(read, [0x2D, 0x2E]);
        assert!(lock(&decoder).last_result.is_ok());
    }

    #[test]
    fn both_paths_observe_and_mutate_one_target_state() {
        let bus = I2cBus::new();
        let bus = {
            let mut bus = bus;
            bus.add_target(Box::new(PointerTarget {
                addr: I2cAddr::seven(0x40).unwrap(),
                pointer: 0,
                writes: Vec::new(),
            }));
            bus.shared()
        };
        let decoder = Arc::new(Mutex::new(I2cBitBang::new(Arc::clone(&bus))));
        let (mut sda, mut scl) = master_ports(&decoder);
        let mut ctx = DummyCtx;

        // Bit-bang write moves the register pointer; the transaction-level
        // read then sees exactly what the pin-level frame committed.
        start(&mut sda, &mut scl, &mut ctx);
        pulse_byte(&mut sda, &mut scl, &mut ctx, 0x80);
        ack_pulse(&mut sda, &mut scl, &mut ctx, None);
        pulse_byte(&mut sda, &mut scl, &mut ctx, 0x20);
        ack_pulse(&mut sda, &mut scl, &mut ctx, None);
        stop(&mut sda, &mut scl, &mut ctx);
        assert!(lock(&decoder).last_result.is_ok());
        let mut data = [0u8; 1];
        lock(&bus)
            .read(&mut ctx, I2cAddr::seven(0x40).unwrap(), &mut data)
            .unwrap();
        assert_eq!(data, [0x40]);

        // And the other direction: a transaction-level write is visible to a
        // bit-bang read of the same part.
        lock(&bus)
            .write(&mut ctx, I2cAddr::seven(0x40).unwrap(), &[0x10])
            .unwrap();
        start(&mut sda, &mut scl, &mut ctx);
        pulse_byte(&mut sda, &mut scl, &mut ctx, 0x81);
        ack_pulse(&mut sda, &mut scl, &mut ctx, None);
        let mut byte = 0u8;
        for _ in 0..8 {
            byte <<= 1;
            if read_bit(&mut sda, &mut scl, &mut ctx) == PinLevel::High {
                byte |= 1;
            }
        }
        ack_pulse(&mut sda, &mut scl, &mut ctx, Some(PinLevel::High));
        stop(&mut sda, &mut scl, &mut ctx);
        assert_eq!(byte, 0x30);
    }

    #[test]
    fn bit_bang_nacks_unknown_addresses() {
        let bus = I2cBus::new().shared();
        let decoder = Arc::new(Mutex::new(I2cBitBang::new(Arc::clone(&bus))));
        let (mut sda, mut scl) = master_ports(&decoder);
        let mut ctx = DummyCtx;
        start(&mut sda, &mut scl, &mut ctx);
        // 0x82 = address 0x41, absent. With no target present the ACK slot
        // stays released (NACK).
        pulse_byte(&mut sda, &mut scl, &mut ctx, 0x82);
        assert_eq!(
            ack_pulse(&mut sda, &mut scl, &mut ctx, None),
            PinLevel::High
        );
        stop(&mut sda, &mut scl, &mut ctx);
        let decoder = lock(&decoder);
        assert!(decoder.phases.contains(&I2cPhase::Acknowledge(Ack::Nack)));
    }
}
