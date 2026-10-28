//! PS/2 keyboard and mouse devices (QEMU's `hw/input/ps2.c` role): the
//! devices behind an 8042's two channels.
//!
//! Clean-room from the IBM PS/2 keyboard and mouse interface references:
//! the keyboard's command set (`0xED` LEDs, `0xF2` identify, `0xF3`
//! typematic, `0xF4/0xF5` enable/disable, `0xFF` reset answering BAT
//! `0xAA`) with set-1 scancodes, and the mouse's 3-byte packets with
//! 1:1/2:1 scaling, resolution and sample-rate configuration, stream and
//! remote modes, and its own command set.

use std::collections::VecDeque;

use crate::ip::input::i8042::Ps2Device;

/// The PS/2 keyboard: set-1 scancodes, a small command interpreter.
#[derive(Debug, Default)]
pub struct Ps2Keyboard {
    output: VecDeque<u8>,
    enabled: bool,
    /// Whether scan codes are currently suppressed (0xF5).
    scanning: bool,
    modifiers: u8,
}

impl Ps2Keyboard {
    pub fn new() -> Self {
        Self {
            output: VecDeque::new(),
            enabled: true,
            scanning: true,
            modifiers: 0,
        }
    }

    /// Send one key event (a `make` or `break`); set-1 scancodes follow.
    pub fn send_key(&mut self, scancode_set1: u8, down: bool) {
        if !self.scanning {
            return;
        }
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
            // Break code: 0xF0 prefix in set 2; set 1 is `code | 0x80`.
            self.output.push_back(scancode_set1 | 0x80);
        }
    }

    /// Whether the modifier state has shift/ctrl (a diagnostics accessor).
    pub fn modifiers(&self) -> u8 {
        self.modifiers
    }

    /// The host wrote a byte to `60h`: it is a keyboard command.
    pub fn send_byte(&mut self, byte: u8) {
        match byte {
            0xED => self.output.push_back(0xFA), // ACK; LED bits follow
            0xF0 => self.output.push_back(0xFA), // ACK; scan set select follows
            0xF2 => {
                // Identify: ACK then two bytes.
                self.output.push_back(0xFA);
                self.output.push_back(0xAB);
                self.output.push_back(0x41);
            }
            0xF3 => self.output.push_back(0xFA), // ACK; typematic follows
            0xF4 => {
                self.scanning = true;
                self.output.push_back(0xFA);
            }
            0xF5 => {
                self.scanning = false;
                self.output.push_back(0xFA);
            }
            0xFF => {
                // Reset: ACK, BAT.
                self.scanning = true;
                self.modifiers = 0;
                self.output.push_back(0xFA);
                self.output.push_back(0xAA);
            }
            _ => {
                if self.enabled {
                    self.output.push_back(0xFA); // ACK for set bytes
                }
            }
        }
    }
}

impl Ps2Device for Ps2Keyboard {
    fn take_output(&mut self) -> Option<u8> {
        self.output.pop_front()
    }

    fn send_byte(&mut self, byte: u8) {
        Ps2Keyboard::send_byte(self, byte);
    }
}

/// The PS/2 mouse: 3-byte packets, stream/remote modes, scaling.
#[derive(Debug)]
pub struct Ps2Mouse {
    output: VecDeque<u8>,
    enabled: bool,
    scaling21: bool,
    /// Sample rate divisor the `0xF3` sequence programs.
    sample_rate: u8,
    resolution: u8,
    remote_mode: bool,
    position: (i32, i32),
    buttons: u8,
    /// Pending packet bytes assembled from the next report.
    packet: VecDeque<u8>,
}

impl Default for Ps2Mouse {
    fn default() -> Self {
        Self::new()
    }
}

impl Ps2Mouse {
    pub fn new() -> Self {
        Self {
            output: VecDeque::new(),
            enabled: false,
            scaling21: false,
            sample_rate: 100,
            resolution: 4,
            remote_mode: false,
            position: (0, 0),
            buttons: 0,
            packet: VecDeque::new(),
        }
    }

    /// Send one motion/button report (remote or stream mode alike).
    pub fn send_report(&mut self, dx: i32, dy: i32, buttons: u8) {
        self.position.0 = self.position.0.saturating_add(dx);
        self.position.1 = self.position.1.saturating_add(dy);
        self.buttons = buttons & 0x07;
        let overflow_x = self.position.0 < -256 || self.position.0 > 255;
        let overflow_y = self.position.1 < -256 || self.position.1 > 255;
        let byte0 = ((buttons & 0x07)
            | (u8::from(dx < 0) << 4)
            | (u8::from(dy < 0) << 5)
            | (u8::from(overflow_x) << 6)
            | (u8::from(overflow_y) << 7))
            | 0x08; // The always-set bit 3.
        let clamp = |value: i32, scale: bool| -> u8 {
            let scaled = if scale && value < 0 { value * 2 } else { value };
            scaled.clamp(-256, 255) as u8
        };
        self.packet.clear();
        self.packet.push_back(byte0);
        self.packet.push_back(clamp(dx, self.scaling21));
        self.packet.push_back(clamp(-dy, self.scaling21)); // Y grows upward.
        if self.remote_mode {
            // Remote mode: packets wait for 0xEB read data.
            return;
        }
        if self.enabled {
            while let Some(byte) = self.packet.pop_front() {
                self.output.push_back(byte);
            }
        }
    }

    /// The host wrote a byte to `60h`: it is a mouse command.
    pub fn send_byte(&mut self, byte: u8) {
        match byte {
            0xE6 => {
                self.scaling21 = false;
                self.output.push_back(0xFA);
            }
            0xE7 => {
                self.scaling21 = true;
                self.output.push_back(0xFA);
            }
            0xE8 => {
                self.output.push_back(0xFA); // ACK; resolution follows
            }
            0xE9 => {
                // Status: ACK, then 3 bytes (mode, resolution, sample rate).
                self.output.push_back(0xFA);
                self.output
                    .push_back(0x08 | u8::from(self.enabled) | (u8::from(self.remote_mode) << 6));
                self.output.push_back(self.resolution);
                self.output.push_back(self.sample_rate);
            }
            0xEA => {
                self.remote_mode = false;
                self.output.push_back(0xFA);
            }
            0xEB => {
                // Read data (remote mode): ACK then the pending packet.
                self.output.push_back(0xFA);
                while let Some(byte) = self.packet.pop_front() {
                    self.output.push_back(byte);
                }
            }
            0xEC => {
                self.scaling21 = false;
                self.output.push_back(0xFA);
            }
            0xF0 => {
                self.remote_mode = true;
                self.output.push_back(0xFA);
            }
            0xF2 => {
                self.output.push_back(0xFA);
                self.output.push_back(0x00);
                self.output.push_back(0x00);
            }
            0xF3 => self.output.push_back(0xFA), // ACK; sample rate follows
            0xF4 => {
                self.enabled = true;
                self.output.push_back(0xFA);
            }
            0xF5 => {
                self.enabled = false;
                self.output.push_back(0xFA);
            }
            0xF6 => {
                // Set defaults.
                self.enabled = false;
                self.scaling21 = false;
                self.sample_rate = 100;
                self.resolution = 4;
                self.remote_mode = false;
                self.output.push_back(0xFA);
            }
            0xFF => {
                self.enabled = false;
                self.scaling21 = false;
                self.remote_mode = false;
                self.output.push_back(0xFA);
                self.output.push_back(0xAA);
                self.output.push_back(0x00);
            }
            _ => self.output.push_back(0xFA),
        }
    }
}

impl Ps2Device for Ps2Mouse {
    fn take_output(&mut self) -> Option<u8> {
        if let Some(byte) = self.output.pop_front() {
            return Some(byte);
        }
        // Remote-mode packets wait for 0xEB: do not consume on rejection.
        if self.enabled && !self.remote_mode {
            self.packet.pop_front()
        } else {
            None
        }
    }

    fn send_byte(&mut self, byte: u8) {
        Ps2Mouse::send_byte(self, byte);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn make_and_break_use_set1_codes() {
        let mut keyboard = Ps2Keyboard::new();
        keyboard.send_key(0x1C, true);
        keyboard.send_key(0x1C, false);
        assert_eq!(keyboard.take_output(), Some(0x1C));
        assert_eq!(keyboard.take_output(), Some(0x9C));
    }

    #[test]
    fn reset_answers_the_bat() {
        let mut keyboard = Ps2Keyboard::new();
        keyboard.send_byte(0xFF);
        assert_eq!(keyboard.take_output(), Some(0xFA));
        assert_eq!(keyboard.take_output(), Some(0xAA));
    }

    #[test]
    fn a_motion_report_becomes_three_bytes() {
        let mut mouse = Ps2Mouse::new();
        mouse.send_byte(0xF4); // enable (streaming)
        assert_eq!(mouse.take_output(), Some(0xFA));
        mouse.send_report(10, -5, 0x01);
        let byte0 = mouse.take_output().unwrap();
        let dx = mouse.take_output().unwrap();
        let dy = mouse.take_output().unwrap();
        assert_eq!(byte0 & 0x07, 0x01); // left button
        assert_eq!(dx, 10);
        assert_eq!(dy, 5); // inverted Y
    }

    #[test]
    fn remote_mode_holds_packets_until_read_data() {
        let mut mouse = Ps2Mouse::new();
        mouse.send_byte(0xF0); // remote mode
        assert_eq!(mouse.take_output(), Some(0xFA));
        mouse.send_report(3, 3, 0);
        assert_eq!(mouse.take_output(), None);
        mouse.send_byte(0xEB); // read data
        assert_eq!(mouse.take_output(), Some(0xFA));
        assert_eq!(mouse.take_output(), Some(0x08)); // always-set bit, no buttons
        assert_eq!(mouse.take_output(), Some(3));
    }
}
