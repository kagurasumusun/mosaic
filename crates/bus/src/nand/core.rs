//! `nand` bus entity (ADR 0029): raw NAND flash bus (CLE/ALE/RE/WE).

use mosaic_contract::model::Ctx;
use mosaic_contract::nand::{NandDevice, NandError, NandHost, NandOp};

pub struct NandBus {
    device: Option<Box<dyn NandDevice>>,
}

impl Default for NandBus {
    fn default() -> Self {
        Self::new()
    }
}

impl NandBus {
    pub fn new() -> Self {
        Self { device: None }
    }
    pub fn attach_device(&mut self, d: Box<dyn NandDevice>) {
        self.device = Some(d);
    }
}

impl NandHost for NandBus {
    fn execute(&mut self, ctx: &mut dyn Ctx, op: &NandOp) -> Result<Vec<u8>, NandError> {
        match self.device.as_deref_mut() {
            Some(d) => d.on_command(ctx, op),
            None => Err(NandError::NoDevice),
        }
    }
    fn ready(&mut self, _ctx: &mut dyn Ctx) -> Result<bool, NandError> {
        match self.device.as_deref() {
            Some(d) => Ok(d.ready()),
            None => Err(NandError::NoDevice),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::nand::NandDevice;

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

    struct Flash {
        page: Vec<u8>,
    }
    impl NandDevice for Flash {
        fn on_command(&mut self, _ctx: &mut dyn Ctx, op: &NandOp) -> Result<Vec<u8>, NandError> {
            if op.command == 0x00 {
                return Ok(self.page.clone());
            }
            if op.command == 0xFF {
                return Ok(vec![0x00]);
            }
            Err(NandError::BadCommand)
        }
        fn ready(&self) -> bool {
            true
        }
    }

    #[test]
    fn read_page_reaches_device() {
        let mut bus = NandBus::new();
        bus.attach_device(Box::new(Flash {
            page: vec![0xDE, 0xAD],
        }));
        let mut ctx = NoopCtx;
        let op = NandOp {
            command: 0x00,
            address: vec![0x00],
            data: vec![],
        };
        assert_eq!(bus.execute(&mut ctx, &op).unwrap(), vec![0xDE, 0xAD]);
        assert!(bus.ready(&mut ctx).unwrap());
    }

    #[test]
    fn no_device_yields_error() {
        let mut bus = NandBus::new();
        let mut ctx = NoopCtx;
        assert_eq!(
            bus.execute(&mut ctx, &NandOp::default()),
            Err(NandError::NoDevice)
        );
    }

    #[test]
    fn unknown_command_is_rejected() {
        let mut bus = NandBus::new();
        bus.attach_device(Box::new(Flash { page: vec![] }));
        let mut ctx = NoopCtx;
        let op = NandOp {
            command: 0x99,
            address: vec![],
            data: vec![],
        };
        assert_eq!(bus.execute(&mut ctx, &op), Err(NandError::BadCommand));
    }
}
