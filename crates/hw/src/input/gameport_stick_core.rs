//! GamePort analog joystick functional core — `contract::gameport::GamePortStick` adapter.
//!
//! One shared core behind every gameport leaf part (Microsoft SideWinder,
//! Logitech WingMan, CH Products F-16 Combatstick, ThrustMaster F-22 Pro,
//! Gravis GamePad Pro, etc.). Implements the standard 4-axis / 4-button
//! analog state plus an optional DAC channel for force-feedback or
//! programmable resistance.
//!
//! The IBM PC gameport reads each axis through an NE555-style one-shot
//! whose pulse width is proportional to a joystick potentiometer. Mosaic
//! models that as a `u16` value per axis (0..=65535), with the bus entity
//! (`bus::gameport::GamePortBus`) translating to whatever the host's
//! GamePort controller exposes (typically the 8253-style timer latch).

use mosaic_contract::gameport::{GamePortError, GamePortState, GamePortStick};
use mosaic_contract::model::Ctx;

#[derive(Debug)]
pub struct GamePortStickCore {
    state: GamePortState,
    /// DAC scratch for force-feedback / programmable resistance; 4 channels.
    dac: [u8; 4],
}

impl Default for GamePortStickCore {
    fn default() -> Self {
        Self {
            state: GamePortState {
                axis_x1: 0x8000,
                axis_y1: 0x8000,
                axis_x2: 0x8000,
                axis_y2: 0x8000,
                button1: false,
                button2: false,
                button3: false,
                button4: false,
            },
            dac: [0; 4],
        }
    }
}

impl GamePortStickCore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Override the centered (idle) axis value. Some gamepads report
    /// midscale, some report 0; this lets each leaf part set its baseline.
    pub fn with_centered(mut self, value: u16) -> Self {
        self.state.axis_x1 = value;
        self.state.axis_y1 = value;
        self.state.axis_x2 = value;
        self.state.axis_y2 = value;
        self
    }

    /// Update the stick state from the host environment (e.g. a host
    /// joystick event, or a scripted test). All four axes + four buttons
    /// are pushed atomically.
    pub fn set_state(&mut self, state: GamePortState) {
        self.state = state;
    }

    /// Update a single axis by index (0..=3 = X1/Y1/X2/Y2).
    pub fn set_axis(&mut self, index: u8, value: u16) {
        match index {
            0 => self.state.axis_x1 = value,
            1 => self.state.axis_y1 = value,
            2 => self.state.axis_x2 = value,
            3 => self.state.axis_y2 = value,
            _ => {}
        }
    }

    /// Update a single button by index (0..=3).
    pub fn set_button(&mut self, index: u8, pressed: bool) {
        match index {
            0 => self.state.button1 = pressed,
            1 => self.state.button2 = pressed,
            2 => self.state.button3 = pressed,
            3 => self.state.button4 = pressed,
            _ => {}
        }
    }

    pub fn dac(&self, channel: u8) -> u8 {
        self.dac[channel as usize % 4]
    }
}

impl GamePortStick for GamePortStickCore {
    fn state(&self) -> GamePortState {
        self.state
    }

    fn on_dac(&mut self, _ctx: &mut dyn Ctx, channel: u8, value: u8) -> Result<(), GamePortError> {
        let idx = channel as usize % 4;
        self.dac[idx] = value;
        Ok(())
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
    fn default_state_is_centered_midscale() {
        let s = GamePortStickCore::new();
        let st = s.state();
        assert_eq!(st.axis_x1, 0x8000);
        assert_eq!(st.axis_y1, 0x8000);
        assert_eq!(st.axis_x2, 0x8000);
        assert_eq!(st.axis_y2, 0x8000);
        assert!(!st.button1);
    }

    #[test]
    fn set_axis_updates_correct_lane() {
        let mut s = GamePortStickCore::new();
        s.set_axis(2, 0x1000);
        let st = s.state();
        assert_eq!(st.axis_x2, 0x1000);
    }

    #[test]
    fn set_button_toggles_state() {
        let mut s = GamePortStickCore::new();
        s.set_button(0, true);
        s.set_button(2, true);
        let st = s.state();
        assert!(st.button1);
        assert!(!st.button2);
        assert!(st.button3);
    }

    #[test]
    fn dac_round_trips() {
        let mut s = GamePortStickCore::new();
        let mut ctx = NoopCtx;
        s.on_dac(&mut ctx, 0, 0x80).unwrap();
        s.on_dac(&mut ctx, 3, 0xFF).unwrap();
        assert_eq!(s.dac(0), 0x80);
        assert_eq!(s.dac(3), 0xFF);
    }

    #[test]
    fn dac_channel_wraps_modulo_4() {
        let mut s = GamePortStickCore::new();
        let mut ctx = NoopCtx;
        s.on_dac(&mut ctx, 5, 0x42).unwrap(); // 5 % 4 = 1
        assert_eq!(s.dac(1), 0x42);
        assert_eq!(s.dac(5), 0x42); // also wraps on read
    }
}
