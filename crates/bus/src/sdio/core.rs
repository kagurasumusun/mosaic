//! `sdio` bus entity (ADR 0029): SD I/O card bus (functions 0-7).

use mosaic_contract::model::Ctx;
use mosaic_contract::sdio::{SdioCard, SdioCommand, SdioError, SdioHost};

pub struct SdioBus {
    card: Option<Box<dyn SdioCard>>,
}

impl Default for SdioBus {
    fn default() -> Self {
        Self::new()
    }
}

impl SdioBus {
    pub fn new() -> Self {
        Self { card: None }
    }
    pub fn attach_card(&mut self, c: Box<dyn SdioCard>) {
        self.card = Some(c);
    }
}

impl SdioHost for SdioBus {
    fn send_cmd(&mut self, ctx: &mut dyn Ctx, command: &SdioCommand) -> Result<Vec<u8>, SdioError> {
        match self.card.as_deref_mut() {
            Some(c) => c.on_cmd(ctx, command),
            None => Err(SdioError::NoCard),
        }
    }
    fn io_rw_direct(
        &mut self,
        ctx: &mut dyn Ctx,
        function: u8,
        address: u32,
        write: bool,
        data: u8,
    ) -> Result<u8, SdioError> {
        if function > 7 {
            return Err(SdioError::BadFunction);
        }
        match self.card.as_deref_mut() {
            Some(c) => c.io_rw_direct(ctx, function, address, write, data),
            None => Err(SdioError::NoCard),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::sdio::{SdioCard, SdioCommand};

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

    struct WifiCard;
    impl SdioCard for WifiCard {
        fn on_cmd(
            &mut self,
            _ctx: &mut dyn Ctx,
            _command: &SdioCommand,
        ) -> Result<Vec<u8>, SdioError> {
            Ok(vec![0x32, 0x00, 0x00, 0x00])
        }
        fn io_rw_direct(
            &mut self,
            _ctx: &mut dyn Ctx,
            function: u8,
            _address: u32,
            write: bool,
            data: u8,
        ) -> Result<u8, SdioError> {
            if write {
                Ok(data)
            } else {
                Ok((function as u8) | 0x80)
            }
        }
    }

    #[test]
    fn command_reaches_card() {
        let mut bus = SdioBus::new();
        bus.attach_card(Box::new(WifiCard));
        let mut ctx = NoopCtx;
        let cmd = SdioCommand { cmd: 5, arg: 0 };
        assert_eq!(bus.send_cmd(&mut ctx, &cmd).unwrap()[0], 0x32);
    }

    #[test]
    fn no_card_yields_error() {
        let mut bus = SdioBus::new();
        let mut ctx = NoopCtx;
        assert_eq!(
            bus.send_cmd(&mut ctx, &SdioCommand::default()),
            Err(SdioError::NoCard)
        );
    }

    #[test]
    fn function_above_seven_is_rejected() {
        let mut bus = SdioBus::new();
        bus.attach_card(Box::new(WifiCard));
        let mut ctx = NoopCtx;
        assert_eq!(
            bus.io_rw_direct(&mut ctx, 8, 0, false, 0),
            Err(SdioError::BadFunction)
        );
    }
}
