//! `rs485` bus entity (ADR 0025): RS-485 multi-drop serial: half-duplex with direction control.

use mosaic_contract::model::Ctx;
use mosaic_contract::rs485::{Rs485Error, Rs485Initiator, Rs485Target};
use std::collections::BTreeMap;

pub struct Rs485Bus {
    responders: BTreeMap<u8, Box<dyn Rs485Target>>,
}

impl Default for Rs485Bus {
    fn default() -> Self {
        Self::new()
    }
}

impl Rs485Bus {
    pub fn new() -> Self {
        Self {
            responders: BTreeMap::new(),
        }
    }
    pub fn add_responder(&mut self, t: Box<dyn Rs485Target>) {
        self.responders.insert(t.address(), t);
    }
}

impl Rs485Initiator for Rs485Bus {
    fn send_to(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: u8,
        payload: &[u8],
    ) -> Result<Vec<u8>, Rs485Error> {
        let resp = self
            .responders
            .get_mut(&addr)
            .ok_or(Rs485Error::NoResponder)?
            .receive(ctx, payload)?;
        resp.ok_or(Rs485Error::NoResponder)
    }
    fn broadcast(&mut self, ctx: &mut dyn Ctx, payload: &[u8]) -> Result<(), Rs485Error> {
        for t in self.responders.values_mut() {
            let _ = t.receive(ctx, payload);
        }
        Ok(())
    }
}
