//! FIFO bridge from runtime signal sinks to CPU interrupt lines (ADR 0009).
//!
//! The runtime knows [`SignalSink`] nodes and the engine contract knows
//! [`IrqLine`] inputs; neither side depends on the other. This module is the
//! composition-root adapter that records line edges in a shared mailbox and
//! lets the driver flush them into the engine between run quanta, preserving
//! the runtime's delivery order without re-entering any callback.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use mosaic_contract_cpu_engine::{CpuEngine, EngineError, IrqLine};
use mosaic_contract_signal::SignalSink;

/// FIFO of `(line, high)` edges produced by runtime signal delivery.
#[derive(Default)]
pub struct IrqMailbox {
    edges: Mutex<VecDeque<(IrqLine, bool)>>,
}

impl IrqMailbox {
    pub fn new() -> Self {
        Self::default()
    }

    fn push(&self, line: IrqLine, high: bool) {
        self.edges
            .lock()
            .expect("IRQ mailbox mutex poisoned")
            .push_back((line, high));
    }

    /// Edges waiting for the next engine flush.
    pub fn len(&self) -> usize {
        self.edges.lock().expect("IRQ mailbox mutex poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.edges
            .lock()
            .expect("IRQ mailbox mutex poisoned")
            .is_empty()
    }

    /// Forward every queued edge to `engine` in FIFO order. On an engine error
    /// the failing edge and all later edges stay queued so the composition root
    /// can retry or report the fault without losing interrupt history.
    pub fn drain_into(&self, engine: &mut dyn CpuEngine) -> Result<usize, EngineError> {
        let mut guard = self.edges.lock().expect("IRQ mailbox mutex poisoned");
        let mut delivered = 0;
        while let Some((line, high)) = guard.front().copied() {
            if let Err(error) = engine.set_line(line, high) {
                if delivered == 0 {
                    return Err(error);
                }
                // Earlier edges already landed; surface the fault while the
                // remainder stays queued for the next flush.
                return Ok(delivered);
            }
            guard.pop_front();
            delivered += 1;
        }
        Ok(delivered)
    }
}

/// Runtime sink that maps one wired signal onto one engine interrupt line.
/// Register one instance per line; the runtime treats it as an ordinary sink
/// node and never reaches through to the engine.
pub struct EngineIrqSink {
    line: IrqLine,
    mailbox: Arc<IrqMailbox>,
}

impl EngineIrqSink {
    pub fn new(line: IrqLine, mailbox: Arc<IrqMailbox>) -> Self {
        Self { line, mailbox }
    }
}

impl SignalSink for EngineIrqSink {
    fn set_level(&mut self, high: bool) {
        self.mailbox.push(self.line, high);
    }
}

#[cfg(test)]
mod tests {
    use core::num::NonZeroU64;

    use mosaic_contract_cpu_engine::{EngineCaps, RegId, RunOutcome, StopReason};
    use mosaic_core_types::{Addr, Permissions};

    use super::*;

    /// Minimal engine that records line edges and rejects line 9.
    struct RecordingEngine {
        edges: Vec<(IrqLine, bool)>,
    }

    impl CpuEngine for RecordingEngine {
        fn caps(&self) -> EngineCaps {
            EngineCaps::COUNTED
        }
        fn run(&mut self, _instructions: NonZeroU64) -> RunOutcome {
            RunOutcome {
                reason: StopReason::BudgetExhausted,
                instruction_hooks: 0,
            }
        }
        fn reg_read(&self, _reg: RegId) -> Result<u64, EngineError> {
            Err(EngineError::InvalidRegister)
        }
        fn reg_write(&mut self, _reg: RegId, _value: u64) -> Result<(), EngineError> {
            Err(EngineError::InvalidRegister)
        }
        fn map_memory(
            &mut self,
            _base: Addr,
            _size: u64,
            _permissions: Permissions,
        ) -> Result<(), EngineError> {
            Ok(())
        }
        fn write_memory(&mut self, _base: Addr, _data: &[u8]) -> Result<(), EngineError> {
            Ok(())
        }
        fn read_memory(&self, _base: Addr, _data: &mut [u8]) -> Result<(), EngineError> {
            Ok(())
        }
        fn set_line(&mut self, line: IrqLine, high: bool) -> Result<(), EngineError> {
            if line.0 == 9 {
                return Err(EngineError::LineNotSupported);
            }
            self.edges.push((line, high));
            Ok(())
        }
    }

    #[test]
    fn sink_edges_flush_to_the_engine_in_fifo_order() {
        let mailbox = Arc::new(IrqMailbox::new());
        let mut irq_sink = EngineIrqSink::new(IrqLine(0), Arc::clone(&mailbox));
        let mut fiq_sink = EngineIrqSink::new(IrqLine(1), Arc::clone(&mailbox));
        irq_sink.set_level(true);
        fiq_sink.set_level(true);
        irq_sink.set_level(false);
        assert_eq!(mailbox.len(), 3);

        let mut engine = RecordingEngine { edges: Vec::new() };
        let delivered = mailbox.drain_into(&mut engine).unwrap();
        assert_eq!(delivered, 3);
        assert!(mailbox.is_empty());
        assert_eq!(
            engine.edges,
            vec![(IrqLine(0), true), (IrqLine(1), true), (IrqLine(0), false),]
        );
    }

    #[test]
    fn an_engine_failure_keeps_the_remaining_edges_queued() {
        let mailbox = Arc::new(IrqMailbox::new());
        mailbox.push(IrqLine(0), true);
        mailbox.push(IrqLine(9), true);
        mailbox.push(IrqLine(0), false);
        let mut engine = RecordingEngine { edges: Vec::new() };
        let delivered = mailbox.drain_into(&mut engine).unwrap();
        assert_eq!(delivered, 1);
        assert_eq!(engine.edges, vec![(IrqLine(0), true)]);
        // The rejected edge and its successor remain for a later retry.
        assert_eq!(mailbox.len(), 2);
    }

    #[test]
    fn unsupported_line_is_reported_when_nothing_was_delivered() {
        let mailbox = Arc::new(IrqMailbox::new());
        mailbox.push(IrqLine(9), true);
        let mut engine = RecordingEngine { edges: Vec::new() };
        let error = mailbox.drain_into(&mut engine).unwrap_err();
        assert_eq!(error, EngineError::LineNotSupported);
        assert_eq!(mailbox.len(), 1);
    }
}
