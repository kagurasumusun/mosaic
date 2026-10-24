//! Machine-owned device arena with take-out callbacks.
//!
//! Devices are indexed by [`NodeId`] and never hold references to each other.
//! While an MMIO callback runs, its target is removed from the arena; any
//! nested access to the same node (for example future DMA re-entrancy) finds
//! an empty slot and is rejected instead of aliasing the borrowed device.
//! This replaces `Rc<RefCell<_>>` with structural borrow checking, per the
//! ownership design in the configuration document.

use std::collections::BTreeMap;

use mosaic_core_types::NodeId;

use crate::machine::{MachineDevice, MachineError};

#[derive(Default)]
pub struct DeviceArena {
    /// `None` means the node is registered but currently taken out.
    nodes: BTreeMap<u32, Option<Box<dyn MachineDevice>>>,
}

impl DeviceArena {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(
        &mut self,
        node: NodeId,
        device: Box<dyn MachineDevice>,
    ) -> Result<(), MachineError> {
        if self.nodes.contains_key(&node.0) {
            return Err(MachineError::DuplicateNode(node));
        }
        self.nodes.insert(node.0, Some(device));
        Ok(())
    }

    /// Remove the device for the duration of a callback. `None` means the node
    /// is unknown or already taken; callers distinguish the cases with
    /// [`Self::is_registered`] to report unmapped versus reentrant access.
    pub fn take(&mut self, node: NodeId) -> Option<Box<dyn MachineDevice>> {
        self.nodes.get_mut(&node.0)?.take()
    }

    /// Return a previously taken device. Unknown node ids are ignored because
    /// callbacks only ever restore ids that the same machine registered.
    pub fn restore(&mut self, node: NodeId, device: Box<dyn MachineDevice>) {
        if let Some(slot) = self.nodes.get_mut(&node.0) {
            *slot = Some(device);
        }
    }

    /// Mutable timer facet of a registered device. `None` for an unknown node
    /// or a device currently taken by an outer callback.
    pub fn event_target(&mut self, node: NodeId) -> Option<&mut (dyn MachineDevice + 'static)> {
        self.nodes.get_mut(&node.0)?.as_deref_mut()
    }

    pub fn get(&self, node: NodeId) -> Option<&(dyn MachineDevice + 'static)> {
        self.nodes.get(&node.0)?.as_deref()
    }

    pub fn get_mut(&mut self, node: NodeId) -> Option<&mut (dyn MachineDevice + 'static)> {
        self.nodes.get_mut(&node.0)?.as_deref_mut()
    }

    pub fn node_ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.nodes.keys().copied().map(NodeId)
    }

    pub fn is_registered(&self, node: NodeId) -> bool {
        self.nodes.contains_key(&node.0)
    }

    pub fn is_available(&self, node: NodeId) -> bool {
        self.nodes.get(&node.0).is_some_and(|slot| slot.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract_mmio::MmioTarget;
    use mosaic_contract_model::{ContextError, Ctx, EventTarget};
    use mosaic_core_types::{
        AccessKind, AccessSize, Attr, BusFault, Endian, EventId, Security, SignalId, VTime,
    };

    struct Probe {
        ticks: u64,
    }

    impl MmioTarget for Probe {
        fn read(
            &mut self,
            _ctx: &mut dyn Ctx,
            offset: u64,
            _size: AccessSize,
            _attr: &Attr,
        ) -> Result<u64, BusFault> {
            Ok(offset + self.ticks)
        }

        fn write(
            &mut self,
            _ctx: &mut dyn Ctx,
            _offset: u64,
            _size: AccessSize,
            _value: u64,
            _attr: &Attr,
        ) -> Result<(), BusFault> {
            Ok(())
        }
    }

    impl EventTarget for Probe {
        fn on_timer(&mut self, _ctx: &mut dyn Ctx, token: u64) -> Result<(), ContextError> {
            self.ticks = token;
            Ok(())
        }
    }

    fn attr() -> Attr {
        Attr {
            master: NodeId(0),
            security: Security::NonSecure,
            privileged: false,
            kind: AccessKind::Data,
            exclusive: false,
            byte_enable: u8::MAX,
            endian: Endian::Little,
        }
    }

    #[test]
    fn a_taken_device_is_unavailable_until_restored() {
        let mut arena = DeviceArena::new();
        let node = NodeId(7);
        arena.insert(node, Box::new(Probe { ticks: 0 })).unwrap();
        assert!(arena.is_available(node));

        let mut device = arena.take(node).unwrap();
        assert!(arena.is_registered(node));
        assert!(!arena.is_available(node));
        assert!(arena.take(node).is_none());
        assert!(arena.event_target(node).is_none());
        assert_eq!(
            device
                .read(&mut DummyCtx, 4, AccessSize::Word, &attr())
                .unwrap(),
            4
        );

        arena.restore(node, device);
        assert!(arena.is_available(node));
        arena
            .event_target(node)
            .unwrap()
            .on_timer(&mut DummyCtx, 9)
            .unwrap();
        let mut again = arena.take(node).unwrap();
        assert_eq!(
            again
                .read(&mut DummyCtx, 1, AccessSize::Byte, &attr())
                .unwrap(),
            10
        );
    }

    #[test]
    fn unknown_nodes_are_distinct_from_taken_nodes() {
        let mut arena = DeviceArena::new();
        assert!(!arena.is_registered(NodeId(9)));
        assert!(arena.take(NodeId(9)).is_none());
        arena
            .insert(NodeId(1), Box::new(Probe { ticks: 0 }))
            .unwrap();
        assert!(arena.take(NodeId(1)).is_some());
        assert!(arena.is_registered(NodeId(1)));
        assert!(!arena.is_available(NodeId(1)));
    }

    #[test]
    fn duplicate_registration_is_rejected() {
        let mut arena = DeviceArena::new();
        let node = NodeId(1);
        arena.insert(node, Box::new(Probe { ticks: 0 })).unwrap();
        assert_eq!(
            arena.insert(node, Box::new(Probe { ticks: 0 })),
            Err(MachineError::DuplicateNode(node))
        );
        let taken = arena.take(node).unwrap();
        assert_eq!(
            arena.insert(node, Box::new(Probe { ticks: 0 })),
            Err(MachineError::DuplicateNode(node))
        );
        arena.restore(node, taken);
        assert!(arena.is_available(node));
    }

    struct DummyCtx;

    impl Ctx for DummyCtx {
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
            Err(ContextError::UnknownSignal)
        }
    }
}
