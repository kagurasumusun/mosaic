//! `firewire` bus entity (ADR 0025 step 3): IEEE 1394.

use mosaic_contract::firewire::{FireWireError, FireWireInitiator, FireWireNode, FireWirePacket};
use mosaic_contract::model::Ctx;

pub struct FireWireBus {
    nodes: Vec<(u16, Box<dyn FireWireNode>)>,
}

impl Default for FireWireBus {
    fn default() -> Self {
        Self::new()
    }
}

impl FireWireBus {
    pub fn new() -> Self {
        Self { nodes: Vec::new() }
    }
    pub fn attach(&mut self, node_id: u16, n: Box<dyn FireWireNode>) {
        self.nodes.push((node_id, n));
    }
}

impl FireWireInitiator for FireWireBus {
    fn send(&mut self, ctx: &mut dyn Ctx, p: &FireWirePacket) -> Result<Vec<u8>, FireWireError> {
        for (id, n) in self.nodes.iter_mut() {
            if *id == p.destination_id {
                return n.on_packet(ctx, p);
            }
        }
        Err(FireWireError::NoNode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::firewire::{FireWireNode, FireWirePacket};
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

    struct Node {
        id: u16,
        data: Vec<u8>,
    }
    impl FireWireNode for Node {
        fn on_packet(
            &mut self,
            _: &mut dyn Ctx,
            _p: &FireWirePacket,
        ) -> Result<Vec<u8>, FireWireError> {
            Ok(self.data.clone())
        }
    }

    #[test]
    fn packet_routes_to_destination() {
        let mut bus = FireWireBus::new();
        bus.attach(
            0xFFC0,
            Box::new(Node {
                id: 0xFFC0,
                data: vec![0xDE, 0xAD],
            }),
        );
        bus.attach(
            0xFFC1,
            Box::new(Node {
                id: 0xFFC1,
                data: vec![0xBE, 0xEF],
            }),
        );
        let mut ctx = NoopCtx;
        let p = FireWirePacket {
            source_id: 0xFFC1,
            destination_id: 0xFFC0,
            tlabel: 1,
            tcode: 0,
            address: 0,
            data: vec![],
        };
        assert_eq!(bus.send(&mut ctx, &p).unwrap(), vec![0xDE, 0xAD]);
    }

    #[test]
    fn unknown_destination_errors() {
        let mut bus = FireWireBus::new();
        let mut ctx = NoopCtx;
        let p = FireWirePacket {
            source_id: 0,
            destination_id: 0xFFFF,
            tlabel: 0,
            tcode: 0,
            address: 0,
            data: vec![],
        };
        assert_eq!(bus.send(&mut ctx, &p), Err(FireWireError::NoNode));
    }
}
