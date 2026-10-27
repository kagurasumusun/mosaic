//! `pcmcia` bus entity (ADR 0029): 16-bit PC Card bus.

use mosaic_contract::model::Ctx;
use mosaic_contract::pcmcia::{PcmciaCard, PcmciaError, PcmciaHost};

pub struct PcmciaBus {
    card: Option<Box<dyn PcmciaCard>>,
}

impl Default for PcmciaBus {
    fn default() -> Self {
        Self::new()
    }
}

impl PcmciaBus {
    pub fn new() -> Self {
        Self { card: None }
    }
    pub fn attach_card(&mut self, c: Box<dyn PcmciaCard>) {
        self.card = Some(c);
    }
}

impl PcmciaHost for PcmciaBus {
    fn attribute_read(&mut self, ctx: &mut dyn Ctx, offset: u32) -> Result<u16, PcmciaError> {
        match self.card.as_deref_mut() {
            Some(c) => c.attribute_read(ctx, offset),
            None => Err(PcmciaError::NoCard),
        }
    }
    fn attribute_write(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u32,
        value: u16,
    ) -> Result<(), PcmciaError> {
        match self.card.as_deref_mut() {
            Some(c) => c.attribute_write(ctx, offset, value),
            None => Err(PcmciaError::NoCard),
        }
    }
    fn io_read(&mut self, ctx: &mut dyn Ctx, offset: u32) -> Result<u8, PcmciaError> {
        match self.card.as_deref_mut() {
            Some(c) => c.io_read(ctx, offset),
            None => Err(PcmciaError::NoCard),
        }
    }
    fn io_write(&mut self, ctx: &mut dyn Ctx, offset: u32, value: u8) -> Result<(), PcmciaError> {
        match self.card.as_deref_mut() {
            Some(c) => c.io_write(ctx, offset, value),
            None => Err(PcmciaError::NoCard),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::pcmcia::PcmciaCard;

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

    struct SramCard;
    impl PcmciaCard for SramCard {
        fn attribute_read(&mut self, _ctx: &mut dyn Ctx, _offset: u32) -> Result<u16, PcmciaError> {
            Ok(0x0001)
        }
        fn attribute_write(
            &mut self,
            _ctx: &mut dyn Ctx,
            _offset: u32,
            _value: u16,
        ) -> Result<(), PcmciaError> {
            Ok(())
        }
        fn io_read(&mut self, _ctx: &mut dyn Ctx, offset: u32) -> Result<u8, PcmciaError> {
            Ok(offset as u8)
        }
        fn io_write(
            &mut self,
            _ctx: &mut dyn Ctx,
            _offset: u32,
            _value: u8,
        ) -> Result<(), PcmciaError> {
            Ok(())
        }
    }

    #[test]
    fn cis_read_reaches_card() {
        let mut bus = PcmciaBus::new();
        bus.attach_card(Box::new(SramCard));
        let mut ctx = NoopCtx;
        assert_eq!(bus.attribute_read(&mut ctx, 0).unwrap(), 0x0001);
        assert_eq!(bus.io_read(&mut ctx, 0x42).unwrap(), 0x42);
    }

    #[test]
    fn empty_slot_yields_no_card() {
        let mut bus = PcmciaBus::new();
        let mut ctx = NoopCtx;
        assert_eq!(bus.io_read(&mut ctx, 0), Err(PcmciaError::NoCard));
    }
}
