//! `lin` bus entity (ADR 0025): LIN (Local Interconnect Network): single-master framed bus.

use std::collections::BTreeMap;
use mosaic_contract::lin::{LinError, LinInitiator, LinTarget};
use mosaic_contract::model::Ctx;

pub struct LinBus { responders: BTreeMap<u8, Box<dyn LinTarget>> }

impl Default for LinBus { fn default() -> Self { Self::new() } }

impl LinBus {
    pub fn new() -> Self { Self { responders: BTreeMap::new() } }
    pub fn add_responder(&mut self, t: Box<dyn LinTarget>) { self.responders.insert(t.pid(), t); }
}

impl LinInitiator for LinBus {
    fn send_header(&mut self, _ctx: &mut dyn Ctx, _id: u8) -> Result<(), LinError> { Ok(()) }
    fn exchange(&mut self, ctx: &mut dyn Ctx, id: u8, write: &mut [u8]) -> Result<usize, LinError> {
        self.responders.get_mut(&id).ok_or(LinError::NoResponder)?.exchange(ctx, id, write)
    }
}

/// LIN protected identifier (PID): id[5:0] with parity bits P0/P1 in [7:6].
pub fn pid(id: u8) -> u8 {
    let p0 = ((id >> 0) ^ (id >> 1) ^ (id >> 2) ^ (id >> 4)) & 1;
    let p1 = !((id >> 1) ^ (id >> 3) ^ (id >> 4) ^ (id >> 5)) & 1;
    (id & 0x3F) | (p0 << 6) | (p1 << 7)
}

/// LIN 2.x enhanced checksum (includes PID).
pub fn checksum(pid: u8, data: &[u8]) -> u8 {
    let mut sum: u16 = pid as u16;
    for &b in data { sum = sum.wrapping_add(b as u16); }
    let mut inv = !sum as u8;
    inv
}
