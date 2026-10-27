//! PS/2 mouse functional core — `contract::ps2::Ps2Device` adapter.
//!
//! One shared core behind every PS/2 mouse / trackpoint / touchpad leaf
//! part (Logitech PS/2 mouse, Microsoft IntelliMouse, IBM TrackPoint II,
//! ALPS GlidePoint, Synaptics touchpad, etc.). Standard Logitech-compatible
//! 3-byte packets (or 4-byte IntelliMouse extension when wheel mode is
//! enabled), stream and remote modes, 1:1 / 2:1 scaling, sample-rate and
//! resolution configuration, and the standard PS/2 mouse command set.
//!
//! Like [`ps2_keyboard_core`], this speaks the `contract::ps2::Ps2Device`
//! trait that `bus::ps2::Ps2Bus` calls; the existing `hw/input/ps2_mouse.rs`
//! speaks `i8042::Ps2Device` directly for the i8042 channel.

use std::collections::VecDeque;

use mosaic_contract::model::Ctx;
use mosaic_contract::ps2::{Ps2Device, Ps2Error};

#[derive(Debug)]
pub struct Ps2MouseCore {
    output: VecDeque<u8>,
    enabled: bool,
    scaling21: bool,
    sample_rate: u8,
    resolution: u8,
    remote_mode: bool,
    /// IntelliMouse extension state: 0 = disabled, 1 = waiting for 200,
    /// 2 = waiting for 100, 3 = waiting for 80 → 4-byte packet mode.
    wheel_step: u8,
    wheel_mode: bool,
    position: (i32, i32),
    buttons: u8,
    wheel: i8,
    /// Pending packet bytes (3 or 4 depending on `wheel_mode`).
    packet: VecDeque<u8>,
}

impl Default for Ps2MouseCore {
    fn default() -> Self {
        Self {
            output: VecDeque::new(),
            enabled: false,
            scaling21: false,
            sample_rate: 100,
            resolution: 2,
            remote_mode: false,
            wheel_step: 0,
            wheel_mode: false,
            position: (0, 0),
            buttons: 0,
            wheel: 0,
            packet: VecDeque::new(),
        }
    }
}

impl Ps2MouseCore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a movement + button report. The host will see it as the next
    /// packet (3 or 4 bytes depending on wheel mode).
    pub fn report(&mut self, dx: i32, dy: i32, buttons: u8, wheel: i8) {
        self.position.0 = dx.clamp(-256, 255);
        self.position.1 = dy.clamp(-256, 255);
        self.buttons = buttons & 0x07;
        self.wheel = wheel;
        self.assemble_packet();
    }

    fn assemble_packet(&mut self) {
        let (dx, dy) = self.position;
        let b = self.buttons;
        let byte0 = 0x08
            | (b & 0x01)
            | ((b & 0x02) << 1)
            | ((b & 0x04) << 1)
            | ((dx < 0) as u8) << 4
            | ((dy < 0) as u8) << 5
            | (self.overflow_x() as u8) << 6
            | (self.overflow_y() as u8) << 7;
        self.packet.push_back(byte0);
        self.packet.push_back(dx as i8 as u8);
        self.packet.push_back(dy as i8 as u8);
        if self.wheel_mode {
            self.packet.push_back(self.wheel as u8);
        }
    }

    fn overflow_x(&self) -> bool {
        self.position.0 < -256 || self.position.0 > 255
    }
    fn overflow_y(&self) -> bool {
        self.position.1 < -256 || self.position.1 > 255
    }

    /// Host wrote a byte (mouse command).
    pub fn host_send(&mut self, byte: u8) {
        match byte {
            0xFF => {
                // Reset: ACK + 0xAA + 0x00 (BAT OK + ID).
                self.output.push_back(0xFA);
                self.output.push_back(0xAA);
                self.output.push_back(0x00);
                self.enabled = false;
                self.wheel_mode = false;
                self.wheel_step = 0;
            }
            0xF6 => {
                // Set defaults.
                self.output.push_back(0xFA);
                self.scaling21 = false;
                self.sample_rate = 100;
                self.resolution = 2;
                self.remote_mode = false;
            }
            0xF4 => {
                self.enabled = true;
                self.output.push_back(0xFA);
            }
            0xF5 => {
                self.enabled = false;
                self.output.push_back(0xFA);
            }
            0xF3 => {
                self.output.push_back(0xFA);
                // Sample rate follows in next byte; used for the IntelliMouse
                // 200/100/80 magic sequence.
                self.wheel_step = match self.wheel_step {
                    0 => 1,
                    1 => 2,
                    2 if self.sample_rate == 200 => 3,
                    _ => 0,
                };
                if self.wheel_step == 3 && self.sample_rate == 80 {
                    self.wheel_mode = true;
                    self.wheel_step = 0;
                }
            }
            0xF2 => {
                // Identify.
                self.output.push_back(0xFA);
                self.output
                    .push_back(if self.wheel_mode { 0x03 } else { 0x00 });
            }
            0xE6 => {
                // 2:1 scaling.
                self.scaling21 = true;
                self.output.push_back(0xFA);
            }
            0xE7 => {
                self.scaling21 = false;
                self.output.push_back(0xFA);
            }
            0xE8 => {
                // Resolution follows; just ACK.
                self.output.push_back(0xFA);
            }
            _ => {
                if self.enabled {
                    self.output.push_back(0xFA);
                }
            }
        }
    }
}

impl Ps2Device for Ps2MouseCore {
    fn on_recv(&mut self, _ctx: &mut dyn Ctx, byte: u8) -> Result<(), Ps2Error> {
        self.host_send(byte);
        Ok(())
    }

    fn poll(&mut self, _ctx: &mut dyn Ctx) -> Option<u8> {
        // Drain the pending packet first, then any pending ACK/BAT bytes.
        if let Some(b) = self.packet.pop_front() {
            return Some(b);
        }
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
    fn three_byte_packet_for_basic_mouse() {
        let mut m = Ps2MouseCore::new();
        m.enabled = true;
        m.report(10, -5, 0x01, 0);
        let mut ctx = NoopCtx;
        let b0 = m.poll(&mut ctx).unwrap();
        let b1 = m.poll(&mut ctx).unwrap();
        let b2 = m.poll(&mut ctx).unwrap();
        assert_eq!(b0 & 0x01, 0x01); // left button
        assert_eq!(b1, 10);
        assert_eq!(b2, (-5i8) as u8);
        assert!(m.poll(&mut ctx).is_none());
    }

    #[test]
    fn reset_emits_ack_bat_id() {
        let mut m = Ps2MouseCore::new();
        let mut ctx = NoopCtx;
        m.on_recv(&mut ctx, 0xFF).unwrap();
        assert_eq!(m.poll(&mut ctx), Some(0xFA));
        assert_eq!(m.poll(&mut ctx), Some(0xAA));
        assert_eq!(m.poll(&mut ctx), Some(0x00));
    }

    #[test]
    fn enable_emits_ack_only() {
        let mut m = Ps2MouseCore::new();
        let mut ctx = NoopCtx;
        m.on_recv(&mut ctx, 0xF4).unwrap();
        assert_eq!(m.poll(&mut ctx), Some(0xFA));
        assert!(m.poll(&mut ctx).is_none());
    }

    #[test]
    fn identify_returns_00_in_basic_mode() {
        let mut m = Ps2MouseCore::new();
        let mut ctx = NoopCtx;
        m.on_recv(&mut ctx, 0xF2).unwrap();
        assert_eq!(m.poll(&mut ctx), Some(0xFA));
        assert_eq!(m.poll(&mut ctx), Some(0x00));
    }
}
