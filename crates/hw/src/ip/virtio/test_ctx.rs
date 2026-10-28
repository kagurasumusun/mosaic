//! Shared test context for the virtio modules (cfg(test) only): a `Ctx`
//! that records signal edges and scheduled events, so device tests can
//! assert interrupt behaviour without a full machine.

#[cfg(test)]
pub(crate) mod support {
    use mosaic_contract::model::{ContextError, Ctx};
    use mosaic_exec::{EventId, SignalId, VTime};

    pub(crate) struct RecCtx {
        pub signals: Vec<(SignalId, bool)>,
        pub scheduled: Vec<(VTime, u64)>,
    }

    impl RecCtx {
        pub(crate) fn new() -> Self {
            Self {
                signals: Vec::new(),
                scheduled: Vec::new(),
            }
        }

        pub(crate) fn level(&self, id: SignalId) -> Option<bool> {
            self.signals
                .iter()
                .rev()
                .find(|(signal, _)| *signal == id)
                .map(|(_, high)| *high)
        }
    }

    impl Default for RecCtx {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Ctx for RecCtx {
        fn now(&self) -> VTime {
            VTime::ZERO
        }

        fn schedule(&mut self, at: VTime, token: u64) -> Result<EventId, ContextError> {
            self.scheduled.push((at, token));
            Ok(EventId(self.scheduled.len() as u64))
        }

        fn cancel(&mut self, _event: EventId) -> bool {
            true
        }

        fn set_signal(&mut self, signal: SignalId, high: bool) -> Result<(), ContextError> {
            self.signals.push((signal, high));
            Ok(())
        }
    }
}
