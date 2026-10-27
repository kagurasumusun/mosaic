//! `hyperbus` bus entity (ADR 0029): HyperFlash/HyperRAM 16-bit DDR bus.

use mosaic_contract::hyperbus::{HyperbusDevice, HyperbusError, HyperbusHost};
use mosaic_contract::model::Ctx;

pub struct Hyperbus {
    device: Option<Box<dyn HyperbusDevice>>,
}

impl Default for Hyperbus {
    fn default() -> Self {
        Self::new()
    }
}

impl Hyperbus {
    pub fn new() -> Self {
        Self { device: None }
    }
    pub fn attach_device(&mut self, d: Box<dyn HyperbusDevice>) {
        self.device = Some(d);
    }
}

impl HyperbusHost for Hyperbus {
    fn read(
        &mut self,
        ctx: &mut dyn Ctx,
        address: u64,
        words: usize,
    ) -> Result<Vec<u16>, HyperbusError> {
        match self.device.as_deref_mut() {
            Some(d) => d.on_read(ctx, address, words),
            None => Err(HyperbusError::NoDevice),
        }
    }
    fn write(
        &mut self,
        ctx: &mut dyn Ctx,
        address: u64,
        words: &[u16],
    ) -> Result<(), HyperbusError> {
        match self.device.as_deref_mut() {
            Some(d) => d.on_write(ctx, address, words),
            None => Err(HyperbusError::NoDevice),
        }
    }
    fn register_read(&mut self, ctx: &mut dyn Ctx, register: u16) -> Result<u16, HyperbusError> {
        match self.device.as_deref_mut() {
            Some(d) => d.on_register_read(ctx, register),
            None => Err(HyperbusError::NoDevice),
        }
    }
    fn register_write(
        &mut self,
        ctx: &mut dyn Ctx,
        register: u16,
        value: u16,
    ) -> Result<(), HyperbusError> {
        match self.device.as_deref_mut() {
            Some(d) => d.on_register_write(ctx, register, value),
            None => Err(HyperbusError::NoDevice),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::hyperbus::HyperbusDevice;

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

    struct HyperRam;
    impl HyperbusDevice for HyperRam {
        fn on_read(
            &mut self,
            _ctx: &mut dyn Ctx,
            address: u64,
            words: usize,
        ) -> Result<Vec<u16>, HyperbusError> {
            Ok(vec![(address as u16) | 1; words.max(1)])
        }
        fn on_write(
            &mut self,
            _ctx: &mut dyn Ctx,
            _address: u64,
            _words: &[u16],
        ) -> Result<(), HyperbusError> {
            Ok(())
        }
        fn on_register_read(
            &mut self,
            _ctx: &mut dyn Ctx,
            register: u16,
        ) -> Result<u16, HyperbusError> {
            Ok(register)
        }
        fn on_register_write(
            &mut self,
            _ctx: &mut dyn Ctx,
            _register: u16,
            _value: u16,
        ) -> Result<(), HyperbusError> {
            Ok(())
        }
    }

    #[test]
    fn reads_reach_the_device() {
        let mut bus = Hyperbus::new();
        bus.attach_device(Box::new(HyperRam));
        let mut ctx = NoopCtx;
        assert_eq!(bus.read(&mut ctx, 0x10, 2).unwrap(), vec![0x11, 0x11]);
        assert_eq!(bus.register_read(&mut ctx, 0x0000).unwrap(), 0x0000);
    }

    #[test]
    fn no_device_yields_error() {
        let mut bus = Hyperbus::new();
        let mut ctx = NoopCtx;
        assert_eq!(bus.read(&mut ctx, 0, 1), Err(HyperbusError::NoDevice));
    }
}
