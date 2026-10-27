//! `hda` contract: Intel High Definition Audio (HDA): CORB/RIRB command ring.

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum HdaError {
    NoCodec,
    BadCodec,
    BadNode,
    BadVerb,
    RirbTimeout,
    Overflow,
}

/// 4-bit codec address + 20-bit verb (12-bit node + 8-bit payload).
pub fn hda_verb(codec: u8, node: u8, payload: u8) -> u32 {
    ((codec as u32 & 0xF) << 28) | ((node as u32 & 0xFFF) << 8) | (payload as u32 & 0xFF)
}

pub trait HdaInitiator {
    fn send_verb(&mut self, ctx: &mut dyn Ctx, verb: u32) -> Result<u32, HdaError>;
    fn stream(&mut self, ctx: &mut dyn Ctx, codec: u8, data: &[u8]) -> Result<Vec<u8>, HdaError>;
}

pub trait HdaTarget {
    fn codec_address(&self) -> u8;
    fn send_verb(&mut self, ctx: &mut dyn Ctx, node: u8, payload: u8) -> Result<u32, HdaError>;
    fn stream(&mut self, ctx: &mut dyn Ctx, data: &[u8]) -> Result<Vec<u8>, HdaError>;
}
