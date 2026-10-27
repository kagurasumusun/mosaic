//! PS/2 keyboard functional core — `contract::ps2::Ps2Device` adapter.
//!
//! One shared core behind every PS/2 keyboard part (IBM Model M, IBM
//! SpaceSaver, Microsoft Natural 4000, Logitech Deluxe 250, Sun Type 6,
//! etc.). Set-1 (XT) scancodes by default, with the standard command
//! interpreter (LEDs / identify / typematic / enable-disable / reset → BAT).
//!
//! The contract-level `Ps2Device` trait is what `bus::ps2::Ps2Bus` calls;
//! the existing `hw/input/ps2.rs` `Ps2Keyboard` speaks the `i8042::Ps2Device`
//! trait directly. The two are kept separate so the i8042 controller can
//! own its own device pair without going through a bus entity, while board
//! manifests that wire a keyboard onto a `bus::ps2::Ps2Bus` use this core
//! via the contract trait.

use std::collections::VecDeque;

use mosaic_contract::model::Ctx;
use mosaic_contract::ps2::{Ps2Device, Ps2Error};

/// A PS/2 keyboard on a `bus::ps2::Ps2Bus`.
#[derive(Debug)]
pub struct Ps2KeyboardCore {
    output: VecDeque<u8>,
    enabled: bool,
    scanning: bool,
    modifiers: u8,
    /// Identify bytes returned by `0xF2`. Default = standard AT keyboard
    /// (`0xAB 0x41`); variant keyboards override via the builder.
    identify: [u8; 2],
}

impl Default for Ps2KeyboardCore {
    fn default() -> Self {
        Self {
            output: VecDeque::new(),
            enabled: true,
            scanning: true,
            modifiers: 0,
            identify: [0xAB, 0x41],
        }
    }
}

impl Ps2KeyboardCore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Override the identify bytes returned by `0xF2` (e.g. Sun Type 6
    /// returns `0xAB 0x81` for a Sun keyboard).
    pub fn with_identify(mut self, b0: u8, b1: u8) -> Self {
        self.identify = [b0, b1];
        self
    }

    /// Push a key event: the make/break scancode (set 1) is enqueued.
    pub fn key_event(&mut self, scancode_set1: u8, down: bool) {
        match (scancode_set1, down) {
            (0x2A, true) | (0x36, true) => self.modifiers |= 0x02, // shift
            (0x2A, false) | (0x36, false) => self.modifiers &= !0x02,
            (0x1D, true) => self.modifiers |= 0x01, // ctrl
            (0x1D, false) => self.modifiers &= !0x01,
            _ => {}
        }
        if down {
            self.output.push_back(scancode_set1);
        } else {
            self.output.push_back(scancode_set1 | 0x80);
        }
    }

    pub fn modifiers(&self) -> u8 {
        self.modifiers
    }

    /// Host wrote a byte to the keyboard (command).
    pub fn host_send(&mut self, byte: u8) {
        match byte {
            0xED => self.output.push_back(0xFA), // LEDs: ACK; bits follow
            0xF0 => self.output.push_back(0xFA), // Scan set select: ACK
            0xF2 => {
                // Identify: ACK + two bytes.
                self.output.push_back(0xFA);
                self.output.push_back(self.identify[0]);
                self.output.push_back(self.identify[1]);
            }
            0xF3 => self.output.push_back(0xFA), // typematic: ACK; rate follows
            0xF4 => {
                self.scanning = true;
                self.output.push_back(0xFA);
            }
            0xF5 => {
                self.scanning = false;
                self.output.push_back(0xFA);
            }
            0xFF => {
                self.scanning = true;
                self.modifiers = 0;
                self.output.push_back(0xFA);
                self.output.push_back(0xAA); // BAT OK
            }
            _ => {
                if self.enabled {
                    self.output.push_back(0xFA);
                }
            }
        }
    }
}

impl Ps2Device for Ps2KeyboardCore {
    fn on_recv(&mut self, _ctx: &mut dyn Ctx, byte: u8) -> Result<(), Ps2Error> {
        self.host_send(byte);
        Ok(())
    }

    fn poll(&mut self, _ctx: &mut dyn Ctx) -> Option<u8> {
        self.output.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn make_break_pair_round_trips() {
        let mut kb = Ps2KeyboardCore::new();
        kb.key_event(0x1C, true); // 'A' make
        kb.key_event(0x1C, false); // 'A' break
        let mut ctx = NoopCtx;
        assert_eq!(kb.poll(&mut ctx), Some(0x1C));
        assert_eq!(kb.poll(&mut ctx), Some(0x9C));
        assert_eq!(kb.poll(&mut ctx), None);
    }

    #[test]
    fn ctrl_modifier_toggled() {
        let mut kb = Ps2KeyboardCore::new();
        kb.key_event(0x1D, true);
        assert_eq!(kb.modifiers(), 0x01);
        kb.key_event(0x1D, false);
        assert_eq!(kb.modifiers(), 0x00);
    }

    #[test]
    fn reset_emits_ack_then_bat() {
        let mut kb = Ps2KeyboardCore::new();
        let mut ctx = NoopCtx;
        kb.on_recv(&mut ctx, 0xFF).unwrap();
        assert_eq!(kb.poll(&mut ctx), Some(0xFA));
        assert_eq!(kb.poll(&mut ctx), Some(0xAA));
    }

    #[test]
    fn identify_returns_default_ab41() {
        let mut kb = Ps2KeyboardCore::new();
        let mut ctx = NoopCtx;
        kb.on_recv(&mut ctx, 0xF2).unwrap();
        assert_eq!(kb.poll(&mut ctx), Some(0xFA));
        assert_eq!(kb.poll(&mut ctx), Some(0xAB));
        assert_eq!(kb.poll(&mut ctx), Some(0x41));
    }

    #[test]
    fn identify_overridable_for_sun_type6() {
        let mut kb = Ps2KeyboardCore::new().with_identify(0xAB, 0x81);
        let mut ctx = NoopCtx;
        kb.on_recv(&mut ctx, 0xF2).unwrap();
        assert_eq!(kb.poll(&mut ctx), Some(0xFA));
        assert_eq!(kb.poll(&mut ctx), Some(0xAB));
        assert_eq!(kb.poll(&mut ctx), Some(0x81));
    }
}
