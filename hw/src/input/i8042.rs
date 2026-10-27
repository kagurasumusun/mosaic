//! The Intel 8042 keyboard controller (PS/2) — QEMU's `hw/input/pckbd.c`
//! role: the status/data/command port trio at `60h/64h`, the A20 gate and
//! the reset line the output port drives, and the two PS/2 channels (a
//! keyboard and an auxiliary/mouse port) whose input the controller buffers.
//!
//! Clean-room from the IBM PS/2 hardware technical reference and the
//! keyboard controller's documented command set: `0xAA` self-test answers
//! `0x55`, `0xAB` interface test answers `0x00`, `0xAD/AE` gate the
//! channels, `0xD1` writes the output port (A20 + reset), `0xD2/D3` inject
//! into the output buffer, `0xF0..0xFF` pulse output lines.

use std::collections::VecDeque;

use mosaic_contract::model::{set_optional_signal, ContextError, Ctx, EventTarget};
use mosaic_exec::{SignalId, Snapshot, SnapshotError};

/// Status register bits.
#[allow(dead_code)] // documented status bits the read path composes
mod status {
    pub const OUTPUT_FULL: u8 = 1 << 0;
    pub const INPUT_FULL: u8 = 1 << 1;
    pub const SYSTEM_FLAG: u8 = 1 << 2;
    pub const COMMAND_DATA: u8 = 1 << 3; // Last write was to 64h
    pub const TIMEOUT: u8 = 1 << 6;
    pub const PARITY: u8 = 1 << 7;
}

/// Output-port bits (`0xD1` writes): A20 gate and the reset line.
mod out_port {
    pub const A20: u8 = 1 << 1;
    pub const RESET: u8 = 1 << 0;
    pub const GATE_ALL: u8 = 0xCF; // bits the controller owns
}

/// One PS/2 device the controller buffers (a keyboard or a mouse).
pub trait Ps2Device: Send {
    /// Bytes the device has for the host, in order (None = nothing pending).
    fn take_output(&mut self) -> Option<u8>;

    /// The host wrote a byte to the device (a keyboard/mouse command).
    fn send_byte(&mut self, byte: u8);
}

/// The 8042 controller.
pub struct I8042 {
    /// The controller's combined IRQ output (IRQ1 keyboard / IRQ12 mouse
    /// arrive on the machine's PIC wiring; the model exposes one line).
    irq: SignalId,
    status: u8,
    /// The output buffer (what `IN 60h` reads).
    output: VecDeque<u8>,
    /// The last byte written to `64h`, while its parameter is pending.
    pending_command: Option<u8>,
    /// The output port: A20 + reset lines.
    output_port: u8,
    /// Whether the channels are enabled (0xAD/0xAE).
    keyboard_enabled: bool,
    aux_enabled: bool,
    /// Attached devices: keyboard then auxiliary.
    keyboard: Option<Box<dyn Ps2Device>>,
    aux: Option<Box<dyn Ps2Device>>,
    /// A20 and reset output signals the machine wires.
    a20_signal: Option<SignalId>,
    reset_signal: Option<SignalId>,
}

impl I8042 {
    pub fn new(irq: SignalId) -> Self {
        Self {
            irq,
            status: status::SYSTEM_FLAG,
            output: VecDeque::new(),
            pending_command: None,
            output_port: out_port::GATE_ALL & !out_port::A20, // A20 off at reset
            keyboard_enabled: true,
            aux_enabled: true,
            keyboard: None,
            aux: None,
            a20_signal: None,
            reset_signal: None,
        }
    }

    /// Attach the keyboard device (channel 0).
    pub fn attach_keyboard(&mut self, device: Box<dyn Ps2Device>) {
        self.keyboard = Some(device);
    }

    /// Attach the auxiliary (mouse) device (channel 1).
    pub fn attach_aux(&mut self, device: Box<dyn Ps2Device>) {
        self.aux = Some(device);
    }

    /// Wire the A20 gate output signal (the machine's line, not the board's).
    pub fn bind_a20(&mut self, signal: SignalId) {
        self.a20_signal = Some(signal);
    }

    /// Wire the reset pulse output signal.
    pub fn bind_reset(&mut self, signal: SignalId) {
        self.reset_signal = Some(signal);
    }

    /// Whether the A20 gate is open (the output port's bit).
    pub fn a20_enabled(&self) -> bool {
        self.output_port & out_port::A20 != 0
    }

    /// Poll the attached devices into the output buffer: the controller
    /// moves one byte per read of `60h`, the way the real 8042's firmware
    /// shuttles bytes. Called from `IN 60h` while the buffer is empty.
    fn poll_devices(&mut self) {
        if self.output.is_empty() {
            if self.keyboard_enabled {
                let mut taken = false;
                if let Some(keyboard) = self.keyboard.as_mut() {
                    if let Some(byte) = keyboard.take_output() {
                        self.output.push_back(byte);
                        taken = true;
                    }
                }
                let _ = taken;
            }
            if self.output.is_empty() && self.aux_enabled {
                if let Some(aux) = self.aux.as_mut() {
                    if let Some(byte) = aux.take_output() {
                        self.output.push_back(byte);
                    }
                }
            }
        }
    }

    fn update_status(&mut self) {
        if self.output.is_empty() {
            self.status &= !status::OUTPUT_FULL;
        } else {
            self.status |= status::OUTPUT_FULL;
        }
    }

    fn update_irq(&mut self, ctx: &mut dyn Ctx) -> Result<(), ContextError> {
        let pending = !self.output.is_empty();
        set_optional_signal(ctx, self.irq, pending)
    }

    /// `IN 60h`: the output buffer.
    pub fn read_data(&mut self, ctx: &mut dyn Ctx) -> u8 {
        self.poll_devices();
        let byte = self.output.pop_front().unwrap_or(0);
        self.update_status();
        self.update_irq(ctx).ok();
        byte
    }

    /// `IN 64h`: the status register.
    pub fn read_status(&self) -> u8 {
        self.status
    }

    /// `OUT 60h`: data for the controller or the pending command's operand.
    pub fn write_data(&mut self, ctx: &mut dyn Ctx, value: u8) -> Result<(), ContextError> {
        if let Some(command) = self.pending_command.take() {
            self.command_operand(ctx, command, value)?;
            return Ok(());
        }
        // Data for the keyboard channel: the attached device consumes it.
        if let Some(keyboard) = self.keyboard.as_mut() {
            keyboard.send_byte(value);
        }
        Ok(())
    }

    /// `OUT 64h`: a controller command.
    pub fn write_command(&mut self, ctx: &mut dyn Ctx, value: u8) -> Result<(), ContextError> {
        self.status |= status::COMMAND_DATA;
        match value {
            0x20 => {
                // Read the controller output port into the buffer.
                self.output.push_back(self.output_port);
                self.update_status();
                self.update_irq(ctx).ok();
            }
            0x60 => self.pending_command = Some(0x60),
            0xAA => {
                // Self-test: passed.
                self.output.push_back(0x55);
                self.update_status();
                self.update_irq(ctx).ok();
            }
            0xAB => {
                // Keyboard interface test: passed.
                self.output.push_back(0x00);
                self.update_status();
                self.update_irq(ctx).ok();
            }
            0xA9 => {
                // Auxiliary interface test: passed.
                self.output.push_back(0x00);
                self.update_status();
                self.update_irq(ctx).ok();
            }
            0xAD => self.keyboard_enabled = false,
            0xAE => self.keyboard_enabled = true,
            0xA7 => self.aux_enabled = false,
            0xA8 => self.aux_enabled = true,
            0xD1 => self.pending_command = Some(0xD1),
            0xD2 | 0xD3 => self.pending_command = Some(value),
            0xF0..=0xFF => {
                // Pulse output lines: bits 0..2 inverted into the port.
                let pulse = !value & 0x03;
                if pulse & out_port::RESET != 0 {
                    set_optional_signal(ctx, self.reset_signal.unwrap_or(self.irq), true)?;
                    set_optional_signal(ctx, self.reset_signal.unwrap_or(self.irq), false)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn command_operand(
        &mut self,
        ctx: &mut dyn Ctx,
        command: u8,
        value: u8,
    ) -> Result<(), ContextError> {
        match command {
            // 0x60's operand: the controller RAM's command byte — a
            // configuration write with no immediate guest-visible effect.
            0x60 => {}
            // 0xD1's operand: the output port — A20 and reset take effect now.
            0xD1 => {
                self.output_port = value & out_port::GATE_ALL;
                set_optional_signal(ctx, self.a20_signal.unwrap_or(self.irq), self.a20_enabled())?;
                if value & out_port::RESET == 0 {
                    // Reset line asserted (active low).
                    set_optional_signal(ctx, self.reset_signal.unwrap_or(self.irq), true)?;
                }
            }
            // 0xD2/0xD3: inject a byte as the keyboard/aux channel would.
            0xD2 | 0xD3 => {
                self.output.push_back(value);
                self.update_status();
                self.update_irq(ctx).ok();
            }
            _ => {}
        }
        Ok(())
    }
}

impl mosaic_contract::mmio::MmioTarget for I8042 {
    /// Offsets relative to the controller's I/O base: 0 = `60h` (data),
    /// 4 = `64h` (status read / command write).
    fn read(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        size: mosaic_exec::AccessSize,
        _attr: &mosaic_exec::Attr,
    ) -> Result<u64, mosaic_exec::BusFault> {
        if size != mosaic_exec::AccessSize::Byte {
            return Err(mosaic_exec::BusFault::InvalidSize);
        }
        Ok(u64::from(match offset {
            0 => self.read_data(ctx),
            4 => self.read_status(),
            _ => 0,
        }))
    }

    fn write(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        size: mosaic_exec::AccessSize,
        value: u64,
        _attr: &mosaic_exec::Attr,
    ) -> Result<(), mosaic_exec::BusFault> {
        if size != mosaic_exec::AccessSize::Byte {
            return Err(mosaic_exec::BusFault::InvalidSize);
        }
        let byte = value as u8;
        match offset {
            0 => self
                .write_data(ctx, byte)
                .map_err(|_| mosaic_exec::BusFault::Device)?,
            4 => self
                .write_command(ctx, byte)
                .map_err(|_| mosaic_exec::BusFault::Device)?,
            _ => {}
        }
        Ok(())
    }
}

impl EventTarget for I8042 {
    fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
        Ok(())
    }
}

/// Checkpoint state (ADR 0004): the register state; devices check themselves.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct I8042Checkpoint {
    pub schema: u32,
    pub status: u8,
    pub output: Vec<u8>,
    pub pending_command: Option<u8>,
    pub output_port: u8,
    pub keyboard_enabled: bool,
    pub aux_enabled: bool,
}

impl Snapshot for I8042 {
    type State = I8042Checkpoint;
    const SCHEMA_VERSION: u32 = 1;

    fn save(&self) -> Self::State {
        Self::State {
            schema: Self::SCHEMA_VERSION,
            status: self.status,
            output: self.output.iter().copied().collect(),
            pending_command: self.pending_command,
            output_port: self.output_port,
            keyboard_enabled: self.keyboard_enabled,
            aux_enabled: self.aux_enabled,
        }
    }

    fn validate(&self, version: u32, _state: &Self::State) -> Result<(), SnapshotError> {
        if version == Self::SCHEMA_VERSION {
            Ok(())
        } else {
            Err(SnapshotError::VersionMismatch)
        }
    }

    fn load(&mut self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        self.validate(version, state)?;
        self.status = state.status;
        self.output = state.output.iter().copied().collect();
        self.pending_command = state.pending_command;
        self.output_port = state.output_port;
        self.keyboard_enabled = state.keyboard_enabled;
        self.aux_enabled = state.aux_enabled;
        Ok(())
    }

    fn fingerprint(version: u32, state: &Self::State) -> u64 {
        let mut hash = mosaic_exec::Fnv64::default();
        hash.bytes(&version.to_le_bytes());
        hash.bytes(&[state.status, state.output_port]);
        hash.bytes(&state.output);
        hash.bytes(&[state.keyboard_enabled as u8, state.aux_enabled as u8]);
        hash.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_exec::{EventId, VTime};

    struct TestCtx {
        signals: Vec<(SignalId, bool)>,
    }

    impl Ctx for TestCtx {
        fn now(&self) -> VTime {
            VTime::ZERO
        }

        fn schedule(&mut self, _at: VTime, _token: u64) -> Result<EventId, ContextError> {
            Err(ContextError::ResourceExhausted)
        }

        fn cancel(&mut self, _event: EventId) -> bool {
            false
        }

        fn set_signal(&mut self, signal: SignalId, high: bool) -> Result<(), ContextError> {
            self.signals.push((signal, high));
            Ok(())
        }
    }

    struct TestKeyboard {
        pending: Vec<u8>,
    }

    impl Ps2Device for TestKeyboard {
        fn take_output(&mut self) -> Option<u8> {
            if self.pending.is_empty() {
                None
            } else {
                Some(self.pending.remove(0))
            }
        }

        fn send_byte(&mut self, _byte: u8) {}
    }

    /// Give the test keyboard a `send` path (real ones live in ps2.rs).
    impl TestKeyboard {
        fn send(&mut self, byte: u8) {
            self.pending.push(byte);
        }
    }

    #[test]
    fn self_test_answers_55() {
        let mut controller = I8042::new(SignalId(1));
        let mut ctx = TestCtx {
            signals: Vec::new(),
        };
        controller.write_command(&mut ctx, 0xAA).unwrap();
        assert_eq!(
            controller.read_status() & status::OUTPUT_FULL,
            status::OUTPUT_FULL
        );
        assert_eq!(controller.read_data(&mut ctx), 0x55);
    }

    #[test]
    fn the_a20_gate_follows_the_output_port_write() {
        let mut controller = I8042::new(SignalId(1));
        let mut ctx = TestCtx {
            signals: Vec::new(),
        };
        assert!(!controller.a20_enabled());
        // 0xD1 0xDF: the classic A20 enable.
        controller.write_command(&mut ctx, 0xD1).unwrap();
        controller.write_data(&mut ctx, 0xDF).unwrap();
        assert!(controller.a20_enabled());
        // 0xD1 0xDD: A20 off again.
        controller.write_command(&mut ctx, 0xD1).unwrap();
        controller.write_data(&mut ctx, 0xDD).unwrap();
        assert!(!controller.a20_enabled());
    }

    #[test]
    fn device_output_reaches_the_data_port() {
        let mut controller = I8042::new(SignalId(3));
        let mut ctx = TestCtx {
            signals: Vec::new(),
        };
        let mut keyboard = TestKeyboard {
            pending: Vec::new(),
        };
        keyboard.send(0x1C); // Make code of A.
        controller.attach_keyboard(Box::new(keyboard));
        assert_eq!(controller.read_data(&mut ctx), 0x1C);
    }
}
