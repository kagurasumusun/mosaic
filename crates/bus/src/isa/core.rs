//! `isa` bus entity (ADR 0029): classic 8/16-bit ISA I/O port cycles.

use mosaic_contract::isa::{IsaCard, IsaError, IsaHost};
use mosaic_contract::model::Ctx;

pub struct IsaBus {
    cards: Vec<Box<dyn IsaCard>>,
}

impl Default for IsaBus {
    fn default() -> Self {
        Self::new()
    }
}

impl IsaBus {
    pub fn new() -> Self {
        Self { cards: Vec::new() }
    }
    pub fn attach_card(&mut self, c: Box<dyn IsaCard>) {
        self.cards.push(c);
    }
}

impl IsaHost for IsaBus {
    fn io_read(&mut self, ctx: &mut dyn Ctx, port: u16, width: u8) -> Result<u64, IsaError> {
        if self.cards.is_empty() {
            return Err(IsaError::NoCard);
        }
        if width == 0 || width > 4 {
            return Err(IsaError::BadWidth);
        }
        for card in self.cards.iter_mut() {
            if card.claim_io(port) {
                return card.io_read(ctx, port, width);
            }
        }
        Err(IsaError::IoFault)
    }
    fn io_write(
        &mut self,
        ctx: &mut dyn Ctx,
        port: u16,
        width: u8,
        value: u64,
    ) -> Result<(), IsaError> {
        if self.cards.is_empty() {
            return Err(IsaError::NoCard);
        }
        if width == 0 || width > 4 {
            return Err(IsaError::BadWidth);
        }
        for card in self.cards.iter_mut() {
            if card.claim_io(port) {
                return card.io_write(ctx, port, width, value);
            }
        }
        Err(IsaError::IoFault)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::isa::IsaCard;

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

    struct SerialCard {
        base: u16,
    }
    impl IsaCard for SerialCard {
        fn claim_io(&self, port: u16) -> bool {
            (0x3F8..0x400).contains(&port) && port >= self.base
        }
        fn io_read(&mut self, _ctx: &mut dyn Ctx, port: u16, _width: u8) -> Result<u64, IsaError> {
            Ok(port as u64)
        }
        fn io_write(
            &mut self,
            _ctx: &mut dyn Ctx,
            _port: u16,
            _width: u8,
            _value: u64,
        ) -> Result<(), IsaError> {
            Ok(())
        }
    }

    #[test]
    fn claimed_port_reaches_card() {
        let mut bus = IsaBus::new();
        bus.attach_card(Box::new(SerialCard { base: 0x3F8 }));
        let mut ctx = NoopCtx;
        assert_eq!(bus.io_read(&mut ctx, 0x3F8, 1).unwrap(), 0x3F8);
    }

    #[test]
    fn unclaimed_port_is_an_io_fault() {
        let mut bus = IsaBus::new();
        bus.attach_card(Box::new(SerialCard { base: 0x3F8 }));
        let mut ctx = NoopCtx;
        assert_eq!(bus.io_read(&mut ctx, 0x200, 1), Err(IsaError::IoFault));
    }

    #[test]
    fn empty_bus_has_no_cards() {
        let mut bus = IsaBus::new();
        let mut ctx = NoopCtx;
        assert_eq!(bus.io_read(&mut ctx, 0x3F8, 1), Err(IsaError::NoCard));
    }
}
