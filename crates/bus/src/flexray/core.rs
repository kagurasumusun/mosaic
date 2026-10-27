//! `flexray` bus entity (ADR 0025): FlexRay communication system (TDMA dual-channel).

use mosaic_contract::flexray::{FlexRayError, FlexRayInitiator, FlexRayTarget, FrChannel};
use mosaic_contract::model::Ctx;
use std::collections::BTreeMap;

pub struct FlexRayBus {
    nodes: BTreeMap<u8, Box<dyn FlexRayTarget>>,
}

impl Default for FlexRayBus {
    fn default() -> Self {
        Self::new()
    }
}

impl FlexRayBus {
    pub fn new() -> Self {
        Self {
            nodes: BTreeMap::new(),
        }
    }
    pub fn add_node(&mut self, t: Box<dyn FlexRayTarget>) {
        self.nodes.insert(t.node_id(), t);
    }
    fn owner(
        &mut self,
        ch: FrChannel,
        slot: u16,
    ) -> Result<&mut Box<dyn FlexRayTarget>, FlexRayError> {
        self.nodes
            .values_mut()
            .find(|n| n.accepts(ch, slot))
            .ok_or(FlexRayError::NoNode)
    }
}

impl FlexRayInitiator for FlexRayBus {
    fn send(
        &mut self,
        ctx: &mut dyn Ctx,
        ch: FrChannel,
        slot: u16,
        payload: &[u8],
    ) -> Result<(), FlexRayError> {
        self.owner(ch, slot)?.send(ctx, ch, slot, payload)
    }
    fn recv(
        &mut self,
        ctx: &mut dyn Ctx,
        ch: FrChannel,
        slot: u16,
    ) -> Result<Vec<u8>, FlexRayError> {
        self.owner(ch, slot)?.recv(ctx, ch, slot)
    }
}
