//! `ps2` bus entity (ADR 0025 step 3): PS/2 keyboard/mouse serial.

use mosaic_contract::model::Ctx;
use mosaic_contract::ps2::{Ps2Device, Ps2Error, Ps2Host};
use std::collections::VecDeque;

pub struct Ps2Bus {
    device: Option<Box<dyn Ps2Device>>,
    /// Bytes the device produced that the host has not pulled yet.
    rx_queue: VecDeque<u8>,
}

impl Default for Ps2Bus {
    fn default() -> Self {
        Self::new()
    }
}

impl Ps2Bus {
    pub fn new() -> Self {
        Self {
            device: None,
            rx_queue: VecDeque::new(),
        }
    }
    pub fn attach(&mut self, d: Box<dyn Ps2Device>) {
        self.device = Some(d);
    }
    pub fn pump(&mut self, ctx: &mut dyn Ctx) {
        if let Some(d) = self.device.as_deref_mut() {
            while let Some(b) = d.poll(ctx) {
                if self.rx_queue.len() >= 32 {
                    self.rx_queue.pop_front();
                }
                self.rx_queue.push_back(b);
            }
        }
    }
}

impl Ps2Host for Ps2Bus {
    fn send(&mut self, ctx: &mut dyn Ctx, byte: u8) -> Result<(), Ps2Error> {
        match self.device.as_deref_mut() {
            Some(d) => d.on_recv(ctx, byte),
            None => Err(Ps2Error::NoTarget),
        }
    }
    fn recv(&mut self, _ctx: &mut dyn Ctx) -> Result<u8, Ps2Error> {
        self.rx_queue.pop_front().ok_or(Ps2Error::NoTarget)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::model::Ctx;
    use mosaic_contract::ps2::Ps2Device;

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

    struct Keyboard {
        tx: std::collections::VecDeque<u8>,
        rx: Vec<u8>,
    }
    impl Ps2Device for Keyboard {
        fn on_recv(&mut self, _: &mut dyn Ctx, byte: u8) -> Result<(), Ps2Error> {
            self.rx.push(byte);
            Ok(())
        }
        fn poll(&mut self, _: &mut dyn Ctx) -> Option<u8> {
            self.tx.pop_front()
        }
    }

    #[test]
    fn host_recv_pulls_from_device_queue_after_pump() {
        let mut bus = Ps2Bus::new();
        let mut kb = Keyboard {
            tx: std::collections::VecDeque::new(),
            rx: vec![],
        };
        kb.tx.push_back(0x1C); // make code for 'A'
        kb.tx.push_back(0x1C); // break code prefix
        bus.attach(Box::new(kb));
        let mut ctx = NoopCtx;
        bus.pump(&mut ctx);
        assert_eq!(bus.recv(&mut ctx), Ok(0x1C));
        assert_eq!(bus.recv(&mut ctx), Ok(0x1C));
        assert_eq!(bus.recv(&mut ctx), Err(Ps2Error::NoTarget));
    }

    #[test]
    fn host_send_reaches_device() {
        let mut bus = Ps2Bus::new();
        bus.attach(Box::new(Keyboard {
            tx: std::collections::VecDeque::new(),
            rx: vec![],
        }));
        let mut ctx = NoopCtx;
        assert!(bus.send(&mut ctx, 0xFF).is_ok()); // reset
    }
}
