//! Minimal `Ctx` for in-module part tests: no scheduler, signal edges are
//! only recorded. Parts that raise no signals simply pass `&mut TestCtx`.

use mosaic_contract::model::{ContextError, Ctx};
use mosaic_exec::{EventId, SignalId, VDelta, VTime};

/// Signal-edge recorder shared by the part-core unit tests.
#[derive(Default)]
pub(crate) struct TestCtx {
    /// Every `set_signal` call, in order.
    pub edges: Vec<(SignalId, bool)>,
}

impl Ctx for TestCtx {
    fn now(&self) -> VTime {
        VTime::ZERO
    }

    fn schedule(&mut self, _at: VTime, _token: u64) -> Result<EventId, ContextError> {
        Err(ContextError::ResourceExhausted)
    }

    fn schedule_after(
        &mut self,
        _delay: VDelta,
        _token: u64,
    ) -> Result<EventId, ContextError> {
        Err(ContextError::ResourceExhausted)
    }

    fn cancel(&mut self, _event: EventId) -> bool {
        false
    }

    fn set_signal(&mut self, signal: SignalId, high: bool) -> Result<(), ContextError> {
        self.edges.push((signal, high));
        Ok(())
    }
}
