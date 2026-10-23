#![forbid(unsafe_code)]
//! I2C bus entity: address resolution, transaction dispatch and pin-level
//! bit-bang decoding (構成設計書 §11).
//!
//! One [`I2cBus`] owns the target list and serves both access paths:
//! register models drive the transaction-level [`I2cInitiator`] port, while
//! GPIO bit-bang masters feed SDA/SCL edges into [`I2cBitBang`], which
//! reconstructs the same frames and dispatches through the identical target
//! trait. A target therefore cannot tell which path reached it.

use std::sync::{Arc, Mutex, MutexGuard};

use mosaic_contract_gpio::{GpioSink, PinLevel};
use mosaic_contract_i2c::{
    Ack, I2cAddr, I2cError, I2cInitiator, I2cPhase, I2cSpeed, I2cTarget,
};
use mosaic_contract_model::Ctx;

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

    fn write(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: I2cAddr,
        bytes: &[u8],
    ) -> Result<(), I2cError> {
        self.phases.push(I2cPhase::Start);
        self.phases.push(I2cPhase::Address {
            addr,
            read: false,
        });
        for byte in bytes {
            self.phases.push(I2cPhase::Data(*byte));
        }
        let result = self.dispatch_write(ctx, addr, bytes);
        self.phases.push(I2cPhase::Stop);
        result
    }

    fn read(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: I2cAddr,
        buf: &mut [u8],
    ) -> Result<(), I2cError> {
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
        self.phases.push(I2cPhase::Address {
            addr,
            read: false,
        });
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

    pub fn phases(&self) -> &[I2cPhase] {
        &self.phases
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
            return;
        }
        self.complete_slot(ctx);
        self.bit = 0;
        self.shift = 0;
    }

    /// Handle the ninth SCL pulse: address decode, ACK and read prefetch.
    fn complete_slot(&mut self, ctx: &mut dyn Ctx) {
        if self.addr.is_none() {
            let addr = match I2cAddr::seven(self.shift >> 1) {
                Ok(addr) => addr,
                Err(error) => {
                    self.last_result = Err(error);
                    return;
                }
            };
            self.read = self.shift & 1 == 1;
            self.target_present = lock(&self.bus).target_index(&addr).is_some();
            self.addr = Some(addr);
            self.phases.push(I2cPhase::Address {
                addr,
                read: self.read,
            });
            self.phases.push(I2cPhase::Acknowledge(self.ack_level()));
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
            Ack::Nack => Err(I2cError::Nack),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract_model::ContextError;

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
        fn now(&self) -> mosaic_core_types::VTime {
            mosaic_core_types::VTime::ZERO
        }
        fn schedule(
            &mut self,
            _at: mosaic_core_types::VTime,
            _token: u64,
        ) -> Result<mosaic_core_types::EventId, ContextError> {
            Err(ContextError::ResourceExhausted)
        }
        fn cancel(&mut self, _event: mosaic_core_types::EventId) -> bool {
            false
        }
        fn set_signal(
            &mut self,
            _signal: mosaic_core_types::SignalId,
            _high: bool,
        ) -> Result<(), ContextError> {
            Ok(())
        }
    }

    /// Drive one byte (MSB first) plus an ACK slot using raw pin edges.
    fn pulse_byte(
        sda: &mut BitBangLine,
        scl: &mut BitBangLine,
        ctx: &mut dyn Ctx,
        byte: u8,
    ) {
        for bit in (0..8).rev() {
            let level = if byte & (1 << bit) != 0 {
                PinLevel::High
            } else {
                PinLevel::Low
            };
            sda.level_changed(ctx, level);
            scl.level_changed(ctx, PinLevel::Low);
            scl.level_changed(ctx, PinLevel::High);
        }
        // ACK slot: master releases SDA, one more clock pulse.
        sda.level_changed(ctx, PinLevel::High);
        scl.level_changed(ctx, PinLevel::Low);
        scl.level_changed(ctx, PinLevel::High);
    }

    fn start(sda: &mut BitBangLine, scl: &mut BitBangLine, ctx: &mut dyn Ctx) {
        sda.level_changed(ctx, PinLevel::High);
        scl.level_changed(ctx, PinLevel::High);
        sda.level_changed(ctx, PinLevel::Low);
    }

    fn stop(sda: &mut BitBangLine, scl: &mut BitBangLine, ctx: &mut dyn Ctx) {
        sda.level_changed(ctx, PinLevel::Low);
        scl.level_changed(ctx, PinLevel::Low);
        scl.level_changed(ctx, PinLevel::High);
        sda.level_changed(ctx, PinLevel::High);
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
        bus.write(
            &mut ctx,
            I2cAddr::seven(0x40).unwrap(),
            &[0x0D, 0x01],
        )
        .unwrap();
        let mut data = [0u8; 2];
        bus.read(&mut ctx, I2cAddr::seven(0x40).unwrap(), &mut data)
            .unwrap();
        assert_eq!(data, [0x2E, 0x2F]);
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
        let (mut sda, mut scl) = I2cBitBang::lines(Arc::clone(&decoder));
        let mut ctx = DummyCtx;

        // START + address write 0x40 (0x80) + register pointer 0x0D + STOP.
        start(&mut sda, &mut scl, &mut ctx);
        pulse_byte(&mut sda, &mut scl, &mut ctx, 0x80);
        pulse_byte(&mut sda, &mut scl, &mut ctx, 0x0D);
        stop(&mut sda, &mut scl, &mut ctx);
        assert!(lock(&decoder).last_result.is_ok());

        // START + address read 0x40 (0x81), sample two bytes, NACK, STOP.
        start(&mut sda, &mut scl, &mut ctx);
        pulse_byte(&mut sda, &mut scl, &mut ctx, 0x81);
        let mut read = [0u8; 2];
        for byte in read.iter_mut() {
            for _ in 0..8 {
                // Sample the target-driven bit while SCL is low, then clock.
                scl.level_changed(&mut ctx, PinLevel::Low);
                let bit = lock(&decoder).sda_readback();
                scl.level_changed(&mut ctx, PinLevel::High);
                *byte <<= 1;
                if bit == PinLevel::High {
                    *byte |= 1;
                }
            }
            // ACK (low) then clock pulse.
            sda.level_changed(&mut ctx, PinLevel::Low);
            scl.level_changed(&mut ctx, PinLevel::Low);
            scl.level_changed(&mut ctx, PinLevel::High);
        }
        sda.level_changed(&mut ctx, PinLevel::High);
        stop(&mut sda, &mut scl, &mut ctx);
        assert_eq!(read, [0x2E, 0x2F]);
        assert!(lock(&decoder).last_result.is_ok());
    }

    #[test]
    fn bit_bang_nacks_unknown_addresses() {
        let bus = I2cBus::new().shared();
        let decoder = Arc::new(Mutex::new(I2cBitBang::new(bus)));
        let (mut sda, mut scl) = I2cBitBang::lines(decoder);
        let mut ctx = DummyCtx;
        start(&mut sda, &mut scl, &mut ctx);
        pulse_byte(&mut sda, &mut scl, &mut ctx, 0x82); // 0x41, absent
        stop(&mut sda, &mut scl, &mut ctx);
        // The missing target leaves a NACK in the reconstructed frame.
        let decoder = lock(&sda.decoder);
        assert!(decoder.phases.contains(&I2cPhase::Acknowledge(Ack::Nack)));
    }
}
