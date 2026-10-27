//! GPIB instrument core (IEEE-488 talker/listener) — the shared functional
//! core behind every `part:*/...` GPIB instrument.
//!
//! One family, one core (ADR 0026 step 2 pattern): an instrument is a
//! primary address (0-30), a `*IDN?` identity string, a FIFO of readings the
//! controller reads back with a budget, and the last interface command.
//! SCPI parsing is the front-end's job; the core models what crosses the
//! bus: addressing, command broadcast, and byte transfers.

use std::collections::VecDeque;

use mosaic_contract::gpib::{GpibDevice, GpibError};
use mosaic_contract::model::Ctx;

/// One GPIB instrument.
pub struct GpibInstrumentCore {
    pad: u8,
    idn: String,
    readings: VecDeque<u8>,
    last_command: Option<u8>,
    addressed: bool,
}

impl Default for GpibInstrumentCore {
    fn default() -> Self {
        Self::new()
    }
}

impl GpibInstrumentCore {
    /// An unaddressed placeholder instrument at PAD 22.
    pub fn new() -> Self {
        Self {
            pad: 22,
            idn: "MOSAIC,Instrument Core,0,1.0\n".to_owned(),
            readings: VecDeque::new(),
            last_command: None,
            addressed: false,
        }
    }

    /// Primary address 0-30 (values above 30 are masked to 30).
    pub fn at(mut self, pad: u8) -> Self {
        self.pad = pad.min(30);
        self
    }

    /// The `*IDN?` response string.
    pub fn with_idn(mut self, idn: impl Into<String>) -> Self {
        self.idn = idn.into();
        self
    }

    /// Queue a reading (a message terminator-terminated byte string).
    pub fn push_reading(&mut self, bytes: impl IntoIterator<Item = u8>) {
        self.readings.extend(bytes);
    }

    /// The configured primary address.
    pub fn address(&self) -> u8 {
        self.pad
    }

    /// The last broadcast interface command, if any.
    pub fn last_command(&self) -> Option<u8> {
        self.last_command
    }

    /// True while addressed as a listener by the controller.
    pub fn addressed(&self) -> bool {
        self.addressed
    }
}

impl GpibDevice for GpibInstrumentCore {
    fn address(&self) -> u8 {
        self.pad
    }

    fn on_command(&mut self, _ctx: &mut dyn Ctx, command: u8) -> Result<(), GpibError> {
        self.last_command = Some(command);
        if command == 0x3F {
            // UNL (unlisten): no device is addressed any more.
            self.addressed = false;
        } else if command == 0x20 | self.pad {
            // MLA (my listen address): this instrument is addressed.
            self.addressed = true;
        }
        Ok(())
    }

    fn on_write(&mut self, _ctx: &mut dyn Ctx, data: &[u8]) -> Result<(), GpibError> {
        if !self.addressed {
            // A write to an unaddressed instrument is a bus-protocol error.
            return Err(GpibError::NoListener);
        }
        // Program messages are acknowledged; query responses are queued.
        if data.ends_with(b"?") {
            let reply: Vec<u8> = self.idn.bytes().collect();
            self.push_reading(reply);
        }
        Ok(())
    }

    fn read(&mut self, _ctx: &mut dyn Ctx, budget: usize) -> Result<Vec<u8>, GpibError> {
        if self.readings.is_empty() {
            return Err(GpibError::NotTalker);
        }
        let mut out = Vec::with_capacity(budget.min(self.readings.len()));
        for _ in 0..budget {
            match self.readings.pop_front() {
                Some(byte) => out.push(byte),
                None => break,
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoopCtx;
    impl Ctx for NoopCtx {
        fn now(&self) -> mosaic_exec::VTime {
            mosaic_exec::VTime::ZERO
        }
        fn schedule(
            &mut self,
            _: mosaic_exec::VTime,
            _: u64,
        ) -> Result<mosaic_exec::EventId, mosaic_contract::model::ContextError> {
            Err(mosaic_contract::model::ContextError::ResourceExhausted)
        }
        fn cancel(&mut self, _: mosaic_exec::EventId) -> bool {
            false
        }
        fn set_signal(
            &mut self,
            _: mosaic_exec::SignalId,
            _: bool,
        ) -> Result<(), mosaic_contract::model::ContextError> {
            Ok(())
        }
    }

    #[test]
    fn addressed_instrument_answers_idn() {
        let mut dmm = GpibInstrumentCore::new()
            .at(22)
            .with_idn("HP,34401A,0,9.0\n");
        let mut ctx = NoopCtx;
        // MLA 22 = 0x20 | 22
        dmm.on_command(&mut ctx, 0x20 | 22).unwrap();
        assert!(dmm.addressed());
        dmm.on_write(&mut ctx, b"*IDN?").unwrap();
        let reply = dmm.read(&mut ctx, 32).unwrap();
        assert_eq!(reply, b"HP,34401A,0,9.0\n".to_vec());
    }

    #[test]
    fn unaddressed_write_is_no_listener() {
        let mut dmm = GpibInstrumentCore::new();
        let mut ctx = NoopCtx;
        assert_eq!(dmm.on_write(&mut ctx, b"*IDN?"), Err(GpibError::NoListener));
    }

    #[test]
    fn read_without_a_query_is_not_talker() {
        let mut dmm = GpibInstrumentCore::new();
        let mut ctx = NoopCtx;
        assert_eq!(dmm.read(&mut ctx, 8), Err(GpibError::NotTalker));
    }

    #[test]
    fn unlisten_clears_addressing() {
        let mut dmm = GpibInstrumentCore::new().at(7);
        let mut ctx = NoopCtx;
        dmm.on_command(&mut ctx, 0x20 | 7).unwrap();
        assert!(dmm.addressed());
        dmm.on_command(&mut ctx, 0x3F).unwrap();
        assert!(!dmm.addressed());
        assert_eq!(dmm.last_command(), Some(0x3F));
    }

    #[test]
    fn budget_bounds_the_transfer() {
        let mut dmm = GpibInstrumentCore::new();
        dmm.push_reading(*b"+1.234E+01\n");
        let mut ctx = NoopCtx;
        assert_eq!(dmm.read(&mut ctx, 4).unwrap(), b"+1.2".to_vec());
        assert_eq!(dmm.read(&mut ctx, 64).unwrap(), b"34E+01\n".to_vec());
    }
}
