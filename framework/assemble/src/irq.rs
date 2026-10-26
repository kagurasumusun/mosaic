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
use mosaic_core_types::{Fnv64, Snapshot, SnapshotError};

/// Pending IRQ edges, independent of which engine the mailbox is bound to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IrqMailboxState {
    pub schema: u32,
    pub edges: Vec<(u32, bool)>,
}

impl IrqMailboxState {
    pub const SCHEMA_VERSION: u32 = 1;
}

/// FIFO of `(line, high)` edges produced by runtime signal delivery.
#[derive(Default)]
pub struct IrqMailbox {
    edges: Mutex<VecDeque<(IrqLine, bool)>>,
    /// Serializes consumers without holding the queue lock across engine code.
    drain_lock: Mutex<()>,
}

impl IrqMailbox {
    pub fn new() -> Self {
        Self::default()
    }

    fn push(&self, line: IrqLine, high: bool) {
        self.edges
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push_back((line, high));
    }

    /// Edges waiting for the next engine flush.
    pub fn len(&self) -> usize {
        self.edges
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.edges
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .is_empty()
    }

    /// Edges waiting for the next engine flush, in delivery order.
    ///
    /// This is the IRQ half of a driver checkpoint (ADR 0011). It does not
    /// include CPU register context. Call it between quanta: a concurrent
    /// drain is serialized on the same consumer lock.
    pub fn save_edges(&self) -> IrqMailboxState {
        let _consumer = self
            .drain_lock
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let edges = self
            .edges
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        IrqMailboxState {
            schema: IrqMailboxState::SCHEMA_VERSION,
            edges: edges.iter().map(|(line, high)| (line.0, *high)).collect(),
        }
    }

    /// Replace queued edges. The destination mailbox keeps its engine binding;
    /// only the pending FIFO is restored. A schema mismatch leaves the queue
    /// unchanged.
    pub fn load_edges(&self, state: &IrqMailboxState) -> Result<(), SnapshotError> {
        if state.schema != IrqMailboxState::SCHEMA_VERSION {
            return Err(SnapshotError::VersionMismatch);
        }
        let restored: VecDeque<_> = state
            .edges
            .iter()
            .map(|(line, high)| (IrqLine(*line), *high))
            .collect();
        let _consumer = self
            .drain_lock
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let mut edges = self
            .edges
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        *edges = restored;
        Ok(())
    }

    pub fn edge_fingerprint(state: &IrqMailboxState) -> u64 {
        let mut hash = Fnv64::new();
        hash.u32(state.schema);
        hash.usize(state.edges.len());
        for (line, high) in &state.edges {
            hash.u32(*line);
            hash.bool(*high);
        }
        hash.finish()
    }

    /// Forward queued edges to `engine` in FIFO order. On an engine error,
    /// return the error even if earlier edges were delivered; those earlier
    /// edges have been removed, while the rejected edge and every successor
    /// remain queued. This avoids treating a partial flush as success.
    ///
    /// The consumer lock serializes drains, but the queue lock is released
    /// around `set_line`: backend code must not execute while holding the lock
    /// used by signal producers. Producers can safely append during a flush.
    pub fn drain_into(&self, engine: &mut dyn CpuEngine) -> Result<usize, EngineError> {
        let _consumer = self
            .drain_lock
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let mut delivered = 0;
        loop {
            let next = self
                .edges
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .front()
                .copied();
            let Some((line, high)) = next else {
                return Ok(delivered);
            };

            // Do not remove the head until the engine confirms delivery.
            // The consumer lock guarantees no other drain can change it.
            engine.set_line(line, high)?;
            let mut edges = self
                .edges
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            debug_assert_eq!(edges.front().copied(), Some((line, high)));
            edges.pop_front();
            delivered += 1;
        }
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

impl Snapshot for EngineIrqSink {
    type State = IrqMailboxState;
    const SCHEMA_VERSION: u32 = IrqMailboxState::SCHEMA_VERSION;

    fn save(&self) -> Self::State {
        self.mailbox.save_edges()
    }

    fn validate(&self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        if version != Self::SCHEMA_VERSION || state.schema != Self::SCHEMA_VERSION {
            return Err(SnapshotError::VersionMismatch);
        }
        Ok(())
    }

    fn load(&mut self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        self.validate(version, state)?;
        self.mailbox.load_edges(state)
    }

    fn fingerprint(version: u32, state: &Self::State) -> u64 {
        let mut hash = Fnv64::new();
        hash.u32(version);
        hash.u64(IrqMailbox::edge_fingerprint(state));
        hash.finish()
    }
}

#[cfg(test)]
mod tests {
    use core::num::NonZeroU64;

    use mosaic_contract_cpu_engine::{EngineCaps, RegId, RunOutcome, StopReason};
    use mosaic_core_types::{Addr, Permissions, SnapshotError};

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
    fn an_engine_failure_is_reported_after_partial_delivery() {
        let mailbox = Arc::new(IrqMailbox::new());
        mailbox.push(IrqLine(0), true);
        mailbox.push(IrqLine(9), true);
        mailbox.push(IrqLine(0), false);
        let mut engine = RecordingEngine { edges: Vec::new() };
        let error = mailbox.drain_into(&mut engine).unwrap_err();
        assert_eq!(error, EngineError::LineNotSupported);
        assert_eq!(engine.edges, vec![(IrqLine(0), true)]);
        // The rejected edge and its successor remain for a later retry, but
        // the partial success is no longer misreported as an overall success.
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

    #[test]
    fn mailbox_edges_restore_without_replacing_the_engine_binding() {
        let mailbox = Arc::new(IrqMailbox::new());
        mailbox.push(IrqLine(3), true);
        mailbox.push(IrqLine(4), false);
        let saved = mailbox.save_edges();
        assert_eq!(
            IrqMailbox::edge_fingerprint(&saved),
            IrqMailbox::edge_fingerprint(&mailbox.save_edges())
        );
        mailbox.push(IrqLine(1), true);
        let other = IrqMailbox::new();
        other.load_edges(&saved).unwrap();
        assert_eq!(other.save_edges(), saved);
        let mut rejected = saved.clone();
        rejected.schema = 99;
        assert_eq!(
            other.load_edges(&rejected),
            Err(SnapshotError::VersionMismatch)
        );
        assert_eq!(other.save_edges(), saved);
    }
}
