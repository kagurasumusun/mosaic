#![forbid(unsafe_code)]
//! CAN bus entity with deterministic identifier arbitration.
//!
//! Frames submitted while the bus is busy enter a finite queue; arbitration
//! always releases the lowest numeric identifier first and breaks ties by
//! submission order, so two identical simulations schedule identical traffic.
//! Immediate transmission is available through the
//! [`mosaic_contract_can::CanController`] port for models that do not model
//! contention.

use std::collections::VecDeque;

use mosaic_contract_can::{CanController, CanError, CanFrame, CanId, CanReceiver};
use mosaic_contract_model::Ctx;

struct QueuedFrame {
    frame: CanFrame,
    sequence: u64,
}

/// CAN bus holding receiver filters and a bounded arbitration queue.
pub struct CanBus {
    receivers: Vec<Box<dyn CanReceiver>>,
    pending: VecDeque<QueuedFrame>,
    capacity: usize,
    next_sequence: u64,
}

impl CanBus {
    pub fn new(capacity: usize) -> Self {
        Self {
            receivers: Vec::new(),
            pending: VecDeque::new(),
            capacity,
            next_sequence: 0,
        }
    }

    pub fn add_receiver(&mut self, receiver: Box<dyn CanReceiver>) {
        self.receivers.push(receiver);
    }

    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Queue a frame for arbitration. Returns [`CanError::Full`] when the
    /// bounded queue cannot accept another frame.
    pub fn enqueue(&mut self, frame: CanFrame) -> Result<(), CanError> {
        if self.pending.len() >= self.capacity {
            return Err(CanError::Full);
        }
        let queued = QueuedFrame {
            frame,
            sequence: self.next_sequence,
        };
        self.next_sequence += 1;
        self.pending.push_back(queued);
        Ok(())
    }

    /// Arbitrate and deliver one frame to every accepting receiver.
    pub fn arbitrate_one(&mut self, ctx: &mut dyn Ctx) -> Option<CanFrame> {
        let index = self
            .pending
            .iter()
            .enumerate()
            .min_by_key(|(_, queued)| (queued.frame.id().priority(), queued.sequence))
            .map(|(index, _)| index)?;
        let queued = self.pending.remove(index)?;
        self.deliver(ctx, &queued.frame);
        Some(queued.frame)
    }

    fn deliver(&mut self, ctx: &mut dyn Ctx, frame: &CanFrame) {
        for receiver in &mut self.receivers {
            if receiver.accepts(&frame.id()) {
                receiver.receive(ctx, frame);
            }
        }
    }
}

impl CanController for CanBus {
    fn transmit(
        &mut self,
        ctx: &mut dyn Ctx,
        frame: CanFrame,
    ) -> Result<(), CanError> {
        self.deliver(ctx, &frame);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use mosaic_contract_model::ContextError;

    /// Receiver with an identifier allow-list and an external delivery log.
    struct FilteredReceiver {
        allowed: Vec<CanId>,
        log: Arc<Mutex<Vec<CanId>>>,
    }

    impl CanReceiver for FilteredReceiver {
        fn accepts(&self, id: &CanId) -> bool {
            self.allowed.contains(id)
        }

        fn receive(&mut self, _ctx: &mut dyn Ctx, frame: &CanFrame) {
            self.log
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(frame.id());
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

    fn frame(id: u16, data: &[u8]) -> CanFrame {
        CanFrame::new(CanId::base(id).unwrap(), data, false).unwrap()
    }

    #[test]
    fn arbitration_releases_the_lowest_identifier_first() {
        let mut bus = CanBus::new(8);
        bus.enqueue(frame(0x300, &[1])).unwrap();
        bus.enqueue(frame(0x100, &[2])).unwrap();
        bus.enqueue(frame(0x200, &[3])).unwrap();
        let first = bus.arbitrate_one(&mut DummyCtx).unwrap();
        let second = bus.arbitrate_one(&mut DummyCtx).unwrap();
        let third = bus.arbitrate_one(&mut DummyCtx).unwrap();
        assert_eq!(first.id(), CanId::Base(0x100));
        assert_eq!(second.id(), CanId::Base(0x200));
        assert_eq!(third.id(), CanId::Base(0x300));
        assert!(bus.arbitrate_one(&mut DummyCtx).is_none());
    }

    #[test]
    fn only_accepting_receivers_see_a_frame() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut bus = CanBus::new(4);
        bus.add_receiver(Box::new(FilteredReceiver {
            allowed: vec![CanId::Base(0x100)],
            log: Arc::clone(&log),
        }));
        bus.transmit(&mut DummyCtx, frame(0x100, &[9])).unwrap();
        bus.transmit(&mut DummyCtx, frame(0x200, &[9])).unwrap();
        assert_eq!(
            log.lock().unwrap_or_else(|e| e.into_inner()).as_slice(),
            &[CanId::Base(0x100)]
        );
    }

    #[test]
    fn the_queue_is_bounded() {
        let mut bus = CanBus::new(1);
        bus.enqueue(frame(0x1, &[])).unwrap();
        assert_eq!(
            bus.enqueue(frame(0x2, &[])),
            Err(CanError::Full)
        );
    }
}
