//! `gpib` bus entity (ADR 0029): IEEE-488 instrument bus.

use mosaic_contract::gpib::{GpibController, GpibDevice, GpibError};
use mosaic_contract::model::Ctx;

pub struct GpibBus {
    devices: Vec<Box<dyn GpibDevice>>,
}

impl Default for GpibBus {
    fn default() -> Self {
        Self::new()
    }
}

impl GpibBus {
    pub fn new() -> Self {
        Self {
            devices: Vec::new(),
        }
    }
    pub fn attach_device(&mut self, d: Box<dyn GpibDevice>) {
        self.devices.push(d);
    }
    fn find(&mut self, pad: u8) -> Result<&mut dyn GpibDevice, GpibError> {
        self.devices
            .iter_mut()
            .map(|d| d.as_mut() as &mut dyn GpibDevice)
            .find(|d| d.address() == pad)
            .ok_or(GpibError::NoListener)
    }
}

impl GpibController for GpibBus {
    fn send_command(&mut self, ctx: &mut dyn Ctx, command: u8) -> Result<(), GpibError> {
        if self.devices.is_empty() {
            return Err(GpibError::NoListener);
        }
        for device in self.devices.iter_mut() {
            device.on_command(ctx, command)?;
        }
        Ok(())
    }
    fn write(&mut self, ctx: &mut dyn Ctx, pad: u8, data: &[u8]) -> Result<(), GpibError> {
        if pad > 30 {
            return Err(GpibError::NoListener);
        }
        self.find(pad)?.on_write(ctx, data)
    }
    fn read(&mut self, ctx: &mut dyn Ctx, pad: u8, budget: usize) -> Result<Vec<u8>, GpibError> {
        if pad > 30 {
            return Err(GpibError::NotTalker);
        }
        self.find(pad)?.read(ctx, budget)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::gpib::GpibDevice;

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

    struct Multimeter {
        pad: u8,
        readings: Vec<u8>,
    }
    impl GpibDevice for Multimeter {
        fn address(&self) -> u8 {
            self.pad
        }
        fn on_command(&mut self, _ctx: &mut dyn Ctx, _command: u8) -> Result<(), GpibError> {
            Ok(())
        }
        fn on_write(&mut self, _ctx: &mut dyn Ctx, _data: &[u8]) -> Result<(), GpibError> {
            Ok(())
        }
        fn read(&mut self, _ctx: &mut dyn Ctx, budget: usize) -> Result<Vec<u8>, GpibError> {
            Ok(self.readings.iter().copied().take(budget).collect())
        }
    }

    #[test]
    fn read_reaches_the_addressed_instrument() {
        let mut bus = GpibBus::new();
        bus.attach_device(Box::new(Multimeter {
            pad: 22,
            readings: b"+1.234E+01\n".to_vec(),
        }));
        let mut ctx = NoopCtx;
        let bytes = bus.read(&mut ctx, 22, 11).unwrap();
        assert_eq!(bytes, b"+1.234E+01\n".to_vec());
    }

    #[test]
    fn unassigned_primary_address_is_no_listener() {
        let mut bus = GpibBus::new();
        bus.attach_device(Box::new(Multimeter {
            pad: 22,
            readings: vec![],
        }));
        let mut ctx = NoopCtx;
        assert_eq!(bus.write(&mut ctx, 5, b"*IDN?"), Err(GpibError::NoListener));
    }

    #[test]
    fn broadcast_command_reaches_every_instrument() {
        let mut bus = GpibBus::new();
        bus.attach_device(Box::new(Multimeter {
            pad: 1,
            readings: vec![],
        }));
        bus.attach_device(Box::new(Multimeter {
            pad: 2,
            readings: vec![],
        }));
        let mut ctx = NoopCtx;
        assert!(bus.send_command(&mut ctx, 0x3F).is_ok());
    }
}
