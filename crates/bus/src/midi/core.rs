//! `midi` bus entity (ADR 0025 step 3): MIDI 1.0.

use mosaic_contract::midi::{MidiError, MidiInput, MidiMessage, MidiOutput};
use mosaic_contract::model::Ctx;

pub struct MidiBus {
    inputs: Vec<Box<dyn MidiInput>>,
}

impl Default for MidiBus {
    fn default() -> Self {
        Self::new()
    }
}

impl MidiBus {
    pub fn new() -> Self {
        Self { inputs: Vec::new() }
    }
    pub fn attach(&mut self, i: Box<dyn MidiInput>) {
        self.inputs.push(i);
    }
}

impl MidiOutput for MidiBus {
    fn send(&mut self, ctx: &mut dyn Ctx, m: &MidiMessage) -> Result<(), MidiError> {
        if self.inputs.is_empty() {
            return Err(MidiError::NoReceiver);
        }
        for i in self.inputs.iter_mut() {
            i.on_message(ctx, m)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::midi::{MidiInput, MidiMessage};
    use mosaic_contract::model::Ctx;

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

    #[derive(Default)]
    struct In {
        msgs: u32,
    }
    impl MidiInput for In {
        fn on_message(&mut self, _: &mut dyn Ctx, _m: &MidiMessage) -> Result<(), MidiError> {
            self.msgs += 1;
            Ok(())
        }
    }

    #[test]
    fn send_reaches_all_inputs() {
        let mut bus = MidiBus::new();
        bus.attach(Box::new(In::default()));
        bus.attach(Box::new(In::default()));
        let mut ctx = NoopCtx;
        let m = MidiMessage {
            status: 0x90,
            data1: 60,
            data2: 100,
        };
        assert!(bus.send(&mut ctx, &m).is_ok());
    }

    #[test]
    fn no_inputs_yields_error() {
        let mut bus = MidiBus::new();
        let mut ctx = NoopCtx;
        let m = MidiMessage {
            status: 0x80,
            data1: 60,
            data2: 0,
        };
        assert_eq!(bus.send(&mut ctx, &m), Err(MidiError::NoReceiver));
    }
}
