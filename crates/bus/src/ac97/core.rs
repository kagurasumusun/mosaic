//! `ac97` bus entity (ADR 0025): AC'97 audio codec bus (Intel): AC-Link 20-bit frames.

use mosaic_contract::ac97::{Ac97Error, Ac97Initiator, Ac97Target};
use mosaic_contract::model::Ctx;
use std::collections::BTreeMap;

pub struct Ac97Bus {
    codecs: BTreeMap<u8, Box<dyn Ac97Target>>,
}

impl Default for Ac97Bus {
    fn default() -> Self {
        Self::new()
    }
}

impl Ac97Bus {
    pub fn new() -> Self {
        Self {
            codecs: BTreeMap::new(),
        }
    }
    pub fn add_codec(&mut self, t: Box<dyn Ac97Target>) {
        self.codecs.insert(t.codec_id(), t);
    }
    fn codec(&mut self, id: u8) -> Result<&mut Box<dyn Ac97Target>, Ac97Error> {
        self.codecs.get_mut(&id).ok_or(Ac97Error::NoCodec)
    }
}

impl Ac97Initiator for Ac97Bus {
    fn read_register(
        &mut self,
        ctx: &mut dyn Ctx,
        codec_id: u8,
        reg: u8,
    ) -> Result<u16, Ac97Error> {
        self.codec(codec_id)?.read_register(ctx, reg)
    }
    fn write_register(
        &mut self,
        ctx: &mut dyn Ctx,
        codec_id: u8,
        reg: u8,
        value: u16,
    ) -> Result<(), Ac97Error> {
        self.codec(codec_id)?.write_register(ctx, reg, value)
    }
    fn stream(&mut self, ctx: &mut dyn Ctx, pcm_out: &[u16]) -> Result<Vec<u16>, Ac97Error> {
        if self.codecs.is_empty() {
            return Err(Ac97Error::NoCodec);
        }
        let first = self.codecs.iter_mut().next().map(|(_, v)| v).unwrap();
        first.stream(ctx, pcm_out)
    }
}
