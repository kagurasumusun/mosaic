//! `hda` bus entity (ADR 0025): Intel High Definition Audio (HDA): CORB/RIRB command ring.

use mosaic_contract::hda::{HdaError, HdaInitiator, HdaTarget};
use mosaic_contract::model::Ctx;
use std::collections::BTreeMap;

pub struct HdaBus {
    codecs: BTreeMap<u8, Box<dyn HdaTarget>>,
}

impl Default for HdaBus {
    fn default() -> Self {
        Self::new()
    }
}

impl HdaBus {
    pub fn new() -> Self {
        Self {
            codecs: BTreeMap::new(),
        }
    }
    pub fn add_codec(&mut self, t: Box<dyn HdaTarget>) {
        self.codecs.insert(t.codec_address(), t);
    }
}

impl HdaInitiator for HdaBus {
    fn send_verb(&mut self, ctx: &mut dyn Ctx, verb: u32) -> Result<u32, HdaError> {
        let codec = ((verb >> 28) & 0xF) as u8;
        let node = ((verb >> 8) & 0xFFF) as u8;
        let payload = (verb & 0xFF) as u8;
        self.codecs
            .get_mut(&codec)
            .ok_or(HdaError::NoCodec)?
            .send_verb(ctx, node, payload)
    }
    fn stream(&mut self, ctx: &mut dyn Ctx, codec: u8, data: &[u8]) -> Result<Vec<u8>, HdaError> {
        self.codecs
            .get_mut(&codec)
            .ok_or(HdaError::NoCodec)?
            .stream(ctx, data)
    }
}
